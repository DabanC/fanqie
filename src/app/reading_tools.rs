//! Bookmark and current-chapter search pickers.
use std::collections::HashMap;

use regex::RegexBuilder;
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use super::{
    picker::{Choice, ChoiceAction},
    App, Editor, Overlay,
};

const MAX_MATCHES: usize = 250;
const MAX_QUERY_BYTES: usize = 1024;
const EXCERPT_BYTES: usize = 256;

impl App {
    pub(super) fn add_bookmark(&mut self, label: &str) {
        let Some(book) = &self.current else {
            self.note("请先打开一本小说。", false);
            return;
        };
        let Some(chapter) = book.chapters.get(self.chapter) else {
            self.note("当前章节不可用。", true);
            return;
        };
        let fraction = self.restore_fraction.unwrap_or_else(|| self.fraction());
        let label = if label.trim().is_empty() {
            format!("第 {} 章 · {:.0}%", self.chapter + 1, fraction * 100.0)
        } else {
            label.trim().to_owned()
        };
        match self
            .store
            .add_bookmark(&book.id, &chapter.id, fraction, &label)
        {
            Ok(()) => self.note(format!("已添加书签：{label}"), false),
            Err(error) => self.note(format!("添加书签失败：{error}"), true),
        }
    }

    pub(super) fn show_bookmarks(&mut self, remove: bool) {
        let Some(book) = &self.current else {
            self.note("请先打开一本小说。", false);
            return;
        };
        let bookmarks = match self.store.bookmarks(&book.id) {
            Ok(bookmarks) if bookmarks.is_empty() => {
                self.note("本书还没有书签。试试 /bookmark。", false);
                return;
            }
            Ok(bookmarks) => bookmarks,
            Err(error) => {
                self.note(format!("读取书签失败：{error}"), true);
                return;
            }
        };
        let titles: HashMap<_, _> = book
            .chapters
            .iter()
            .map(|chapter| (chapter.id.as_str(), chapter.title.as_str()))
            .collect();
        let choices = bookmarks
            .into_iter()
            .rev()
            .map(|bookmark| Choice {
                title: bookmark.label,
                detail: format!(
                    "{} · {:.0}%",
                    titles
                        .get(bookmark.chapter_id.as_str())
                        .copied()
                        .unwrap_or("章节已不在目录中"),
                    bookmark.fraction * 100.0
                ),
                action: if remove {
                    ChoiceAction::RemoveBookmark {
                        book_id: book.id.clone(),
                        bookmark_id: bookmark.id,
                    }
                } else {
                    ChoiceAction::JumpBookmark {
                        book_id: book.id.clone(),
                        bookmark_id: bookmark.id,
                    }
                },
            })
            .collect();
        let (title, subtitle) = if remove {
            ("移除书签", "输入关键词筛选 · Enter 移除所选书签 · Esc 返回")
        } else {
            ("书签", "输入关键词筛选 · Enter 跳转 · Esc 返回")
        };
        self.overlay = Some(Overlay {
            title: format!("{title} / {}", book.title),
            subtitle: subtitle.into(),
            filter: Editor::default(),
            choices,
            selected: 0,
        });
    }

    pub(super) fn jump_bookmark(&mut self, book_id: &str, bookmark_id: &str) {
        let Some(book) = self.current.as_ref().filter(|book| book.id == book_id) else {
            self.note("此书签不属于正在阅读的小说。", true);
            return;
        };
        let bookmark = match self.store.bookmarks(book_id) {
            Ok(bookmarks) => bookmarks.into_iter().find(|entry| entry.id == bookmark_id),
            Err(error) => {
                self.note(format!("读取书签失败：{error}"), true);
                return;
            }
        };
        let Some(bookmark) = bookmark else {
            self.note("这个书签已不存在。", false);
            return;
        };
        let Some(index) = book
            .chapters
            .iter()
            .position(|chapter| chapter.id == bookmark.chapter_id)
        else {
            self.note("书签所在的章节已不在目录中。", true);
            return;
        };
        if self.busy.is_some() {
            self.note("正在处理上一项操作，请稍候。", false);
            return;
        }
        if index != self.chapter {
            let book = book.clone();
            if self.save_progress() {
                self.load_chapter(book, index, bookmark.fraction);
            }
            return;
        }
        self.logical_fraction = bookmark.fraction;
        if self.wrap_width == 0 {
            self.restore_fraction = Some(bookmark.fraction);
        } else {
            self.scroll = (bookmark.fraction * self.max_scroll() as f64).round() as usize;
        }
        self.progress_dirty = true;
        self.command_focus = false;
        self.note(format!("已跳转到书签：{}", bookmark.label), false);
    }

    pub(super) fn remove_bookmark(&mut self, book_id: &str, bookmark_id: &str) {
        match self.store.remove_bookmark(book_id, bookmark_id) {
            Ok(true) => self.note("已移除书签。", false),
            Ok(false) => self.note("这个书签已不存在。", false),
            Err(error) => self.note(format!("移除书签失败：{error}"), true),
        }
    }

