//! Bounded single-chapter remote access. No upstream error or URL is displayed.
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use url::Url;

use crate::{
    models::{Book, SearchReport, Settings},
    net,
};

mod font;
mod parsing;

use super::app_api;
use parsing::{
    api_url, content, html_text, json_content, metadata, page_catalog, page_states, parse_catalog,
    parse_search, reader_content, upstream_id, valid_id,
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const CONTENT_HELP: &str = "番茄正文暂时不可用，请检查网络或稍后重试；自定义书源可使用 /api。";
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Debug)]
struct WebRateLimited;

impl std::fmt::Display for WebRateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("番茄正在限流，请稍后再试")
    }
}

impl std::error::Error for WebRateLimited {}

#[derive(Debug)]
struct WebPreview;

impl std::fmt::Display for WebPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("官网只提供本章试读，未授权全文")
    }
}

impl std::error::Error for WebPreview {}

struct Response {
    status: u16,
    oversize: bool,
    bytes: Vec<u8>,
}
// Unit transport failures deliberately carry no URL, response body or credentials.
type Transport = Arc<dyn Fn(&str) -> std::result::Result<Response, ()> + Send + Sync>;

pub(super) struct Remote {
    content_api: String,
    search_api: String,
    data_dir: Option<PathBuf>,
    transport: Transport,
}

impl Remote {
    pub(super) fn new(settings: &Settings) -> Result<Self> {
        if !settings.content_api.is_empty() {
            api_url(
                &settings.content_api,
                &[("chapter_id", "1"), ("book_id", "1")],
                ("item_ids", "1"),
            )?;
        }
        if !settings.search_api.is_empty() {
            api_url(&settings.search_api, &[("query", "test")], ("q", "test"))?;
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Mozilla/5.0")
            .build()
            .map_err(|_| anyhow!("无法初始化网络客户端"))?;
        let transport: Transport = Arc::new(move |url| {
            if let Ok(mut last) = LAST_REQUEST.lock() {
                if let Some(previous) = *last {
                    net::wait(Duration::from_millis(500).saturating_sub(previous.elapsed()))
                        .map_err(|_| ())?;
                }
                *last = Some(Instant::now());
            }
            let builder = client
                .get(url)
                .header("Accept-Encoding", "identity")
                .header("Referer", "https://fanqienovel.com/")
                .header(
                    "User-Agent",
                    if url.starts_with("https://novel.snssdk.com/") {
                        "com.dragon.read"
                    } else {
                        "Mozilla/5.0"
                    },
                );
            let reply = net::request(builder, MAX_BYTES).map_err(|_| ())?;
            Ok(Response {
                status: reply.status,
                oversize: reply
                    .content_length
                    .is_some_and(|size| size > MAX_BYTES as u64),
                bytes: reply.bytes,
            })
        });
        Ok(Self {
            content_api: settings.content_api.clone(),
            search_api: settings.search_api.clone(),
            data_dir: None,
            transport,
        })
    }

    pub(super) fn with_data_dir(settings: &Settings, data_dir: &Path) -> Result<Self> {
        let mut remote = Self::new(settings)?;
        remote.data_dir = Some(data_dir.to_path_buf());
        Ok(remote)
    }

    fn request(&self, url: &str) -> Result<String> {
        for attempt in 0..=2 {
            net::check()?;
            match (self.transport)(url) {
                Ok(response) if response.status == 200 => {
                    if response.oversize || response.bytes.len() > MAX_BYTES {
                        bail!("上游响应超过大小限制");
                    }
                    if response.bytes.is_empty() {
                        bail!("上游返回空响应，请稍后再试");
                    }
                    return String::from_utf8(response.bytes)
                        .map(|s| s.trim_start_matches('\u{feff}').to_owned())
                        .map_err(|_| anyhow!("上游响应不是有效的 UTF-8 文本"));
                }
                Ok(response) if response.status == 429 => return Err(WebRateLimited.into()),
                Ok(response) => {
                    if attempt == 2 || !matches!(response.status, 408 | 500 | 502 | 503 | 504) {
                        bail!("上游服务返回失败状态，请稍后重试");
                    }
                }
                Err(()) if net::is_cancelled() => net::check()?,
                Err(()) if attempt < 2 => {}
                Err(()) => bail!("网络请求失败，请检查网络或稍后重试"),
            }
        }
        bail!("上游请求失败，请稍后重试")
    }

    pub(super) fn catalog(&self, book_id: &str) -> Result<Book> {
        valid_id(book_id)?;
        let directory = self.request(&format!(
            "https://fanqienovel.com/api/reader/directory/detail?bookId={book_id}"
        ));
        let directory = match directory {
            Err(error) if error.is::<WebRateLimited>() => return Err(error),
            other => other,
        };
        if let Ok(raw) = directory {
            if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                if let Ok(mut book) = parse_catalog(&value, book_id) {
                    if book.title == format!("Book {book_id}")
                        || book.author.is_empty()
                        || book.description.is_empty()
                    {
                        if let Ok(page) =
                            self.request(&format!("https://fanqienovel.com/page/{book_id}"))
                        {
                            for state in page_states(&page) {
                                let (title, author, description) = metadata(&state);
                                let mut enriched = book.clone();
                                if enriched.title == format!("Book {book_id}") {
                                    if let Some(title) = title {
                                        enriched.title = title;
                                    }
                                }
                                if enriched.author.is_empty() {
                                    enriched.author = author.unwrap_or_default();
                                }
                                if enriched.description.is_empty() {
                                    enriched.description =
                                        html_text(&description.unwrap_or_default());
                                }
                                if enriched.validate().is_ok() {
                                    book = enriched;
                                }
                            }
                        }
                    }
                    return Ok(book);
                }
            }
        }
        let page = self.request(&format!("https://fanqienovel.com/page/{book_id}"))?;
        page_catalog(&page, book_id)
    }

