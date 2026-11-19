// Protocol port of Jees996/fanqie-novel-downloader core/appapi.py (MIT).
// Copyright 2026 xian lee; upstream commit 486f2844b1c5e5e113cb4f0abf46b75e995f0c4f.
// License: licenses/fanqie-novel-downloader-MIT.txt.
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use md5::Md5;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha512};
use std::io::Write;

const SIGN_KEY: [u8; 32] = [
    0xac, 0x1a, 0xda, 0xae, 0x95, 0xa7, 0xaf, 0x94, 0xa5, 0x11, 0x4a, 0xb3, 0xb3, 0xa9, 0x7d, 0xd8,
    0x00, 0x50, 0xaa, 0x0a, 0x39, 0x31, 0x4c, 0x40, 0x52, 0x8c, 0xae, 0xc9, 0x52, 0x56, 0xc2, 0x8c,
];

fn pad(data: &[u8]) -> Vec<u8> {
    let n = 16 - data.len() % 16;
    let mut out = data.to_vec();
    out.resize(out.len() + n, n as u8);
    out
}

fn crypt(key: &[u8], iv: &[u8], data: &[u8], enc: bool) -> Result<Vec<u8>> {
    if iv.len() != 16 || (!enc && (data.is_empty() || !data.len().is_multiple_of(16))) {
        return Err(anyhow!("invalid AES envelope"));
    }
    macro_rules! cbc_crypt {
        ($cipher:ty) => {
            if enc {
                Ok(cbc::Encryptor::<$cipher>::new_from_slices(key, iv)
                    .map_err(|_| anyhow!("invalid AES parameters"))?
                    .encrypt_padded_vec_mut::<Pkcs7>(data))
            } else {
                cbc::Decryptor::<$cipher>::new_from_slices(key, iv)
                    .map_err(|_| anyhow!("invalid AES parameters"))?
                    .decrypt_padded_vec_mut::<Pkcs7>(data)
                    .map_err(|_| anyhow!("invalid AES envelope"))
            }
        };
    }
    match key.len() {
        16 => cbc_crypt!(aes::Aes128),
        24 => cbc_crypt!(aes::Aes192),
        32 => cbc_crypt!(aes::Aes256),
        _ => Err(anyhow!("invalid AES key length")),
    }
}
pub(super) fn encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    crypt(key, iv, data, true)
}
pub(super) fn decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    crypt(key, iv, data, false)
}

