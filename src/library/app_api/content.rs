use super::{check_status, crypto, AppResult, Fault, MAX_BYTES};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use flate2::read::MultiGzDecoder;
use scraper::{ElementRef, Html, Selector};
use serde_json::{json, Value};
use std::io::Read;

pub(super) fn decrypt_envelope(encoded: &str, key: &[u8]) -> AppResult<Vec<u8>> {
    if encoded.len() > MAX_BYTES {
        return Err(Fault::InvalidContent);
    }
    let raw = B64.decode(encoded).map_err(|_| Fault::InvalidContent)?;
    if raw.len() < 32 || raw.len() % 16 != 0 {
        return Err(Fault::InvalidContent);
    }
    crypto::decrypt(key, &raw[..16], &raw[16..]).map_err(|_| Fault::InvalidContent)
}
fn validate_identity(value: &Value, names: &[&str], expected: &str) -> AppResult<()> {
    for name in names {
        if let Some(id) = value.get(*name) {
            let matches = match id {
                Value::String(s) => s == expected,
                Value::Number(n) => n.as_u64().is_some_and(|n| n.to_string() == expected),
                _ => false,
            };
            if !matches {
                return Err(Fault::IdentityMismatch);
            }
        }
    }
    Ok(())
}
pub(super) fn parse_response(
    payload: &Value,
    book: &str,
    chapter: &str,
    key: &[u8],
) -> AppResult<String> {
    check_status(payload)?;
    validate_identity(payload, &["book_id", "bookId"], book)?;
    validate_identity(payload, &["item_id", "chapter_id", "chapterId"], chapter)?;
    let data = payload
        .get("data")
        .and_then(Value::as_object)
        .ok_or(Fault::InvalidResponse)?;
    // Never take the first map entry or return another chapter on a partial response.
    let item = data.get(chapter).ok_or(Fault::InvalidResponse)?;
    if data.len() != 1 {
        return Err(Fault::IdentityMismatch);
    }
    check_status(item)?;
    for object in [payload, item] {
        for name in [
            "need_pay",
            "needPay",
            "isChapterLock",
            "is_chapter_lock",
            "is_locked",
        ] {
            if object.get(name).is_some_and(|v| {
                v == &json!(true)
                    || v == &json!(1)
                    || v.as_str().is_some_and(|s| matches!(s, "1" | "true"))
            }) {
                return Err(Fault::Rejected);
            }
        }
    }
    validate_identity(item, &["book_id", "bookId"], book)?;
    validate_identity(item, &["item_id", "chapter_id", "chapterId"], chapter)?;
    let content = item
        .get("content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(Fault::InvalidResponse)?;
    if content == "Invalid" {
        return Err(Fault::KeyExpired);
    }
    let compressed = match item.get("compress_status") {
        None => false,
        Some(value) if value.as_u64() == Some(0) => false,
        Some(value) if value.as_u64() == Some(1) => true,
        _ => return Err(Fault::InvalidContent),
    };
    let plaintext = match item.get("encrypt_status") {
        Some(value) if value.as_u64() == Some(0) => true,
        Some(value) if value.as_u64() == Some(1) => false,
        None => content.trim_start().starts_with('<'),
        _ => return Err(Fault::InvalidContent),
    };
    let bytes = if plaintext {
        if compressed {
            B64.decode(content).map_err(|_| Fault::InvalidContent)?
        } else {
            content.as_bytes().to_vec()
        }
    } else {
        decrypt_envelope(content, key)?
    };
    let plain = if compressed {
        let mut plain = Vec::new();
        MultiGzDecoder::new(bytes.as_slice())
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut plain)
            .map_err(|_| Fault::InvalidContent)?;
        if plain.len() > MAX_BYTES {
            return Err(Fault::InvalidContent);
        }
        plain
    } else {
        bytes
    };
    if plain.len() > MAX_BYTES {
        return Err(Fault::InvalidContent);
    }
    let text = std::str::from_utf8(&plain).map_err(|_| Fault::InvalidContent)?;
    paragraphs(text)
}

pub(super) fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
        } else if !c.is_control()
            && !matches!(c as u32,
            0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f | 0xfeff)
        {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}
pub(super) fn element_text(element: ElementRef<'_>) -> String {
    let mut text = String::new();
    for node in element.descendants() {
        if node.ancestors().any(|n| {
            n.value()
                .as_element()
                .is_some_and(|e| matches!(e.name(), "script" | "style" | "noscript" | "template"))
        }) {
            continue;
        }
        if node.value().as_element().is_some_and(|e| e.name() == "br") {
            text.push(' ');
        }
        if let Some(s) = node.value().as_text() {
            text.push_str(s);
        }
    }
    clean(&text)
}
pub(super) fn paragraphs(raw: &str) -> AppResult<String> {
    let mut lines = Vec::new();
    if raw.trim_start().starts_with('<') {
        let document = Html::parse_document(raw);
        let p = Selector::parse("p").map_err(|_| Fault::InvalidContent)?;
        for e in document.select(&p) {
            let text = element_text(e);
            if !text.is_empty() {
                lines.push(text);
            }
        }
        if lines.is_empty() {
            let blk = Selector::parse("blk").map_err(|_| Fault::InvalidContent)?;
            for e in document.select(&blk) {
                if e.ancestors().any(|n| {
                    n.value().as_element().is_some_and(|e| {
                        matches!(e.name(), "h1" | "h2" | "script" | "style" | "template")
                    })
                }) {
                    continue;
                }
                let text = element_text(e);
                if !text.is_empty() {
                    lines.push(text);
                }
            }
        }
    } else {
        lines.extend(raw.lines().map(clean).filter(|s| !s.is_empty()));
    }
    let text = lines.join("\n\n");
    // App plaintext has no font obfuscation; private-use leftovers are not readable text.
    if text.is_empty()
        || text.chars().any(|c| {
            matches!(c as u32,
        0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
        })
    {
        return Err(Fault::InvalidContent);
    }
    Ok(text)
}
