//! Per-book bookmarks, separate from the existing reading-progress record.
use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};

use super::{valid_id, Store};

const MAX_BOOKMARKS: usize = 4096;
const MAX_LABEL_BYTES: usize = 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Bookmark {
    pub(crate) id: String,
    pub(crate) chapter_id: String,
    pub(crate) fraction: f64,
    pub(crate) label: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedBookmarks {
    book_id: String,
    entries: Vec<Bookmark>,
}

impl Bookmark {
    fn validate(&self) -> Result<()> {
        validate_bookmark_id(&self.id)?;
        valid_id(&self.chapter_id)?;
        ensure!(
            (0.0..=1.0).contains(&self.fraction),
            "书签进度必须是 0 到 1 之间的有限数值"
        );
        ensure!(
            !self.label.trim().is_empty()
                && self.label.len() <= MAX_LABEL_BYTES
                && !self.label.chars().any(char::is_control),
            "书签名称为空、过长或含控制字符"
        );
        Ok(())
    }
}

fn validate_bookmark_id(id: &str) -> Result<()> {
    ensure!(
        id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "无效的书签标识"
    );
    Ok(())
}

impl Store {
    fn bookmarks_path(&self, book_id: &str) -> Result<PathBuf> {
        Ok(self.book_dir(book_id)?.join("bookmarks.json"))
    }

    pub(crate) fn bookmarks(&self, book_id: &str) -> Result<Vec<Bookmark>> {
        let Some(saved): Option<SavedBookmarks> = Self::read(&self.bookmarks_path(book_id)?)?
        else {
            return Ok(Vec::new());
        };
        ensure!(saved.book_id == book_id, "书签记录的书籍标识不匹配");
        ensure!(saved.entries.len() <= MAX_BOOKMARKS, "书签数量过多");
        let mut seen = HashSet::new();
        for entry in &saved.entries {
            entry.validate()?;
            ensure!(seen.insert(&entry.id), "书签记录含重复标识");
        }
        Ok(saved.entries)
    }

    pub(crate) fn add_bookmark(
        &self,
        book_id: &str,
        chapter_id: &str,
        fraction: f64,
        label: &str,
    ) -> Result<()> {
        let mut entries = self.bookmarks(book_id)?;
        ensure!(entries.len() < MAX_BOOKMARKS, "本书的书签已达上限");
        let id = format!("{:032x}", rand::random::<u128>());
        if entries.iter().any(|entry| entry.id == id) {
            bail!("无法分配书签标识，请重试");
        }
        let entry = Bookmark {
            id,
            chapter_id: chapter_id.to_owned(),
            fraction,
            label: label.to_owned(),
        };
        entry.validate()?;
        entries.push(entry);
        self.write_bookmarks(book_id, entries)
    }

    pub(crate) fn remove_bookmark(&self, book_id: &str, bookmark_id: &str) -> Result<bool> {
        validate_bookmark_id(bookmark_id)?;
        let mut entries = self.bookmarks(book_id)?;
        let Some(index) = entries.iter().position(|entry| entry.id == bookmark_id) else {
            return Ok(false);
        };
        entries.remove(index);
        self.write_bookmarks(book_id, entries)?;
        Ok(true)
    }

    fn write_bookmarks(&self, book_id: &str, entries: Vec<Bookmark>) -> Result<()> {
        Self::write(
            &self.bookmarks_path(book_id)?,
            SavedBookmarks {
                book_id: book_id.to_owned(),
                entries,
            },
        )
    }
}

#[cfg(test)]
mod tests;
