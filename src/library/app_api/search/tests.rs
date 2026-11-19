use super::super::{content::clean, is_rate_limited};
use super::*;
use std::{collections::VecDeque, fs};

const WANTED: &str = "校长写日记骂我，我在南洋当总统";
const SIMILAR: &str = "我下南洋当总统，你写日记骂什么";

fn device() -> Device {
    Device {
        device_id: "123456789".into(),
        install_id: "987654321".into(),
        device_type: "P30".into(),
        device_brand: "realme".into(),
    }
}

fn record(id: &str, title: &str) -> Value {
    json!({"book_id": id, "title": title, "author": "测试作者"})
}

fn page(books: Vec<Value>, more: bool, cursor: &str) -> Value {
    json!({"code": 0, "data": {"search_tabs": [
        {"tab_type": 2, "data": [{"book_data": [record("99", WANTED)]}]},
        {"tab_type": 3, "data": [{"book_data": books}], "has_more": more, "passback": cursor}
    ]}})
}

#[test]
fn nested_shapes_title_aliases_and_string_ids() {
    let id = "7682739645964094488";
    let mut entry = record(id, WANTED);
    entry["original_book_name"] = json!("原书名");
    let nested = page(vec![entry.clone()], false, "");
    let parsed = parse_page(&nested).unwrap();
    assert_eq!(parsed.hits.len(), 1);
    let book = &parsed.hits[0].book;
    assert_eq!(
        (&*book.id, &*book.title, &*book.author),
        (id, WANTED, "测试作者")
    );
    assert!(book.description.contains("官方 APP 原书名：原书名"));
    book.validate().unwrap();
    for fixture in [
        nested["data"].clone(),
        nested["data"]["search_tabs"][1].clone(),
        json!({"code":0,"data":{"data":[{"book_data":[entry]}]}}),
    ] {
        assert_eq!(parse_page(&fixture).unwrap().hits[0].book.id, id);
    }
    let fallback = json!({"data":[{"book_data":[{"book_id":18446744073709551615_u64,"book_name":"", "original_book_name":WANTED}]}]});
    let parsed = parse_page(&fallback).unwrap();
    assert_eq!(parsed.hits[0].book.id, "18446744073709551615");
    assert_eq!(parsed.hits[0].book.title, WANTED);
}

#[test]
fn recommendation_markers_non_novels_and_dedupe() {
    let fixture = json!({"search_tabs":[{"tab_type":3,"data":[
        {"use_recommend":true,"book_data":[record("1", WANTED)]},
        {"use_recommend":1,"book_data":[record("2", WANTED)]},
        {"use_recommend":"1","book_data":[record("3", WANTED)]},
        {"tab_type":2,"book_data":[record("4", WANTED)]},
        {"content_type":8,"book_data":[record("5", WANTED)]},
        {"video_data":[record("6", WANTED)]},
        {"book_data":[{"book_id":"7","title":WANTED,"is_audio":1},
            {"book_id":"8","title":WANTED,"content_type":11},
            {"book_id":"9","title":WANTED,"is_recommend":"1"}]},
        {"use_recommend":0,"book_data":[record("10", WANTED),record("10", WANTED)]},
        {"book_id":"11","book_data":[{"book_name":WANTED}]}
    ]}]});
    let hits = parse_page(&fixture).unwrap().hits;
    assert_eq!(
        hits.iter().map(|h| h.book.id.as_str()).collect::<Vec<_>>(),
        ["1", "2", "3", "9", "10", "10", "11"]
    );
    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![fixture.clone()]);
    let books = search_with(dir.path(), &mut gateway, WANTED).unwrap();
    assert_eq!(
        books
            .iter()
            .map(|book| book.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3", "9", "10", "11"]
    );
    assert!(hits[..4].iter().all(|hit| hit.recommended));
    assert!(hits[4..].iter().all(|hit| !hit.recommended));
    let mut global = fixture;
    global["use_recommend"] = json!(true);
    assert!(parse_page(&global)
        .unwrap()
        .hits
        .iter()
        .all(|hit| hit.recommended));
}

