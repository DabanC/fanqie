use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{library::Library, models::Chapter};

#[test]
fn failed_width_command_does_not_modify_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    let width = app.settings.reader_width;
    app.execute("/width 2");
    assert_eq!(app.settings.reader_width, width);
    assert!(app.is_error);
}
fn settle(app: &mut App) {
    settle_with_timeout(app, std::time::Duration::from_secs(5));
}

fn settle_with_timeout(app: &mut App, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while app.busy.is_some() && std::time::Instant::now() < deadline {
        app.poll_results();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.busy.is_none(), "background job timed out");
    assert!(!app.is_error, "{}", app.status);
}

#[test]
fn reading_workflow_preserves_progress_and_chapter_navigation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut app = App::new(store.clone()).unwrap();
    app.demo();
    settle(&mut app);
    assert!(app.current.as_ref().unwrap().chapters.len() >= 3);
    app.prepare_lines(40, 5);
    assert!(app.max_scroll() > 0);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(app.scroll > 0);
    let fraction = app.fraction();
    app.prepare_lines(70, 8);
    assert!((app.fraction() - fraction).abs() < 0.15);
    app.save_progress();
    let id = app.current.as_ref().unwrap().id.clone();
    let saved = store.progress(&id).unwrap();
    assert!(saved.fraction > 0.0);
    app.execute("/next");
    settle(&mut app);
    app.prepare_lines(70, 8);
    assert_eq!(app.chapter, 1);
    app.save_progress();
    let mut restarted = App::new(store).unwrap();
    restarted.execute("/resume");
    settle(&mut restarted);
    assert_eq!(restarted.chapter, 1);
}

#[test]
#[ignore = "访问番茄公开目录和章节；可能在临时目录注册一个匿名 APP 设备"]
fn live_goto_509_reads_full_chapter() {
    check_live_goto(509, "7709158341783601688", "第509章 宣传三", 2_039);
}

#[test]
#[ignore = "访问番茄公开目录和章节；可能在临时目录注册一个匿名 APP 设备"]
fn live_goto_510_reads_full_chapter() {
    check_live_goto(
        510,
        "7709522412122931774",
        "第510章 到了把精力放在军事上的时候了",
        2_082,
    );
}

fn check_live_goto(number: usize, chapter_id: &str, title: &str, word_count: usize) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut app = App::new(store.clone()).unwrap();
    let timeout = std::time::Duration::from_secs(60);
    app.execute("/open 7635637943461235736");
    settle_with_timeout(&mut app, timeout);
    app.execute(&format!("/goto {number}"));
    settle_with_timeout(&mut app, timeout);

    let book = app.current.as_ref().unwrap();
    assert_eq!(book.id, "7635637943461235736");
    assert_eq!(app.chapter, number - 1);
    let chapter = &book.chapters[app.chapter];
    assert_eq!(chapter.id, chapter_id);
    assert_eq!(chapter.title, title);
    let count = app.content.chars().filter(|c| !c.is_whitespace()).count();
    assert!(
        count >= word_count,
        "官网标注 {word_count} 字，仅返回 {count} 个非空白字符"
    );
    assert!(!app
        .content
        .chars()
        .any(|c| matches!(c as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)));
    let cached = store.cached_chapter(&book.id, &chapter.id).unwrap();
    assert!(cached.as_deref() == Some(&app.content), "缓存与正文不一致");
    eprintln!("/goto {number}：{count} 个非空白字符，缓存与正文一致");
}

#[test]
fn theme_does_not_save_session_only_api_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut app = App::new(store.clone()).unwrap();
    app.settings.content_api = "https://example.com/?token=session-secret".into();
    app.execute("/theme");
    assert!(store.settings().unwrap().content_api.is_empty());
    assert!(store.settings().unwrap().light_theme);
}

#[test]
fn explicit_api_save_persists_even_when_equal_to_cli_override() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut app = App::new(store.clone()).unwrap();
    app.settings.content_api = "https://example.com".into();
    app.execute("/api https://example.com");
    assert_eq!(store.settings().unwrap().content_api, "https://example.com");
    app.settings.content_api.clear();
    app.execute("/api off");
    assert!(store.settings().unwrap().content_api.is_empty());
}

