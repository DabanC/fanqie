//! Background task cancellation and optional one-chapter lookahead.
use std::thread;

use anyhow::Result;

use super::{App, Message, Outcome};
use crate::{library::Library, models::Book, net::Cancellation};

pub(super) struct Prefetch {
    book_id: String,
    chapter_id: String,
    pub(super) pending: Option<(Book, usize, f64)>,
}

impl App {
    pub(super) fn job(
        &mut self,
        label: &str,
        operation: impl FnOnce(Library) -> Result<Outcome> + Send + 'static,
    ) {
        if self.busy.is_some() {
            self.note("正在处理上一项操作；Ctrl+X 或 /cancel 可取消。", false);
            return;
        }
        self.busy = Some(label.into());
        self.spawn_job(operation);
    }

    fn spawn_job(&mut self, operation: impl FnOnce(Library) -> Result<Outcome> + Send + 'static) {
        self.stop_loading();
        let generation = self.generation;
        let sender = self.sender.clone();
        let store = self.store.clone();
        let settings = self.settings.clone();
        let token = Cancellation::new();
        self.cancellation = Some(token.clone());
        self.inflight += 1;
        thread::spawn(move || {
            let result = token.scope(|| Library::new(store, settings).and_then(operation));
            let _ = sender.send(Message { generation, result });
        });
    }

    pub(super) fn next_job_result(&mut self) -> Option<Result<Outcome>> {
        while let Ok(message) = self.receiver.try_recv() {
            // A cancelled worker still counts as in flight until it sends its result.
            self.inflight = self.inflight.saturating_sub(1);
            if message.generation != self.generation {
                continue;
            }
            self.cancellation = None;
            let result = match self.prefetch.take() {
                Some(prefetch) => {
                    let Some((book, index, fraction)) = prefetch.pending else {
                        continue;
                    };
                    message.result.map(|outcome| match outcome {
                        Outcome::Prefetched(text) => Outcome::Chapter {
                            book,
                            index,
                            text,
                            fraction,
                        },
                        other => other,
                    })
                }
                None => message.result,
            };
            self.busy = None;
            return Some(result);
        }
        None
    }

    pub(crate) fn stop_loading(&mut self) {
        if let Some(token) = self.cancellation.take() {
            token.cancel();
        }
        self.prefetch = None;
        self.generation += 1;
    }

    pub(super) fn cancel_loading(&mut self) {
        if self.cancellation.is_none() && self.busy.is_none() {
            self.note("没有正在加载的任务。", false);
            return;
        }
        self.stop_loading();
        self.busy = None;
        self.note("已取消加载，保留当前位置。", false);
    }

    pub(super) fn background_busy(&self) -> bool {
        self.inflight != 0 || self.busy.is_some()
    }

    pub(super) fn configure_prefetch(&mut self, argument: &str) {
        match argument {
            "on" => {
                self.prefetch_enabled = true;
                self.note("本次会话已启用下一章预加载。", false);
            }
            "off" => {
                self.prefetch_enabled = false;
                if self
                    .prefetch
                    .as_ref()
                    .is_some_and(|task| task.pending.is_none())
                {
                    self.stop_loading();
                }
                self.note("本次会话已关闭下一章预加载。", false);
            }
            "" => self.note(
                if self.prefetch_enabled {
                    "下一章预加载已启用；/prefetch off 关闭。"
                } else {
                    "下一章预加载已关闭；/prefetch on 启用。"
                },
                false,
            ),
            _ => self.note("请输入 /prefetch on 或 /prefetch off。", true),
        }
    }

    pub(crate) fn maybe_prefetch(&mut self) {
        if !self.prefetch_enabled
            || self.quit
            || self.background_busy()
            || self.command_focus
            || self.overlay.is_some()
            || self.wrap_width == 0
            || self.lines.is_empty()
            || (self.max_scroll() != 0 && self.fraction() < 0.75)
        {
            return;
        }
        let Some(book) = self.current.as_ref().filter(|book| book.source == "fanqie") else {
            return;
        };
        let index = self.chapter + 1;
        let Some(next) = book.chapters.get(index) else {
            return;
        };
        let key = (book.id.clone(), book.chapters[self.chapter].id.clone());
        if self.prefetch_attempted.as_ref() == Some(&key) {
            return;
        }
        self.prefetch_attempted = Some(key);
        let book = book.clone();
        let task = Prefetch {
            book_id: book.id.clone(),
            chapter_id: next.id.clone(),
            pending: None,
        };
        self.spawn_job(move |library| library.chapter(&book, index).map(Outcome::Prefetched));
        self.prefetch = Some(task);
    }

    pub(super) fn promote_prefetch(&mut self, book: &Book, index: usize, fraction: f64) -> bool {
        let Some(task) = &mut self.prefetch else {
            return false;
        };
        if self.busy.is_some()
            || book.id != task.book_id
            || book
                .chapters
                .get(index)
                .is_none_or(|chapter| chapter.id != task.chapter_id)
        {
            return false;
        }
        task.pending = Some((book.clone(), index, fraction));
        self.busy = Some("加载章节".into());
        true
    }
}