#[test]
fn live_shape_exact_recommendation_survives_without_unrelated_recs() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = json!({"code":0,"search_tabs":[{"tab_type":3,"has_more":true,"passback":"unused","data":[
        {"use_recommend":true,"book_data":[{"book_id":"7635637943461235736",
            "title":WANTED,"original_book_name":"军阀：我奉化人，当个总统怎么了","author":"爱吃西瓜的狗哥"}]},
        {"use_recommend":true,"book_data":[record("7682739645964094488", SIMILAR)]},
        {"use_recommend":true,"book_data":[record("123", "无关推荐")]}]}]});
    let mut gateway = fake(vec![fixture.clone()]);
    let books = search_with(dir.path(), &mut gateway, WANTED).unwrap();
    assert_eq!(gateway.cursors.len(), 1);
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, WANTED);
    assert_eq!(books[0].id, "7635637943461235736");
    assert_eq!(books[0].author, "爱吃西瓜的狗哥");
    assert!(books[0].description.starts_with("官方 APP 推荐标记=1"));
    books[0].validate().unwrap();
    let mut fixture = fixture;
    fixture["search_tabs"][0]["has_more"] = json!(false);
    let mut gateway = fake(vec![fixture]);
    assert_eq!(
        search_with(dir.path(), &mut gateway, "南洋当总统")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn invalid_shapes_and_nested_rate_errors_are_safe() {
    for value in [
        json!(null),
        json!([]),
        json!({}),
        json!({"data":null}),
        json!({"search_tabs":[]}),
        json!({"search_tabs":[{"tab_type":2,"data":[]}]}),
        json!({"data":[],"passback":{"unexpected":1}}),
    ] {
        assert!(parse_page(&value).is_err());
        let dir = tempfile::tempdir().unwrap();
        let mut gateway = fake(vec![value]);
        let error = search_report_with(dir.path(), &mut gateway, WANTED).unwrap_err();
        assert_eq!(error.downcast_ref::<Fault>(), Some(&Fault::InvalidResponse));
        assert_eq!(gateway.cursors, ["0"]);
    }
    assert_eq!(
        parse_page(&json!({"data":{"code":429,"message":"SECRET"}})).err(),
        Some(Fault::RateLimited)
    );
    let invalid = json!({"data":[{"book_data":[
        {"book_id":1.5,"title":WANTED}, {"book_id":"01","title":WANTED},
        {"book_id":"18446744073709551616","title":WANTED},
        {"book_id":"1","title":""}
    ]}]});
    assert!(parse_page(&invalid).unwrap().hits.is_empty());
}

#[test]
fn exact_title_is_not_a_similar_title() {
    assert_eq!(
        normalized_title(WANTED),
        normalized_title("《校长写日记骂我, 我在南洋当总统》")
    );
    assert_ne!(normalized_title(WANTED), normalized_title(SIMILAR));
    assert_ne!(normalized_title(WANTED), normalized_title("校长写日记骂我"));
}

struct Fake {
    pages: VecDeque<AppResult<SearchPage>>,
    cursors: Vec<String>,
    registrations: usize,
    paced: bool,
    devices: Vec<(String, String, String, String)>,
    terms: Vec<String>,
    clock: u64,
    waits: Vec<u64>,
    cancel_during_wait: Option<crate::net::Cancellation>,
}
impl Gateway for Fake {
    fn pace(&self) -> bool {
        self.paced
    }
    fn register_device(&mut self) -> AppResult<Device> {
        self.registrations += 1;
        Ok(device())
    }
    fn register_key(&mut self, _: &Device) -> AppResult<(Vec<u8>, Option<Value>)> {
        panic!("search must not register a chapter key")
    }
    fn fetch(&mut self, _: &Credentials, _: &str, _: &str) -> AppResult<String> {
        panic!("search must not fetch chapters")
    }
}
impl SearchGateway for Fake {
    fn clock_ms(&self) -> u64 {
        self.clock
    }
    fn wait(&mut self, millis: u64) {
        self.waits.push(millis);
        if let Some(cancel) = self.cancel_during_wait.take() {
            cancel.cancel();
            return;
        }
        self.clock = self.clock.saturating_add(millis);
    }
    fn search_page(&mut self, device: &Device, term: &str, cursor: &str) -> AppResult<SearchPage> {
        self.devices.push((
            device.device_id.clone(),
            device.install_id.clone(),
            device.device_type.clone(),
            device.device_brand.clone(),
        ));
        self.terms.push(term.to_owned());
        self.cursors.push(cursor.to_owned());
        self.pages
            .pop_front()
            .expect("unexpected additional search page")
    }
}
fn fake(pages: Vec<Value>) -> Fake {
    Fake {
        pages: pages.iter().map(parse_page).collect(),
        cursors: Vec::new(),
        registrations: 0,
        paced: false,
        devices: Vec::new(),
        terms: Vec::new(),
        clock: 1_000_000,
        waits: Vec::new(),
        cancel_during_wait: None,
    }
}

fn search_report_with(dir: &Path, gateway: &mut Fake, term: &str) -> Result<SearchReport> {
    search_page_with(dir, gateway, term, None)
}

fn more_with(dir: &Path, gateway: &mut Fake, previous: &SearchReport) -> Result<SearchReport> {
    let continuation = previous
        .continuation
        .as_ref()
        .expect("next cursor expected");
    search_page_with(dir, gateway, &continuation.query, Some(continuation))
}

fn search_with(dir: &Path, gateway: &mut Fake, term: &str) -> Result<Vec<Book>> {
    Ok(search_report_with(dir, gateway, term)?.books)
}

#[test]
fn one_page_per_action_remains_bounded_even_after_an_exact_hit() {
    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![
        page(vec![record("1", WANTED)], true, "opaque+cursor"),
        page(vec![], true, "next"),
        page(vec![record("2", SIMILAR)], true, "never"),
    ]);
    let first = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
    assert_eq!(gateway.cursors, ["0"]);
    assert_eq!(first.books[0].title, WANTED);
    assert!(first.notice.is_none());

    let second = more_with(dir.path(), &mut gateway, &first).unwrap();
    assert_eq!(gateway.cursors, ["0", "opaque+cursor"]);
    assert!(second.books.is_empty());

    let third = more_with(dir.path(), &mut gateway, &second).unwrap();
    assert_eq!(third.books[0].id, "2");
    assert!(third.continuation.is_none());
    assert!(third.notice.unwrap().contains("3 页"));
    assert_eq!(gateway.cursors, ["0", "opaque+cursor", "next"]);
    assert_eq!(gateway.terms, [WANTED, WANTED, WANTED]);
    assert_eq!(gateway.registrations, 1);
}

