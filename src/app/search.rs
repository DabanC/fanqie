//! Search result picker and explicit, bounded pagination.
use super::{
    picker::{Choice, ChoiceAction},
    App, Editor, Outcome, Overlay,
};
use crate::models::{Book, SearchReport};

impl App {
    pub(super) fn search_more(&mut self) {
        let Some(report) = self
            .search_results
            .clone()
            .filter(|r| r.continuation.is_some())
        else {
            self.note("当前没有可加载的下一页。", false);
            return;
        };
        self.job("加载更多搜索结果", move |library| {
            library.search_more(&report).map(Outcome::Results)
        });
    }

    pub(super) fn apply_search_results(&mut self, report: SearchReport) {
        let more = report.continuation.is_some();
        let title = if report.notice.is_some() {
            "搜索结果 · 降级模式"
        } else {
            "搜索结果"
        };
        let hint = if more {
            "↑↓ 选择 · Enter 打开 · /more 加载更多 · Esc 返回"
        } else {
            "↑↓ 选择 · Enter 打开 · Esc 返回"
        };
        let mut choices: Vec<_> = report
            .books
            .iter()
            .cloned()
            .map(|book| Choice::book(book, ChoiceAction::SearchBook))
            .collect();
        if more {
            choices.push(Choice {
                title: "加载更多".into(),
                detail: "Enter 请求下一页".into(),
                action: ChoiceAction::MoreSearchResults,
            });
        }
        self.overlay = Some(Overlay {
            title: title.into(),
            subtitle: report
                .notice
                .as_ref()
                .map_or_else(|| hint.to_owned(), |notice| format!("{notice} · {hint}")),
            filter: Editor::default(),
            selected: 0,
            choices,
        });
        let count = report.books.len();
        let status = match &report.notice {
            Some(notice) => format!("{notice} · {count} 本。"),
            None => format!("找到 {count} 本书。{hint}"),
        };
        self.search_results = Some(report);
        self.note(status, false);
    }

    pub(super) fn open_search_hit(&mut self, hit: Book) {
        if hit.source != "fanqie" {
            self.begin_book(hit);
            return;
        }
        self.job("打开搜索结果", move |library| {
            library.open_search_hit(&hit).map(Outcome::Book)
        });
    }
}
