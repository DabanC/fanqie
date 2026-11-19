use super::content::paragraphs;
use super::*;
use std::{collections::VecDeque, fs, io::Write, path::PathBuf};

const KEY: &[u8] = b"0123456789abcdef";
// IV || AES-CBC ciphertext produced by the unchanged Python reference.
const RAW: &str = "YWJjZGVmZ2hpamtsbW5vcNmbVQ3x2Z1YDPkNYJrNp9Tu9Sq4T24DfhnsxirXDLArA6M3QyjR6RV9C3yb+PNix9iF6+4ftCi250gB7ed1RNrIN1n1Zf5sVDG2Eq/NWZhV";
const GZIP: &str = "YWJjZGVmZ2hpamtsbW5vcARKBEwcksUP4Ex1hZkzId7mifaBMbetL38MZGHsAbcHPRjH8Fg//+nqm4EeIsDJi6ADPP48+YU2tlFyEpmkkysoyILdjN6UOzjo0/lSHDSY";
const REGISTER_KEY: &str = "YWJjZGVmZ2hpamtsbW5vcHaLxcbge57lGDxecN+/eTowDoVHAmN+gRN10Me42oN3";

fn device() -> Device {
    Device {
        device_id: "123456789".into(),
        install_id: "987654321".into(),
        device_type: "P30".into(),
        device_brand: "realme".into(),
    }
}
fn state() -> Credentials {
    Credentials {
        schema: 1,
        device: device(),
        key: Some(B64.encode(KEY)),
        key_version: Some(json!(1)),
        next_request_ms: 0,
    }
}
fn payload(content: &str, compressed: u8) -> Value {
    json!({"code": 0, "data": {"456": {"book_id":"123", "item_id":"456", "content":content, "compress_status":compressed}}})
}

fn http_response(status: &str, body: &[u8], declared_length: Option<usize>) -> AppResult<Value> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let status = status.to_owned();
    let body = body.to_vec();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let length = stream.read(&mut buffer).unwrap();
            if length == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..length]);
            assert!(request.len() < 16 * 1024);
        }
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            declared_length.unwrap_or(body.len())
        );
        stream.write_all(headers.as_bytes()).unwrap();
        let _ = stream.write_all(&body);
    });
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let result = AppGateway(client.clone())
        .response(client.get(format!("http://{address}/?secret=DO_NOT_LOG")));
    if let Err(error) = &result {
        assert!(!format!("{error:?}").contains("DO_NOT_LOG"));
    }
    server.join().unwrap();
    result
}

