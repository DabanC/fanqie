//! Keyboard routing, paste handling, and command history.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{App, COMMANDS};

impl App {
    pub fn paste(&mut self, text: &str) {
        if let Some(overlay) = &mut self.overlay {
            overlay.filter.insert(text);
            overlay.selected = 0;
        } else {
            self.command_focus = true;
            self.input.insert(text);
        }
    }
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q') | KeyCode::Char('c') => {
                    self.quit = true;
                    return;
                }
                KeyCode::Char('x') => {
                    self.cancel_loading();
                    return;
                }
                KeyCode::Char('k') => {
                    self.show_commands();
                    return;
                }
                KeyCode::Char('b') => {
                    self.show_books("我的书架", self.shelf.clone());
                    return;
                }
                KeyCode::Char('t') => {
                    self.show_toc();
                    return;
                }
                KeyCode::Char('o') => {
                    self.overlay = None;
                    self.command_focus = true;
                    self.input.set("/import ");
                    return;
                }
                KeyCode::Char('l') => {
                    self.input.set("");
                    return;
                }
                _ => return,
            }
        }
        if self.overlay.is_some() {
            self.overlay_key(key.code);
            return;
        }
        if key.code == KeyCode::Tab && !self.command_focus {
            self.command_focus = true;
            return;
        }
        if key.code == KeyCode::Esc {
            self.command_focus = !self.command_focus;
            return;
        }
        if self.command_focus {
            match key.code {
                KeyCode::Enter => {
                    let command = std::mem::take(&mut self.input.text).trim().to_owned();
                    self.input.cursor = 0;
                    self.history_index = None;
                    if command.trim().is_empty() {
                        self.command_focus = false;
                    } else {
                        let token = command.split_whitespace().next().unwrap_or_default();
                        if !matches!(token, "/api" | "/search-api") {
                            if self.history.last() != Some(&command) {
                                self.history.push(command.clone());
                            }
                            if self.history.len() > 100 {
                                self.history.remove(0);
                            }
                        }
                        self.execute(command.trim());
                    }
                }
                KeyCode::Tab => {
                    if self.input.text.is_empty() {
                        self.command_focus = false;
                    } else {
                        let matches: Vec<_> = COMMANDS
                            .iter()
                            .filter(|(cmd, _)| cmd.starts_with(&self.input.text))
                            .collect();
                        if matches.len() == 1 {
                            self.input.set(matches[0].0);
                        } else {
                            self.show_commands();
                        }
                    }
                }
                KeyCode::Up => self.history_move(false),
                KeyCode::Down => self.history_move(true),
                _ => self.input.key(key.code),
            }
            return;
        }
        match key.code {
            KeyCode::Char('/') => {
                self.command_focus = true;
                self.input.set("/");
            }
            KeyCode::Char(':') => {
                self.command_focus = true;
                self.input.set("");
            }
            KeyCode::Right | KeyCode::Char('n') => self.next_chapter(true),
            KeyCode::Left | KeyCode::Char('p') => self.next_chapter(false),
            KeyCode::Char('t') => self.show_toc(),
            KeyCode::Char('?') => self.show_commands(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('r') if self.current.is_none() => self.resume(),
            KeyCode::Char('d') if self.current.is_none() => self.demo(),
            KeyCode::Down | KeyCode::Char('j') if self.current.is_none() => {
                self.home_selection =
                    (self.home_selection + 1).min(self.shelf.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') if self.current.is_none() => {
                self.home_selection = self.home_selection.saturating_sub(1);
            }
            KeyCode::Enter if self.current.is_none() => {
                if let Some(book) = self.shelf.get(self.home_selection).cloned() {
                    self.begin_book(book);
                } else {
                    self.demo();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
            KeyCode::Char(' ') if self.current.is_some() && self.wrap_width == 0 => {}
            KeyCode::Char(' ') if self.current.is_some() && self.scroll >= self.max_scroll() => {
                if key.kind == KeyEventKind::Press && self.busy.is_none() {
                    self.next_chapter(true);
                }
            }
            KeyCode::PageDown | KeyCode::Char(' ') | KeyCode::Char('f') => {
                self.scroll_by(self.viewport.saturating_sub(2).max(1) as isize);
            }
            KeyCode::PageUp | KeyCode::Char('b') => {
                self.scroll_by(-(self.viewport.saturating_sub(2).max(1) as isize))
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll = 0;
                self.logical_fraction = 0.0;
                self.progress_dirty = true;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.scroll = self.max_scroll();
                self.logical_fraction = 1.0;
                self.progress_dirty = true;
            }
            _ => {}
        }
    }
    fn history_move(&mut self, next: bool) {
        if self.history.is_empty() {
            return;
        }
        let index = match (self.history_index, next) {
            (None, false) => self.history.len() - 1,
            (None, true) => return,
            (Some(index), false) => index.saturating_sub(1),
            (Some(index), true) => index + 1,
        };
        if index >= self.history.len() {
            self.history_index = None;
            self.input.set("");
        } else {
            self.history_index = Some(index);
            self.input.set(self.history[index].clone());
        }
    }
}
