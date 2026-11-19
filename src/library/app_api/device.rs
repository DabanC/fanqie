use super::{AppResult, Fault};
#[cfg(unix)]
use super::{CACHE, LOCK};
use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(unix)]
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::Write;
use std::{
    fs::File,
    io::Read,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
pub(super) mod windows;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Device {
    pub(super) device_id: String,
    pub(super) install_id: String,
    pub(super) device_type: String,
    pub(super) device_brand: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Credentials {
    pub(super) schema: u8,
    pub(super) device: Device,
    pub(super) key: Option<String>,
    pub(super) key_version: Option<Value>,
    #[serde(default)]
    pub(super) next_request_ms: u64,
}
impl Credentials {
    pub(super) fn key(&self) -> AppResult<Vec<u8>> {
        let key = B64
            .decode(self.key.as_deref().ok_or(Fault::InvalidContent)?)
            .map_err(|_| Fault::InvalidContent)?;
        if !matches!(key.len(), 16 | 24 | 32) {
            return Err(Fault::InvalidContent);
        }
        Ok(key)
    }
}

pub(super) fn valid_id(id: &str) -> Result<&str> {
    if id.is_empty()
        || id.len() > 20
        || id.starts_with('0')
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.parse::<u64>().is_err()
    {
        bail!("书籍和章节 ID 必须是不含前导零的非零十进制 u64 整数");
    }
    Ok(id)
}

// Never include reqwest/serde/crypto errors: they can contain signed URLs or secrets.
fn private_error() -> anyhow::Error {
    anyhow!("无法访问官方 APP 匿名设备的私有存储")
}

#[cfg(unix)]
fn private_create(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    options.open(path).map_err(|_| private_error())
}

pub(super) struct DeviceLock {
    _file: File,
    #[cfg(windows)]
    _directory: File,
}
impl DeviceLock {
    #[cfg(unix)]
    pub(super) fn acquire(dir: &Path) -> Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir).map_err(|_| private_error())?;
        let directory = fs::symlink_metadata(dir).map_err(|_| private_error())?;
        if !directory.file_type().is_dir() {
            return Err(private_error());
        }
        if directory.permissions().mode() & 0o022 != 0 {
            return Err(private_error());
        }
        let path = dir.join(LOCK);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.file_type().is_file() {
                return Err(private_error());
            }
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600);
        let file = options.open(&path).map_err(|_| private_error())?;
        let metadata = file.metadata().map_err(|_| private_error())?;
        if !metadata.is_file()
            || directory.uid() != metadata.uid()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err(private_error());
        }
        lock_file(&file)?;
        Ok(Self { _file: file })
    }

    #[cfg(windows)]
    pub(super) fn acquire(dir: &Path) -> Result<Self> {
        let (directory, file) = windows::open_lock(dir)?;
        lock_file(&file)?;
        Ok(Self {
            _file: file,
            _directory: directory,
        })
    }

    #[cfg(not(any(unix, windows)))]
    pub(super) fn acquire(_: &Path) -> Result<Self> {
        Err(private_error())
    }
}

#[cfg(any(unix, windows))]
fn lock_file(file: &File) -> Result<()> {
    // Kernel locks survive neither process exit nor Ctrl+Q; the marker may
    // remain on disk without permanently blocking a future reader.
    file.try_lock()
        .map_err(|_| anyhow!("另一个阅读进程正在请求番茄正文，请稍后重试"))
}

const MAX_CACHE: u64 = 16 * 1024;

#[cfg(unix)]
fn open_cache(dir: &Path) -> Result<Option<File>> {
    use std::os::unix::fs::MetadataExt;
    let path = dir.join(CACHE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(private_error()),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_CACHE {
        return Err(private_error());
    }
    let file = File::open(&path).map_err(|_| private_error())?;
    let opened = file.metadata().map_err(|_| private_error())?;
    let directory = fs::metadata(dir).map_err(|_| private_error())?;
    if !opened.is_file()
        || opened.mode() & 0o777 != 0o600
        || opened.uid() != directory.uid()
        || opened.dev() != metadata.dev()
        || opened.ino() != metadata.ino()
    {
        return Err(private_error());
    }
    Ok(Some(file))
}

#[cfg(windows)]
use windows::open_cache;

#[cfg(not(any(unix, windows)))]
fn open_cache(_: &Path) -> Result<Option<File>> {
    Err(private_error())
}

pub(super) fn load_cache(dir: &Path) -> Result<Option<Credentials>> {
    let Some(file) = open_cache(dir)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_CACHE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| private_error())?;
    if bytes.len() as u64 > MAX_CACHE {
        return Err(private_error());
    }
    let state: Credentials = serde_json::from_slice(&bytes).map_err(|_| private_error())?;
    if state.schema != 1
        || valid_id(&state.device.device_id).is_err()
        || valid_id(&state.device.install_id).is_err()
        || state.device.device_type.is_empty()
        || state.device.device_type.len() > 64
        || state.device.device_brand.is_empty()
        || state.device.device_brand.len() > 64
        || !valid_key_version(state.key_version.as_ref())
        || (state.key.is_some() && state.key().is_err())
    {
        return Err(private_error());
    }
    Ok(Some(state))
}

