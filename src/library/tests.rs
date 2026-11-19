use super::search::{normalized_title, rank_search_results};
use super::*;

#[test]
fn book_input_validation() {
    assert_eq!(book_id(" 123456 ").unwrap(), "123456");
    assert_eq!(
        book_id("https://fanqienovel.com/page/123?from=share").unwrap(),
        "123"
    );
    assert_eq!(
        book_id("https://fanqienovel.com/share?book_id=456").unwrap(),
        "456"
    );
    for bad in [
        "",
        "../123",
        "-12",
        "https://evilfanqienovel.com/page/123",
        "https://fanqienovel.com.evil/page/123",
        "https://fanqienovel.com/reader/123",
        "file:///page/123",
        "https://private:secret@fanqienovel.com/page/123",
    ] {
        assert!(book_id(bad).is_err());
    }
}

#[test]
fn import_snapshot_survives_source_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("novel.txt");
    std::fs::write(
        &input,
        "第一章 初见\n门前开着一树白花。\n第二章 归来\n他沿着旧路回到河边。\n",
    )
    .unwrap();
    let store = Store::new(directory.path().join("state")).unwrap();
    let library = Library::new(store.clone(), Settings::default()).unwrap();
    let book = library.import_txt(&input).unwrap();
    std::fs::remove_file(&input).unwrap();
    let reopened = Library::new(store.clone(), Settings::default()).unwrap();
    let cached = store.get_book(&book.id).unwrap().unwrap();
    assert_eq!(cached.chapters.len(), 2);
    assert!(reopened.chapter(&cached, 0).unwrap().contains("白花"));
    assert!(reopened.chapter(&cached, 1).unwrap().contains("河边"));
    assert!(reopened.chapter(&cached, 2).is_err());
}

#[test]
fn demo_is_explicit_and_local_search_works_without_online_dependency() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().into()).unwrap();
    let library = Library::new(store, Settings::default()).unwrap();
    let book = library.demo().unwrap();
    assert_eq!(book.source, "demo");
    assert!(book.chapters.len() >= 3);
    assert!(book.description.contains("原创"));
    for index in 0..book.chapters.len() {
        assert!(library.chapter(&book, index).unwrap().chars().count() >= 1000);
    }
    assert_eq!(library.local_search(&book.title).unwrap().len(), 1);
    assert!(library
        .local_search("unlikely to match any local book")
        .unwrap()
        .is_empty());
    assert!(library.search(" ").is_err());
}

#[test]
#[ignore = "访问番茄官方服务：验证真实搜索、网页全文与第 50 章之后的匿名阅读"]
fn live_online_search_and_full_chapters() {
    let data_dir = std::env::var_os("FANQIE_LIVE_DATA_DIR").map(std::path::PathBuf::from);
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(data_dir.unwrap_or_else(|| temporary.path().to_path_buf())).unwrap();
    let library = Library::new(store.clone(), Settings::default()).unwrap();
    let found = library
        .search("十日终焉")
        .expect("在线搜索必须无需 API 配置即可工作");
    assert!(found
        .iter()
        .any(|book| book.id == "7143038691944959011" && book.title == "十日终焉"));
    println!("官方搜索成功：{} 本小说", found.len());
    let searched_book = library.open_remote("7143038691944959011").unwrap();
    assert_eq!(searched_book.title, "十日终焉");
    let opening = library.chapter(&searched_book, 0).unwrap();
    assert!(opening.chars().count() >= 1000);
    println!(
        "搜索结果直接打开成功：{} / 首章 {} 字符",
        searched_book.title,
        opening.chars().count()
    );
    let book = library.open_remote("7069948840148732967").unwrap();
    assert_eq!(book.title, "部族荣光");
    assert!(book.chapters.len() > 50);
    println!("真实目录：{} / {} 章", book.title, book.chapters.len());
    for index in [0, 49, 50] {
        let text = library
            .chapter(&book, index)
            .unwrap_or_else(|error| panic!("第 {} 章直连失败：{error:#}", index + 1));
        assert!(text.chars().count() >= 1000, "不接受试读片段冒充完整章节");
        assert!(!text.chars().any(
            |ch| matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
        ));
        assert_eq!(
            store
                .cached_chapter(&book.id, &book.chapters[index].id)
                .unwrap()
                .unwrap(),
            text
        );
        println!(
            "第 {} 章完整正文：{} 字符（已校验缓存、无乱码）",
            index + 1,
            text.chars().count()
        );
    }
}

#[test]
fn exact_long_title_precedes_upstream_recommendations_and_local_books() {
    let make_book = |id: &str, title: &str| Book {
        id: id.into(),
        title: title.into(),
        author: String::new(),
        description: String::new(),
        source: "fanqie".into(),
        chapters: Vec::new(),
    };
    let mut books = vec![
        make_book("1", "军阀：我奉化人，当个总统怎么了"),
        make_book("2", "我下南洋当总统，你写日记骂什么"),
        make_book("3", "校长写日记骂我,我在南洋当总统"),
        make_book("4", "另一部推荐书"),
    ];
    rank_search_results("《校长写日记骂我，我在南洋当总统》", &mut books);
    assert_eq!(books[0].id, "3");
    assert_eq!(books[1].id, "2");
    rank_search_results("南洋当总统", &mut books);
    assert!(books[..2]
        .iter()
        .all(|book| normalized_title(&book.title).contains("南洋当总统")));
}

