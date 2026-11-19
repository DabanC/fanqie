use super::*;
fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
// Generated from the Python reference; OpenSSL supplies only its AES backend.
#[test]
fn primitive_vectors() {
    assert_eq!(
        hex(&sm3(b"abc")),
        "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
    );
    assert_eq!(
        hex(&sm3(&[b'a'; 64])),
        "616ec433c359e7c2b19f360e2b8f2a1b6e9ed76b8dc1a7d207b31a5341c611e9"
    );
    assert_eq!(
        simon([1, 2], [3, 4, 5, 6]),
        [14627215761150969683, 13380517976421277178]
    );
    assert_eq!(
        hex(&speck(&std::array::from_fn(|i| i as u8), b"hello")),
        "970bdd3ea486b6b9e6203b01888b8101"
    );
}
#[test]
fn signing_vectors() {
    let h = sign_at(
        "device_id=123456789&version_name=7.0.1.32&a=x%20y",
        Some(b"hello"),
        1700000000123,
        123456789,
        [0, 1, 2, 3],
    )
    .unwrap();
    assert_eq!(h["x-argus"],"8oEzzYYQyj8eTZ27kE36t4leAIHdoIk7gpm/pA5oXyaDQPdspbGCc9h/PWd7n4hzZLVKeujUUUSmf3nIusnT4st5j5bkLyGqiwxNx/cpX/ZTbu8IOYuaJUrp507N9ixKLKC1r+Ksl0KYwD9HWE5J4SLVYB59UpHFRxI7civ8dDQ5FlU3545S3bykZWF/1lUQDxJoI+ofKUJOsL6YYaNkBTp2lYfIWp1gAKzf3oNlMiNpMNLlqlFg3RIyyiMXrufMKFEfVWRbxxrMYdPedcxjuUjr");
    assert_eq!(
        h["x-ladon"],
        "AAECAxldfsveM4b2yUqRx76tSh+ZH95OOqV9QmSE27uyCI5y"
    );
    assert_eq!(h["x-khronos"], "1700000000");
    assert_eq!(h["x-ss-req-ticket"], "1700000000123");
    assert_eq!(h["x-ss-stub"], "5d41402abc4b2a76b9719d911017c592");
    assert!(!sign_at("", Some(b""), 0, 0, [0; 4])
        .unwrap()
        .contains_key("x-ss-stub"));
}
#[test]
fn tt_fixture_vector() {
    let entropy = std::array::from_fn(|i| i as u8);
    assert_eq!(hex(&tt_derive(&entropy)),"70bf044d2853f0a51dde9276af723096d89e403db9a79aab6fe435399bee8a0d31daced0f57c2ca7800aefd700095fd21f516600f54ec0664bb9f3444a4176e0");
    // Use Python's fixed gzip bytes: compressor implementations can differ.
    let fixture = unhex("1f8b08000000000000ffcb48cdc9c9070086a6103605000000");
    let out = tt_compressed(&fixture, entropy).unwrap();
    assert_eq!(hex(&out),"746305100000000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f379bdba6cfd2ae478dc51063deee4e74034c9b7510b1d4d62d6ee18277b7bbaf191faf349bc2d5a36d192dccf5176960451dc9e4396bfaef7e4a85b9fefdbec3a5f5fba1498b51459673e33a513428390bf3901ec8aae1866af8a1b1aa2a27af");
    let d = tt_derive(&entropy);
    let plain = decrypt(&d[..16], &d[16..32], &out[38..]).unwrap();
    assert_eq!(&plain[..64], Sha512::digest(&fixture).as_slice());
    let mut decoder = flate2::read::GzDecoder::new(&plain[64..]);
    let mut decompressed = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut decompressed).unwrap();
    assert_eq!(decompressed, b"hello");
}
#[test]
fn aes_envelopes_and_padding() {
    let shared = unhex("ac25c67ddd8f38c1b37a2348828e222e");
    let iv: [u8; 16] = std::array::from_fn(|i| i as u8);
    // registerkey uses the low eight bytes of the integer device ID, LE.
    let id: u128 = (1u128 << 100) + 123456789;
    let le = id.to_le_bytes();
    let encrypted = encrypt(&shared, &iv, &le[..8]).unwrap();
    assert_eq!(hex(&encrypted), "d9b552dcd85bfc83648f424dfdbf969a");
    assert_eq!(decrypt(&shared, &iv, &encrypted).unwrap(), le[..8]);
    for size in [16, 24, 32] {
        let key = vec![0x42; size];
        for n in [0, 1, 15, 16, 17, 64] {
            let data = vec![0x61; n];
            assert_eq!(
                decrypt(&key, &iv, &encrypt(&key, &iv, &data).unwrap()).unwrap(),
                data
            );
        }
    }
    assert!(decrypt(&shared, &iv, &[]).is_err());
    assert!(decrypt(&shared, &iv, &[0; 15]).is_err());
    assert!(encrypt(&shared, &iv[..15], b"x").is_err());
    assert!(encrypt(&[0; 17], &iv, b"x").is_err());
    // Modify the preceding CBC block to create each invalid padding form.
    let valid = encrypt(&shared, &iv, &[0; 16]).unwrap();
    for (offset, mask) in [(15, 16), (15, 1), (0, 1)] {
        let mut invalid = valid.clone();
        invalid[offset] ^= mask;
        assert!(decrypt(&shared, &iv, &invalid).is_err());
    }
}