#[test]
fn original_name_is_displayed_when_it_matches_and_the_current_name_is_cached() {
    let dir = tempfile::tempdir().unwrap();
    let mut entry = record("1", "现书名");
    entry["original_book_name"] = json!(WANTED);
    let mut gateway = fake(vec![page(vec![entry], true, "next")]);
    let report = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
    assert_eq!(report.books.len(), 1);
    assert_eq!(report.books[0].id, "1");
    assert_eq!(report.books[0].title, WANTED);
    assert!(report
        .cache_aliases
        .iter()
        .any(|book| book.title == "现书名"));
    assert!(report.continuation.is_some());
    assert_eq!(gateway.cursors, ["0"]);
}

#[test]
fn best_matching_title_is_chosen_without_mixing_metadata_between_records() {
    let dir = tempfile::tempdir().unwrap();
    let mut exact = record("1", "带前缀的查询书名");
    exact["book_name"] = json!("官方显示名");
    exact["original_book_name"] = json!("查询书名");
    exact["author"] = json!("对应作者");
    let mut gateway = fake(vec![page(vec![record("1", "旧书名"), exact], false, "")]);
    let report = search_report_with(dir.path(), &mut gateway, "《查询书名》").unwrap();
    assert_eq!(report.books.len(), 1);
    assert_eq!(report.books[0].id, "1");
    assert_eq!(report.books[0].title, "查询书名");
    assert_eq!(report.books[0].author, "对应作者");
    assert_eq!(report.cache_aliases.len(), 4);
}

