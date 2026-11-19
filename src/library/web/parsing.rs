use anyhow::{anyhow, bail, Result};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

use crate::models::{Book, Chapter};

use super::{font, CONTENT_HELP};

pub(super) fn valid_id(id: &str) -> Result<&str> {
    if id.is_empty()
        || id.len() > 32
        || id.starts_with('0')
        || !id.bytes().all(|c| c.is_ascii_digit())
    {
        bail!("书籍或章节 ID 无效：必须为不超过 32 位的正十进制数字");
    }
    Ok(id)
}

fn field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| value.get(*key).filter(|v| !v.is_null()))
}

fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    field(value, keys)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(super) fn upstream_id(value: &Value) -> Result<String> {
    let id = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) if n.is_u64() => n.to_string(),
        _ => bail!("上游返回的书籍或章节 ID 无效"),
    };
    valid_id(&id).map_err(|_| anyhow!("上游返回的书籍或章节 ID 无效"))?;
    Ok(id)
}

fn nodes(root: &Value) -> Vec<&Value> {
    let mut stack = vec![root];
    let mut result = Vec::new();
    while let Some(node) = stack.pop() {
        match node {
            Value::Object(map) => {
                result.push(node);
                stack.extend(map.values().rev());
            }
            Value::Array(items) => stack.extend(items.iter().rev()),
            _ => {}
        }
    }
    result
}

fn normalize(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn html_text(raw: &str) -> String {
    let document = Html::parse_fragment(raw);
    let mut output = String::new();
    for node in document.root_element().descendants() {
        let hidden = node.ancestors().any(|ancestor| {
            ancestor
                .value()
                .as_element()
                .map(|e| matches!(e.name(), "script" | "style" | "noscript" | "template"))
                .unwrap_or(false)
        });
        if hidden {
            continue;
        }
        if let Some(element) = node.value().as_element() {
            if matches!(
                element.name(),
                "p" | "div" | "br" | "section" | "li" | "h1" | "h2"
            ) {
                output.push('\n');
            }
        }
        if let Some(text) = node.value().as_text() {
            output.push_str(text);
        }
    }
    normalize(&output)
}

fn private_use(c: char) -> bool {
    matches!(c as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

pub(super) fn content(raw: &str, reader: bool) -> Result<String> {
    let tag = Regex::new(r"</?[A-Za-z][^>]*>").unwrap();
    let clean = if tag.is_match(raw) {
        html_text(raw)
    } else {
        normalize(raw)
    };
    let blocked = Regex::new(concat!(
        r"(?i)captcha|access denied|verify you are human|sign in to (read|continue)|",
        r"log ?in to (read|continue)|login required|authentication required|",
        r"subscribe to read|unlock this chapter|purchase (this |the )?chapter|",
        r"^(?:error|failed|not found|login|sign in|service unavailable|forbidden|unauthorized)[.!。！\s]*$|",
        r"验证码|人机验证|访问受限|请求过于频繁|请(?:先)?登录|登录后(阅读|查看)|",
        r"付费(阅读|章节)|购买(本章|章节)|解锁(本章|章节)|内容加载失败"
    ))
    .unwrap();
    if clean.is_empty()
        || blocked.is_match(&clean)
        || clean.chars().any(private_use)
        || clean
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        || (reader
            && (clean.chars().count() < 40
                || clean.chars().filter(|c| c.is_alphabetic()).count() < 20))
    {
        bail!(CONTENT_HELP);
    }
    Ok(clean)
}

fn chapter_value<'a>(payload: &'a Value, id: &str) -> Option<&'a Value> {
    let mut value = payload;
    for _ in 0..12 {
        if value.is_string() || value.is_array() {
            return Some(value);
        }
        let obj = value.as_object()?;
        let denied = [
            "encrypted",
            "is_encrypted",
            "encrypt",
            "is_locked",
            "isLocked",
            "need_pay",
            "needPay",
            "need_login",
            "needLogin",
        ]
        .iter()
        .any(|key| match obj.get(*key) {
            Some(Value::Bool(value)) => *value,
            Some(Value::Number(value)) => value.as_u64().map(|v| v != 0).unwrap_or(true),
            Some(Value::String(value)) => !matches!(
                value.to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "none"
            ),
            _ => false,
        });
        if denied || obj.get("success").and_then(Value::as_bool) == Some(false) {
            return None;
        }
        if let Some(status) = obj.get("status") {
            let success = match status {
                Value::Null => true,
                Value::Bool(value) => *value,
                Value::Number(value) => matches!(value.as_i64(), Some(0 | 200)),
                Value::String(value) => matches!(
                    value.to_ascii_lowercase().as_str(),
                    "ok" | "success" | "successful" | "0" | "200"
                ),
                _ => false,
            };
            if !success {
                return None;
            }
        }
        if let Some(error) = field(value, &["error", "errors"]) {
            if !matches!(error, Value::Bool(false))
                && error.as_str() != Some("")
                && !error.as_array().map(Vec::is_empty).unwrap_or(false)
                && !error
                    .as_object()
                    .map(serde_json::Map::is_empty)
                    .unwrap_or(false)
            {
                return None;
            }
        }
        if let Some(message) = field(value, &["message", "msg"]).and_then(Value::as_str) {
            // Explicit service errors are never chapter prose, even with content.
            if !message.trim().is_empty() && content(message, false).is_err() {
                return None;
            }
        }
        if let Some(code) = obj.get("code") {
            if !code.is_null()
                && ![
                    serde_json::json!(0),
                    serde_json::json!(200),
                    serde_json::json!("0"),
                    serde_json::json!("200"),
                ]
                .contains(code)
            {
                return None;
            }
        }
        value = obj.get(id).or_else(|| {
            field(
                value,
                &[
                    "content",
                    "text",
                    "body",
                    "paragraphs",
                    "data",
                    "result",
                    "chapter",
                    "chapterInfo",
                    "chapter_info",
                ],
            )
        })?;
    }
    None
}

pub(super) fn json_content(payload: &Value, id: &str) -> Result<String> {
    let value = chapter_value(payload, id).ok_or_else(|| anyhow!(CONTENT_HELP))?;
    let raw = match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().ok_or_else(|| anyhow!(CONTENT_HELP)))
            .collect::<Result<Vec<_>>>()?
            .join("\n"),
        _ => bail!(CONTENT_HELP),
    };
    content(&raw, false)
}

