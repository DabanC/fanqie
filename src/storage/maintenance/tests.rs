use super::*;
use crate::models::Chapter;

fn make_book(store: &Store, id: &str, source: &str) -> Book {
    let book = Book {
        id: id.into(),
        title: format!("书名 {id}"),
        author: String::new(),
        description: String::new(),
        source: source.into(),
        chapters: vec![Chapter {
            id: "chapter".into(),
            title: "第一章".into(),
        }],
    };
    store.save_book(&book).unwrap();
    store.cache_chapter(id, "chapter", "正文").unwrap();
    book
}

#[test]
fn clear_counts_and_deletes_only_known_remote_chapters() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let remote = make_book(&store, "123", "fanqie");
    let local = make_book(&store, "local", "local");
    let demo = make_book(&store, "demo", "demo");
    let broken = make_book(&store, "456", "fanqie");
    fs::write(
        store.book_dir(&broken.id).unwrap().join("metadata.json"),
        b"invalid",
    )
    .unwrap();
    store
        .cache_chapter(&remote.id, "not-in-catalog", "未知章节")
        .unwrap();
    store
        .cache_chapter("unpublished", "chapter", "未完成快照")
        .unwrap();
    store.save_progress(&remote.id, 0, 0.5).unwrap();
    store
        .cache_search_books(std::slice::from_ref(&remote))
        .unwrap();
    store
        .add_bookmark(&remote.id, "chapter", 0.6, "书签")
        .unwrap();

    let summary = store.chapter_cache_summary().unwrap();
    assert_eq!(summary.remote, CacheUsage { files: 1, bytes: 6 });
    assert_eq!(
        summary.offline,
        CacheUsage {
            files: 2,
            bytes: 12
        }
    );
    assert_eq!(summary.skipped, 3);
    let result = store.clear_remote_chapter_cache().unwrap();
    assert_eq!(result.removed, summary.remote);
    assert_eq!(result.skipped, 3);
    assert_eq!(store.get_book(&remote.id).unwrap(), Some(remote.clone()));
    assert_eq!(store.cached_chapter(&remote.id, "chapter").unwrap(), None);
    assert_eq!(store.progress(&remote.id).unwrap().fraction, 0.5);
    assert_eq!(
        store.last_book().unwrap().as_deref(),
        Some(remote.id.as_str())
    );
    assert_eq!(store.bookmarks(&remote.id).unwrap()[0].fraction, 0.6);
    assert_eq!(store.cached_search_books(&remote.id).unwrap().len(), 1);
    for id in [&local.id, &demo.id, &broken.id, "unpublished"] {
        assert!(store.cached_chapter(id, "chapter").unwrap().is_some());
    }
    assert!(store
        .cached_chapter(&remote.id, "not-in-catalog")
        .unwrap()
        .is_some());
    assert_eq!(store.clear_remote_chapter_cache().unwrap().removed.files, 0);
}

#[test]
fn remove_retains_progress_and_bookmarks_and_only_clears_matching_recent() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let local = make_book(&store, "../local/书", "local");
    let remote = make_book(&store, "123", "fanqie");
    store.save_progress(&local.id, 0, 0.5).unwrap();
    store.save_progress(&remote.id, 0, 0.25).unwrap();
    store
        .add_bookmark(&local.id, "chapter", 0.6, "书签")
        .unwrap();
    assert!(
        !store
            .remove_shelf_book(&local, None)
            .unwrap()
            .cleanup_pending
    );
    assert_eq!(store.get_book(&local.id).unwrap(), None);
    assert_eq!(store.cached_chapter(&local.id, "chapter").unwrap(), None);
    assert_eq!(store.progress(&local.id).unwrap().fraction, 0.5);
    assert_eq!(store.bookmarks(&local.id).unwrap()[0].fraction, 0.6);
    assert_eq!(
        store.last_book().unwrap().as_deref(),
        Some(remote.id.as_str())
    );

    assert!(
        !store
            .remove_shelf_book(&remote, None)
            .unwrap()
            .cleanup_pending
    );
    assert_eq!(store.last_book().unwrap(), None);
    assert_eq!(store.progress(&remote.id).unwrap().fraction, 0.25);
    store.save_book(&local).unwrap();
    assert_eq!(store.progress(&local.id).unwrap().fraction, 0.5);
}

