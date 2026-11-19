use super::*;
use std::fs;

#[test]
fn duplicate_labels_are_independent_and_progress_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let book = "../小说";
    store.save_progress(book, 2, 0.25).unwrap();
    let progress_path = store.book_dir(book).unwrap().join("progress.json");
    let progress_bytes = fs::read(&progress_path).unwrap();
    let recent_path = store.root().join("recent.json");
    let recent_bytes = fs::read(&recent_path).unwrap();
    assert!(store.bookmarks(book).unwrap().is_empty());
    store.add_bookmark(book, "章甲", 0.5, "夜读🌙").unwrap();
    store.add_bookmark(book, "章乙", 1.0, "夜读🌙").unwrap();
    store.add_bookmark("other", "章甲", 0.0, "同名").unwrap();

    let reopened = Store::new(dir.path().into()).unwrap();
    let entries = reopened.bookmarks(book).unwrap();
    assert_ne!(entries[0].id, entries[1].id);
    assert_eq!(entries[0].chapter_id, "章甲");
    assert_eq!(entries[0].fraction, 0.5);
    assert_eq!(entries[0].label, "夜读🌙");
    assert!(reopened.remove_bookmark(book, &entries[0].id).unwrap());
    assert!(!reopened.remove_bookmark(book, &entries[0].id).unwrap());
    assert_eq!(reopened.bookmarks(book).unwrap()[0].id, entries[1].id);
    assert_eq!(reopened.bookmarks("other").unwrap().len(), 1);
    assert_eq!(fs::read(progress_path).unwrap(), progress_bytes);
    assert_eq!(fs::read(recent_path).unwrap(), recent_bytes);
}

#[test]
fn rejects_invalid_inputs_without_creating_a_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    for fraction in [f64::NAN, f64::INFINITY, -0.01, 1.01] {
        assert!(store.add_bookmark("book", "ch", fraction, "label").is_err());
    }
    for label in [" ", "bad\x1b[31m", &"长".repeat(MAX_LABEL_BYTES)] {
        assert!(store.add_bookmark("book", "ch", 0.0, label).is_err());
    }
    assert!(store.add_bookmark("book", "\n", 0.0, "label").is_err());
    assert!(store.add_bookmark("\0", "ch", 0.0, "label").is_err());
    assert!(store.remove_bookmark("book", "../bookmarks").is_err());
    assert!(!store.bookmarks_path("book").unwrap().exists());
}

#[test]
fn rejects_invalid_or_incompatible_records_without_overwriting_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let path = store.bookmarks_path("book").unwrap();
    let id = "a".repeat(32);
    let entry =
        serde_json::json!({"id": id, "chapter_id": "ch", "fraction": 0.5, "label": "label"});
    let mut bad_fraction = entry.clone();
    bad_fraction["fraction"] = serde_json::json!(1.5);
    let record = |version, book_id, entries| {
        serde_json::to_vec(&serde_json::json!({
            "version": version, "value": {"book_id": book_id, "entries": entries}
        }))
        .unwrap()
    };
    for bytes in [
        b"{broken".to_vec(),
        record(2, "book", vec![entry.clone()]),
        record(1, "another-book", vec![entry.clone()]),
        record(1, "book", vec![entry.clone(), entry.clone()]),
        record(1, "book", vec![bad_fraction]),
    ] {
        Store::atomic(&path, &bytes).unwrap();
        assert!(store.bookmarks("book").is_err());
        assert!(store.add_bookmark("book", "ch", 0.0, "new").is_err());
        assert!(store.remove_bookmark("book", &id).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