pub(super) fn reader_content(page: &str) -> Result<String> {
    for state in page_states(page) {
        if let Some(chapter) = state.pointer("/reader/chapterData") {
            if chapter.get("isChapterLock").and_then(Value::as_bool) == Some(true)
                || chapter.get("needPay").and_then(Value::as_u64).unwrap_or(0) != 0
            {
                bail!("官网只提供本章试读片段，需要使用直连阅读通道");
            }
            if let Some(raw) = chapter.get("content").and_then(Value::as_str) {
                let plain = html_text(raw);
                let decoded = font::decode(page, &plain)?;
                let expected = chapter
                    .get("chapterWordNumber")
                    .and_then(|value| {
                        value
                            .as_str()
                            .and_then(|s| s.parse::<usize>().ok())
                            .or_else(|| value.as_u64().map(|n| n as usize))
                    })
                    .unwrap_or(0);
                if expected > 0 && decoded.chars().count() < expected.saturating_mul(3) / 4 {
                    bail!("网页正文不完整，需要使用直连阅读通道");
                }
                return content(&decoded, true);
            }
        }
    }
    let doc = Html::parse_document(page);
    let selector = Selector::parse(
        ".muye-reader-content, .reader-content, #reader-content, .novel-content, .chapter-content",
    )
    .unwrap();
    for element in doc.select(&selector) {
        let plain = html_text(&element.inner_html());
        if let Ok(decoded) = font::decode(page, &plain) {
            if let Ok(text) = content(&decoded, true) {
                return Ok(text);
            }
        }
    }
    bail!(CONTENT_HELP)
}

pub(super) fn api_url(
    template: &str,
    values: &[(&str, &str)],
    parameter: (&str, &str),
) -> Result<String> {
    let templated = values
        .iter()
        .any(|(key, _)| template.contains(&format!("{{{key}}}")));
    let mut rendered = template.to_owned();
    for (key, value) in values {
        let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
        // Form encoding uses '+'; path/query templates require percent-encoded spaces.
        rendered = rendered.replace(&format!("{{{key}}}"), &encoded.replace('+', "%20"));
    }
    if rendered.contains(['{', '}']) {
        bail!("API 地址含不支持的模板占位符");
    }
    let mut url = Url::parse(&rendered).map_err(|_| anyhow!("API 地址无效"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("API 地址必须为 HTTP(S)，且不能含用户密码或片段");
    }
    if !templated {
        if url.path() == "/" || url.path().is_empty() {
            url.set_path(if parameter.0 == "item_ids" {
                "/reading/reader/batch_full/v"
            } else {
                "/api/search"
            });
        }
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| key != parameter.0)
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(pairs)
            .append_pair(parameter.0, parameter.1);
    }
    Ok(url.into())
}