#[test]
fn exhausted_missing_or_repeated_cursors_do_not_offer_more() {
    let dir = tempfile::tempdir().unwrap();
    for pages in [
        vec![page(vec![], false, "1")],
        vec![page(vec![], true, "0")],
        vec![page(vec![], true, "")],
        vec![page(vec![], true, "1"), page(vec![], true, "1")],
        vec![page(vec![], true, "1"), page(vec![], true, "0")],
    ] {
        let expected = pages.len();
        let mut gateway = fake(pages);
        let mut report = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
        if expected == 2 {
            report = more_with(dir.path(), &mut gateway, &report).unwrap();
        }
        assert!(report.books.is_empty());
        assert!(report.continuation.is_none());
        assert_eq!(gateway.cursors.len(), expected);
    }
}

#[test]
fn a_continuation_rejects_a_changed_device_without_requesting() {
    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![page(vec![], true, "next")]);
    let first = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
    let mut state = load_cache(dir.path()).unwrap().unwrap();
    state.device.device_id = "222222222".into();
    save_cache(dir.path(), &state).unwrap();

    let error = more_with(dir.path(), &mut gateway, &first).unwrap_err();
    assert!(error.to_string().contains("设备已更改"));
    assert_eq!(gateway.cursors, ["0"]);
    assert_eq!(gateway.registrations, 1);
}

#[test]
fn cancellation_while_waiting_for_next_page_does_not_send_or_change_pacing() {
    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![page(vec![record("1", WANTED)], true, "next")]);
    gateway.paced = true;
    let first = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
    let before = load_cache(dir.path()).unwrap().unwrap().next_request_ms;
    let cancel = crate::net::Cancellation::new();
    gateway.cancel_during_wait = Some(cancel.clone());
    let error = cancel
        .scope(|| more_with(dir.path(), &mut gateway, &first))
        .unwrap_err();
    assert_eq!(error.downcast_ref::<Fault>(), Some(&Fault::Cancelled));
    assert_eq!(gateway.cursors, ["0"]);
    assert_eq!(gateway.waits, [0, REQUEST_INTERVAL_MS]);
    assert_eq!(
        load_cache(dir.path()).unwrap().unwrap().next_request_ms,
        before
    );
}

#[test]
fn rate_errors_persist_cooldown_without_retry_or_renewal() {
    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![]);
    gateway.paced = true;
    gateway.pages.push_back(Err(Fault::RateLimited));
    let error = search_report_with(dir.path(), &mut gateway, WANTED).unwrap_err();
    assert!(is_rate_limited(&error));
    assert!(format!("{error:#}").contains("请求受限"));
    assert_eq!(gateway.cursors, ["0"]);
    assert_eq!(gateway.registrations, 1);
    let mut state = load_cache(dir.path()).unwrap().unwrap();
    assert_eq!(state.next_request_ms, gateway.clock + RATE_COOLDOWN_MS);
    assert!(state.key.is_none());

    // A subsequent search waits the whole explicit cooldown before requesting.
    let mut next = fake(vec![page(vec![], false, "")]);
    next.paced = true;
    next.clock = gateway.clock;
    search_report_with(dir.path(), &mut next, WANTED).unwrap();
    assert_eq!(next.waits, [RATE_COOLDOWN_MS]);
    assert_eq!(next.registrations, 0);

    // Longer persisted cooldowns do not get truncated to the usual 60 seconds.
    state.next_request_ms = next.clock + 120_000;
    save_cache(dir.path(), &state).unwrap();
    let mut blocked = fake(vec![]);
    blocked.paced = true;
    blocked.clock = next.clock;
    let error = search_report_with(dir.path(), &mut blocked, WANTED).unwrap_err();
    assert!(is_rate_limited(&error));
    assert!(error.to_string().contains("冷却期"));
    assert!(blocked.cursors.is_empty());
    assert_eq!(blocked.registrations, 0);
}

#[test]
fn transient_empty_or_network_retries_same_cursor_and_device_after_pacing() {
    for failure in [Fault::EmptyResponse, Fault::Network] {
        let dir = tempfile::tempdir().unwrap();
        let mut gateway = fake(vec![]);
        gateway.paced = true;
        gateway.pages.extend([
            Err(failure),
            parse_page(&page(vec![record("1", WANTED)], true, "unused")),
        ]);
        let report = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
        assert_eq!(report.books.len(), 1);
        assert!(report.notice.is_none());
        assert_eq!(gateway.cursors, ["0", "0"]);
        assert_eq!(gateway.terms, [WANTED, WANTED]);
        assert_eq!(gateway.devices[0], gateway.devices[1]);
        assert_eq!(gateway.registrations, 1);
        assert_eq!(gateway.waits, [0, REQUEST_INTERVAL_MS]);
        let state = load_cache(dir.path()).unwrap().unwrap();
        assert_eq!(state.device.device_id, gateway.devices[0].0);
        assert!(state.key.is_none());
    }
}