#[test]
fn api_commands_with_leading_spaces_never_enter_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.input.set("  /api https://example.com/?token=private");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.history.is_empty());
}

#[test]
fn shrinking_after_full_chapter_view_retains_position() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.content = (0..100).map(|_| "一行小说正文。\n").collect();
    app.prepare_lines(40, 10);
    app.scroll_by(40);
    let fraction = app.fraction();
    app.prepare_lines(120, 200);
    assert_eq!(app.max_scroll(), 0);
    assert_eq!(app.fraction(), fraction);
    app.prepare_lines(40, 10);
    assert!((app.fraction() - fraction).abs() < 0.02);
}

#[test]
fn reopening_current_book_does_not_restore_stale_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.demo();
    settle(&mut app);
    app.prepare_lines(40, 5);
    app.scroll_by(10);
    let position = app.scroll;
    app.begin_book(app.current.clone().unwrap());
    assert!(app.busy.is_none());
    assert_eq!(app.scroll, position);
}

#[test]
fn home_keeps_live_position_when_progress_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.demo();
    settle(&mut app);
    app.prepare_lines(40, 5);
    app.scroll_by(10);
    std::fs::create_dir(dir.path().join("recent.json")).unwrap();
    app.execute("/home");
    assert!(app.current.is_some());
    assert!(app.is_error);
    assert!(app.progress_dirty);
}

#[test]
fn absolute_quoted_txt_path_is_imported_without_search() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("我的 小说.txt");
    std::fs::write(
        &path,
        "第一章 出发\n风吹过了平静的小镇。\n第二章 归来\n故事继续了。\n",
    )
    .unwrap();
    let mut app = App::new(Store::new(dir.path().join("data")).unwrap()).unwrap();
    app.execute(&format!("\"{}\"", path.display()));
    settle(&mut app);
    assert_eq!(app.current.as_ref().unwrap().source, "local");
}

#[test]
fn degraded_search_notice_is_visible_inside_the_picker() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.sender
        .send(Message {
            generation: app.generation,
            result: Ok(Outcome::Results(SearchReport {
                books: Vec::new(),
                notice: Some("APP 空响应，已切换官方备用搜索".into()),
                ..SearchReport::default()
            })),
        })
        .unwrap();
    app.poll_results();
    let overlay = app.overlay.as_ref().unwrap();
    assert!(overlay.title.contains("降级"));
    assert!(overlay.subtitle.contains("备用搜索"));
    assert!(!app.is_error);
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn space_scrolls_to_the_end_before_loading_the_next_chapter() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.demo();
    settle(&mut app);
    app.prepare_lines(40, 5);
    assert!(app.max_scroll() > 1);
    app.scroll = app.max_scroll() - 1;
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.scroll, app.max_scroll());
    assert!(app.busy.is_none());

    press(&mut app, KeyCode::PageDown);
    press(&mut app, KeyCode::Char('f'));
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ));
    assert!(app.busy.is_none());

    press(&mut app, KeyCode::Char(' '));
    let generation = app.generation;
    assert!(app.busy.is_some());
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.generation, generation);
    settle(&mut app);
    app.prepare_lines(40, 5);
    assert_eq!((app.chapter, app.scroll), (1, 0));
    assert!(app.save_progress());
    let id = &app.current.as_ref().unwrap().id;
    assert_eq!(app.store.progress(id).unwrap().chapter, 1);
}

#[test]
fn space_handles_single_page_chapters_and_stops_at_the_last_chapter() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.demo();
    settle(&mut app);
    press(&mut app, KeyCode::Char(' '));
    assert!(app.busy.is_none(), "chapter has not been laid out yet");
    app.prepare_lines(80, 10_000);
    assert_eq!(app.max_scroll(), 0);
    press(&mut app, KeyCode::Char(' '));
    settle(&mut app);
    assert_eq!(app.chapter, 1);

    let count = app.current.as_ref().unwrap().chapters.len();
    app.execute(&format!("/goto {count}"));
    settle(&mut app);
    app.prepare_lines(80, 10_000);
    press(&mut app, KeyCode::Char(' '));
    assert!(app.busy.is_none());
    assert_eq!(app.chapter, count - 1);
    assert!(app.status.contains("最后一章"));
}

