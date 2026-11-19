//! Signed, bounded novel-only search; no lite-search or local-index fallback.
use super::{
    check_status, element_text, load_cache, now_ms, query, save_cache, valid_id, AppGateway,
    AppResult, Client, Credentials, Device, DeviceLock, Fault, Gateway, BASE,
};
use crate::models::{Book, SearchContinuation, SearchReport};
use anyhow::{bail, Result};
use scraper::Html;
use serde_json::{json, Value};
use std::{cmp::Reverse, collections::HashSet, path::Path, time::Duration};

use crate::library::search::{normalized_title, rank_and_dedup, title_match};

const MAX_PAGES: usize = 3;
const REQUEST_INTERVAL_MS: u64 = 6_000;
const RATE_COOLDOWN_MS: u64 = 60_000;

/// Search the first official Android novel page using this reader's anonymous device.
/// Non-novels and unrelated recommendations are excluded; no chapter key is needed.
#[allow(dead_code)] // Retained public compatibility API for callers without notices.
pub(in crate::library) fn search(data_dir: &Path, query: &str) -> Result<Vec<Book>> {
    Ok(search_report(data_dir, query, None)?.books)
}

/// Fetch a single page. A continuation is advanced only on success.
pub(in crate::library) fn search_report(
    data_dir: &Path,
    query: &str,
    continuation: Option<&SearchContinuation>,
) -> Result<SearchReport> {
    let term = query.trim();
    if term.is_empty() || term.chars().count() > 100 || term.chars().any(char::is_control) {
        bail!("搜索词不能为空、不能包含控制字符，且最多 100 个字符");
    }
    if crate::net::is_cancelled() {
        return Err(Fault::Cancelled.into());
    }
    let _lock = DeviceLock::acquire(data_dir)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("com.dragon.read")
        .build()
        .map_err(|_| Fault::Network)?;
    search_page_with(data_dir, &mut AppGateway(client), term, continuation)
}

struct Hit {
    book: Book,
    titles: Vec<String>,
    recommended: bool,
}

impl Hit {
    fn choose(mut self, wanted: &str) -> Option<(Book, Vec<Book>)> {
        let (_, title) = self
            .titles
            .iter()
            .enumerate()
            .max_by_key(|(index, title)| (title_match(wanted, title), Reverse(*index)))?;
        if self.recommended {
            if title_match(wanted, title) == 0 {
                return None;
            }
            self.book
                .description
                .insert_str(0, "官方 APP 推荐标记=1；请核对书名与作者。\n\n");
            self.book.validate().ok()?;
        }
        self.book.title.clone_from(title);
        let aliases = self
            .titles
            .into_iter()
            .map(|title| Book {
                title,
                ..self.book.clone()
            })
            .collect();
        Some((self.book, aliases))
    }
}

struct SearchPage {
    hits: Vec<Hit>,
    cursor: Option<String>,
    has_more: bool,
}

trait SearchGateway: Gateway {
    fn clock_ms(&self) -> u64 {
        now_ms()
    }
    fn wait(&mut self, millis: u64) {
        let _ = crate::net::wait(Duration::from_millis(millis));
    }
    fn search_page(&mut self, device: &Device, term: &str, cursor: &str) -> AppResult<SearchPage>;
}

impl SearchGateway for AppGateway {
    fn search_page(&mut self, device: &Device, term: &str, cursor: &str) -> AppResult<SearchPage> {
        let url = format!(
            "{BASE}/bookapi/search/tab/v?{}",
            query(
                device,
                &[
                    ("query", term),
                    ("passback", cursor),
                    ("selected_items", ""),
                    ("tab_type", "3"),
                ],
            )
        );
        let payload = self.response(self.signed(&url, None)?)?;
        parse_page(&payload)
    }
}

