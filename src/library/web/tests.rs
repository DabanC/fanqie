use super::*;
use serde_json::json;
use std::{collections::VecDeque, sync::Mutex};

type Calls = Arc<Mutex<Vec<String>>>;
fn fake(replies: Vec<std::result::Result<(u16, Vec<u8>), ()>>) -> (Remote, Calls) {
    let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let captured = calls.clone();
    let remote = Remote {
        content_api: String::new(),
        search_api: String::new(),
        data_dir: None,
        transport: Arc::new(move |url| {
            captured.lock().unwrap().push(url.to_owned());
            queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected network attempt")
                .map(|(status, bytes)| Response {
                    status,
                    bytes,
                    oversize: false,
                })
        }),
    };
    (remote, calls)
}
fn ok(text: &str) -> std::result::Result<(u16, Vec<u8>), ()> {
    Ok((200, text.as_bytes().to_vec()))
}
fn catalog_json() -> Value {
    json!({"data":{"bookInfo":{"bookName":"A book","author":"Writer","abstract":"<p>About</p>"},
            "chapterListWithVolume":[[{"itemId":"21","title":"One"},{"itemId":"22","title":"Two"}]]}})
}
fn book() -> Book {
    parse_catalog(&catalog_json(), "11").unwrap()
}

#[test]
fn catalog_directory_preserves_order_and_metadata() {
    let (remote, calls) = fake(vec![ok(&catalog_json().to_string())]);
    let result = remote.catalog("11").unwrap();
    assert_eq!(result.title, "A book");
    assert_eq!(result.author, "Writer");
    assert_eq!(result.description, "About");
    assert_eq!(
        result
            .chapters
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["21", "22"]
    );
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        ["https://fanqienovel.com/api/reader/directory/detail?bookId=11"]
    );
}

