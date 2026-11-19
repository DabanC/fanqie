use std::collections::HashSet;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::storage::valid_id;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Chapter {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub description: String,
    pub source: String,
    pub chapters: Vec<Chapter>,
}

#[derive(Clone, Debug, Default)]
pub struct SearchReport {
    pub books: Vec<Book>,
    pub notice: Option<String>,
    pub continuation: Option<SearchContinuation>,
    pub(crate) cache_aliases: Vec<Book>,
}

impl SearchReport {
    pub fn add_notice(&mut self, message: impl AsRef<str>) {
        let message = message.as_ref();
        self.notice = Some(match self.notice.take() {
            Some(previous) => format!("{previous}；{message}"),
            None => message.to_owned(),
        });
    }
}

/// In-memory state for the next page of a signed search.
#[derive(Clone)]
pub struct SearchContinuation {
    pub(crate) query: String,
    pub(crate) device_id: String,
    pub(crate) install_id: String,
    // Includes the initial cursor and the cursor to request next.
    pub(crate) cursors: Vec<String>,
}

impl std::fmt::Debug for SearchContinuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchContinuation").finish_non_exhaustive()
    }
}

impl Book {
    pub fn validate(&self) -> Result<()> {
        valid_id(&self.id)?;
        if self.title.trim().is_empty()
            || self.title.len() > 8192
            || self.title.chars().any(char::is_control)
        {
            bail!("书名为空、过长或含控制字符");
        }
        if self.author.len() > 8192 || self.author.chars().any(char::is_control) {
            bail!("作者字段无效");
        }
        if self.description.len() > 1024 * 1024 || self.description.contains('\0') {
            bail!("书籍简介过长或含无效字符");
        }
        if !matches!(self.source.as_str(), "local" | "demo" | "fanqie") {
            bail!("未知书籍来源");
        }
        if self.chapters.len() > 200_000 {
            bail!("章节目录过大");
        }
        let mut seen = HashSet::new();
        for chapter in &self.chapters {
            valid_id(&chapter.id)?;
            if chapter.title.trim().is_empty()
                || chapter.title.len() > 8192
                || chapter.title.chars().any(char::is_control)
            {
                bail!("章节标题无效");
            }
            if !seen.insert(&chapter.id) {
                bail!("章节目录含重复标识");
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub chapter: usize,
    pub fraction: f64,
}

impl Progress {
    pub fn validate(&self) -> Result<()> {
        if !self.fraction.is_finite() || !(0.0..=1.0).contains(&self.fraction) {
            bail!("阅读进度必须是 0 到 1 之间的有限数值");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub content_api: String,
    pub search_api: String,
    pub reader_width: u16,
    pub light_theme: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            content_api: String::new(),
            search_api: String::new(),
            reader_width: 88,
            light_theme: false,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        validate_api(&self.content_api, &["chapter_id", "book_id"])?;
        validate_api(&self.search_api, &["query"])?;
        if !(40..=120).contains(&self.reader_width) {
            bail!("正文宽度必须在 40 到 120 之间");
        }
        Ok(())
    }
}

pub(crate) fn validate_api(api: &str, allowed: &[&str]) -> Result<()> {
    if api.is_empty() {
        return Ok(());
    }
    if api.len() > 8192 || api.chars().any(char::is_whitespace) || api.chars().any(char::is_control)
    {
        bail!("API 地址无效：应为完整 HTTP(S) URL，不可包含空白或控制字符");
    }
    let mut rest = api;
    while let Some(position) = rest.find(['{', '}']) {
        if rest.as_bytes()[position] != b'{' {
            bail!("API URL 模板括号不匹配");
        }
        rest = &rest[position + 1..];
        let Some(end) = rest.find('}') else {
            bail!("API URL 模板括号不匹配")
        };
        if !allowed.contains(&&rest[..end]) {
            bail!("API URL 模板包含不支持的占位符");
        }
        rest = &rest[end + 1..];
    }
    let parsed = url::Url::parse(api).map_err(|_| anyhow::anyhow!("API 地址不是有效 URL"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        bail!("API 只支持 HTTP(S)，不可包含 URL 用户名、密码或片段；密钥可放在查询参数中");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_urls_and_templates_without_exposing_tokens() {
        for value in [
            "file:///tmp/key",
            "https://user:secret@host/api",
            "https://host/{other}",
            "https://host/{query",
            "https://host/?q=}",
            "https://host/#token",
            "https://host/\nsecret",
        ] {
            let error = validate_api(value, &["query"]).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
        validate_api(
            "http://127.0.0.1:8888/api/search?q={query}&key=private",
            &["query"],
        )
        .unwrap();
    }

    #[test]
    fn rejects_bad_progress_and_settings() {
        for fraction in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(Progress {
                chapter: 0,
                fraction
            }
            .validate()
            .is_err());
        }
        assert!(Settings {
            reader_width: 0,
            ..Settings::default()
        }
        .validate()
        .is_err());
        assert!(serde_json::from_str::<Settings>(r#"{"unknown_secret":"private"}"#).is_err());
        Settings::default().validate().unwrap();
    }

    #[test]
    fn rejects_empty_and_duplicate_identifiers() {
        let mut book = Book {
            id: "book".into(),
            title: "Title".into(),
            author: String::new(),
            description: String::new(),
            source: "local".into(),
            chapters: vec![
                Chapter {
                    id: "c".into(),
                    title: "Chapter".into()
                };
                2
            ],
        };
        assert!(book.validate().is_err());
        book.chapters.pop();
        book.validate().unwrap();
        book.id = "\n".into();
        assert!(book.validate().is_err());
    }
}