#[test]
fn remove_refuses_unknown_chapters_or_unreadable_progress_before_mutation() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let book = make_book(&store, "local", "local");
    let orphan = store.chapter_path(&book.id, "orphan").unwrap();
    fs::write(&orphan, "其他内容").unwrap();
    assert!(store.remove_shelf_book(&book, None).is_err());
    fs::remove_file(orphan).unwrap();
    let progress = store.book_dir(&book.id).unwrap().join("progress.json");
    fs::write(&progress, b"unreadable").unwrap();
    assert!(store
        .remove_shelf_book(&book, Some(Progress::default()))
        .is_err());
    assert_eq!(fs::read(progress).unwrap(), b"unreadable");
    assert!(store.cached_chapter(&book.id, "chapter").unwrap().is_some());
    assert_eq!(store.get_book(&book.id).unwrap(), Some(book));
}

#[test]
fn scan_is_bounded_by_entries_and_catalog_bytes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let _remote = make_book(&store, "123", "fanqie");
    for mut budget in [
        ScanBudget {
            entries: 1,
            ..ScanBudget::default()
        },
        ScanBudget {
            catalog_bytes: 4,
            ..ScanBudget::default()
        },
    ] {
        assert!(Store::inventory(root.path(), true, &mut budget).is_err());
        assert_eq!(
            store.cached_chapter("123", "chapter").unwrap().as_deref(),
            Some("正文")
        );
    }
}

#[cfg(unix)]
#[test]
fn clear_and_remove_never_use_linked_chapters_or_metadata() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let remote = make_book(&store, "123", "fanqie");
    let file = store.chapter_path(&remote.id, "chapter").unwrap();
    let chapters = file.parent().unwrap();
    let external = outside.path().join(file.file_name().unwrap());
    fs::rename(&file, &external).unwrap();
    symlink(&external, &file).unwrap();
    assert_eq!(store.clear_remote_chapter_cache().unwrap().removed.files, 0);
    assert!(store.remove_shelf_book(&remote, None).is_err());
    assert!(file.symlink_metadata().unwrap().file_type().is_symlink());

    fs::remove_file(&file).unwrap();
    fs::remove_dir(chapters).unwrap();
    symlink(outside.path(), chapters).unwrap();
    assert_eq!(store.clear_remote_chapter_cache().unwrap().removed.files, 0);
    assert!(store.remove_shelf_book(&remote, None).is_err());
    assert_eq!(fs::read_to_string(&external).unwrap(), "正文");

    fs::remove_file(chapters).unwrap();
    fs::create_dir(chapters).unwrap();
    fs::copy(&external, &file).unwrap();
    let book_record = chapters.parent().unwrap().join("metadata.json");
    let external_record = outside.path().join("metadata.json");
    fs::rename(&book_record, &external_record).unwrap();
    symlink(&external_record, &book_record).unwrap();
    assert_eq!(store.clear_remote_chapter_cache().unwrap().removed.files, 0);
    assert!(store.remove_shelf_book(&remote, None).is_err());
    assert!(external_record.exists());
    assert!(file.exists());
}

#[cfg(unix)]
#[test]
fn a_linked_books_directory_is_refused() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into()).unwrap();
    let external_store = Store::new(outside.path().into()).unwrap();
    let remote = make_book(&external_store, "123", "fanqie");
    symlink(outside.path().join("books"), root.path().join("books")).unwrap();
    assert!(store.chapter_cache_summary().is_err());
    assert!(store.clear_remote_chapter_cache().is_err());
    assert!(store.remove_shelf_book(&remote, None).is_err());
    assert_eq!(
        external_store.get_book(&remote.id).unwrap(),
        Some(remote.clone())
    );
    assert!(external_store
        .cached_chapter(&remote.id, "chapter")
        .unwrap()
        .is_some());
}