    pub(super) fn chapter(&self, book: &Book, index: usize) -> Result<String> {
        valid_id(&book.id)?;
        let chapter = book
            .chapters
            .get(index)
            .ok_or_else(|| anyhow!("章节序号超出目录范围"))?;
        valid_id(&chapter.id)?;
        if self.content_api.is_empty() {
            let web_error = match self.web_chapter(&book.id, &chapter.id) {
                Ok(text) => return Ok(text),
                Err(error) if error.is::<WebRateLimited>() => return Err(error),
                Err(error) => error,
            };
            net::check()?;
            if let Some(data_dir) = &self.data_dir {
                return app_api::chapter(data_dir, &book.id, &chapter.id)
                    .and_then(|text| content(&text, false))
                    .with_context(|| format!("网页正文不可用（{web_error:#}）"));
            }
            return Err(web_error.context(CONTENT_HELP));
        }
        let url = api_url(
            &self.content_api,
            &[("chapter_id", &chapter.id), ("book_id", &book.id)],
            ("item_ids", &chapter.id),
        )?;
        let raw = self.request(&url)?;
        let value = serde_json::from_str::<Value>(&raw).map_err(|_| anyhow!(CONTENT_HELP))?;
        json_content(&value, &chapter.id)
    }

    fn web_chapter(&self, book_id: &str, chapter_id: &str) -> Result<String> {
        let page = self.request(&format!("https://fanqienovel.com/reader/{chapter_id}"))?;
        net::check()?;
        // Reject text belonging to another chapter before attempting to decode it.
        for state in page_states(&page) {
            let Some(data) = state.pointer("/reader/chapterData") else {
                continue;
            };
            for (field, expected) in [("bookId", book_id), ("itemId", chapter_id)] {
                if let Some(id) = data.get(field) {
                    if !upstream_id(id).is_ok_and(|id| id == expected) {
                        bail!("网页返回的书籍或章节不匹配");
                    }
                }
            }
        }
        reader_content(&page)
    }

    #[cfg(test)]
    fn search(&self, query: &str) -> Result<Vec<Book>> {
        Ok(self.search_report(query)?.books)
    }

    pub(super) fn search_report(&self, query: &str) -> Result<SearchReport> {
        if query.trim().is_empty()
            || query.chars().count() > 500
            || query.chars().any(char::is_control)
        {
            bail!("搜索关键词不能为空、含控制字符或超过 500 个字符");
        }
        if self.search_api.is_empty() {
            if let Some(data_dir) = &self.data_dir {
                let term = query.trim();
                return self.recover_search(term, app_api::search_report(data_dir, term, None));
            }
        }
        Ok(SearchReport {
            books: self.lightweight_search(query.trim())?,
            ..SearchReport::default()
        })
    }

    pub(super) fn search_more(
        &self,
        continuation: &crate::models::SearchContinuation,
    ) -> Result<SearchReport> {
        let Some(data_dir) = self
            .data_dir
            .as_ref()
            .filter(|_| self.search_api.is_empty())
        else {
            bail!("当前搜索来源不支持继续加载");
        };
        // Cursors belong to the signed query; fallback has its own result set.
        app_api::search_report(data_dir, &continuation.query, Some(continuation))
    }

    fn recover_search(&self, query: &str, primary: Result<SearchReport>) -> Result<SearchReport> {
        match primary {
            Ok(report) => Ok(report),
            Err(error) if crate::net::is_cancelled() || app_api::is_rate_limited(&error) => {
                Err(error)
            }
            Err(error) => match self.lightweight_search(query) {
                Ok(books) => Ok(SearchReport {
                    books,
                    notice: Some(
                        "APP 搜索暂不可用，已切换官方备用搜索（可能显示原书名和相关推荐）".into(),
                    ),
                    ..SearchReport::default()
                }),
                Err(_) => Err(error
                    .context("APP 与官方备用搜索均不可用，请检查网络；已有书架和搜索缓存仍可使用")),
            },
        }
    }

    fn lightweight_search(&self, query: &str) -> Result<Vec<Book>> {
        let url = if self.search_api.is_empty() {
            let mut url = Url::parse(
                "https://novel.snssdk.com/api/novel/channel/homepage/search/search/v1/",
            )?;
            url.query_pairs_mut()
                .append_pair("aid", "1967")
                .append_pair("q", query.trim())
                .append_pair("offset", "0");
            String::from(url)
        } else {
            api_url(
                &self.search_api,
                &[("query", query.trim())],
                ("q", query.trim()),
            )?
        };
        let raw = self.request(&url)?;
        let value: Value =
            serde_json::from_str(&raw).map_err(|_| anyhow!("上游搜索响应不是有效的 JSON"))?;
        parse_search(&value)
    }
}

#[cfg(test)]
mod tests;