    pub(super) fn find_in_chapter(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() || query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control)
        {
            self.note("请输入有效的查找词，例如 /find 夜色。", true);
            return;
        }
        let Some(book) = &self.current else {
            self.note("请先打开一本小说。", false);
            return;
        };
        let Some(chapter) = book.chapters.get(self.chapter) else {
            self.note("当前章节不可用。", true);
            return;
        };
        // Match the original text so case folding cannot change source offsets.
        let Ok(pattern) = RegexBuilder::new(&regex::escape(query))
            .case_insensitive(true)
            .build()
        else {
            self.note("查找词过于复杂。", true);
            return;
        };
        let mut matches = pattern.find_iter(&self.content);
        let choices: Vec<_> = matches
            .by_ref()
            .take(MAX_MATCHES)
            .enumerate()
            .map(|(index, found)| Choice {
                title: format!("匹配 {}", index + 1),
                detail: excerpt(&self.content, found.start()),
                action: ChoiceAction::FindMatch {
                    book_id: book.id.clone(),
                    chapter_id: chapter.id.clone(),
                    offset: found.start(),
                },
            })
            .collect();
        if choices.is_empty() {
            self.note("本章没有找到匹配内容。", false);
            return;
        }
        let subtitle = if matches.next().is_some() {
            format!("仅显示前 {MAX_MATCHES} 处，请缩小查找范围 · Enter 跳转 · Esc 返回")
        } else {
            "输入关键词筛选片段 · Enter 跳转 · Esc 返回".into()
        };
        self.overlay = Some(Overlay {
            title: format!("本章查找 / {}", chapter.title),
            subtitle,
            filter: Editor::default(),
            choices,
            selected: 0,
        });
    }

    pub(super) fn jump_to_match(&mut self, book_id: &str, chapter_id: &str, offset: usize) {
        let same_chapter = self.current.as_ref().is_some_and(|book| {
            book.id == book_id
                && book
                    .chapters
                    .get(self.chapter)
                    .is_some_and(|chapter| chapter.id == chapter_id)
        });
        if !same_chapter || offset >= self.content.len() || !self.content.is_char_boundary(offset) {
            self.note("查找结果已失效，请在本章重新查找。", true);
            return;
        }
        if self.wrap_width == 0 {
            self.prepare_lines(self.settings.reader_width as usize, self.viewport);
        }
        let row = wrapped_row_at(&self.content, offset, self.wrap_width);
        let max_scroll = self.max_scroll();
        self.scroll = row.min(max_scroll);
        self.logical_fraction = if max_scroll == 0 {
            let last_row = self.lines.len().saturating_sub(1);
            row.min(last_row) as f64 / last_row.max(1) as f64
        } else {
            self.fraction()
        };
        self.progress_dirty = true;
        self.command_focus = false;
        self.note("已跳转到查找结果。", false);
    }
}

fn wrapped_row_at(content: &str, mut offset: usize, width: usize) -> usize {
    // Preserve spaces omitted by the renderer; line breaks stay identical and
    // each resulting slice accounts for its exact number of source bytes.
    let options = textwrap::Options::new(width).preserve_trailing_space(true);
    let mut row = 0;
    for paragraph in content.split('\n') {
        if paragraph.trim().is_empty() {
            if offset <= paragraph.len() {
                return row;
            }
            row += 1;
        } else {
            let lines = textwrap::wrap(paragraph, &options);
            if offset <= paragraph.len() {
                let mut end = 0;
                for (index, line) in lines.iter().enumerate() {
                    end += line.len();
                    if offset < end {
                        return row + index;
                    }
                }
                return row + lines.len().saturating_sub(1);
            }
            row += lines.len();
        }
        offset -= paragraph.len() + 1;
    }
    row.saturating_sub(1)
}

fn excerpt(content: &str, offset: usize) -> String {
    let mut start = offset;
    let mut cursor = GraphemeCursor::new(offset, content.len(), true);
    for _ in 0..6 {
        let Ok(Some(previous)) = cursor.prev_boundary(content, 0) else {
            break;
        };
        if content[previous..start].contains('\n') || offset - previous > EXCERPT_BYTES / 4 {
            break;
        }
        start = previous;
    }
    let mut result = String::new();
    if start > 0 && content.as_bytes()[start - 1] != b'\n' {
        result.push('…');
    }
    for (index, cluster) in content[start..].graphemes(true).enumerate() {
        if cluster.contains('\n') {
            break;
        }
        if index == 36 || result.len() + cluster.len() > EXCERPT_BYTES {
            result.push('…');
            break;
        }
        result.extend(cluster.chars().filter_map(|ch| {
            if ch.is_whitespace() {
                Some(' ')
            } else if ch.is_control() {
                None
            } else {
                Some(ch)
            }
        }));
    }
    result
}

#[cfg(test)]
mod tests;