#[test]
fn page_fallback_supports_both_hydration_formats() {
    for script in [
        format!("window.__INITIAL_STATE__ = {};", catalog_json()),
        catalog_json().to_string(),
    ] {
        let page = format!(
            "<html><script id='__NEXT_DATA__' type='application/json'>{script}</script></html>"
        );
        let (remote, calls) = fake(vec![Ok((404, Vec::new())), ok(&page)]);
        assert_eq!(remote.catalog("11").unwrap().chapters.len(), 2);
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}

#[test]
fn volume_aliases_empty_and_bad_ids() {
    let value = json!({"book_name":"Novel","chapters":[{"id":"7","name":"Volume","chapter_list":[
            {"chapter_id":23,"chapter_name":"Three"},{"chapter_id":"23","chapter_name":"Three"}]}]});
    assert_eq!(parse_catalog(&value, "11").unwrap().chapters[0].id, "23");
    for value in [
        json!({"chapters":[]}),
        json!({"chapters":[{"id":"../2","title":"Bad"}]}),
    ] {
        assert!(parse_catalog(&value, "11").is_err());
    }
    assert!(page_catalog("<p>No chapters</p>", "11").is_err());
}

#[test]
fn batch_content_adds_one_id_preserves_other_query() {
    let (mut remote, calls) = fake(vec![ok(
        r#"{"data":{"22":{"content":"<p>Hello &amp; welcome.</p><script>secret()</script><p>Next.</p>"}}}"#,
    )]);
    remote.content_api =
        "https://example.test/reading/reader/batch_full/v?token=SECRET&item_ids=99".into();
    assert_eq!(
        remote.chapter(&book(), 1).unwrap(),
        "Hello & welcome.\nNext."
    );
    let guard = calls.lock().unwrap();
    let parsed = Url::parse(&guard[0]).unwrap();
    assert_eq!(
        parsed
            .query_pairs()
            .find(|(k, _)| k == "item_ids")
            .unwrap()
            .1,
        "22"
    );
    assert_eq!(
        parsed.query_pairs().find(|(k, _)| k == "token").unwrap().1,
        "SECRET"
    );
    assert_eq!(guard.len(), 1);
}

#[test]
fn url_templates_encode_values_and_do_not_add_parameters() {
    let url = api_url(
        "https://example.test/{book_id}/{chapter_id}?key=x",
        &[("book_id", "11"), ("chapter_id", "22")],
        ("item_ids", "22"),
    )
    .unwrap();
    assert_eq!(url, "https://example.test/11/22?key=x");
    let url = api_url(
        "https://example.test/search/{query}?key=x",
        &[("query", "a /&?#中")],
        ("q", "ignored"),
    )
    .unwrap();
    assert!(url.contains("a%20%2F%26%3F%23%E4%B8%AD"));
    assert!(!url.contains("q="));
    for bad in [
        "file:///tmp/key",
        "https://user:SECRET@example.test",
        "https://example.test/{unknown}",
    ] {
        let error = api_url(bad, &[("query", "x")], ("q", "x"))
            .unwrap_err()
            .to_string();
        assert!(!error.contains("SECRET"));
    }
}

#[test]
fn content_shapes_and_escaped_html() {
    for value in [
        json!({"data":{"content":"Short chapter."}}),
        json!({"result":{"chapter":{"body":"Short chapter."}}}),
        json!({"data":{"22":{"text":"Short chapter."}}}),
        json!({"content":["Short chapter."]}),
    ] {
        assert_eq!(json_content(&value, "22").unwrap(), "Short chapter.");
    }
    assert_eq!(
        json_content(
            &json!({"content":"<p>&lt;b&gt;literal&lt;/b&gt;</p>"}),
            "22"
        )
        .unwrap(),
        "<b>literal</b>"
    );
    for value in [
        json!({"data":{}}),
        json!({"code":403,"content":"blocked"}),
        json!({"encrypted":true,"content":"cipher"}),
        json!({"content":"\u{e001}"}),
        json!({"content":""}),
    ] {
        assert!(json_content(&value, "22")
            .unwrap_err()
            .to_string()
            .contains("/api"));
    }
}

#[test]
fn reader_requires_scoped_plausible_unobfuscated_plaintext() {
    let prose =
        "This is a sufficiently long chapter with ordinary words. The story continues at dawn.";
    assert_eq!(
        reader_content(&format!(
            "<nav>Login</nav><div class='muye-reader-content'><p>{prose}</p></div>"
        ))
        .unwrap(),
        prose
    );
    for raw in [format!("<body>{prose}</body>"),
                    "<div class='reader-content'>Please complete CAPTCHA</div>".into(),
                    format!("<div class='reader-content'>{prose}\u{f0000}</div>"),
                    "<div class='reader-content'><script>long hidden script text never is a chapter</script></div>".into(),
                    "<div class='reader-content'>购买本章</div>".into()] {
            assert!(reader_content(&raw).is_err());
        }
}

#[test]
fn retries_only_transient_statuses_and_transport_failures() {
    for status in [408, 500, 502, 503, 504] {
        let (remote, calls) = fake(vec![Ok((status, Vec::new())), Err(()), ok("done")]);
        assert_eq!(
            remote
                .request("https://example.test/?token=SECRET")
                .unwrap(),
            "done"
        );
        assert_eq!(calls.lock().unwrap().len(), 3);
    }
    for status in [301, 400, 401, 403, 404, 409, 429, 501] {
        let (remote, calls) = fake(vec![Ok((status, b"SECRET".to_vec()))]);
        let error = remote
            .request("https://example.test/?token=SECRET")
            .unwrap_err();
        assert!(!format!("{error:#}").contains("SECRET"));
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
    let (remote, calls) = fake(vec![Err(()), Err(()), Err(())]);
    assert!(remote.request("https://example.test/SECRET").is_err());
    assert_eq!(calls.lock().unwrap().len(), 3);
}

#[test]
fn finite_size_utf8_and_malformed_json_fail_without_retries() {
    for bytes in [vec![b'x'; MAX_BYTES + 1], vec![255]] {
        let (remote, calls) = fake(vec![Ok((200, bytes))]);
        assert!(remote.request("https://example.test").is_err());
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
    let (mut remote, calls) = fake(vec![ok("SECRET invalid JSON")]);
    remote.content_api = "https://example.test".into();
    assert!(!remote
        .chapter(&book(), 0)
        .unwrap_err()
        .to_string()
        .contains("SECRET"));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn official_search_is_default_and_custom_wrappers_still_work() {
    let payload = json!({"code":0,"data":{"ret_data":[
        {"book_id":"7143038691944959011","title":"十日终焉","author":"杀虫队队员","genre":"0"},
        {"book_id":"7281195245084691518","title":"十日终焉有声版","genre":"4"},
        {"book_id":"7143038691944959011","title":"重复书籍","genre":"0"}
    ]}});
    let (remote, calls) = fake(vec![ok(&payload.to_string())]);
    let found = remote.search("十日终焉").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "十日终焉");
    let calls = calls.lock().unwrap();
    let url = Url::parse(&calls[0]).unwrap();
    assert_eq!(url.host_str(), Some("novel.snssdk.com"));
    assert_eq!(
        url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
        "十日终焉"
    );
    for wrapper in [
        json!([{"book_id":"11","book_name":"Novel","author":"Writer"}]),
        json!({"data":{"results":[{"book_id":11,"book_name":"Novel","author":"Writer"}]}}),
    ] {
        let (mut remote, calls) = fake(vec![ok(&wrapper.to_string())]);
        remote.search_api = "https://example.test/find?key=SECRET".into();
        assert_eq!(remote.search("a & b").unwrap()[0].title, "Novel");
        let guard = calls.lock().unwrap();
        let url = Url::parse(&guard[0]).unwrap();
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "q").unwrap().1,
            "a & b"
        );
    }
    assert!(parse_search(&json!({"items":[{"book_id":"bad","book_name":"Novel"}]})).is_err());
}

#[test]
fn invalid_input_never_performs_io() {
    let (remote, calls) = fake(vec![]);
    for bad in [
        "",
        "../11",
        "1?token=SECRET",
        "-1",
        "0",
        "１２",
        "123456789012345678901234567890123",
    ] {
        assert!(remote.catalog(bad).is_err());
    }
    assert!(remote.chapter(&book(), 5).is_err());
    assert!(remote.search("\n").is_err());
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn bare_origins_derive_paths_without_replacing_explicit_endpoints() {
    for origin in ["https://example.test", "https://example.test/?token=SECRET"] {
        let url = api_url(
            origin,
            &[("chapter_id", "22"), ("book_id", "11")],
            ("item_ids", "22"),
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        assert_eq!(parsed.path(), "/reading/reader/batch_full/v");
        assert_eq!(
            parsed
                .query_pairs()
                .find(|(k, _)| k == "item_ids")
                .unwrap()
                .1,
            "22"
        );
    }
    let url = api_url(
        "https://example.test?token=SECRET",
        &[("query", "a b")],
        ("q", "a b"),
    )
    .unwrap();
    let parsed = Url::parse(&url).unwrap();
    assert_eq!(parsed.path(), "/api/search");
    assert_eq!(
        parsed.query_pairs().find(|(k, _)| k == "token").unwrap().1,
        "SECRET"
    );
    for (path, values, param) in [
        (
            "/custom-content",
            vec![("chapter_id", "22")],
            ("item_ids", "22"),
        ),
        ("/custom-search", vec![("query", "novel")], ("q", "novel")),
    ] {
        let url = api_url(&format!("https://example.test{path}?key=x"), &values, param).unwrap();
        assert_eq!(Url::parse(&url).unwrap().path(), path);
    }
}

#[test]
fn error_shaped_content_is_never_plaintext() {
    for text in [
        "login required",
        "error",
        "Forbidden",
        "请登录",
        "unauthorized",
        "service unavailable",
    ] {
        assert!(json_content(&json!({"content":text}), "22").is_err());
    }
    for value in [
        json!({"status":403,"content":"Short valid chapter."}),
        json!({"status":"error","content":"Short valid chapter."}),
        json!({"message":"login required","content":"Short valid chapter."}),
        json!({"error":"SECRET","content":"Short valid chapter."}),
    ] {
        let error = json_content(&value, "22").unwrap_err().to_string();
        assert!(error.contains("/api"));
        assert!(!error.contains("SECRET"));
    }
    assert_eq!(
        json_content(
            &json!({"status":"ok","message":"success","content":"Short valid chapter."}),
            "22"
        )
        .unwrap(),
        "Short valid chapter."
    );
}

#[test]
fn directory_metadata_is_enriched_from_metadata_only_page() {
    let directory = json!({"data":{"chapterList":[{"itemId":"21","title":"One"}]}});
    let page = r#"<script id="__NEXT_DATA__">{"props":{"bookInfo":{"title":"Real title","author":"Writer","description":"About"}}}</script>"#;
    let (remote, calls) = fake(vec![ok(&directory.to_string()), ok(page)]);
    let result = remote.catalog("11").unwrap();
    assert_eq!(result.title, "Real title");
    assert_eq!(result.author, "Writer");
    assert_eq!(result.description, "About");
    assert_eq!(result.chapters.len(), 1);
    assert_eq!(calls.lock().unwrap().len(), 2);
    let (remote, _) = fake(vec![ok(&directory.to_string()), Ok((404, Vec::new()))]);
    assert_eq!(remote.catalog("11").unwrap().chapters.len(), 1);
}

#[test]
fn default_reader_http_denial_provides_configuration_help() {
    for status in [401, 403] {
        let (remote, calls) = fake(vec![Ok((status, Vec::new()))]);
        assert!(remote
            .chapter(&book(), 0)
            .unwrap_err()
            .to_string()
            .contains("/api"));
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
    assert!(
        parse_search(&json!([{"book_id":"11","book_name":"Name\u{0007}","author":"Writer"}]))
            .is_err()
    );
    assert!(
        parse_search(&json!([{"book_id":"11","book_name":"Name","author":"Writer\u{0007}"}]))
            .is_err()
    );
}

#[test]
fn explicit_lock_and_encoding_flags_are_not_chapters() {
    for flag in [
        "need_pay",
        "needPay",
        "is_locked",
        "need_login",
        "encrypt",
        "is_encrypted",
    ] {
        for denied in [json!(true), json!(1), json!("true")] {
            let mut value = json!({"content":"Short valid chapter."});
            value[flag] = denied;
            assert!(json_content(&value, "22").is_err());
        }
    }
    assert!(json_content(
        &json!({"is_paid":true,"content":"The forbidden forest was quiet."}),
        "22"
    )
    .is_ok());
}

#[test]
fn locked_and_incomplete_reader_states_are_never_cached_as_full_chapters() {
    let prose = "This is a plausible long teaser, but it is not the complete chapter that the reader requested.";
    for chapter in [
        json!({"isChapterLock":true,"content":prose}),
        json!({"needPay":1,"content":prose}),
        json!({"chapterWordNumber":"5000","content":prose}),
    ] {
        let state = json!({"reader":{"chapterData":chapter}});
        let page = format!("<script>window.__INITIAL_STATE__={state};</script><div class='reader-content'>{prose}</div>");
        assert!(reader_content(&page).is_err());
    }
}

#[test]
fn app_empty_response_uses_official_fallback_with_an_explicit_notice() {
    let payload = json!({"code":0,"data":{"ret_data":[{"book_id":"7635637943461235736","title":"军阀：我奉化人，当个总统怎么了","genre":"0"}]}});
    let (remote, calls) = fake(vec![ok(&payload.to_string())]);
    let report = remote
        .recover_search("校长写日记骂我", Err(anyhow!("APP 响应为空")))
        .unwrap();
    assert_eq!(report.books[0].id, "7635637943461235736");
    assert!(report.notice.unwrap().contains("备用搜索"));
    assert!(report.continuation.is_none());
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn app_failure_and_fallback_failure_are_not_a_successful_empty_search() {
    let (remote, _) = fake(vec![Ok((403, Vec::new()))]);
    assert!(remote
        .recover_search("书名", Err(anyhow!("APP 响应为空")))
        .is_err());
    let (remote, calls) = fake(vec![]);
    let report = remote
        .recover_search("书名", Ok(SearchReport::default()))
        .unwrap();
    assert!(report.books.is_empty());
    assert!(report.notice.is_none());
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn cancelled_app_search_never_starts_fallback() {
    let (remote, calls) = fake(vec![]);
    let cancel = crate::net::Cancellation::new();
    cancel.cancel();
    cancel.scope(|| {
        assert!(remote
            .recover_search("书名", Err(anyhow!("操作已取消")))
            .is_err());
    });
    assert!(calls.lock().unwrap().is_empty());
}