pub(super) fn parse_catalog(value: &Value, book_id: &str) -> Result<Book> {
    let all = nodes(value);
    let aliases = [
        "chapterListWithVolume",
        "chapter_list_with_volume",
        "chapterList",
        "chapter_list",
        "chapters",
        "item_list",
    ];
    let entries = all
        .iter()
        .find_map(|node| {
            field(node, &aliases).filter(|v| v.as_array().map(|a| !a.is_empty()).unwrap_or(false))
        })
        .ok_or_else(|| anyhow!("上游返回的章节目录为空"))?;
    let mut chapters = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for node in nodes(entries) {
        if aliases
            .iter()
            .any(|key| node.get(*key).map(Value::is_array).unwrap_or(false))
        {
            continue;
        }
        let Some(id) = field(
            node,
            &["itemId", "item_id", "chapterId", "chapter_id", "id"],
        ) else {
            continue;
        };
        let id = upstream_id(id)?;
        let title = text_field(
            node,
            &[
                "title",
                "chapterName",
                "chapter_name",
                "itemTitle",
                "item_title",
                "name",
            ],
        )
        .filter(|s| !s.trim().is_empty() && s.chars().count() <= 2000)
        .ok_or_else(|| anyhow!("上游返回的章节标题无效"))?;
        if seen.insert(id.clone()) {
            chapters.push(Chapter { id, title });
        }
    }
    if chapters.is_empty() {
        bail!("上游返回的章节目录为空");
    }
    let (title, author, description) = metadata(value);
    let title = title.unwrap_or_else(|| format!("Book {book_id}"));
    let author = author.unwrap_or_default();
    let description = description.unwrap_or_default();
    if title.trim().is_empty()
        || title.chars().count() > 2000
        || author.chars().count() > 2000
        || description.chars().count() > 100000
    {
        bail!("上游返回的书籍信息无效");
    }
    let book = Book {
        id: book_id.to_owned(),
        title,
        author,
        description: html_text(&description),
        source: "fanqie".into(),
        chapters,
    };
    book.validate()
        .map_err(|_| anyhow!("上游返回的书籍信息或章节目录无效"))?;
    Ok(book)
}

pub(super) fn metadata(value: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let all = nodes(value);
    let scoped: Vec<&Value> = all
        .iter()
        .filter_map(|node| {
            field(node, &["bookInfo", "book_info", "book"]).filter(|v| v.is_object())
        })
        .collect();
    let title = scoped
        .iter()
        .find_map(|node| text_field(node, &["bookName", "book_name", "title", "name"]))
        .or_else(|| {
            all.iter()
                .find_map(|node| text_field(node, &["bookName", "book_name"]))
        });
    let find = |keys: &[&str]| {
        scoped
            .iter()
            .chain(all.iter())
            .find_map(|node| text_field(node, keys))
    };
    (
        title,
        find(&["author", "authorName", "author_name"]),
        find(&["abstract", "description", "intro"]),
    )
}

pub(super) fn page_catalog(page: &str, book_id: &str) -> Result<Book> {
    for value in page_states(page) {
        if let Ok(book) = parse_catalog(&value, book_id) {
            return Ok(book);
        }
    }
    bail!("未取得有效且非空的章节目录")
}

pub(super) fn page_states(page: &str) -> Vec<Value> {
    let mut states = Vec::new();
    let doc = Html::parse_document(page);
    let selector = Selector::parse("script").unwrap();
    let assignment = Regex::new(r"(?:window\s*\.\s*)?__INITIAL_STATE__\s*=\s*").unwrap();
    for script in doc.select(&selector) {
        let raw = script.inner_html();
        let candidate = if let Some(m) = assignment.find(&raw) {
            &raw[m.end()..]
        } else {
            raw.trim()
        };
        // Streaming deserialization accepts a trailing semicolon but never runs JS.
        if let Some(Ok(value)) = serde_json::Deserializer::from_str(candidate)
            .into_iter::<Value>()
            .next()
        {
            states.push(value);
        }
    }
    states
}

pub(super) fn parse_search(value: &Value) -> Result<Vec<Book>> {
    if let Some(code) = value.get("code") {
        if !matches!(code.as_i64(), Some(0 | 200)) && !matches!(code.as_str(), Some("0" | "200")) {
            bail!("番茄搜索暂时不可用或正在限流，请稍后重试");
        }
    }
    let mut payload = value;
    for _ in 0..8 {
        if payload.is_array() {
            break;
        }
        payload = field(payload, &["ret_data", "items", "results", "data"])
            .ok_or_else(|| anyhow!("上游返回的搜索结果无效"))?;
    }
    let items = payload
        .as_array()
        .ok_or_else(|| anyhow!("上游返回的搜索结果无效"))?;
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if let Some(genre) = item.get("genre") {
            if !matches!(genre.as_str(), Some("0")) && genre.as_i64() != Some(0) {
                continue;
            }
        }
        let id = upstream_id(
            field(item, &["book_id", "bookId"]).ok_or_else(|| anyhow!("搜索结果缺少书籍 ID"))?,
        )?;
        let title = text_field(item, &["book_name", "bookName", "title"])
            .filter(|s| !s.trim().is_empty() && s.chars().count() <= 2000)
            .ok_or_else(|| anyhow!("搜索结果的书名无效"))?;
        let author = text_field(item, &["author", "author_name", "authorName"]).unwrap_or_default();
        if author.chars().count() > 2000 {
            bail!("搜索结果的作者信息无效");
        }
        if seen.insert(id.clone()) {
            let book = Book {
                id,
                title,
                author,
                description: html_text(
                    &text_field(item, &["abstract", "description"]).unwrap_or_default(),
                ),
                source: "fanqie".into(),
                chapters: Vec::new(),
            };
            book.validate()
                .map_err(|_| anyhow!("上游返回的搜索书籍信息无效"))?;
            results.push(book);
        }
    }
    Ok(results)
}
