//! Anonymous official-app access for novel search and one requested chapter.
//! Protocol constants and algorithms follow core/appapi.py (APP 7.0.1.32).
//! Port of Jees996/fanqie-novel-downloader core/appapi.py, MIT, copyright
//! 2026 xian lee; upstream commit 486f2844b1c5e5e113cb4f0abf46b75e995f0c4f.
//! License text is retained in licenses/fanqie-novel-downloader-MIT.txt.
mod content;
mod crypto;
mod device;
mod search;
#[allow(unused_imports)] // Compatibility API; normal callers use the report.
pub(super) use search::search;
pub(super) use search::search_report;

use content::{decrypt_envelope, element_text, parse_response};
use device::{
    load_cache, registration_body, save_cache, valid_id, valid_key_version, Credentials, Device,
    DeviceLock,
};

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::{Client, RequestBuilder};
use serde_json::{json, Value};
use std::{
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::io::Read;

use crate::net;

const BASE: &str = "https://reading.snssdk.com/reading";
const REGISTER: &str = "https://i.snssdk.com/service/2/device_register/?tt_data=a";
const SHARED_KEY: [u8; 16] = [
    172, 37, 198, 125, 221, 143, 56, 193, 179, 122, 35, 72, 130, 142, 34, 46,
];
const MAX_BYTES: usize = 8 * 1024 * 1024;
const CACHE: &str = "official-app-device-v1.json";
const LOCK: &str = "official-app-device-v1.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    DeviceExpired,
    KeyExpired,
    RateLimited,
    Network,
    Cancelled,
    EmptyResponse,
    Rejected,
    InvalidResponse,
    InvalidContent,
    IdentityMismatch,
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DeviceExpired => "官方 APP 明确报告匿名设备已过期",
            Self::KeyExpired => "官方 APP 正文密钥已失效",
            Self::RateLimited => "官方 APP 请求受限，请稍后重试；不会通过更换设备绕过限流",
            Self::Network => "官方 APP 网络请求失败或超时",
            Self::Cancelled => "操作已取消",
            Self::EmptyResponse => "官方 APP 返回空响应；未更换设备",
            Self::Rejected => "官方 APP 拒绝请求（可能需付费、受限或协议已变更）",
            Self::InvalidResponse => "官方 APP 响应格式无效；未更换设备",
            Self::InvalidContent => "官方 APP 正文解密或文本校验失败",
            Self::IdentityMismatch => "官方 APP 响应与所请求的书籍或章节不符",
        })
    }
}
impl std::error::Error for Fault {}
type AppResult<T> = std::result::Result<T, Fault>;

/// Inspect typed causes, including contextual search errors, without matching text.
pub(super) fn is_rate_limited(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<Fault>() == Some(&Fault::RateLimited))
}

trait Gateway {
    fn pace(&self) -> bool {
        false
    }
    fn register_device(&mut self) -> AppResult<Device>;
    fn register_key(&mut self, device: &Device) -> AppResult<(Vec<u8>, Option<Value>)>;
    fn fetch(&mut self, state: &Credentials, book: &str, chapter: &str) -> AppResult<String>;
}

/// Read exactly one chapter using only this reader's anonymous device/key.
/// No login cookies, external device credentials, or batch prefetch.
/// Requests have 8-second connect / 25-second total timeouts and bounded response sizes.
/// Only an explicit device-expired response permits one renewal; empty/rate-limited
/// responses never renew a device. A documented `Invalid` key permits one key refresh.
pub(super) fn chapter(data_dir: &Path, book_id: &str, chapter_id: &str) -> Result<String> {
    valid_id(book_id)?;
    valid_id(chapter_id)?;
    let _lock = DeviceLock::acquire(data_dir)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("com.dragon.read")
        .build()
        .map_err(|_| Fault::Network)?;
    read_one(data_dir, &mut AppGateway(client), book_id, chapter_id)
}

