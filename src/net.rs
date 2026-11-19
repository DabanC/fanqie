//! Cancellable network I/O for the synchronous library worker threads.
use std::{cell::OnceCell, future::Future, time::Duration};

use anyhow::{anyhow, Result};
use tokio::{runtime::Runtime, sync::watch};

#[derive(Clone)]
pub(crate) struct Cancellation(watch::Sender<bool>);

thread_local! {
    static CANCEL: std::cell::RefCell<Option<Cancellation>> = const { std::cell::RefCell::new(None) };
    static RUNTIME: OnceCell<std::result::Result<Runtime, ()>> = const { OnceCell::new() };
}

impl Cancellation {
    pub(crate) fn new() -> Self {
        Self(watch::channel(false).0)
    }

    pub(crate) fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub(crate) fn scope<T>(self, action: impl FnOnce() -> T) -> T {
        struct Reset(Option<Cancellation>);
        impl Drop for Reset {
            fn drop(&mut self) {
                CANCEL.replace(self.0.take());
            }
        }
        let _reset = Reset(CANCEL.replace(Some(self)));
        action()
    }
}

pub(crate) fn is_cancelled() -> bool {
    CANCEL.with_borrow(|token| token.as_ref().is_some_and(|token| *token.0.borrow()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IoError {
    Cancelled,
    Network,
}

pub(crate) fn check() -> Result<()> {
    if is_cancelled() {
        return Err(anyhow!("操作已取消"));
    }
    Ok(())
}

fn run<T>(future: impl Future<Output = T>) -> std::result::Result<T, IoError> {
    let cancel = CANCEL.with_borrow(|token| token.as_ref().map(|token| token.0.subscribe()));
    RUNTIME.with(|slot| {
        let runtime = slot
            .get_or_init(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| ())
            })
            .as_ref()
            .map_err(|_| IoError::Network)?;
        runtime.block_on(async move {
            if let Some(mut cancel) = cancel {
                tokio::select! {
                    biased;
                    _ = cancel.wait_for(|&cancelled| cancelled) => Err(IoError::Cancelled),
                    output = future => Ok(output),
                }
            } else {
                Ok(future.await)
            }
        })
    })
}

pub(crate) fn wait(duration: Duration) -> Result<()> {
    check()?;
    if !duration.is_zero() {
        run(async { tokio::time::sleep(duration).await }).map_err(|error| match error {
            IoError::Cancelled => anyhow!("操作已取消"),
            IoError::Network => anyhow!("无法初始化网络运行时"),
        })?;
    }
    Ok(())
}

pub(crate) struct Reply {
    pub status: u16,
    pub content_length: Option<u64>,
    pub bytes: Vec<u8>,
}

pub(crate) fn request(
    builder: reqwest::RequestBuilder,
    max_bytes: usize,
) -> std::result::Result<Reply, IoError> {
    run(async move {
        let mut response = builder.send().await.map_err(|_| IoError::Network)?;
        let status = response.status().as_u16();
        let content_length = response.content_length();
        let mut bytes = Vec::new();
        if status == 200 && content_length.is_none_or(|size| size <= max_bytes as u64) {
            while let Some(chunk) = response.chunk().await.map_err(|_| IoError::Network)? {
                let remaining = max_bytes.saturating_add(1).saturating_sub(bytes.len());
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if bytes.len() > max_bytes {
                    break;
                }
            }
        }
        Ok(Reply {
            status,
            content_length,
            bytes,
        })
    })?
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use super::*;

    #[test]
    fn cancellation_interrupts_an_in_flight_body_and_closes_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (reading, ready) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut received = Vec::new();
            let mut byte = [0];
            while !received.ends_with(b"\r\n\r\n") {
                connection.read_exact(&mut byte).unwrap();
                received.push(byte[0]);
                assert!(received.len() < 16 * 1024);
            }
            connection
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\npartial")
                .unwrap();
            reading.send(()).unwrap();
            match connection.read(&mut byte) {
                Ok(0) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                result => panic!("expected the cancelled connection to close: {result:?}"),
            }
        });
        let token = Cancellation::new();
        let task = token.clone();
        let worker = thread::spawn(move || {
            task.scope(|| {
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap();
                request(client.get(format!("http://{address}")), 32).err()
            })
        });
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        token.cancel();
        assert_eq!(worker.join().unwrap(), Some(IoError::Cancelled));
        server.join().unwrap();
    }

    #[test]
    fn a_cancelled_task_does_not_wait_for_a_cooldown() {
        let token = Cancellation::new();
        token.cancel();
        token.scope(|| assert!(wait(Duration::from_secs(60)).is_err()));
        assert!(!is_cancelled());
    }
}
