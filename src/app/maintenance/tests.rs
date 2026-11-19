use super::*;
use crate::{models::Chapter, storage::Store};
use crossterm::event::KeyCode;

fn setup(source: &str) -> (tempfile::TempDir, App, Book) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let book = Book {
        id: "123".into(),
        title: "书名".into(),
        author: "作者".into(),
        description: String::new(),
        source: source.into(),
        chapters: vec![Chapter {
            id: "456".into(),
            title: "第一章".into(),
        }],
    };
    store.save_book(&book).unwrap();
    store
        .cache_chapter(&book.id, &book.chapters[0].id, "正文")
        .unwrap();
    let app = App::new(store).unwrap();
    (dir, app, book)
}

#[test]
fn shelf_remove_requires_a_separate_confirmation_and_defaults_to_cancel() {
    let (_dir, mut app, book) = setup("local");
    app.remove_command();
    app.overlay_key(KeyCode::Enter); // Pick the book without removing it.
    assert!(app.overlay.is_some());
    assert_eq!(app.store.get_book(&book.id).unwrap(), Some(book.clone()));
    app.overlay_key(KeyCode::Enter); // Cancel.
    assert!(app.overlay.is_none());
    assert_eq!(app.store.get_book(&book.id).unwrap(), Some(book.clone()));
    app.remove_command();
    app.overlay_key(KeyCode::Enter);
    app.overlay_key(KeyCode::Down);
    app.overlay_key(KeyCode::Enter);
    assert!(app.shelf.is_empty());
    assert_eq!(app.store.get_book(&book.id).unwrap(), None);
}

#[test]
fn removing_current_book_preserves_live_fraction_and_resets_reader() {
    let (_dir, mut app, book) = setup("local");
    app.current = Some(book.clone());
    app.content = "正文".into();
    app.logical_fraction = 0.42;
    app.progress_dirty = true;
    app.remove_command();
    app.overlay_key(KeyCode::Down);
    app.overlay_key(KeyCode::Enter);
    assert!(app.current.is_none());
    assert!(app.content.is_empty());
    assert!(app.save_progress());
    assert_eq!(app.store.progress(&book.id).unwrap().fraction, 0.42);
    assert_eq!(app.store.last_book().unwrap(), None);
}

#[test]
fn cache_clear_keeps_the_open_chapter_and_has_a_cancel_default() {
    let (_dir, mut app, book) = setup("fanqie");
    app.current = Some(book.clone());
    app.content = "正文".into();
    app.cache_command();
    assert!(app.status.contains("在线 1 章"));
    app.prompt_clear_cache();
    app.overlay_key(KeyCode::Enter);
    assert_eq!(
        app.store
            .cached_chapter(&book.id, &book.chapters[0].id)
            .unwrap()
            .as_deref(),
        Some("正文")
    );
    app.prompt_clear_cache();
    app.overlay_key(KeyCode::Down);
    app.overlay_key(KeyCode::Enter);
    assert_eq!(app.content, "正文");
    assert_eq!(app.current, Some(book.clone()));
    assert_eq!(
        app.store
            .cached_chapter(&book.id, &book.chapters[0].id)
            .unwrap(),
        None
    );
}

#[test]
fn active_or_cancelled_but_unfinished_workers_block_destructive_actions() {
    let (_dir, mut app, book) = setup("fanqie");
    app.busy = Some("前台任务".into());
    app.remove_command();
    app.prompt_clear_cache();
    assert!(app.overlay.is_none());
    app.cache_command();
    assert!(app.status.contains("在线 1 章"));

    app.busy = None;
    app.prompt_clear_cache();
    app.inflight = 1;
    app.overlay_key(KeyCode::Down);
    app.overlay_key(KeyCode::Enter);
    app.confirm_remove_book(book.id.clone());
    assert!(app.status.contains("后台操作"));
    assert_eq!(app.store.get_book(&book.id).unwrap(), Some(book.clone()));
    assert!(app
        .store
        .cached_chapter(&book.id, &book.chapters[0].id)
        .unwrap()
        .is_some());
}
