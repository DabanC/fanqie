use super::*;
use crate::models::Chapter;

fn book(id: &str) -> Book {
    Book {
        id: id.into(),
        title: "测试小说".into(),
        author: "作者".into(),
        description: String::new(),
        source: "local".into(),
        chapters: vec![Chapter {
            id: "../chapter/章节".into(),
            title: "第一章".into(),
        }],
    }
}

#[test]
fn round_trip_hashes_paths_and_keeps_progress_per_book() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let first = book("../../outside/书籍");
    let second = book("another");
    store.save_book(&first).unwrap();
    store.save_book(&second).unwrap();
    store
        .cache_chapter(&first.id, &first.chapters[0].id, "正文内容")
        .unwrap();
    store.save_progress(&first.id, 3, 0.75).unwrap();
    store.save_progress(&second.id, 1, 0.25).unwrap();
    let reopened = Store::new(root.path().into()).unwrap();
    assert_eq!(reopened.get_book(&first.id).unwrap(), Some(first.clone()));
    assert_eq!(
        reopened
            .cached_chapter(&first.id, &first.chapters[0].id)
            .unwrap()
            .as_deref(),
        Some("正文内容")
    );
    assert_eq!(
        reopened.progress(&first.id).unwrap(),
        Progress {
            chapter: 3,
            fraction: 0.75
        }
    );
    assert_eq!(
        reopened.progress(&second.id).unwrap(),
        Progress {
            chapter: 1,
            fraction: 0.25
        }
    );
    assert_eq!(reopened.last_book().unwrap().as_deref(), Some("another"));
    assert_eq!(reopened.books().unwrap().len(), 2);
    let path = reopened
        .chapter_path(&first.id, &first.chapters[0].id)
        .unwrap();
    assert!(path.starts_with(root.path()));
    assert_eq!(path.file_stem().unwrap().to_string_lossy().len(), 64);
    assert!(!path.to_string_lossy().contains("outside"));
    assert!(reopened.get_book("").is_err());
    assert!(reopened.cached_chapter("book", "\n").is_err());
    assert!(reopened.save_progress("book", 0, f64::NAN).is_err());
}

#[test]
fn concurrent_catalog_and_progress_never_clobber_each_other() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let mut original = book("concurrent");
    store.save_book(&original).unwrap();
    let metadata_store = store.clone();
    let progress_store = store.clone();
    let catalog = std::thread::spawn(move || {
        for index in 0..40 {
            original.description = format!("catalog-{index}");
            metadata_store.save_book(&original).unwrap();
        }
    });
    let progress = std::thread::spawn(move || {
        for index in 0..40 {
            progress_store
                .save_progress("concurrent", index, 0.5)
                .unwrap();
        }
    });
    catalog.join().unwrap();
    progress.join().unwrap();
    assert_eq!(
        store.get_book("concurrent").unwrap().unwrap().description,
        "catalog-39"
    );
    assert_eq!(
        store.progress("concurrent").unwrap(),
        Progress {
            chapter: 39,
            fraction: 0.5
        }
    );
    assert_eq!(store.last_book().unwrap().as_deref(), Some("concurrent"));
    assert!(fs::read_dir(store.book_dir("concurrent").unwrap())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".pending")));
}

#[test]
fn corruption_is_reported_and_original_bytes_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let good = book("good");
    let bad = book("bad");
    store.save_book(&good).unwrap();
    store.save_book(&bad).unwrap();
    let metadata = store.book_dir("bad").unwrap().join("metadata.json");
    fs::write(&metadata, b"{bad JSON").unwrap();
    assert_eq!(store.books().unwrap(), vec![good.clone()]);
    assert!(!store.take_warnings().is_empty());
    assert_eq!(fs::read(&metadata).unwrap(), b"{bad JSON");
    assert!(store.get_book("bad").is_err());
    store.cache_chapter("good", "c", "valid").unwrap();
    let cache = store.chapter_path("good", "c").unwrap();
    fs::write(&cache, [0xff, 0xfe]).unwrap();
    assert!(store.cached_chapter("good", "c").unwrap().is_none());
    assert!(!store.take_warnings().is_empty());
    let progress = store.book_dir("good").unwrap().join("progress.json");
    fs::write(&progress, b"broken").unwrap();
    assert!(store.progress("good").is_err());
    assert_eq!(fs::read(&progress).unwrap(), b"broken");
    assert_eq!(store.progress("missing").unwrap(), Progress::default());
}

#[test]
fn corrupt_or_unknown_settings_cannot_be_overwritten_by_defaults() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    assert_eq!(store.settings().unwrap(), Settings::default());
    let settings = Settings {
        content_api: "https://example.test/reading/reader/batch_full/v?token=SECRET".into(),
        ..Settings::default()
    };
    store.save_settings(&settings).unwrap();
    assert_eq!(store.settings().unwrap(), settings);
    let path = root.path().join("settings.json");
    for corrupt in [
        r#"{"token":"SECRET","#,
        r#"{"version":1,"value":{"api_key":"SECRET"}}"#,
        r#"{"version":2,"value":{"content_api":"SECRET"}}"#,
    ] {
        fs::write(&path, corrupt).unwrap();
        let error = store.settings().unwrap_err().to_string();
        assert!(!error.contains("SECRET"));
        assert!(store.save_settings(&Settings::default()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
    }
}

#[cfg(unix)]
#[test]
fn settings_with_tokens_are_not_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    store.save_settings(&Settings::default()).unwrap();
    assert_eq!(
        fs::metadata(root.path().join("settings.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn search_cache_preserves_multiple_names_without_adding_shelf_books() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let mut first = book("7635637943461235736");
    first.source = "fanqie".into();
    first.title = "军阀：我奉化人，当个总统怎么了".into();
    store.cache_search_books(&[first.clone()]).unwrap();
    first.title = "校长写日记骂我，我在南洋当总统".into();
    store.cache_search_books(&[first.clone()]).unwrap();
    let reopened = Store::new(root.path().into()).unwrap();
    assert!(reopened.books().unwrap().is_empty());
    let aliases = reopened.cached_search_books("校长写日记骂我").unwrap();
    assert_eq!(aliases.len(), 1);
    assert_eq!(aliases[0].id, first.id);
    assert!(aliases[0].chapters.is_empty());
    assert_eq!(reopened.cached_search_books("军阀").unwrap().len(), 1);
    assert_eq!(
        reopened
            .cached_search_books("7635637943461235736")
            .unwrap()
            .len(),
        2
    );
    std::fs::write(root.path().join("search-index/broken.json"), "{").unwrap();
    assert_eq!(reopened.cached_search_books("校长").unwrap().len(), 1);
    assert!(!reopened.take_warnings().is_empty());
}
