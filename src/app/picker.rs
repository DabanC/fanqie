//! Search, shelf, chapter, and command pickers.
use crossterm::event::KeyCode;

use super::{source_label, App, Editor, COMMANDS};
use crate::models::Book;

#[derive(Clone)]
pub enum ChoiceAction {
    Book(Book),
    SearchBook(Book),
    MoreSearchResults,
    RemoveBook(String),
    ConfirmRemoveBook(String),
    ConfirmClearCache,
    Cancel,
    JumpBookmark {
        book_id: String,
        bookmark_id: String,
    },
    RemoveBookmark {
        book_id: String,
        bookmark_id: String,
    },
    FindMatch {
        book_id: String,
        chapter_id: String,
        offset: usize,
    },
    Chapter(usize),
    Command(String),
}
#[derive(Clone)]
pub struct Choice {
    pub title: String,
    pub detail: String,
    pub action: ChoiceAction,
}

impl Choice {
    pub(super) fn book(book: Book, action: fn(Book) -> ChoiceAction) -> Self {
        let author = if book.author.is_empty() {
            "佚名"
        } else {
            &book.author
        };
        Self {
            title: book.title.clone(),
            detail: format!("{author} · {}", source_label(&book.source)),
            action: action(book),
        }
    }
}

pub struct Overlay {
    pub title: String,
    pub subtitle: String,
    pub filter: Editor,
    pub choices: Vec<Choice>,
    pub selected: usize,
}
impl Overlay {
    pub fn filtered(&self) -> Vec<usize> {
        let needle = self.filter.text.to_lowercase();
        self.choices
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                (needle.is_empty()
                    || matches!(row.action, ChoiceAction::MoreSearchResults)
                    || format!("{} {}", row.title, row.detail)
                        .to_lowercase()
                        .contains(&needle))
                .then_some(i)
            })
            .collect()
    }
}

impl App {
    pub(super) fn show_books(&mut self, title: &str, books: Vec<Book>) {
        let choices = books
            .into_iter()
            .map(|book| Choice::book(book, ChoiceAction::Book))
            .collect();
        self.overlay = Some(Overlay {
            title: title.into(),
            subtitle: "输入书名筛选 · ↑↓ 选择 · Enter 打开 · Esc 返回".into(),
            filter: Editor::default(),
            choices,
            selected: 0,
        });
    }
    pub(super) fn show_toc(&mut self) {
        let Some(book) = &self.current else {
            self.note("请先打开一本小说。", false);
            return;
        };
        self.overlay = Some(Overlay {
            title: format!("章节目录 / {}", book.title),
            subtitle: "输入标题或序号筛选 · ↑↓ 选择 · Enter 跳转".into(),
            filter: Editor::default(),
            selected: self.chapter,
            choices: book
                .chapters
                .iter()
                .enumerate()
                .map(|(index, chapter)| Choice {
                    title: format!("{:>4}  {}", index + 1, chapter.title),
                    detail: if index == self.chapter {
                        "正在阅读".into()
                    } else {
                        String::new()
                    },
                    action: ChoiceAction::Chapter(index),
                })
                .collect(),
        });
    }
    pub(super) fn refresh_open_toc(&mut self) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        if !overlay
            .choices
            .first()
            .is_some_and(|choice| matches!(choice.action, ChoiceAction::Chapter(_)))
        {
            return;
        }
        let filter = std::mem::take(&mut overlay.filter);
        let selected = overlay.selected;
        self.show_toc();
        if let Some(overlay) = &mut self.overlay {
            overlay.filter = filter;
            overlay.selected = selected.min(overlay.filtered().len().saturating_sub(1));
        }
    }

    pub(super) fn show_commands(&mut self) {
        self.overlay = Some(Overlay {
            title: "命令面板".into(),
            subtitle: "输入关键词筛选 · Enter 填入命令 · Esc 返回".into(),
            filter: Editor::default(),
            selected: 0,
            choices: COMMANDS
                .iter()
                .map(|(command, detail)| Choice {
                    title: command.trim().into(),
                    detail: (*detail).into(),
                    action: ChoiceAction::Command((*command).into()),
                })
                .collect(),
        });
    }
    pub(super) fn overlay_key(&mut self, code: KeyCode) {
        let overlay = self.overlay.as_mut().unwrap();
        let filtered = overlay.filtered();
        match code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Up => overlay.selected = overlay.selected.saturating_sub(1),
            KeyCode::Down => {
                overlay.selected = (overlay.selected + 1).min(filtered.len().saturating_sub(1))
            }
            KeyCode::PageDown => {
                overlay.selected = (overlay.selected + 8).min(filtered.len().saturating_sub(1))
            }
            KeyCode::PageUp => overlay.selected = overlay.selected.saturating_sub(8),
            KeyCode::Enter => {
                let action = filtered
                    .get(overlay.selected)
                    .map(|&i| overlay.choices[i].action.clone());
                if let Some(action) = action {
                    self.activate_choice(action);
                }
            }
            _ => {
                overlay.filter.key(code);
                overlay.selected = 0;
            }
        }
    }

    fn activate_choice(&mut self, action: ChoiceAction) {
        if !matches!(action, ChoiceAction::MoreSearchResults) {
            self.overlay = None;
        }
        match action {
            ChoiceAction::Book(book) => self.begin_book(book),
            ChoiceAction::SearchBook(book) => self.open_search_hit(book),
            ChoiceAction::MoreSearchResults => self.search_more(),
            ChoiceAction::RemoveBook(id) => self.prompt_remove_book(id),
            ChoiceAction::ConfirmRemoveBook(id) => self.confirm_remove_book(id),
            ChoiceAction::ConfirmClearCache => self.confirm_clear_cache(),
            ChoiceAction::JumpBookmark {
                book_id,
                bookmark_id,
            } => self.jump_bookmark(&book_id, &bookmark_id),
            ChoiceAction::RemoveBookmark {
                book_id,
                bookmark_id,
            } => self.remove_bookmark(&book_id, &bookmark_id),
            ChoiceAction::FindMatch {
                book_id,
                chapter_id,
                offset,
            } => self.jump_to_match(&book_id, &chapter_id, offset),
            ChoiceAction::Chapter(index) => {
                if let Some(book) = self.current.clone() {
                    if self.save_progress() {
                        self.load_chapter(book, index, 0.0);
                    }
                }
            }
            ChoiceAction::Cancel => {}
            ChoiceAction::Command(command) => {
                self.command_focus = true;
                self.input.set(command);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlay_filter_matches_chinese() {
        let overlay = Overlay {
            title: String::new(),
            subtitle: String::new(),
            filter: Editor {
                text: "章节".into(),
                cursor: 6,
            },
            selected: 0,
            choices: vec![Choice {
                title: "章节目录".into(),
                detail: String::new(),
                action: ChoiceAction::Chapter(0),
            }],
        };
        assert_eq!(overlay.filtered(), vec![0]);
    }
}