#[test]
fn http_empty_transport_schema_permission_and_rate_faults_are_distinct() {
    assert_eq!(
        http_response("200 OK", b"", None),
        Err(Fault::EmptyResponse)
    );
    for body in [
        b" ".as_slice(),
        b"{",
        b"null",
        b"<html>permission denied</html>",
    ] {
        let result = http_response("200 OK", body, None);
        // Valid JSON null is subsequently rejected by the search schema, not retried.
        if body == b"null" {
            assert_eq!(result, Ok(Value::Null));
        } else {
            assert_eq!(result, Err(Fault::InvalidResponse));
        }
    }
    assert_eq!(http_response("200 OK", b"{", Some(20)), Err(Fault::Network));
    assert_eq!(
        http_response("200 OK", b"", Some(MAX_BYTES + 1)),
        Err(Fault::InvalidResponse)
    );
    assert_eq!(
        http_response("403 Forbidden", b"", None),
        Err(Fault::Rejected)
    );
    assert_eq!(
        http_response("429 Too Many Requests", b"", None),
        Err(Fault::RateLimited)
    );
    assert_eq!(
        http_response("200 OK", br#"{"code":429}"#, None),
        Err(Fault::RateLimited)
    );
    assert_eq!(
        http_response("200 OK", br#"{"code":403}"#, None),
        Err(Fault::Rejected)
    );
}

#[test]
fn rate_limit_helper_checks_typed_chain_not_error_messages() {
    let error = anyhow::Error::new(Fault::RateLimited)
        .context("search failed")
        .context("outer context");
    assert!(is_rate_limited(&error));
    assert!(!is_rate_limited(&anyhow!("{}", Fault::RateLimited)));
    for fault in [
        Fault::Network,
        Fault::EmptyResponse,
        Fault::InvalidResponse,
        Fault::Rejected,
    ] {
        let error = anyhow::Error::new(fault).context("请求受限");
        assert!(!is_rate_limited(&error));
    }
}

#[test]
fn gzip_members_and_explicit_encryption_metadata() {
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut writer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        writer.write_all(bytes).unwrap();
        writer.finish().unwrap()
    }
    let mut members = gzip(b"<p>first</p>");
    members.extend(gzip(b"<p>second</p>"));
    let iv = b"abcdefghijklmnop";
    let mut envelope = iv.to_vec();
    envelope.extend(crypto::encrypt(KEY, iv, &members).unwrap());
    assert_eq!(
        parse_response(&payload(&B64.encode(envelope), 1), "123", "456", KEY).unwrap(),
        "first\n\nsecond"
    );
    let mut plain = payload(&B64.encode(&members), 1);
    plain["data"]["456"]["encrypt_status"] = json!(0);
    assert_eq!(
        parse_response(&plain, "123", "456", KEY).unwrap(),
        "first\n\nsecond"
    );
    members.extend_from_slice(b"trailing garbage");
    plain["data"]["456"]["content"] = json!(B64.encode(&members));
    assert_eq!(
        parse_response(&plain, "123", "456", KEY),
        Err(Fault::InvalidContent)
    );
    let mut conflicting = payload("<p>plaintext</p>", 0);
    conflicting["data"]["456"]["encrypt_status"] = json!(1);
    assert_eq!(
        parse_response(&conflicting, "123", "456", KEY),
        Err(Fault::InvalidContent)
    );
    conflicting["data"]["456"]["encrypt_status"] = json!(99);
    assert_eq!(
        parse_response(&conflicting, "123", "456", KEY),
        Err(Fault::InvalidContent)
    );
    let mut locked = payload("<p>teaser</p>", 0);
    locked["data"]["456"]["needPay"] = json!(1);
    assert_eq!(
        parse_response(&locked, "123", "456", KEY),
        Err(Fault::Rejected)
    );
}

#[test]
fn cache_version_bound_and_trusted_directory() {
    let dir = tempfile::tempdir().unwrap();
    save_cache(dir.path(), &state()).unwrap();
    let original = fs::read(dir.path().join(CACHE)).unwrap();
    let mut invalid = state();
    invalid.key_version = Some(json!({"untrusted": "x".repeat(20_000)}));
    assert!(save_cache(dir.path(), &invalid).is_err());
    assert_eq!(fs::read(dir.path().join(CACHE)).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        drop(DeviceLock::acquire(dir.path()).unwrap());
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(DeviceLock::acquire(dir.path()).is_err());
    }
}

#[test]
fn reference_chapter_and_registerkey_vectors() {
    assert_eq!(decrypt_envelope(REGISTER_KEY, &SHARED_KEY).unwrap(), KEY);
    for (encoded, compressed) in [(RAW, 0), (GZIP, 1)] {
        assert_eq!(
            parse_response(&payload(encoded, compressed), "123", "456", KEY).unwrap(),
            "one & two\n\nnext"
        );
    }
    assert!(parse_response(&payload(RAW, 0), "123", "456", b"wrongwrongwrongwr").is_err());
    assert!(parse_response(&payload("YWJj", 0), "123", "456", KEY).is_err());
}

#[test]
fn plaintext_xhtml_and_controls() {
    let html = "<html><head><style>hidden</style></head><body><article><h1><blk>Title</blk></h1><p><blk>one&nbsp;&amp; <b>two</b></blk><script>secret</script></p><p>next<br/>line\u{1b}\u{200b}\u{202e}</p></article></body></html>";
    assert_eq!(
        parse_response(&payload(html, 0), "123", "456", KEY).unwrap(),
        "one & two\n\nnext line"
    );
    assert_eq!(
        paragraphs("<h1><blk>Title</blk></h1><blk>body</blk>").unwrap(),
        "body"
    );
    let mut plain = payload("first\n\nsecond\0", 0);
    plain["data"]["456"]["encrypt_status"] = json!(0);
    assert_eq!(
        parse_response(&plain, "123", "456", KEY).unwrap(),
        "first\n\nsecond"
    );
    assert!(paragraphs("<p>\u{e123}</p>").is_err());
    assert!(paragraphs("<h1>Only a title</h1><style>hidden</style>").is_err());
}

#[test]
fn reject_mismatched_or_missing_identity() {
    let good = payload("<p>text</p>", 0);
    for (book, chapter) in [("999", "456"), ("123", "999")] {
        assert!(parse_response(&good, book, chapter, KEY).is_err());
    }
    for name in ["item_id", "chapter_id", "chapterId"] {
        let mut bad = good.clone();
        bad["data"]["456"][name] = json!("999");
        assert_eq!(
            parse_response(&bad, "123", "456", KEY),
            Err(Fault::IdentityMismatch)
        );
    }
    let mut bad = good.clone();
    bad["data"]["999"] = good["data"]["456"].clone();
    assert_eq!(
        parse_response(&bad, "123", "456", KEY),
        Err(Fault::IdentityMismatch)
    );
    let mut bad = good.clone();
    bad["book_id"] = json!("999");
    assert_eq!(
        parse_response(&bad, "123", "456", KEY),
        Err(Fault::IdentityMismatch)
    );
    let mut numeric = good;
    numeric["data"]["456"]["item_id"] = json!(456);
    numeric["data"]["456"]["book_id"] = json!(123);
    assert_eq!(parse_response(&numeric, "123", "456", KEY).unwrap(), "text");
}

#[test]
fn missing_and_rate_limited_are_not_expired() {
    for value in [
        json!({}),
        json!({"code":0,"data":{}}),
        json!({"code":0,"data":null}),
    ] {
        assert_eq!(
            parse_response(&value, "123", "456", KEY),
            Err(Fault::InvalidResponse)
        );
    }
    assert_eq!(
        check_status(&json!({"code":429,"message":"device_expired"})),
        Err(Fault::RateLimited)
    );
    assert_eq!(
        check_status(&json!({"code":"429","message":"device_expired"})),
        Err(Fault::RateLimited)
    );
    assert_eq!(
        check_status(&json!({"code":123,"status_code":"429","message":"device_expired"})),
        Err(Fault::RateLimited)
    );
    assert_eq!(
        check_status(&json!({"code":0,"status_code":123,"message":"device_expired"})),
        Err(Fault::Rejected)
    );
    assert_eq!(
        check_status(&json!({"code":123,"message":"device_expired"})),
        Err(Fault::DeviceExpired)
    );
    assert_eq!(
        check_status(&json!({"code":123,"message":"device invalid"})),
        Err(Fault::Rejected)
    );
    assert_eq!(
        parse_response(&payload("Invalid", 0), "123", "456", KEY),
        Err(Fault::KeyExpired)
    );
    let mut bad = payload(RAW, 0);
    bad["data"]["456"]["compress_status"] = json!("1");
    assert_eq!(
        parse_response(&bad, "123", "456", KEY),
        Err(Fault::InvalidContent)
    );
}

#[test]
fn strict_ids_and_single_chapter_query() {
    for bad in [
        "",
        "0",
        "01",
        "-1",
        "1,2",
        " 1",
        "1/2",
        "18446744073709551616",
        "1?x=y",
    ] {
        assert!(valid_id(bad).is_err());
    }
    assert!(valid_id("18446744073709551615").is_ok());
    let q = query(&device(), &[("item_ids", "456"), ("book_id", "123")]);
    let pairs: Vec<_> = url::form_urlencoded::parse(q.as_bytes()).collect();
    assert_eq!(pairs.iter().filter(|(k, _)| k == "item_ids").count(), 1);
    assert!(pairs.iter().any(|(k, v)| k == "item_ids" && v == "456"));
    assert!(pairs
        .iter()
        .any(|(k, v)| k == "device_id" && v == "123456789"));
    assert!(pairs
        .iter()
        .any(|(k, v)| k == "version_name" && v == "7.0.1.32"));
    let dir = tempfile::tempdir().unwrap();
    assert!(chapter(dir.path(), "bad", "456").is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

struct Fake {
    devices: usize,
    keys: usize,
    fetches: usize,
    answers: VecDeque<AppResult<String>>,
}
impl Fake {
    fn new(answers: Vec<AppResult<String>>) -> Self {
        Self {
            devices: 0,
            keys: 0,
            fetches: 0,
            answers: answers.into(),
        }
    }
}
impl Gateway for Fake {
    fn register_device(&mut self) -> AppResult<Device> {
        self.devices += 1;
        Ok(device())
    }
    fn register_key(&mut self, _: &Device) -> AppResult<(Vec<u8>, Option<Value>)> {
        self.keys += 1;
        Ok((KEY.to_vec(), Some(json!(1))))
    }
    fn fetch(&mut self, _: &Credentials, book: &str, chapter: &str) -> AppResult<String> {
        assert_eq!((book, chapter), ("123", "456"));
        self.fetches += 1;
        self.answers
            .pop_front()
            .expect("unexpected extra chapter request")
    }
}

#[test]
fn renewal_budget_and_rate_limit_policy() {
    for failure in [
        Fault::RateLimited,
        Fault::InvalidResponse,
        Fault::Network,
        Fault::Rejected,
    ] {
        let dir = tempfile::tempdir().unwrap();
        save_cache(dir.path(), &state()).unwrap();
        let mut fake = Fake::new(vec![Err(failure)]);
        assert!(read_one(dir.path(), &mut fake, "123", "456").is_err());
        assert_eq!((fake.devices, fake.keys, fake.fetches), (0, 0, 1));
    }
    let dir = tempfile::tempdir().unwrap();
    save_cache(dir.path(), &state()).unwrap();
    let mut fake = Fake::new(vec![Err(Fault::DeviceExpired), Err(Fault::DeviceExpired)]);
    assert!(read_one(dir.path(), &mut fake, "123", "456").is_err());
    assert_eq!((fake.devices, fake.keys, fake.fetches), (1, 1, 2));

    let dir = tempfile::tempdir().unwrap();
    save_cache(dir.path(), &state()).unwrap();
    let mut fake = Fake::new(vec![
        Err(Fault::KeyExpired),
        Err(Fault::DeviceExpired),
        Ok("body".into()),
    ]);
    assert_eq!(
        read_one(dir.path(), &mut fake, "123", "456").unwrap(),
        "body"
    );
    assert_eq!((fake.devices, fake.keys, fake.fetches), (1, 2, 3));

    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::new(vec![Err(Fault::DeviceExpired), Err(Fault::DeviceExpired)]);
    assert!(read_one(dir.path(), &mut fake, "123", "456").is_err());
    assert_eq!((fake.devices, fake.keys, fake.fetches), (2, 2, 2));
}

#[test]
fn refresh_key_once_and_reuse_private_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::new(vec![Ok("body".into()), Ok("body".into())]);
    assert_eq!(
        read_one(dir.path(), &mut fake, "123", "456").unwrap(),
        "body"
    );
    assert_eq!(
        read_one(dir.path(), &mut fake, "123", "456").unwrap(),
        "body"
    );
    assert_eq!((fake.devices, fake.keys, fake.fetches), (1, 1, 2));
    let mut fake = Fake::new(vec![Err(Fault::KeyExpired), Err(Fault::KeyExpired)]);
    assert!(read_one(dir.path(), &mut fake, "123", "456").is_err());
    assert_eq!((fake.devices, fake.keys, fake.fetches), (0, 1, 2));
}

#[cfg(unix)]
#[test]
fn atomic_cache_mode_and_lock() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let lock = DeviceLock::acquire(dir.path()).unwrap();
    assert!(DeviceLock::acquire(dir.path()).is_err());
    assert_eq!(
        fs::metadata(dir.path().join(LOCK))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    save_cache(dir.path(), &state()).unwrap();
    save_cache(dir.path(), &state()).unwrap();
    let path = dir.path().join(CACHE);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(load_cache(dir.path()).unwrap().unwrap().key().unwrap(), KEY);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load_cache(dir.path()).is_err());
    fs::remove_file(&path).unwrap();
    let target = dir.path().join("external");
    fs::write(&target, "sensitive").unwrap();
    symlink(&target, &path).unwrap();
    assert!(load_cache(dir.path()).is_err());
    drop(lock);
    assert!(DeviceLock::acquire(dir.path()).is_ok());
}

#[cfg(windows)]
#[test]
fn windows_atomic_cache_replacement_and_cleanup() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("nested").join("data");
    let lock = DeviceLock::acquire(&dir).unwrap();
    assert!(DeviceLock::acquire(&dir).is_err());
    assert!(load_cache(&dir).unwrap().is_none());
    save_cache(&dir, &state()).unwrap();

    let path = dir.join(CACHE);
    let original = fs::read(&path).unwrap();
    let blocker = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .unwrap();
    let mut replacement = state();
    replacement.next_request_ms = 123456;
    assert!(save_cache(&dir, &replacement).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    drop(blocker);

    save_cache(&dir, &replacement).unwrap();
    let loaded = load_cache(&dir).unwrap().unwrap();
    assert_eq!(loaded.next_request_ms, 123456);
    assert_eq!(loaded.key().unwrap(), KEY);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    drop(lock);
    assert!(DeviceLock::acquire(&dir).is_ok());
}

#[cfg(windows)]
#[test]
fn windows_rejects_shared_acls_and_linked_cache() {
    use super::device::windows::grant_everyone_full_access;

    let dir = tempfile::tempdir().unwrap();
    drop(DeviceLock::acquire(dir.path()).unwrap());
    save_cache(dir.path(), &state()).unwrap();
    let path = dir.path().join(CACHE);
    let alias = dir.path().join("alias.json");
    fs::hard_link(&path, &alias).unwrap();
    assert!(load_cache(dir.path()).is_err());
    assert!(save_cache(dir.path(), &state()).is_err());
    fs::remove_file(alias).unwrap();
    assert!(load_cache(dir.path()).unwrap().is_some());

    let original = fs::read(&path).unwrap();
    grant_everyone_full_access(&path).unwrap();
    assert!(load_cache(dir.path()).is_err());
    assert!(save_cache(dir.path(), &state()).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);

    let marker = dir.path().join(LOCK);
    grant_everyone_full_access(&marker).unwrap();
    assert!(DeviceLock::acquire(dir.path()).is_err());
    fs::remove_file(&marker).unwrap();
    grant_everyone_full_access(dir.path()).unwrap();
    assert!(DeviceLock::acquire(dir.path()).is_err());
    assert!(!marker.exists());
}

#[cfg(windows)]
#[test]
fn windows_rejects_reparse_point_cache_and_marker() {
    use std::os::windows::fs::symlink_file;
    use windows_sys::Win32::Foundation::ERROR_PRIVILEGE_NOT_HELD;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::write(&target, b"unchanged").unwrap();
    let cache = dir.path().join(CACHE);
    match symlink_file(&target, &cache) {
        Ok(()) => {}
        // Windows requires Developer Mode or a privilege to create the test fixture.
        Err(error) if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD as i32) => return,
        Err(error) => panic!("could not create reparse point: {error}"),
    }
    assert!(load_cache(dir.path()).is_err());
    assert!(save_cache(dir.path(), &state()).is_err());
    symlink_file(&target, dir.path().join(LOCK)).unwrap();
    assert!(DeviceLock::acquire(dir.path()).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"unchanged");
}