fn cached_serial(store: &Store) -> Book {
    let book = Book {
        id: "11".into(),
        title: "连载小说".into(),
        author: String::new(),
        description: String::new(),
        source: "fanqie".into(),
        chapters: vec![
            Chapter {
                id: "21".into(),
                title: "第一章".into(),
            },
            Chapter {
                id: "22".into(),
                title: "第二章".into(),
            },
        ],
    };
    store.save_book(&book).unwrap();
    store
        .cache_chapter(&book.id, "22", &"这是一行缓存正文。\n".repeat(30))
        .unwrap();
    store.save_progress(&book.id, 1, 0.0).unwrap();
    book
}

fn deliver_catalog(app: &mut App, generation: u64, result: Result<Book>) {
    app.sender
        .send(Message {
            generation,
            result: result.map(Outcome::Catalog),
        })
        .unwrap();
    app.poll_results();
}

#[test]
fn refresh_adds_chapters_without_reloading_text_or_writing_progress() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut book = cached_serial(&store);
    let mut app = App::new(store.clone()).unwrap();
    app.execute("/resume");
    settle(&mut app);
    app.prepare_lines(40, 5);
    app.scroll_by(5);
    let content = app.content.clone();
    let position = app.fraction();
    let saved = store.progress(&book.id).unwrap();
    app.show_toc();
    app.paste("章");

    book.chapters[1].title = "第二章（修订）".into();
    book.chapters.push(Chapter {
        id: "23".into(),
        title: "第三章".into(),
    });
    let generation = app.generation;
    deliver_catalog(&mut app, generation, Ok(book.clone()));
    assert!(!app.is_error, "{}", app.status);
    assert!(app.status.contains("新增 1 章"));
    assert_eq!(app.current.as_ref(), Some(&book));
    assert_eq!(app.shelf, [book.clone()]);
    assert_eq!(app.chapter, 1);
    assert_eq!(app.content, content);
    assert_eq!(app.fraction(), position);
    assert!(app.progress_dirty);
    assert_eq!(store.progress(&book.id).unwrap(), saved);
    assert_eq!(store.get_book(&book.id).unwrap().as_ref(), Some(&book));
    assert!(store.cached_chapter(&book.id, "23").unwrap().is_none());
    let toc = app.overlay.as_ref().unwrap();
    assert_eq!(toc.filter.text, "章");
    assert_eq!(toc.choices.len(), 3);
    assert!(toc.choices[2].title.contains("第三章"));

    deliver_catalog(&mut app, generation, Ok(book));
    assert!(app.status.contains("暂无新章节"));
}

#[test]
fn refresh_rejects_reordered_truncated_and_wrong_book_catalogs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let old = cached_serial(&store);
    let mut app = App::new(store.clone()).unwrap();
    app.current = Some(old.clone());
    let mut reordered = old.clone();
    reordered.chapters.swap(0, 1);
    let mut truncated = old.clone();
    truncated.chapters.pop();
    let mut wrong_book = old.clone();
    wrong_book.id = "12".into();
    let generation = app.generation;
    for invalid in [reordered, truncated, wrong_book] {
        deliver_catalog(&mut app, generation, Ok(invalid));
        assert!(app.is_error);
        assert!(app.status.contains("仍使用原目录"));
        assert_eq!(app.current.as_ref(), Some(&old));
        assert_eq!(store.get_book(&old.id).unwrap().as_ref(), Some(&old));
    }

    let mut newer = old.clone();
    newer.chapters.push(Chapter {
        id: "23".into(),
        title: "第三章".into(),
    });
    store.save_book(&newer).unwrap();
    deliver_catalog(&mut app, generation, Ok(old));
    assert!(app.is_error);
    assert_eq!(store.get_book(&newer.id).unwrap(), Some(newer));
}