fn read_one<G: Gateway>(dir: &Path, gateway: &mut G, book: &str, chapter: &str) -> Result<String> {
    let new_state = |device| Credentials {
        schema: 1,
        device,
        key: None,
        key_version: None,
        next_request_ms: 0,
    };
    let mut state = match load_cache(dir)? {
        Some(state) => state,
        None => {
            let device = gateway
                .register_device()
                .map_err(|e| anyhow!("官方 APP 匿名设备注册失败：{e}"))?;
            let state = new_state(device);
            save_cache(dir, &state)?;
            state
        }
    };
    let mut renewed = false;
    let mut refreshed = false;
    loop {
        let mut stage = "章节请求";
        let result = if state.key.is_none() {
            match gateway.register_key(&state.device) {
                Ok((key, version)) => {
                    state.key = Some(B64.encode(key));
                    state.key_version = version;
                    save_cache(dir, &state)?;
                    fetch_one(dir, gateway, &mut state, book, chapter)?
                }
                Err(error) => {
                    stage = "正文密钥注册";
                    Err(error)
                }
            }
        } else {
            fetch_one(dir, gateway, &mut state, book, chapter)?
        };
        match result {
            Ok(text) => return Ok(text),
            Err(Fault::DeviceExpired) if !renewed => {
                renewed = true;
                let device = gateway
                    .register_device()
                    .map_err(|e| anyhow!("官方 APP 匿名设备更新失败：{e}"))?;
                state = new_state(device);
                save_cache(dir, &state)?;
            }
            Err(Fault::KeyExpired) if !refreshed => {
                refreshed = true;
                state.key = None;
                state.key_version = None;
                save_cache(dir, &state)?;
            }
            Err(error) => return Err(anyhow!("官方 APP {stage}失败：{error}")),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn fetch_one<G: Gateway>(
    dir: &Path,
    gateway: &mut G,
    state: &mut Credentials,
    book: &str,
    chapter: &str,
) -> Result<AppResult<String>> {
    if gateway.pace() {
        let wait = state.next_request_ms.saturating_sub(now_ms());
        if wait > 60_000 {
            return Ok(Err(Fault::RateLimited));
        }
        net::wait(Duration::from_millis(wait))?;
        state.next_request_ms = now_ms().saturating_add(6_000);
        save_cache(dir, state)?;
    }
    let result = gateway.fetch(state, book, chapter);
    if gateway.pace() && result == Err(Fault::RateLimited) {
        state.next_request_ms = now_ms().saturating_add(60_000);
        save_cache(dir, state)?;
    }
    Ok(result)
}

struct AppGateway(Client);
impl AppGateway {
    fn response(&self, request: RequestBuilder) -> AppResult<Value> {
        let response = net::request(request, MAX_BYTES).map_err(|error| match error {
            net::IoError::Cancelled => Fault::Cancelled,
            net::IoError::Network => Fault::Network,
        })?;
        #[cfg(test)]
        if std::env::var_os("FANQIE_APP_SEARCH_DIAGNOSTICS").is_some() {
            eprintln!(
                "APP response stage=headers status={} body_length={:?}",
                response.status, response.content_length
            );
        }
        if response.status == 429 {
            return Err(Fault::RateLimited);
        }
        // HTTP 401/403 alone is not proof that the anonymous device has expired.
        if response.status != 200 {
            return Err(Fault::Rejected);
        }
        if response
            .content_length
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(Fault::InvalidResponse);
        }
        let bytes = response.bytes;
        #[cfg(test)]
        if std::env::var_os("FANQIE_APP_SEARCH_DIAGNOSTICS").is_some() {
            eprintln!(
                "APP response stage=body status=200 body_length={}",
                bytes.len()
            );
        }
        if bytes.len() > MAX_BYTES {
            return Err(Fault::InvalidResponse);
        }
        if bytes.is_empty() {
            return Err(Fault::EmptyResponse);
        }
        let payload: Value = serde_json::from_slice(&bytes).map_err(|_| Fault::InvalidResponse)?;
        check_status(&payload)?;
        Ok(payload)
    }
    fn signed(&self, url: &str, body: Option<&[u8]>) -> AppResult<RequestBuilder> {
        let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
        let headers = crypto::sign(query, body).map_err(|_| Fault::InvalidContent)?;
        let request = if let Some(body) = body {
            self.0
                .post(url)
                .header("Content-Type", "application/json; charset=utf-8")
                .body(body.to_vec())
        } else {
            self.0.get(url)
        };
        Ok(request.headers(headers))
    }
}

fn check_status(payload: &Value) -> AppResult<()> {
    let parse_code = |v: &Value| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    };
    let codes: Vec<_> = ["code", "status_code"]
        .iter()
        .filter_map(|name| payload.get(*name))
        .collect();
    let messages: Vec<_> = ["message", "msg"]
        .iter()
        .filter_map(|name| {
            payload
                .get(*name)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_ascii_lowercase())
        })
        .collect();
    if codes.iter().any(|c| parse_code(c) == Some(429))
        || messages.iter().any(|m| {
            matches!(
                m.as_str(),
                "too many requests"
                    | "rate limit"
                    | "rate_limited"
                    | "\u{8bf7}\u{6c42}\u{8fc7}\u{4e8e}\u{9891}\u{7e41}"
            )
        })
    {
        return Err(Fault::RateLimited);
    }
    if codes.iter().any(|c| parse_code(c).is_none()) {
        return Err(Fault::Rejected);
    }
    if codes.len() == 2 && parse_code(codes[0]) != parse_code(codes[1]) {
        return Err(Fault::Rejected);
    }
    if codes.iter().any(|c| parse_code(c) != Some(0)) {
        // No numeric expiry code is documented by the reference. Empty data,
        // missing chapters, and generic rejection are never expiry evidence.
        if messages.iter().any(|m| {
            matches!(
                m.as_str(),
                "device_expired" | "device expired" | "device_id_expired" | "device_id expired"
            )
        }) {
            return Err(Fault::DeviceExpired);
        }
        return Err(Fault::Rejected);
    }
    Ok(())
}

