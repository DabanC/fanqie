use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::{
    models::{Book, SearchReport, Settings},
    storage::Store,
};

mod app_api;
mod local;
mod search;
mod web;

use search::merge_search_report;

/// All remote content requests are single-chapter and initiated by chapter().
pub struct Library {
    store: Store,
    settings: Settings,
}

impl Library {
    pub fn new(store: Store, settings: Settings) -> Result<Self> {
        settings.validate()?;
        Ok(Self { store, settings })
    }

    #[cfg(test)]
    pub fn search(&self, query: &str) -> Result<Vec<Book>> {
        Ok(self.search_report(query)?.books)
    }

    pub fn search_report(&self, query: &str) -> Result<SearchReport> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 100 || query.chars().any(char::is_control) {
            bail!("请输入有效搜索词（最多 100 个字符）");
        }
        crate::net::check()?;
        let local = self.local_search(query)?;
        let mut cache_notice = None;
        let cached = if self.settings.search_api.is_empty() {
            match self.store.cached_search_books(query) {
                Ok(books) => books,
                Err(_) => {
                    cache_notice = Some("搜索缓存不可读，已跳过");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let mut remote =
            web::Remote::with_data_dir(&self.settings, self.store.root())?.search_report(query);
        crate::net::check()?;
        if let Ok(report) = &mut remote {
            if self.cache_remote_search(report).is_err() {
                cache_notice = Some("搜索结果可用，但缓存写入失败");
            }
        }
        let mut report = merge_search_report(query, local, cached, remote)?;
        if let Some(notice) = cache_notice {
            report.add_notice(notice);
        }
        Ok(report)
    }

    /// Load one more page, leaving the previous report and cursor valid on failure.
    pub fn search_more(&self, report: &SearchReport) -> Result<SearchReport> {
        crate::net::check()?;
        let continuation = report
            .continuation
            .as_ref()
            .context("没有更多搜索结果可加载")?;
        let mut next = web::Remote::with_data_dir(&self.settings, self.store.root())?
            .search_more(continuation)?;
        crate::net::check()?;
        let mut cache_notice = None;
        let cached = if next.notice.is_some() {
            match self.store.cached_search_books(&continuation.query) {
                Ok(books) => books,
                Err(_) => {
                    cache_notice = Some("搜索缓存不可读，已跳过");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        if self.cache_remote_search(&mut next).is_err() {
            cache_notice = Some("搜索结果可用，但缓存写入失败");
        }
        let mut next = search::append_search_page(&continuation.query, report, next, cached);
        if let Some(notice) = cache_notice {
            next.add_notice(notice);
        }
        Ok(next)
    }

    fn cache_remote_search(&self, report: &mut SearchReport) -> Result<()> {
        if !self.settings.search_api.is_empty() {
            return Ok(());
        }
        let aliases = std::mem::take(&mut report.cache_aliases);
        let books = if aliases.is_empty() {
            &report.books
        } else {
            &aliases
        };
        self.store.cache_search_books(books)
    }

    fn local_search(&self, query: &str) -> Result<Vec<Book>> {
        let folded = query.to_lowercase();
        Ok(self
            .store
            .books()?
            .into_iter()
            .filter(|book| {
                book.title.to_lowercase().contains(&folded)
                    || book.author.to_lowercase().contains(&folded)
                    || book.id.contains(query)
            })
            .collect())
    }

    pub fn open_remote(&self, input: &str) -> Result<Book> {
        let id = book_id(input)?;
        if let Some(book) = self.store.get_book(&id)? {
            if !book.chapters.is_empty() {
                return Ok(book);
            }
        }
        let book = self.fetch_catalog(&id)?;
        crate::net::check()?;
        self.store.save_book(&book)?;
        Ok(book)
    }

    pub(crate) fn open_search_hit(&self, hit: &Book) -> Result<Book> {
        hit.validate()?;
        let mut book = self.open_remote(&hit.id)?;
        if book.source == "fanqie" && hit.source == "fanqie" && book.title != hit.title {
            crate::net::check()?;
            book.title.clone_from(&hit.title);
            self.store.save_book(&book)?;
        }
        Ok(book)
    }

    pub(crate) fn fetch_catalog(&self, id: &str) -> Result<Book> {
        let book = web::Remote::new(&self.settings)?.catalog(id)?;
        book.validate()?;
        if book.chapters.is_empty() {
            bail!("上游没有返回有效章节目录");
        }
        if book.id != id {
            bail!("上游书籍标识不匹配");
        }
        Ok(book)
    }

    pub fn chapter(&self, book: &Book, index: usize) -> Result<String> {
        book.validate()?;
        let chapter = book.chapters.get(index).context("章节序号超出目录范围")?;
        if let Some(content) = self.store.cached_chapter(&book.id, &chapter.id)? {
            return Ok(content);
        }
        if book.source != "fanqie" {
            bail!("本地快照章节缺失或损坏；请重新导入 TXT / EPUB，或重新运行 /demo。已导入的完整快照不依赖原文件");
        }
        // Fetch, validate, and cache exactly the requested chapter, never its neighbors.
        let content =
            web::Remote::with_data_dir(&self.settings, self.store.root())?.chapter(book, index)?;
        crate::net::check()?;
        self.store.cache_chapter(&book.id, &chapter.id, &content)?;
        Ok(content)
    }

    fn snapshot(&self, book: Book, chapters: Vec<String>) -> Result<Book> {
        book.validate()?;
        if book.chapters.is_empty() || book.chapters.len() != chapters.len() {
            bail!("本地快照目录与正文不匹配");
        }
        // Publish the metadata only after every local snapshot chapter is durable.
        for (chapter, content) in book.chapters.iter().zip(chapters) {
            crate::net::check()?;
            self.store.cache_chapter(&book.id, &chapter.id, &content)?;
        }
        crate::net::check()?;
        self.store.save_book(&book)?;
        Ok(book)
    }

    pub(crate) fn import(&self, path: &Path) -> Result<Book> {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
        {
            let (book, chapters) = local::import_epub(path)?;
            return self.snapshot(book, chapters);
        }
        self.import_txt(path)
    }

    pub fn import_txt(&self, path: &Path) -> Result<Book> {
        let (book, chapters) = local::import_txt(path)?;
        self.snapshot(book, chapters)
    }

    pub fn demo(&self) -> Result<Book> {
        let (book, chapters) = local::demo();
        self.snapshot(book, chapters)
    }
}

fn book_id(input: &str) -> Result<String> {
    let input = input.trim();
    let valid = |id: &str| {
        !id.is_empty()
            && !id.starts_with('0')
            && id.len() <= 32
            && id.bytes().all(|c| c.is_ascii_digit())
    };
    if valid(input) {
        return Ok(input.to_owned());
    }
    let url = url::Url::parse(input)
        .map_err(|_| anyhow::anyhow!("请输入番茄书籍数字 ID 或完整书籍页面链接"))?;
    let host = url.host_str().unwrap_or_default();
    if !matches!(url.scheme(), "https" | "http")
        || !(host == "fanqienovel.com" || host.ends_with(".fanqienovel.com"))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("仅支持 fanqienovel.com 的书籍页面链接或数字书籍 ID");
    }
    let segments: Vec<_> = url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    if segments.len() == 2 && segments[0] == "page" && valid(segments[1]) {
        return Ok(segments[1].to_owned());
    }
    for (key, value) in url.query_pairs() {
        if matches!(key.as_ref(), "book_id" | "bookId") && valid(&value) {
            return Ok(value.into_owned());
        }
    }
    bail!("链接中没有有效书籍 ID；请使用 /page/数字ID 的书籍页面链接，章节链接不是书籍链接")
}

#[cfg(test)]
mod tests;