#[test]
fn two_transient_failures_stop_without_successful_empty_results() {
    for failure in [Fault::EmptyResponse, Fault::Network] {
        let dir = tempfile::tempdir().unwrap();
        let mut gateway = fake(vec![]);
        gateway.pages.extend([Err(failure), Err(failure)]);
        let error = search_report_with(dir.path(), &mut gateway, WANTED).unwrap_err();
        assert_eq!(error.downcast_ref::<Fault>(), Some(&failure));
        assert!(!is_rate_limited(&error));
        assert_eq!(gateway.cursors, ["0", "0"]);
        assert_eq!(gateway.devices[0], gateway.devices[1]);
        assert_eq!(gateway.registrations, 1);
    }
}

#[test]
fn malformed_permission_and_expiry_faults_are_not_retried() {
    for failure in [
        Fault::InvalidResponse,
        Fault::Rejected,
        Fault::DeviceExpired,
        Fault::KeyExpired,
        Fault::InvalidContent,
        Fault::IdentityMismatch,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut gateway = fake(vec![]);
        gateway.pages.push_back(Err(failure));
        let error = search_report_with(dir.path(), &mut gateway, WANTED).unwrap_err();
        assert_eq!(error.downcast_ref::<Fault>(), Some(&failure));
        assert_eq!(gateway.cursors, ["0"]);
        assert_eq!(gateway.registrations, 1);
    }
}

#[test]
fn later_page_failures_leave_previous_results_and_cursor_retryable() {
    for failure in [
        Fault::EmptyResponse,
        Fault::Network,
        Fault::InvalidResponse,
        Fault::Rejected,
        Fault::DeviceExpired,
        Fault::RateLimited,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut gateway = fake(vec![page(vec![record("1", SIMILAR)], true, "opaque")]);
        gateway.pages.push_back(Err(failure));
        let transient = matches!(failure, Fault::EmptyResponse | Fault::Network);
        if transient {
            gateway.pages.push_back(Err(failure));
        }
        let report = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
        assert_eq!(report.books.len(), 1);
        assert_eq!(report.books[0].id, "1");
        report.books[0].validate().unwrap();
        gateway.paced = true;
        let error = more_with(dir.path(), &mut gateway, &report).unwrap_err();
        assert_eq!(error.downcast_ref::<Fault>(), Some(&failure));
        assert!(error.to_string().contains("第 2 页失败"));
        assert!(report.notice.is_none());
        assert_eq!(gateway.cursors.len(), if transient { 3 } else { 2 });
        assert!(gateway.cursors[1..].iter().all(|cursor| cursor == "opaque"));
        assert!(gateway
            .devices
            .iter()
            .all(|device| device == &gateway.devices[0]));
        assert_eq!(gateway.registrations, 1);

        gateway
            .pages
            .push_back(parse_page(&page(vec![record("2", WANTED)], false, "")));
        let next = more_with(dir.path(), &mut gateway, &report).unwrap();
        assert_eq!(next.books[0].id, "2");
        assert_eq!(gateway.cursors.last().unwrap(), "opaque");
        if failure == Fault::RateLimited {
            assert_eq!(gateway.waits.last(), Some(&RATE_COOLDOWN_MS));
        }
    }
}

#[test]
fn empty_successful_page_followed_by_failure_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![page(vec![], true, "next")]);
    gateway.pages.push_back(Err(Fault::InvalidResponse));
    let first = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
    assert!(first.books.is_empty());
    assert_eq!(gateway.cursors, ["0"]);
    assert!(more_with(dir.path(), &mut gateway, &first).is_err());
    assert_eq!(gateway.cursors, ["0", "next"]);
}