#[test]
fn ranking_keeps_upstream_order_for_equal_matches() {
    let make_book = |id: &str| Book {
        id: id.into(),
        title: "相同书名".into(),
        author: String::new(),
        description: String::new(),
        source: "fanqie".into(),
        chapters: Vec::new(),
    };
    let mut books = vec![make_book("2"), make_book("1")];
    rank_search_results("相同书名", &mut books);
    assert_eq!(
        books
            .iter()
            .map(|book| book.id.as_str())
            .collect::<Vec<_>>(),
        ["2", "1"]
    );
}

#[test]
#[ignore = "访问官方搜索，验证完整书名、截图关键词和故障降级"]
fn live_reported_title_search() {
    let temporary = tempfile::tempdir().unwrap();
    let root = std::env::var_os("FANQIE_LIVE_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| temporary.path().to_path_buf());
    let library = Library::new(Store::new(root).unwrap(), Settings::default()).unwrap();
    for query in [
        "校长写日记骂我，我在南洋当总统",
        "校长写日记骂我",
        "南洋当总统",
    ] {
        let report = library.search_report(query).unwrap();
        let target = report
            .books
            .iter()
            .find(|book| book.id == "7635637943461235736")
            .expect("应返回已核实的目标书籍 ID，不能拿相似小说代替");
        if normalized_title(&target.title) != normalized_title("校长写日记骂我，我在南洋当总统")
        {
            assert!(
                report.notice.is_some(),
                "返回原书名时必须提示备用或缓存模式"
            );
        }
        println!(
            "查询：{query} / 目标：{} / {} / 模式：{}",
            target.title,
            target.id,
            report.notice.as_deref().unwrap_or("APP 实时搜索")
        );
    }
}

fn cached_hit(id: &str, title: &str) -> Book {
    Book {
        id: id.into(),
        title: title.into(),
        author: "作者".into(),
        description: String::new(),
        source: "fanqie".into(),
        chapters: Vec::new(),
    }
}

#[test]
fn cached_alias_wins_over_old_title_when_fallback_is_active() {
    let alias = cached_hit("7635637943461235736", "校长写日记骂我，我在南洋当总统");
    let old = cached_hit(&alias.id, "军阀：我奉化人，当个总统怎么了");
    let report = merge_search_report(
        "校长写日记骂我",
        vec![],
        vec![alias.clone()],
        Ok(SearchReport {
            books: vec![old],
            notice: Some("已切换官方备用搜索".into()),
            ..SearchReport::default()
        }),
    )
    .unwrap();
    assert_eq!(report.books.len(), 1);
    assert_eq!(report.books[0].title, alias.title);
    assert!(report.notice.unwrap().contains("历史书名"));
}

#[test]
fn total_network_failure_keeps_cache_but_never_claims_live_results() {
    let cached = cached_hit("123", "历史搜索书名");
    let report =
        merge_search_report("历史", vec![], vec![cached], Err(anyhow::anyhow!("断网"))).unwrap();
    assert_eq!(report.books.len(), 1);
    assert!(report.notice.unwrap().contains("仅显示"));
    assert!(merge_search_report("未知", vec![], vec![], Err(anyhow::anyhow!("断网"))).is_err());
}

#[test]
fn healthy_live_search_does_not_inject_stale_cached_results() {
    let cached = cached_hit("123", "过期书名");
    let report =
        merge_search_report("书名", vec![], vec![cached], Ok(SearchReport::default())).unwrap();
    assert!(report.books.is_empty());
    assert!(report.notice.is_none());
}

#[test]
fn load_more_keeps_earlier_results_and_upgrades_the_title_for_the_same_id() {
    let previous = SearchReport {
        books: vec![cached_hit("1", "旧书名"), cached_hit("2", "匹配书名：续篇")],
        ..SearchReport::default()
    };
    let page = SearchReport {
        books: vec![
            cached_hit("1", "匹配书名"),
            cached_hit("2", "匹配书名：其他"),
            cached_hit("3", "其他书"),
        ],
        ..SearchReport::default()
    };
    let merged = search::append_search_page("匹配书名", &previous, page, vec![]);
    assert_eq!(
        merged
            .books
            .iter()
            .map(|book| (book.id.as_str(), book.title.as_str()))
            .collect::<Vec<_>>(),
        [("1", "匹配书名"), ("2", "匹配书名：续篇"), ("3", "其他书")]
    );
    assert_eq!(previous.books[0].title, "旧书名");
}

#[test]
fn online_aliases_can_be_found_offline_without_adding_a_shelf_entry() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_owned()).unwrap();
    let library = Library::new(store.clone(), Settings::default()).unwrap();
    let mut page = SearchReport {
        books: vec![cached_hit("1", "检索书名")],
        cache_aliases: vec![cached_hit("1", "检索书名"), cached_hit("1", "另一个书名")],
        ..SearchReport::default()
    };
    library.cache_remote_search(&mut page).unwrap();
    assert!(page.cache_aliases.is_empty());
    for title in ["检索书名", "另一个书名"] {
        let report = merge_search_report(
            title,
            vec![],
            store.cached_search_books(title).unwrap(),
            Err(anyhow::anyhow!("断网")),
        )
        .unwrap();
        assert_eq!(report.books[0].title, title);
        assert_eq!(report.books[0].id, "1");
    }
    assert!(store.books().unwrap().is_empty());
}

#[test]
fn load_more_without_a_continuation_returns_before_searching() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(
        Store::new(dir.path().to_owned()).unwrap(),
        Settings::default(),
    )
    .unwrap();
    let error = library.search_more(&SearchReport::default()).unwrap_err();
    assert!(error.to_string().contains("没有更多"));
}