impl Gateway for AppGateway {
    fn pace(&self) -> bool {
        true
    }
    fn register_device(&mut self) -> AppResult<Device> {
        let body = registration_body();
        let bytes = serde_json::to_vec(&body).map_err(|_| Fault::InvalidResponse)?;
        let encrypted = crypto::tt_encrypt(&bytes).map_err(|_| Fault::InvalidContent)?;
        let payload = self.response(
            self.0
                .post(REGISTER)
                .header("User-Agent", "okhttp/4.10.0")
                .header("Content-Type", "application/octet-stream; tt-data=a")
                .body(encrypted),
        )?;
        let device_id = payload
            .get("device_id_str")
            .and_then(Value::as_str)
            .ok_or(Fault::InvalidResponse)?;
        let install_id = payload
            .get("install_id_str")
            .and_then(Value::as_str)
            .ok_or(Fault::InvalidResponse)?;
        valid_id(device_id).map_err(|_| Fault::InvalidResponse)?;
        valid_id(install_id).map_err(|_| Fault::InvalidResponse)?;
        Ok(Device {
            device_id: device_id.into(),
            install_id: install_id.into(),
            device_type: body["header"]["device_model"]
                .as_str()
                .ok_or(Fault::InvalidResponse)?
                .into(),
            device_brand: body["header"]["device_brand"]
                .as_str()
                .ok_or(Fault::InvalidResponse)?
                .into(),
        })
    }
    fn register_key(&mut self, device: &Device) -> AppResult<(Vec<u8>, Option<Value>)> {
        let url = format!("{BASE}/crypt/registerkey?{}", query(device, &[]));
        let iv: Vec<u8> = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(16)
            .collect();
        // Python reverse_hex(device_id)[:8] is precisely the low 64 bits, LE.
        let id = device
            .device_id
            .parse::<u64>()
            .map_err(|_| Fault::InvalidResponse)?
            .to_le_bytes();
        let mut envelope = iv.clone();
        envelope.extend(crypto::encrypt(&SHARED_KEY, &iv, &id).map_err(|_| Fault::InvalidContent)?);
        let body = serde_json::to_vec(&json!({"content": B64.encode(envelope)}))
            .map_err(|_| Fault::InvalidContent)?;
        let payload = self.response(self.signed(&url, Some(&body))?)?;
        let data = payload.get("data").ok_or(Fault::InvalidResponse)?;
        let encoded = data
            .get("key")
            .and_then(Value::as_str)
            .ok_or(Fault::InvalidResponse)?;
        let key = decrypt_envelope(encoded, &SHARED_KEY)?;
        if !matches!(key.len(), 16 | 24 | 32) {
            return Err(Fault::InvalidContent);
        }
        if !valid_key_version(data.get("keyver")) {
            return Err(Fault::InvalidResponse);
        }
        Ok((key, data.get("keyver").cloned()))
    }
    fn fetch(&mut self, state: &Credentials, book: &str, chapter: &str) -> AppResult<String> {
        // Endpoint is named batch_full, but item_ids always contains exactly ONE ID.
        let url = format!(
            "{BASE}/reader/batch_full/v?{}",
            query(
                &state.device,
                &[
                    ("item_ids", chapter),
                    ("book_id", book),
                    ("novel_text_type", "1"),
                    ("req_type", "1"),
                ]
            )
        );
        let payload = self.response(self.signed(&url, None)?)?;
        parse_response(&payload, book, chapter, &state.key()?)
    }
}

fn query(device: &Device, extra: &[(&str, &str)]) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.extend_pairs([
        ("iid", device.install_id.as_str()),
        ("device_id", device.device_id.as_str()),
        ("ac", "wifi"),
        ("channel", "43536163a"),
        ("aid", "1967"),
        ("app_name", "novelapp"),
        ("version_code", "70132"),
        ("version_name", "7.0.1.32"),
        ("device_platform", "android"),
        ("os", "android"),
        ("ssmix", "a"),
        ("os_version", "10"),
        ("device_type", device.device_type.as_str()),
        ("device_brand", device.device_brand.as_str()),
        ("update_version_code", "70132"),
        ("manifest_version_code", "70132"),
        ("pv_player", "70132"),
    ]);
    q.extend_pairs(extra.iter().copied());
    q.finish()
}

#[cfg(test)]
mod tests;