fn search_page_with<G: SearchGateway>(
    dir: &Path,
    gateway: &mut G,
    term: &str,
    continuation: Option<&SearchContinuation>,
) -> Result<SearchReport> {
    let mut cursors = match continuation {
        None => vec!["0".to_owned()],
        Some(state)
            if state.query == term
                && !state.cursors.is_empty()
                && state.cursors.len() <= MAX_PAGES =>
        {
            state.cursors.clone()
        }
        Some(_) => bail!("搜索分页已失效，请重新搜索"),
    };
    let page_number = cursors.len();
    let cursor = cursors.last().expect("search always has a request cursor");
    let mut state = match load_cache(dir)? {
        Some(state)
            if continuation.is_none_or(|previous| {
                previous.device_id == state.device.device_id
                    && previous.install_id == state.device.install_id
            }) =>
        {
            state
        }
        Some(_) => bail!("官方 APP 搜索设备已更改，请重新搜索"),
        None if continuation.is_some() => bail!("官方 APP 搜索设备不可用，请重新搜索"),
        None => {
            let state = Credentials {
                schema: 1,
                device: gateway
                    .register_device()
                    .map_err(|e| anyhow::Error::new(e).context("官方 APP 搜索设备注册失败"))?,
                key: None,
                key_version: None,
                next_request_ms: 0,
            };
            save_cache(dir, &state)?;
            state
        }
    };
    let page = fetch_search_page(dir, gateway, &mut state, term, cursor).map_err(|error| {
        if page_number == 1 && super::is_rate_limited(&error) {
            error
        } else {
            error.context(format!("官方 APP 搜索第 {page_number} 页失败"))
        }
    })?;
    let wanted = normalized_title(term);
    let mut report = SearchReport::default();
    let mut seen_aliases = HashSet::new();
    for hit in page.hits {
        let Some((book, aliases)) = hit.choose(&wanted) else {
            continue;
        };
        report.books.push(book);
        report.cache_aliases.extend(
            aliases
                .into_iter()
                .filter(|alias| seen_aliases.insert((alias.id.clone(), alias.title.clone()))),
        );
    }
    rank_and_dedup(term, &mut report.books);
    if page.has_more {
        if let Some(next) = page.cursor.filter(|next| !cursors.contains(next)) {
            if page_number == MAX_PAGES {
                report.add_notice(format!(
                    "官方 APP 搜索已达到最多 {MAX_PAGES} 页，仅显示已获取的分页结果"
                ));
            } else {
                cursors.push(next);
                report.continuation = Some(SearchContinuation {
                    query: term.to_owned(),
                    device_id: state.device.device_id,
                    install_id: state.device.install_id,
                    cursors,
                });
            }
        }
    }
    Ok(report)
}

fn fetch_search_page<G: SearchGateway>(
    dir: &Path,
    gateway: &mut G,
    state: &mut Credentials,
    term: &str,
    cursor: &str,
) -> Result<SearchPage> {
    for attempt in 0..2 {
        if gateway.pace() {
            let wait = state.next_request_ms.saturating_sub(gateway.clock_ms());
            // A persisted long cooldown must not be truncated or bypassed.
            if wait > RATE_COOLDOWN_MS {
                return Err(anyhow::Error::new(Fault::RateLimited)
                    .context("官方 APP 搜索仍在冷却期，请稍后重试"));
            }
            gateway.wait(wait);
            if crate::net::is_cancelled() {
                return Err(Fault::Cancelled.into());
            }
            state.next_request_ms = gateway.clock_ms().saturating_add(REQUEST_INTERVAL_MS);
            save_cache(dir, state)?;
        }
        match gateway.search_page(&state.device, term, cursor) {
            Ok(page) => return Ok(page),
            Err(error) => {
                if gateway.pace() && error == Fault::RateLimited {
                    state.next_request_ms = gateway.clock_ms().saturating_add(RATE_COOLDOWN_MS);
                    // Preserve the typed rate cause even if persisting the cooldown fails.
                    if save_cache(dir, state).is_err() {
                        return Err(
                            anyhow::Error::new(error).context("官方 APP 搜索限流，冷却期保存失败")
                        );
                    }
                }
                // Reuse the same cursor and device; only transient faults get one retry.
                if attempt == 0 && matches!(error, Fault::EmptyResponse | Fault::Network) {
                    continue;
                }
                return Err(anyhow::Error::new(error).context("官方 APP 小说搜索失败"));
            }
        }
    }
    unreachable!("each page has at most two attempts")
}

fn yes(value: &Value) -> bool {
    value == &json!(true) || value == &json!(1) || value == &json!("1")
}

