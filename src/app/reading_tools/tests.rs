use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::{
    models::{Book, Chapter},
    storage::Store,
};

fn reader(content: &str) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let book = Book {
        id: "book".into(),
        title: "夜读".into(),
        author: String::new(),
        description: String::new(),
        source: "local".into(),
        chapters: ["甲", "乙", "丙"]
            .into_iter()
            .map(|id| Chapter {
                id: id.into(),
                title: format!("章节 {id}"),
            })
            .collect(),
    };
    store.save_book(&book).unwrap();
    store.cache_chapter(&book.id, "甲", content).unwrap();
    store.cache_chapter(&book.id, "乙", "别章独有文字").unwrap();
    let mut app = App::new(store).unwrap();
    app.current = Some(book);
    app.content = content.into();
    (dir, app)
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn bookmark_defaults_to_the_pending_fraction_and_restores_it_in_place() {
    let (_dir, mut app) = reader(&"米\n".repeat(16));
    app.restore_fraction = Some(0.5);
    app.execute("/bookmark");
    let saved = app.store.bookmarks("book").unwrap();
    assert_eq!(saved[0].chapter_id, "甲");
    assert_eq!(saved[0].fraction, 0.5);
    assert!(saved[0].label.contains("50%"));

    app.prepare_lines(20, 5);
    app.scroll = 1;
    app.execute("/bookmarks");
    key(&mut app, KeyCode::Enter);
    assert!(!app.command_focus);
    assert!(app.busy.is_none());
    assert_eq!(app.scroll, 6);
    assert!(app.progress_dirty);
}

#[test]
fn bookmark_jump_uses_chapter_identity_after_catalog_reordering() {
    let text = "山路夜色\n".repeat(16);
    let (_dir, mut app) = reader(&text);
    app.prepare_lines(20, 5);
    app.scroll = 9;
    app.execute("/bookmark 山路🌙");

    let book = app.current.as_mut().unwrap();
    book.chapters.swap(0, 2);
    app.store.save_book(book).unwrap();
    app.chapter = 1;
    app.content = "别章独有文字".into();
    app.wrap_width = 0;
    app.execute("/bookmarks");
    let choice = &app.overlay.as_ref().unwrap().choices[0];
    assert_eq!(choice.title, "山路🌙");
    assert!(choice.detail.contains("章节 甲"));
    key(&mut app, KeyCode::Enter);
    let message = app.receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    app.sender.send(message).unwrap();
    app.poll_results();
    assert!(!app.is_error, "{}", app.status);
    assert_eq!(app.chapter, 2);
    assert_eq!(app.content, text);
    app.prepare_lines(20, 5);
    assert_eq!(app.scroll, 9);
}

#[test]
fn unbookmark_removes_only_the_selected_entry_even_with_duplicate_labels() {
    let (_dir, mut app) = reader("正文");
    app.execute("/bookmark 同名");
    app.execute("/bookmark 同名");
    let saved = app.store.bookmarks("book").unwrap();
    app.execute("/unbookmark");
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.store.bookmarks("book").unwrap().len(), 2);
    app.execute("/unbookmark");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    let remaining = app.store.bookmarks("book").unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, saved[1].id);
}

#[test]
fn removed_chapter_has_no_jump_fallback_but_its_bookmark_can_be_deleted() {
    let (_dir, mut app) = reader("正文");
    app.execute("/bookmark");
    app.current.as_mut().unwrap().chapters.remove(0);
    app.execute("/bookmarks");
    assert!(app.overlay.as_ref().unwrap().choices[0]
        .detail
        .contains("不在目录"));
    key(&mut app, KeyCode::Enter);
    assert!(app.is_error);
    assert!(app.busy.is_none());
    app.execute("/unbookmark");
    key(&mut app, KeyCode::Enter);
    assert!(app.store.bookmarks("book").unwrap().is_empty());
}

#[test]
fn find_lists_literal_unicode_matches_only_in_the_loaded_chapter() {
    let (_dir, mut app) = reader("序\n\n首见：夜色 [É].\n终点：夜色 [é].\n接下来的文字");
    app.prepare_lines(40, 1);
    app.execute("/find 别章独有文字");
    assert!(app.overlay.is_none());
    app.execute("/find ");
    assert!(app.is_error);
    app.execute("/find 夜色 [é].");
    let overlay = app.overlay.as_ref().unwrap();
    assert_eq!(overlay.choices.len(), 2);
    assert!(overlay.choices[0].detail.contains("首见：夜色 [É]."));
    assert!(overlay.choices[1].detail.contains("终点：夜色 [é]."));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.chapter, 0);
    assert_eq!(app.scroll, 3);
    assert!(app.progress_dirty);
    assert!(!app.command_focus);
}

#[test]
fn find_jumps_to_the_start_of_a_match_that_wraps_after_a_resize() {
    let (_dir, mut app) = reader("前几行  \n  \nxxxxxxxxxxxxxTARGETtail\n后记\n最后一行");
    app.prepare_lines(40, 1);
    app.execute("/find TARGET");
    app.prepare_lines(8, 1);
    let row = app
        .lines
        .iter()
        .position(|line| line.ends_with("TAR"))
        .unwrap();
    assert!(app.lines[row + 1].starts_with("GET"));
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.scroll, row);
}

#[test]
fn find_keeps_the_end_of_chapter_visible_and_rejects_stale_positions() {
    let (_dir, mut app) = reader("一\n二\n三\n四\n最后");
    app.prepare_lines(20, 3);
    app.execute("/find 最后");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.scroll, app.max_scroll());
    assert!(app.lines[app.scroll..app.scroll + app.viewport].contains(&"最后".into()));
    let offset = app.content.find("最后").unwrap();
    app.jump_to_match("book", "乙", offset);
    assert!(app.is_error);
    app.jump_to_match("book", "甲", offset + 1);
    assert!(app.is_error);
    assert_eq!(app.scroll, app.max_scroll());
}

#[test]
fn find_retains_a_logical_position_when_the_chapter_fits() {
    let (_dir, mut app) = reader("一\n二\n三\n四\n五");
    app.prepare_lines(20, 10);
    app.execute("/find 四");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.scroll, 0);
    assert_eq!(app.fraction(), 0.75);
    app.prepare_lines(20, 1);
    assert_eq!(app.lines[app.scroll], "四");
}

#[test]
fn excerpts_preserve_graphemes_and_sanitize_control_characters() {
    let text = "宽".repeat(80) + "\te\u{301}👩‍🚀🔎\x1b山\n下一段";
    let excerpt = excerpt(&text, text.find('\u{301}').unwrap());
    assert!(excerpt.starts_with('…'));
    assert!(excerpt.contains("e\u{301}👩‍🚀🔎山"));
    assert!(!excerpt.contains("下一段"));
    assert!(!excerpt.chars().any(char::is_control));
}