#[cfg(any(unix, windows))]
fn credential_bytes(state: &Credentials) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(state).map_err(|_| private_error())?;
    if bytes.len() as u64 > MAX_CACHE || !valid_key_version(state.key_version.as_ref()) {
        return Err(private_error());
    }
    Ok(bytes)
}

#[cfg(unix)]
pub(super) fn save_cache(dir: &Path, state: &Credentials) -> Result<()> {
    let tmp = dir.join(format!(".official-app-{}.tmp", random_hex(16)));
    let mut file = private_create(&tmp)?;
    let result = (|| {
        let bytes = credential_bytes(state)?;
        file.write_all(&bytes).map_err(|_| private_error())?;
        file.sync_all().map_err(|_| private_error())?;
        fs::rename(&tmp, dir.join(CACHE)).map_err(|_| private_error())?;
        File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| private_error())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(windows)]
pub(super) fn save_cache(dir: &Path, state: &Credentials) -> Result<()> {
    windows::save_cache(dir, &credential_bytes(state)?)
}

#[cfg(not(any(unix, windows)))]
pub(super) fn save_cache(_: &Path, _: &Credentials) -> Result<()> {
    Err(private_error())
}

pub(super) fn valid_key_version(version: Option<&Value>) -> bool {
    match version {
        None | Some(Value::Null) => true,
        Some(Value::Number(n)) => n.as_u64().is_some(),
        Some(Value::String(s)) => {
            !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
    }
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{:x}", b % 16)).collect()
}
fn uuid() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 15) | 0x40;
    b[8] = (b[8] & 63) | 0x80;
    let h: String = b.iter().map(|v| format!("{v:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}
fn ipv6() -> String {
    let mut rng = rand::thread_rng();
    let mut s = format!("fd{:02x}", rng.gen::<u8>());
    for _ in 0..7 {
        s.push_str(&format!(":{:04x}", rng.gen::<u16>()));
    }
    s
}
pub(super) fn registration_body() -> Value {
    let mut rng = rand::thread_rng();
    let versions = [
        ("9", 29),
        ("10", 30),
        ("11", 31),
        ("12", 32),
        ("13", 33),
        ("14", 34),
    ];
    let (os_version, os_api) = versions[rng.gen_range(0..versions.len())];
    let models = [
        "RMX1931",
        "MI8",
        "Honor10",
        "P30",
        "V1921A",
        "Redmi Note 7",
        "Redmi K20 Pro",
        "MI 9",
        "Mi 10 Pro",
        "SM-G9750",
        "Pixel 6",
        "HD1910",
        "M2011K2C",
        "LIO-AN00",
        "VOG-TL00",
        "PCLM10",
        "GM1900",
        "Pixel 7 Pro",
        "Pixel 6a",
        "SM-N9760",
        "POCO F1",
    ];
    let brands = [
        "realme", "Xiaomi", "Huawei", "OPPO", "vivo", "samsung", "OnePlus", "google", "Redmi",
        "HONOR", "motorola", "POCO",
    ];
    let brand = brands[rng.gen_range(0..brands.len())];
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let rom = format!(
        "coloros__{}.{:06}.{:08} release-keys",
        random_hex(4).to_uppercase(),
        rng.gen_range(0..1_000_000),
        rng.gen_range(0..100_000_000)
    );
    let mut header = json!({
            "display_name": "\u{756a}\u{8304}\u{514d}\u{8d39}\u{5c0f}\u{8bf4}", "aid": 1967, "channel": "43536163a",
            "package": "com.dragon.read", "sdk_version": "3.7.0-rc.25-fanqie-xiaoshuo",
            "sdk_target_version": 29, "git_hash": "711d1a7", "density_dpi": 240,
            "display_density": "hdpi", "resolution": "720x1280", "language": "zh",
            "timezone": 8, "access": "wifi", "not_request_sender": 0,
            "carrier": "CHINA MOBILE", "mcc_mnc": "46000", "region": "CN",
            "tz_name": "Asia/Shanghai", "tz_offset": 28800, "sim_region": "cn",
    });
    let extra = json!({
            "sim_serial_number": [], "oaid_may_support": false, "device_platform": "android",
            "custom": {"host_bit": 32, "dragon_device_type": 0},
            "pre_installed_channel": "", "is_system_app": 0, "sdk_flavor": "china",
            "guest_mode": 0, "os": "Android", "os_version": os_version, "os_api": os_api,
            "device_model": models[rng.gen_range(0..models.len())], "device_brand": brand,
            "device_manufacturer": brand, "cpu_abi": "arm64-v8a", "release_build": random_hex(7),
            "cdid": uuid(), "sig_hash": "a4a27c2633195374c15651ffc3c4a497", "openudid": random_hex(20),
            "clientudid": uuid(), "req_id": uuid(), "rom": random_hex(14).to_uppercase(), "rom_version": rom,
            "apk_first_install_time": now.saturating_sub(rng.gen_range(0..365u64) * 86_400_000),
            "ipv6_list": [{"type":"client_anpi","value":"::1"}, {"type":"client_anpi","value":ipv6()}, {"type":"client_anpi","value":ipv6()}]
    });
    header
        .as_object_mut()
        .expect("registration header object")
        .extend(
            extra
                .as_object()
                .expect("registration extra object")
                .clone(),
        );
    json!({"magic_tag": "ss_app_log", "_gen_time": now, "header": header})
}