#[test]
fn retry_rate_limit_stops_and_each_of_three_pages_has_only_one_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![]);
    gateway.paced = true;
    gateway
        .pages
        .extend([Err(Fault::EmptyResponse), Err(Fault::RateLimited)]);
    let error = search_report_with(dir.path(), &mut gateway, WANTED).unwrap_err();
    assert!(is_rate_limited(&error));
    assert_eq!(gateway.cursors, ["0", "0"]);
    assert_eq!(gateway.waits, [0, REQUEST_INTERVAL_MS]);
    assert_eq!(
        load_cache(dir.path()).unwrap().unwrap().next_request_ms,
        gateway.clock + RATE_COOLDOWN_MS
    );

    let dir = tempfile::tempdir().unwrap();
    let mut gateway = fake(vec![]);
    gateway.paced = true;
    for (id, cursor) in [("1", "next"), ("2", "last"), ("3", "unused")] {
        gateway.pages.push_back(Err(Fault::Network));
        gateway
            .pages
            .push_back(parse_page(&page(vec![record(id, SIMILAR)], true, cursor)));
    }
    let first = search_report_with(dir.path(), &mut gateway, WANTED).unwrap();
    assert_eq!(gateway.cursors, ["0", "0"]);
    let second = more_with(dir.path(), &mut gateway, &first).unwrap();
    assert_eq!(gateway.cursors, ["0", "0", "next", "next"]);
    let report = more_with(dir.path(), &mut gateway, &second).unwrap();
    assert_eq!(report.books[0].id, "3");
    assert!(report.notice.unwrap().contains("3 页"));
    assert_eq!(gateway.cursors, ["0", "0", "next", "next", "last", "last"]);
    assert_eq!(gateway.waits, [0, 6_000, 6_000, 6_000, 6_000, 6_000]);
    assert_eq!(gateway.registrations, 1);
    assert!(gateway
        .devices
        .iter()
        .all(|device| device == &gateway.devices[0]));
}

#[test]
fn invalid_search_does_not_create_storage_and_query_is_encoded() {
    let dir = tempfile::tempdir().unwrap();
    for term in [
        "".to_owned(),
        " \t".to_owned(),
        "x\nsecret".to_owned(),
        "书".repeat(101),
    ] {
        assert!(search(dir.path(), &term).is_err());
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    let encoded = query(
        &device(),
        &[
            ("query", WANTED),
            ("passback", "a+b&c"),
            ("selected_items", ""),
            ("tab_type", "3"),
        ],
    );
    let pairs: Vec<_> = url::form_urlencoded::parse(encoded.as_bytes()).collect();
    assert!(pairs
        .iter()
        .any(|(key, value)| key == "query" && value == WANTED));
    assert!(pairs
        .iter()
        .any(|(key, value)| key == "passback" && value == "a+b&c"));
}

/// Manual: set FANQIE_APP_DATA_DIR to an existing private device directory;
/// optional newline-separated FANQIE_APP_SEARCH_QUERIES. An expected title
/// assertion never accepts a similar title. Output is public book metadata only.
#[test]
#[ignore = "live signed official search; explicit private device directory required"]
fn live_search() {
    let dir = std::env::var_os("FANQIE_APP_DATA_DIR").expect("set FANQIE_APP_DATA_DIR");
    let terms = std::env::var("FANQIE_APP_SEARCH_QUERIES").unwrap_or_else(|_| WANTED.into());
    let expected = std::env::var("FANQIE_APP_EXPECT_TITLE").unwrap_or_else(|_| WANTED.into());
    let mut all_found = true;
    let mut tested = 0;
    for term in terms.lines().filter(|s| !s.trim().is_empty()) {
        tested += 1;
        let mut found = false;
        let books = search(Path::new(&dir), term).expect("官方 APP 实时搜索失败");
        eprintln!(
            "官方 APP 查询：{}；首页返回 {} 本（无关推荐已排除，匹配推荐保留标记）",
            clean(term),
            books.len(),
        );
        for book in books {
            let exact = normalized_title(&book.title) == normalized_title(&expected);
            found |= exact;
            eprintln!(
                "书名={}；ID={}；作者={}；目标书名精确匹配={}",
                book.title, book.id, book.author, exact
            );
        }
        all_found &= found;
    }
    assert!(
        tested > 0 && all_found,
        "至少一个官方 APP 查询未发现预期的完整书名；相似书名不算成功"
    );
}