fn novel_kind(node: &Value) -> bool {
    // Only these explicitly documented category fields share tab_type's enum.
    ["tab_type", "content_type"].iter().all(|key| {
        node.get(*key)
            .is_none_or(|value| value == &json!(3) || value == &json!("3"))
    }) && !["is_audio", "is_comic", "is_video"]
        .iter()
        .any(|key| node.get(*key).is_some_and(yes))
}

fn recommendation(node: &Value) -> bool {
    ["use_recommend", "upstream_use_recommend", "is_recommend"]
        .iter()
        .any(|key| node.get(*key).is_some_and(yes))
}

fn text(node: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        let value = node.get(*name)?.as_str()?;
        let value = element_text(Html::parse_fragment(value).root_element());
        (!value.is_empty()).then_some(value)
    })
}

fn book_id(node: &Value) -> Option<String> {
    let value = node.get("book_id")?;
    let id = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) if n.is_u64() => n.to_string(),
        _ => return None,
    };
    valid_id(&id).ok()?;
    Some(id)
}

fn parse_page(payload: &Value) -> AppResult<SearchPage> {
    let mut node = payload;
    let mut layers = Vec::new();
    let mut selected_tab = false;
    let cells = loop {
        if layers.len() >= 12 {
            return Err(Fault::InvalidResponse);
        }
        check_status(node)?;
        if !novel_kind(node) {
            return Err(Fault::InvalidResponse);
        }
        layers.push(node);
        if let Some(tabs) = node.get("search_tabs") {
            if selected_tab {
                return Err(Fault::InvalidResponse);
            }
            node = tabs
                .as_array()
                .and_then(|tabs| {
                    tabs.iter().find(|tab| {
                    matches!(tab.get("tab_type"), Some(v) if v == &json!(3) || v == &json!("3"))
                })
                })
                .ok_or(Fault::InvalidResponse)?;
            selected_tab = true;
        } else if let Some(cells) = node.get("data").and_then(Value::as_array) {
            break cells;
        } else {
            node = node
                .get("data")
                .filter(|v| v.is_object())
                .ok_or(Fault::InvalidResponse)?;
        }
    };
    let mut hits = Vec::new();
    let recommended = layers.iter().any(|node| recommendation(node));
    for cell in cells {
        if !novel_kind(cell) {
            continue;
        }
        let Some(books) = cell.get("book_data").and_then(Value::as_array) else {
            continue;
        };
        for record in books {
            if !novel_kind(record) {
                continue;
            }
            let Some(id) = book_id(record).or_else(|| book_id(cell)) else {
                continue;
            };
            let mut titles = ["book_name", "title", "original_book_name"]
                .iter()
                .filter_map(|key| text(record, &[*key]))
                .filter(|title| title.len() <= 8192 && !title.chars().any(char::is_control))
                .collect::<Vec<_>>();
            titles.dedup();
            let Some(title) = titles.first().cloned() else {
                continue;
            };
            let mut description =
                text(record, &["abstract", "description", "book_abstract"]).unwrap_or_default();
            if let Some(original) =
                text(record, &["original_book_name"]).filter(|original| original != &title)
            {
                description.insert_str(0, &format!("官方 APP 原书名：{original}\n\n"));
            }
            let book = Book {
                id,
                title,
                author: text(record, &["author", "author_name"]).unwrap_or_default(),
                description,
                source: "fanqie".into(),
                chapters: Vec::new(),
            };
            if book.validate().is_ok() {
                hits.push(Hit {
                    book,
                    titles,
                    recommended: recommended || recommendation(cell) || recommendation(record),
                });
            }
        }
    }
    let field = |key: &str| {
        layers
            .iter()
            .rev()
            .find_map(|layer| layer.get(key).filter(|v| !v.is_null()))
    };
    let cursor = match field("passback").or_else(|| field("next_offset")) {
        None => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) if s.len() <= 4096 && !s.chars().any(char::is_control) => {
            Some(s.clone())
        }
        Some(Value::Number(n)) if n.is_u64() => Some(n.to_string()),
        _ => return Err(Fault::InvalidResponse),
    };
    let has_more = field("has_more").map(yes).unwrap_or(cursor.is_some());
    Ok(SearchPage {
        hits,
        cursor,
        has_more,
    })
}

#[cfg(test)]
mod tests;
