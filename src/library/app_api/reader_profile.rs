//! Shared APP profile for search and chapter requests.
use serde_json::{json, Value};

use super::{AppResult, Credentials, Device, Fault};

pub(super) const BASE: &str = "https://api5-normal-sinfonlinec.fqnovel.com/reading";
pub(super) const CACHE_DIR: &str = "official-app-reader-74532";
const VERSION_CODE: &str = "74532";
const VERSION_NAME: &str = "7.4.5.32";

pub(super) fn prepare_registration(body: &mut Value) {
    let version = VERSION_CODE.parse::<u32>().expect("numeric app version");
    let header = &mut body["header"];
    for field in [
        "version_code",
        "manifest_version_code",
        "update_version_code",
    ] {
        header[field] = json!(version);
    }
    header["version_name"] = json!(VERSION_NAME);
    header["os_version"] = json!("11");
    header["os_api"] = json!(28);
}

pub(super) fn key_query(device: &Device) -> AppResult<String> {
    let cdid = device.cdid.as_deref().ok_or(Fault::InvalidResponse)?;
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.extend_pairs([
        ("iid", device.install_id.as_str()),
        ("device_id", device.device_id.as_str()),
        ("ac", "wifi"),
        ("channel", "43536163a"),
        ("aid", "1967"),
        ("app_name", "novelapp"),
        ("version_code", VERSION_CODE),
        ("version_name", VERSION_NAME),
        ("device_platform", "android"),
        ("os", "android"),
        ("ssmix", "a"),
        ("os_version", "10"),
        ("device_type", device.device_type.as_str()),
        ("device_brand", device.device_brand.as_str()),
        ("update_version_code", VERSION_CODE),
        ("manifest_version_code", VERSION_CODE),
        ("pv_player", VERSION_CODE),
        ("cdid", cdid),
    ]);
    Ok(query.finish())
}

pub(super) fn chapter_query(
    state: &Credentials,
    book: &str,
    chapter: &str,
    now: u64,
) -> AppResult<String> {
    let ts = state.key_register_ts.unwrap_or_default().to_string();
    profile_query(
        &state.device,
        now,
        &[
            ("item_ids", chapter),
            ("book_id", book),
            ("req_type", "1"),
            ("key_register_ts", &ts),
        ],
    )
}

pub(super) fn search_query(
    device: &Device,
    term: &str,
    cursor: &str,
    now: u64,
) -> AppResult<String> {
    profile_query(
        device,
        now,
        &[
            ("query", term),
            ("passback", cursor),
            ("selected_items", ""),
            ("tab_type", "3"),
        ],
    )
}

fn profile_query(device: &Device, now: u64, extra: &[(&str, &str)]) -> AppResult<String> {
    let cdid = device.cdid.as_deref().ok_or(Fault::InvalidResponse)?;
    let ticket = now.to_string();
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.extend_pairs([
        ("iid", device.install_id.as_str()),
        ("device_id", device.device_id.as_str()),
        ("ac", "wifi"),
        ("channel", "43536163a"),
        ("aid", "1967"),
        ("app_name", "novelapp"),
        ("version_code", VERSION_CODE),
        ("version_name", VERSION_NAME),
        ("device_platform", "android"),
        ("os", "android"),
        ("ssmix", "a"),
        ("os_api", "28"),
        ("os_version", "11"),
        ("device_type", device.device_type.as_str()),
        ("device_brand", device.device_brand.as_str()),
        ("resolution", "1280*720"),
        ("language", "zh"),
        ("dpi", "240"),
        ("manifest_version_code", VERSION_CODE),
        ("update_version_code", VERSION_CODE),
        ("host_abi", "x86_64"),
        ("is_android_pad_screen", "1"),
        ("dragon_device_type", "pad"),
        ("player_so_load", "1"),
        ("pv_player", VERSION_CODE),
        ("compliance_status", "0"),
        ("need_personal_recommend", "1"),
        ("_rticket", &ticket),
    ]);
    query.extend_pairs(extra.iter().copied());
    query.append_pair("cdid", cdid);
    Ok(query.finish())
}
