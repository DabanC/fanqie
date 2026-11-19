//! Shelf removal and chapter-cache maintenance pickers.
use super::{
    picker::{Choice, ChoiceAction},
    source_label, App, Editor, Overlay,
};
use crate::models::{Book, Progress};

impl App {
    pub(super) fn remove_command(&mut self) {
        if !self.maintenance_ready() {
            return;
        }
        if let Some(book) = &self.current {
            self.prompt_remove_book(book.id.clone());
        } else if self.shelf.is_empty() {
            self.note("书架上没有可移除的书籍。", false);
        } else {
            self.overlay = Some(Overlay {
                title: "从书架移除".into(),
                subtitle: "输入书名筛选 · Enter 选择并查看确认 · Esc 返回".into(),
                filter: Editor::default(),
                selected: 0,
                choices: self
                    .shelf
                    .iter()
                    .map(|book| Choice {
                        title: book.title.clone(),
                        detail: format!("{} · {}", book.author, source_label(&book.source)),
                        action: ChoiceAction::RemoveBook(book.id.clone()),
                    })
                    .collect(),
            });
        }
    }

    pub(super) fn prompt_remove_book(&mut self, book_id: String) {
        if !self.maintenance_ready() {
            return;
        }
        let Some(book) = self.removal_target(&book_id) else {
            self.note("书架中已找不到这本书。", true);
            return;
        };
        let detail = match book.source.as_str() {
            "fanqie" => "删除本机章节缓存，下次阅读需重新下载；保留进度及书签。",
            "demo" => "删除离线示例快照，可用 /demo 恢复；保留进度及书签。",
            _ => "删除导入的离线快照，不影响原始文件；保留进度及书签。",
        };
        self.confirm_maintenance(
            format!("确认移除《{}》？", book.title),
            "移除书籍",
            detail,
            ChoiceAction::ConfirmRemoveBook(book_id),
        );
    }

    pub(super) fn confirm_remove_book(&mut self, book_id: String) {
        if !self.maintenance_ready() {
            return;
        }
        let Some(book) = self.removal_target(&book_id) else {
            self.note("书架中已找不到这本书。", true);
            return;
        };
        let removing_current = self.current.as_ref().is_some_and(|book| book.id == book_id);
        let progress = (removing_current && self.progress_dirty).then(|| Progress {
            chapter: self.chapter,
            fraction: self.restore_fraction.unwrap_or_else(|| self.fraction()),
        });
        match self.store.remove_shelf_book(book, progress) {
            Ok(result) => {
                if removing_current {
                    self.current = None;
                    self.chapter = 0;
                    self.content.clear();
                    self.lines.clear();
                    self.scroll = 0;
                    self.wrap_width = 0;
                    self.restore_fraction = None;
                    self.logical_fraction = 0.0;
                    self.progress_dirty = false;
                    self.command_focus = true;
                }
                self.shelf.retain(|book| book.id != book_id);
                self.home_selection = self.home_selection.min(self.shelf.len().saturating_sub(1));
                self.note(
                    if result.cleanup_pending {
                        "书籍已移除，进度和书签已保留；部分本机文件未能清理。"
                    } else {
                        "书籍已从书架移除，阅读进度及书签已保留。"
                    },
                    result.cleanup_pending,
                );
            }
            Err(error) => self.note(format!("移除书籍失败：{error:#}"), true),
        }
    }

    pub(super) fn cache_command(&mut self) {
        match self.store.chapter_cache_summary() {
            Ok(summary) => self.note(
                format!(
                    "章节缓存 · 在线 {} 章（{}） · 离线 {} 章（{}）{}",
                    summary.remote.files,
                    size_label(summary.remote.bytes),
                    summary.offline.files,
                    size_label(summary.offline.bytes),
                    skipped_label(summary.skipped),
                ),
                false,
            ),
            Err(error) => self.note(format!("无法统计章节缓存：{error:#}"), true),
        }
    }

    pub(super) fn prompt_clear_cache(&mut self) {
        if self.maintenance_ready() {
            self.confirm_maintenance(
                "确认清理在线章节缓存？".into(),
                "清理在线缓存",
                "仅移除可重新下载的在线正文；保留书架、离线快照、进度和书签。",
                ChoiceAction::ConfirmClearCache,
            );
        }
    }

    pub(super) fn confirm_clear_cache(&mut self) {
        if !self.maintenance_ready() {
            return;
        }
        match self.store.clear_remote_chapter_cache() {
            Ok(result) => self.note(
                format!(
                    "已清理 {} 章在线缓存（{}）{}。已打开的正文不受影响。",
                    result.removed.files,
                    size_label(result.removed.bytes),
                    skipped_label(result.skipped),
                ),
                false,
            ),
            Err(error) => self.note(format!("清理缓存失败：{error:#}"), true),
        }
    }

    fn removal_target(&self, id: &str) -> Option<&Book> {
        self.current
            .iter()
            .chain(&self.shelf)
            .find(|book| book.id == id)
    }

    fn maintenance_ready(&mut self) -> bool {
        if self.background_busy() {
            self.note(
                "仍有后台操作在进行，请等待完成后再移除书籍或清理缓存。",
                false,
            );
            return false;
        }
        true
    }

    fn confirm_maintenance(
        &mut self,
        title: String,
        label: &str,
        detail: &str,
        action: ChoiceAction,
    ) {
        self.overlay = Some(Overlay {
            title,
            // Put the effect in the subtitle so it is visible even with Cancel selected.
            subtitle: format!("{detail} ↓ 选择确认 · Esc 取消"),
            filter: Editor::default(),
            selected: 0,
            choices: vec![
                Choice {
                    title: "取消".into(),
                    detail: "不修改本机数据".into(),
                    action: ChoiceAction::Cancel,
                },
                Choice {
                    title: label.into(),
                    detail: detail.into(),
                    action,
                },
            ],
        });
    }
}

fn size_label(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * KIB;
    const GIB: u64 = MIB * KIB;
    match bytes {
        GIB.. => format!("{:.1} GiB", bytes as f64 / GIB as f64),
        MIB.. => format!("{:.1} MiB", bytes as f64 / MIB as f64),
        KIB.. => format!("{:.1} KiB", bytes as f64 / KIB as f64),
        _ => format!("{bytes} B"),
    }
}

fn skipped_label(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(" · 跳过 {count} 个无法确认或处理的项目")
    }
}

#[cfg(test)]
mod tests;