fn md5(data: &[u8]) -> Vec<u8> {
    Md5::digest(data).to_vec()
}
fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}
fn sm3(data: &[u8]) -> [u8; 32] {
    sm3::Sm3::digest(data).into()
}
fn word(data: &[u8]) -> u64 {
    u64::from_le_bytes(data.try_into().unwrap())
}
fn simon(pt: [u64; 2], key: [u64; 4]) -> [u64; 2] {
    let mut k = [0u64; 72];
    k[..4].copy_from_slice(&key);
    for i in 4..72 {
        let mut t = k[i - 1].rotate_right(3) ^ k[i - 3];
        t ^= t.rotate_right(1);
        k[i] = !k[i - 4] ^ t ^ ((0x3dc94c3a046d678bu64 >> ((i - 4) % 62)) & 1) ^ 3;
    }
    let [mut x, mut y] = pt;
    for key in k {
        let tmp = y;
        y = x ^ (y.rotate_left(1) & y.rotate_left(8)) ^ y.rotate_left(2) ^ key;
        x = tmp;
    }
    [x, y]
}
fn speck(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let mut ks = vec![word(&key[..8])];
    let mut ls: Vec<u64> = key[8..]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| word(bytes))
        .collect();
    for i in 0..33 {
        let x = ls[i].rotate_right(8).wrapping_add(ks[i]) ^ i as u64;
        ls.push(x);
        ks.push(x ^ ks[i].rotate_left(3));
    }
    let mut out = Vec::new();
    for block in pad(data).as_chunks::<16>().0 {
        let (mut y, mut x) = (word(&block[..8]), word(&block[8..]));
        for k in &ks {
            x = x.rotate_right(8).wrapping_add(y) ^ k;
            y = x ^ y.rotate_left(3);
        }
        out.extend_from_slice(&y.to_le_bytes());
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}
#[derive(Default)]
struct Proto(Vec<u8>);
impl Proto {
    fn raw(&mut self, mut n: u64) {
        while n >= 128 {
            self.0.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        self.0.push(n as u8);
    }
    fn int(&mut self, field: u64, n: u64) {
        self.raw(field << 3);
        self.raw(n);
    }
    fn bytes(&mut self, field: u64, data: &[u8]) {
        self.raw((field << 3) | 2);
        self.raw(data.len() as u64);
        self.0.extend_from_slice(data);
    }
}
fn argus(query: &str, body: Option<&[u8]>, ts: u64, entropy: u32) -> Result<String> {
    // Match Python's raw split and last-value-wins behavior, without URL decoding.
    let mut device = "";
    let mut version = "";
    for part in query.split('&') {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "device_id" => device = v,
                "version_name" => version = v,
                _ => {}
            }
        }
    }
    let body_md5 = body
        .filter(|b| !b.is_empty())
        .map(md5)
        .unwrap_or_else(|| vec![0; 16]);
    let bh = sm3(&body_md5);
    let qh = sm3(if query.is_empty() {
        &[0; 16]
    } else {
        query.as_bytes()
    });
    let mut p = Proto::default();
    p.int(1, 0x20200929 * 2);
    p.int(2, 2);
    p.int(3, (entropy & 0x7fffffff) as u64);
    for (f, s) in [
        (4, "1967"),
        (5, device),
        (6, "1611921764"),
        (7, version),
        (8, "v04.04.05-ov-android"),
    ] {
        p.bytes(f, s.as_bytes());
    }
    p.int(9, 134744640);
    p.bytes(10, &[0; 8]);
    p.int(11, 0);
    p.int(12, ts.wrapping_mul(2));
    p.bytes(13, &bh[..6]);
    p.bytes(14, &qh[..6]);
    let mut sub = Proto::default();
    sub.int(1, 1);
    sub.int(2, 1);
    sub.int(3, 1);
    sub.int(7, 3348294860);
    p.bytes(15, &sub.0);
    p.bytes(16, b"");
    p.bytes(20, b"none");
    p.int(21, 738);
    let mut sub = Proto::default();
    sub.bytes(1, b"NX551J");
    sub.int(2, 8196);
    sub.int(4, 2162219008);
    p.bytes(23, &sub.0);
    p.int(25, 2);
    let mut material = SIGN_KEY.to_vec();
    material.extend_from_slice(&[0xf2, 0x81, 0x61, 0x6f]);
    material.extend_from_slice(&SIGN_KEY);
    let digest = sm3(&material);
    let key = std::array::from_fn(|i| word(&digest[i * 8..i * 8 + 8]));
    let mut data = vec![0xf2, 0xf7, 0xfc, 0xff, 0xf2, 0xf7, 0xfc, 0xff];
    for block in pad(&p.0).as_chunks::<16>().0 {
        for w in simon([word(&block[..8]), word(&block[8..])], key) {
            data.extend_from_slice(&w.to_le_bytes());
        }
    }
    for i in 8..data.len() {
        data[i] ^= data[i % 8];
    }
    data.reverse();
    let mut plain = vec![0xa6, 0x6e, 0xad, 0x9f, 0x77, 1, 0xd0, 0x0c, 0x18];
    plain.extend(data);
    plain.extend_from_slice(&[0x61, 0x6f]);
    let mut out = vec![0xf2, 0x81];
    out.extend(encrypt(
        &md5(&SIGN_KEY[..16]),
        &md5(&SIGN_KEY[16..]),
        &plain,
    )?);
    Ok(STANDARD.encode(out))
}
fn ladon(ts: u64, entropy: [u8; 4]) -> String {
    let mut material = entropy.to_vec();
    material.extend_from_slice(b"1967");
    let key: [u8; 32] = hex(&md5(&material)).into_bytes().try_into().unwrap();
    let mut out = entropy.to_vec();
    out.extend(speck(&key, format!("{ts}-1611921764-1967").as_bytes()));
    STANDARD.encode(out)
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0; N];
    OsRng
        .try_fill_bytes(&mut out)
        .map_err(|_| anyhow!("secure entropy unavailable"))?;
    Ok(out)
}
fn sign_at(
    query: &str,
    body: Option<&[u8]>,
    now_ms: u64,
    argus_random: u32,
    ladon_random: [u8; 4],
) -> Result<reqwest::header::HeaderMap> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in [
        ("x-argus", argus(query, body, now_ms / 1000, argus_random)?),
        ("x-ladon", ladon(now_ms / 1000, ladon_random)),
        ("x-khronos", (now_ms / 1000).to_string()),
        ("x-ss-req-ticket", now_ms.to_string()),
    ] {
        headers.insert(
            name,
            value
                .parse()
                .map_err(|_| anyhow!("invalid signing header"))?,
        );
    }
    if let Some(b) = body.filter(|b| !b.is_empty()) {
        headers.insert(
            "x-ss-stub",
            hex(&md5(b))
                .parse()
                .map_err(|_| anyhow!("invalid signing header"))?,
        );
    }
    Ok(headers)
}
pub(super) fn sign(query: &str, body: Option<&[u8]>) -> Result<reqwest::header::HeaderMap> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| anyhow!("signing clock unavailable"))?
        .as_millis();
    let ms = u64::try_from(ms).map_err(|_| anyhow!("signing clock out of range"))?;
    sign_at(query, body, ms, u32::from_le_bytes(random()?), random()?)
}
fn tt_derive(entropy: &[u8; 32]) -> Vec<u8> {
    let fixed = STANDARD.decode("TdTC5rgxYgkOUrPHpnM7pByyRiuCmrWKGWs521cXdST0m69/COjWjSanLjfBqVovHwWlGJKu8pSXMrYqOKrdWA==").expect("fixed encoding");
    let mut material = Sha512::digest(entropy).to_vec();
    material.extend(fixed);
    Sha512::digest(material).to_vec()
}
fn tt_compressed(compressed: &[u8], entropy: [u8; 32]) -> Result<Vec<u8>> {
    let derived = tt_derive(&entropy);
    let mut plain = Sha512::digest(compressed).to_vec();
    plain.extend_from_slice(compressed);
    let mut out = vec![116, 99, 5, 16, 0, 0];
    out.extend_from_slice(&entropy);
    out.extend(encrypt(&derived[..16], &derived[16..32], &plain)?);
    Ok(out)
}
pub(super) fn tt_encrypt(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::best());
    encoder
        .write_all(data)
        .map_err(|_| anyhow!("registration compression failed"))?;
    let compressed = encoder
        .finish()
        .map_err(|_| anyhow!("registration compression failed"))?;
    tt_compressed(&compressed, random()?)
}

#[cfg(test)]
mod tests;
