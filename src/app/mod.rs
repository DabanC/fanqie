//! Application state and background-job coordination.
mod commands;
mod editor;
mod input;
mod maintenance;
mod picker;
mod reader;
mod reading_tools;
mod search;
mod tasks;

pub(crate) use commands::COMMANDS;
pub(crate) use editor::Editor;
pub(crate) use picker::Overlay;

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

use anyhow::Result;

use crate::{
    models::{Book, SearchReport, Settings},
    net::Cancellation,
    storage::Store,
};

enum Outcome {
    Book(Book),
    Catalog(Book),
    Results(SearchReport),
    Prefetched(String),
    Chapter {
        book: Book,
        index: usize,
        text: String,
        fraction: f64,
    },
}
struct Message {
    generation: u64,
    result: Result<Outcome>,
}

pub struct App {
    pub store: Store,
    pub settings: Settings,
    pub shelf: Vec<Book>,
    pub current: Option<Book>,
    pub chapter: usize,
    pub content: String,
    pub lines: Vec<String>,
    pub scroll: usize,
    pub viewport: usize,
    pub wrap_width: usize,
    pub input: Editor,
    pub command_focus: bool,
    pub overlay: Option<Overlay>,
    pub home_selection: usize,
    pub status: String,
    pub is_error: bool,
    pub busy: Option<String>,
    pub tick: usize,
    pub quit: bool,
    generation: u64,
    cancellation: Option<Cancellation>,
    inflight: usize,
    prefetch_enabled: bool,
    prefetch_attempted: Option<(String, String)>,
    prefetch: Option<tasks::Prefetch>,
    search_results: Option<SearchReport>,
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    restore_fraction: Option<f64>,
    logical_fraction: f64,
    progress_dirty: bool,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl App {
    pub fn new(store: Store) -> Result<Self> {
        let settings = store.settings()?;
        let shelf = store.books()?;
        let warnings = store.take_warnings();
        let (sender, receiver) = mpsc::channel();
        Ok(Self {
            store,
            settings,
            shelf,
            current: None,
            chapter: 0,
            content: String::new(),
            lines: vec![],
            scroll: 0,
            viewport: 1,
            wrap_width: 0,
            input: Editor::default(),
            command_focus: true,
            overlay: None,
            home_selection: 0,
            status: if warnings.is_empty() {
                "输入书名直接在线搜索，或粘贴番茄书籍链接。/ 查看全部命令。".into()
            } else {
                warnings.join("；")
            },
            is_error: !warnings.is_empty(),
            busy: None,
            tick: 0,
            quit: false,
            generation: 0,
            cancellation: None,
            inflight: 0,
            prefetch_enabled: false,
            prefetch_attempted: None,
            prefetch: None,
            search_results: None,
            sender,
            receiver,
            restore_fraction: None,
            logical_fraction: 0.0,
            progress_dirty: false,
            history: vec![],
            history_index: None,
        })
    }
    pub fn note(&mut self, message: impl Into<String>, error: bool) {
        self.status = message.into();
        self.is_error = error;
    }
    pub fn demo(&mut self) {
        self.job("打开离线示例", |library| {
            library.demo().map(Outcome::Book)
        });
    }
    pub fn import(&mut self, path: PathBuf) {
        self.job("导入本地小说", move |library| {
            library.import(&path).map(Outcome::Book)
        });
    }
    pub fn open_remote(&mut self, input: String) {
        self.job("获取书籍与章节目录", move |library| {
            library.open_remote(&input).map(Outcome::Book)
        });
    }
    pub fn poll_results(&mut self) {
        while let Some(result) = self.next_job_result() {
            match result {
                Ok(Outcome::Prefetched(_)) => {}
                Ok(Outcome::Book(book)) => {
                    self.refresh_shelf();
                    if let Some(current) = self
                        .current
                        .as_mut()
                        .filter(|current| current.id == book.id)
                    {
                        current.title.clone_from(&book.title);
                    }
                    self.begin_book(book);
                }
                Ok(Outcome::Catalog(book)) => {
                    if let Err(error) = self.apply_catalog(book) {
                        self.note(format!("刷新目录失败，仍使用原目录：{error:#}"), true);
                    }
                }
                Ok(Outcome::Results(report)) => self.apply_search_results(report),
                Ok(Outcome::Chapter {
                    book,
                    index,
                    text,
                    fraction,
                }) => self.apply_chapter(book, index, text, fraction),
                Err(error) => self.note(format!("{error:#}"), true),
            }
        }
    }
    fn refresh_shelf(&mut self) {
        if let Ok(books) = self.store.books() {
            self.shelf = books;
        }
        self.home_selection = self.home_selection.min(self.shelf.len().saturating_sub(1));
    }
}

pub fn source_label(source: &str) -> &'static str {
    match source {
        "demo" => "离线示例",
        "local" => "本地导入",
        _ => "番茄在线",
    }
}

#[cfg(test)]
mod tests;
