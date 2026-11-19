//! Chapter navigation, text reflow, and durable reading progress.
use anyhow::{ensure, Context, Result};

use super::{App, Outcome};
use crate::models::Book;

impl App {
    pub(super) fn begin_book(&mut self, book: Book) {
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.id == book.id)
        {
            self.command_focus = false;
            self.note("已经在阅读这本书，保留当前位置。", false);
            return;
        }
        if book.chapters.is_empty() {
            self.open_remote(book.id);
            return;
        }
        let progress = match self.store.progress(&book.id) {
            Ok(progress) => progress,
            Err(error) => {
                self.note(format!("无法读取进度：{error}"), true);
                return;
            }
        };
        let index = progress.chapter.min(book.chapters.len() - 1);
        self.load_chapter(book, index, progress.fraction);
    }
    pub(super) fn load_chapter(&mut self, book: Book, index: usize, fraction: f64) {
        if self.promote_prefetch(&book, index, fraction) {
            return;
        }
        self.job("加载章节", move |library| {
            let text = library.chapter(&book, index)?;
            Ok(Outcome::Chapter {
                book,
                index,
                text,
                fraction,
            })
        });
    }

    pub(super) fn apply_chapter(&mut self, book: Book, index: usize, text: String, fraction: f64) {
        if !self.save_progress() {
            return;
        }
        let count = book.chapters.len();
        let fraction = fraction.clamp(0.0, 1.0);
        self.current = Some(book);
        self.chapter = index;
        self.content = text;
        self.wrap_width = 0;
        self.scroll = 0;
        self.restore_fraction = Some(fraction);
        self.logical_fraction = fraction;
        self.command_focus = false;
        self.overlay = None;
        self.progress_dirty = true;
        self.note(
            format!(
                "第 {} / {count} 章 · 正文已缓存，自动保存阅读进度",
                index + 1
            ),
            false,
        );
        self.refresh_shelf();
    }

    pub fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.viewport)
    }
    pub fn fraction(&self) -> f64 {
        if self.max_scroll() == 0 {
            self.logical_fraction
        } else {
            self.scroll as f64 / self.max_scroll() as f64
        }
    }
    pub fn prepare_lines(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        let old_fraction = self.fraction();
        if self.wrap_width != width {
            self.lines = self
                .content
                .split('\n')
                .flat_map(|line| {
                    if line.trim().is_empty() {
                        vec![String::new()]
                    } else {
                        textwrap::wrap(line, width)
                            .into_iter()
                            .map(|part| part.into_owned())
                            .collect()
                    }
                })
                .collect();
            self.wrap_width = width;
            self.viewport = height.max(1);
            let fraction = self.restore_fraction.take().unwrap_or(old_fraction);
            self.logical_fraction = fraction;
            self.scroll = (fraction * self.max_scroll() as f64).round() as usize;
        } else if self.viewport != height.max(1) {
            self.viewport = height.max(1);
            self.logical_fraction = old_fraction;
            self.scroll = (old_fraction * self.max_scroll() as f64).round() as usize;
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }
    pub fn save_progress(&mut self) -> bool {
        if !self.progress_dirty || self.restore_fraction.is_some() {
            return true;
        }
        if let Some(book) = &self.current {
            match self
                .store
                .save_progress(&book.id, self.chapter, self.fraction())
            {
                Ok(()) => self.progress_dirty = false,
                Err(error) => {
                    self.note(format!("保存进度失败，当前位置仍保留：{error}"), true);
                    return false;
                }
            }
        }
        true
    }
    pub(super) fn scroll_by(&mut self, amount: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(amount)
            .min(self.max_scroll());
        self.progress_dirty = true;
    }
    pub(super) fn next_chapter(&mut self, next: bool) {
        let Some(book) = self.current.clone() else {
            self.note("请先打开一本小说。", false);
            return;
        };
        let index = if next {
            self.chapter + 1
        } else {
            self.chapter.saturating_sub(1)
        };
        if index >= book.chapters.len() || index == self.chapter {
            self.note(
                if next {
                    "已经是最后一章。"
                } else {
                    "已经是第一章。"
                },
                false,
            );
            return;
        }
        if !self.save_progress() {
            return;
        }
        self.load_chapter(book, index, 0.0);
    }
    pub(super) fn refresh_catalog(&mut self) {
        let Some(book) = &self.current else {
            self.note("请先打开一本番茄小说。", false);
            return;
        };
        if book.source != "fanqie" {
            self.note("只有番茄在线小说支持刷新目录。", false);
            return;
        }
        let id = book.id.clone();
        self.job("刷新章节目录", move |library| {
            library
                .fetch_catalog(&id)
                .map(Outcome::Catalog)
                .context("刷新目录失败，仍使用原目录")
        });
    }

    pub(super) fn apply_catalog(&mut self, catalog: Book) -> Result<()> {
        let current = self.current.as_ref().context("没有正在阅读的书籍")?;
        ensure!(
            catalog.id == current.id && catalog.source == current.source,
            "书籍标识不匹配"
        );
        let mut saved = self.store.get_book(&current.id)?.context("找不到原目录")?;
        // Progress is saved by chapter index. Only accept updates that leave every
        // known chapter at its existing index, including in a newer on-disk catalog.
        let extends = |book: &Book| {
            catalog.chapters.len() >= book.chapters.len()
                && catalog
                    .chapters
                    .iter()
                    .zip(&book.chapters)
                    .all(|(new, old)| new.id == old.id)
        };
        ensure!(
            extends(current) && extends(&saved) && !catalog.chapters.is_empty(),
            "上游目录缩短或调整了章节顺序，为保留阅读进度暂不更新"
        );
        let added = catalog.chapters.len() - current.chapters.len();
        if saved.chapters != catalog.chapters {
            saved.chapters = catalog.chapters;
            self.store.save_book(&saved).context("保存目录失败")?;
        }
        self.current = Some(saved);
        self.refresh_shelf();
        self.refresh_open_toc();
        self.note(
            if added == 0 {
                "目录已刷新，暂无新章节。".into()
            } else {
                format!("目录已刷新，新增 {added} 章。")
            },
            false,
        );
        Ok(())
    }

    pub(super) fn resume(&mut self) {
        match self.store.last_book().and_then(|id| match id {
            Some(id) => self.store.get_book(&id),
            None => Ok(None),
        }) {
            Ok(Some(book)) => self.begin_book(book),
            Ok(None) => self.note("还没有阅读记录。试试 /demo 或 /import 文件路径。", false),
            Err(error) => self.note(format!("读取记录失败：{error}"), true),
        }
    }
}