#[test]
fn failed_or_abandoned_refresh_does_not_publish_a_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let old = cached_serial(&store);
    let mut app = App::new(store.clone()).unwrap();
    app.current = Some(old.clone());
    let generation = app.generation;
    app.busy = Some("刷新章节目录".into());
    deliver_catalog(&mut app, generation, Err(anyhow::anyhow!("网络请求失败")));
    assert!(app.is_error);
    assert!(app.busy.is_none());
    assert_eq!(app.current.as_ref(), Some(&old));

    let mut update = old.clone();
    update.chapters.push(Chapter {
        id: "23".into(),
        title: "第三章".into(),
    });
    app.execute("/home");
    deliver_catalog(&mut app, generation, Ok(update));
    assert!(app.current.is_none());
    assert_eq!(store.get_book(&old.id).unwrap(), Some(old));
}

#[test]
fn refresh_does_not_start_a_job_without_an_online_book() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Store::new(dir.path().into()).unwrap()).unwrap();
    app.execute("/refresh");
    assert!(app.busy.is_none());
    app.demo();
    settle(&mut app);
    app.execute("/refresh");
    assert!(app.busy.is_none());
    assert!(app.status.contains("只有番茄在线小说"));
}

#[test]
fn prefetch_is_opt_in_and_joined_by_navigation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut book = cached_serial(&store);
    book.chapters.push(Chapter {
        id: "23".into(),
        title: "第三章".into(),
    });
    store.save_book(&book).unwrap();
    store.cache_chapter(&book.id, "23", "下一章正文").unwrap();
    let mut app = App::new(store).unwrap();
    app.execute("/resume");
    settle(&mut app);
    app.prepare_lines(40, 5);
    press(&mut app, KeyCode::End);
    app.maybe_prefetch();
    assert!(app.prefetch.is_none());
    app.execute("/prefetch on");
    app.maybe_prefetch();
    let generation = app.generation;
    assert!(app.prefetch.is_some() && app.busy.is_none());
    app.execute("/next");
    assert_eq!(app.generation, generation, "must reuse the matching task");
    settle(&mut app);
    assert_eq!(app.chapter, 2);
    assert_eq!(app.content, "下一章正文");
    app.prepare_lines(40, 5);
    app.maybe_prefetch();
    assert!(app.prefetch.is_none(), "last chapter has no lookahead");
}

#[test]
fn cancellation_preserves_the_reading_view_and_ignores_old_results() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let current = cached_serial(&store);
    let mut app = App::new(store).unwrap();
    app.execute("/resume");
    settle(&mut app);
    app.prepare_lines(40, 5);
    app.scroll_by(7);
    let generation = app.generation;
    app.busy = Some("加载章节".into());
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    deliver_catalog(&mut app, generation, Ok(current.clone()));
    assert!(app.busy.is_none());
    assert_eq!(app.current, Some(current));
    assert_eq!(app.scroll, 7);
    assert!(app.status.contains("已取消"));
}

#[test]
#[ignore = "仅访问公开番茄目录，使用临时缓存验证 /refresh；不请求正文或注册设备"]
fn live_refresh_restores_a_missing_catalog_tail() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let library = Library::new(store.clone(), Settings::default()).unwrap();
    let id = "7143038691944959011";
    let full = library.fetch_catalog(id).unwrap();
    assert!(full.chapters.len() > 1);
    let mut before = full.clone();
    before.chapters.pop();
    store.save_book(&before).unwrap();
    store.save_progress(id, 0, 0.3).unwrap();
    let mut app = App::new(store.clone()).unwrap();
    app.current = Some(before);
    app.execute("/refresh");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(65);
    while app.busy.is_some() && std::time::Instant::now() < deadline {
        app.poll_results();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(app.busy.is_none() && !app.is_error, "{}", app.status);
    assert!(app.current.as_ref().unwrap().chapters.len() >= full.chapters.len());
    assert_eq!(store.progress(id).unwrap().fraction, 0.3);
    println!("{}：{}", full.title, app.status);
}