#[cfg(any(unix, windows))]
#[test]
fn lock_released_even_on_process_exit() {
    if let Some(path) = std::env::var_os("FANQIE_LOCK_TEST_DIR") {
        if std::env::var_os("FANQIE_LOCK_TEST_HELD").is_some() {
            assert!(DeviceLock::acquire(Path::new(&path)).is_err());
            return;
        }
        let _lock = DeviceLock::acquire(Path::new(&path)).unwrap();
        // Deliberately skip Drop, as when the terminal closes mid-request.
        std::process::exit(0);
    }
    let dir = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "library::app_api::tests::lock_released_even_on_process_exit",
            "--nocapture",
        ])
        .env("FANQIE_LOCK_TEST_DIR", dir.path());
    let lock = DeviceLock::acquire(dir.path()).unwrap();
    assert!(child
        .env("FANQIE_LOCK_TEST_HELD", "1")
        .status()
        .unwrap()
        .success());
    drop(lock);
    let status = child.env_remove("FANQIE_LOCK_TEST_HELD").status().unwrap();
    assert!(status.success());
    assert!(dir.path().join(LOCK).exists());
    assert!(DeviceLock::acquire(dir.path()).is_ok());
}

#[test]
fn errors_do_not_expose_device_or_key_material() {
    let dir = tempfile::tempdir().unwrap();
    save_cache(dir.path(), &state()).unwrap();
    fs::write(
        dir.path().join(CACHE),
        br#"{"device_id":"SECRET_DEVICE","key":"SECRET_KEY"}"#,
    )
    .unwrap();
    let error = load_cache(dir.path()).err().unwrap();
    let text = format!("{error:#?}");
    assert!(!text.contains("SECRET"));
    for fault in [
        Fault::Network,
        Fault::Rejected,
        Fault::InvalidResponse,
        Fault::InvalidContent,
    ] {
        let text = format!("{:#?}", anyhow::Error::from(fault));
        assert!(!text.contains("123456789"));
        assert!(!text.contains("0123456789abcdef"));
    }
}

/// Manual: set FANQIE_APP_BOOK_ID and FANQIE_APP_CHAPTER_ID to one known
/// matching chapter (preferably beyond the website preview). Optionally set
/// FANQIE_APP_DATA_DIR to reuse this reader's anonymous device across runs.
#[test]
#[ignore = "live network, creates one anonymous device; explicit IDs required"]
fn live_chapter() {
    let book = std::env::var("FANQIE_APP_BOOK_ID").expect("set FANQIE_APP_BOOK_ID");
    let chapter_id = std::env::var("FANQIE_APP_CHAPTER_ID").expect("set FANQIE_APP_CHAPTER_ID");
    let temp = tempfile::tempdir().unwrap();
    let dir = std::env::var_os("FANQIE_APP_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| temp.path().to_owned());
    let text = chapter(&dir, &book, &chapter_id).expect("anonymous app read failed");
    assert!(!text.trim().is_empty());
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
    // Never print the device, key, response body, or signed URL.
    eprintln!(
        "official-app chapter returned {} characters",
        text.chars().count()
    );
}
