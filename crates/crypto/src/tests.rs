use super::*;
use base64::engine::general_purpose::STANDARD;
use uuid::Uuid;

fn key_line(version: u8, byte: u8) -> String {
    format!("v{version}:{}", STANDARD.encode([byte; 32]))
}

fn keys_v1() -> DataKeys {
    DataKeys::parse(&key_line(1, 0x11)).unwrap()
}

fn keys_v1_v2() -> DataKeys {
    DataKeys::parse(&format!("{}\n{}\n", key_line(1, 0x11), key_line(2, 0x22))).unwrap()
}

fn aad(row: Uuid) -> Aad<'static> {
    Aad::new("employees", "name", row).unwrap()
}

#[test]
fn aad_bytes_layout() {
    let id = Uuid::from_u128(7);
    let mut expected = b"employees\0name\0".to_vec();
    expected.extend_from_slice(id.as_bytes());
    assert_eq!(aad(id).to_bytes(), expected);
}

#[test]
fn aad_rejects_nul() {
    let id = Uuid::nil();
    assert!(matches!(
        Aad::new("a\0b", "c", id),
        Err(CryptoError::InvalidAad)
    ));
    assert!(matches!(
        Aad::new("a", "c\0", id),
        Err(CryptoError::InvalidAad)
    ));
}

#[test]
fn round_trip() {
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    let blob = keys.encrypt(b"Jan Peeters", &a);
    assert_eq!(blob[0], 1);
    assert_eq!(&*keys.decrypt(&blob, &a).unwrap(), b"Jan Peeters");
}

#[test]
fn empty_plaintext_round_trip() {
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    let blob = keys.encrypt(b"", &a);
    assert_eq!(blob.len(), 1 + 24 + 16);
    assert!(keys.decrypt(&blob, &a).unwrap().is_empty());
}

#[test]
fn wrong_aad_fails() {
    let keys = keys_v1();
    let id = Uuid::new_v4();
    let blob = keys.encrypt(b"secret", &aad(id));
    let other_row = aad(Uuid::new_v4());
    let other_table = Aad::new("sites", "name", id).unwrap();
    let other_column = Aad::new("employees", "email", id).unwrap();
    for bad in [other_row, other_table, other_column] {
        assert!(matches!(
            keys.decrypt(&blob, &bad),
            Err(CryptoError::Decrypt)
        ));
    }
}

#[test]
fn tampered_ciphertext_fails() {
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    let blob = keys.encrypt(b"secret", &a);
    for i in 1..blob.len() {
        let mut t = blob.clone();
        t[i] ^= 0x01;
        assert!(
            matches!(keys.decrypt(&t, &a), Err(CryptoError::Decrypt)),
            "byte {i}"
        );
    }
}

#[test]
fn truncated_blob_is_too_short() {
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    assert!(matches!(keys.decrypt(&[], &a), Err(CryptoError::TooShort)));
    assert!(matches!(
        keys.decrypt(&[1; 40], &a),
        Err(CryptoError::TooShort)
    ));
    let blob = keys.encrypt(b"x", &a);
    assert!(matches!(
        keys.decrypt(&blob[..1 + 24 + 15], &a),
        Err(CryptoError::TooShort)
    ));
    assert!(matches!(
        keys.decrypt(&blob[..blob.len() - 1], &a),
        Err(CryptoError::Decrypt)
    ));
}

#[test]
fn unknown_version_fails() {
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    let mut blob = keys.encrypt(b"x", &a);
    blob[0] = 9;
    assert!(matches!(
        keys.decrypt(&blob, &a),
        Err(CryptoError::UnknownKeyVersion(9))
    ));
}

#[test]
fn same_plaintext_encrypts_differently() {
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    assert_ne!(keys.encrypt(b"same", &a), keys.encrypt(b"same", &a));
}

#[test]
fn rotation() {
    let old = keys_v1();
    let a = aad(Uuid::new_v4());
    let blob = old.encrypt(b"data", &a);
    assert!(!old.needs_rotation(&blob));

    let keys = keys_v1_v2();
    assert_eq!(keys.current_version(), 2);
    assert_eq!(&*keys.decrypt(&blob, &a).unwrap(), b"data");
    assert!(keys.needs_rotation(&blob));

    let new_blob = keys.reencrypt(&blob, &a).unwrap();
    assert_eq!(new_blob[0], 2);
    assert!(!keys.needs_rotation(&new_blob));
    assert_eq!(&*keys.decrypt(&new_blob, &a).unwrap(), b"data");
    // the v1-only keyset cannot read the rotated blob
    assert!(matches!(
        old.decrypt(&new_blob, &a),
        Err(CryptoError::UnknownKeyVersion(2))
    ));
}

#[test]
fn reencrypt_with_wrong_aad_fails() {
    let keys = keys_v1_v2();
    let blob = keys.encrypt(b"data", &aad(Uuid::new_v4()));
    assert!(keys.reencrypt(&blob, &aad(Uuid::new_v4())).is_err());
}

#[test]
fn parse_ignores_comments_and_blank_lines() {
    let text = format!(
        "# comment\n\n  {}  \n# more\n{}\n",
        key_line(3, 1),
        key_line(1, 2)
    );
    let keys = DataKeys::parse(&text).unwrap();
    assert_eq!(keys.current_version(), 3);
}

#[test]
fn parse_errors() {
    assert!(matches!(DataKeys::parse(""), Err(CryptoError::NoKeys)));
    assert!(matches!(
        DataKeys::parse("# only\n\n"),
        Err(CryptoError::NoKeys)
    ));
    assert!(matches!(
        DataKeys::parse("v1:!!!not-base64!!!"),
        Err(CryptoError::InvalidKeyFile { line: 1, .. })
    ));
    let short = format!("v1:{}", STANDARD.encode([0u8; 31]));
    assert!(matches!(
        DataKeys::parse(&short),
        Err(CryptoError::InvalidKeyFile { line: 1, .. })
    ));
    let long = format!("v1:{}", STANDARD.encode([0u8; 33]));
    assert!(matches!(
        DataKeys::parse(&long),
        Err(CryptoError::InvalidKeyFile { line: 1, .. })
    ));
    let dup = format!("{}\n{}", key_line(1, 1), key_line(1, 2));
    assert!(matches!(
        DataKeys::parse(&dup),
        Err(CryptoError::DuplicateKeyVersion(1))
    ));
    for bad in ["v0:AAAA", "v256:AAAA", "1:AAAA", "vx:AAAA", "nocolon"] {
        assert!(
            matches!(
                DataKeys::parse(bad),
                Err(CryptoError::InvalidKeyFile { .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn parse_errors_do_not_leak_key_material() {
    let secret = STANDARD.encode([0xABu8; 33]);
    let err = DataKeys::parse(&format!("v1:{secret}")).unwrap_err();
    assert!(!err.to_string().contains(&secret));
    assert!(!format!("{err:?}").contains(&secret));
}

#[test]
fn from_file_reads_keys_and_reports_missing_file() {
    let dir = std::env::temp_dir().join(format!("crypto-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keys");
    std::fs::write(&path, key_line(1, 5)).unwrap();
    assert_eq!(DataKeys::from_file(&path).unwrap().current_version(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(matches!(
        DataKeys::from_file(&path),
        Err(CryptoError::Io(_))
    ));
}

#[test]
fn generated_key_line_parses() {
    let line = generate_key_line(4);
    assert!(line.starts_with("v4:"));
    assert_eq!(DataKeys::parse(&line).unwrap().current_version(), 4);
    assert_ne!(generate_key_line(1), generate_key_line(1));
    assert!(BlindIndexKey::parse(&generate_key_line(1)).is_ok());
}

#[test]
fn debug_has_no_key_material() {
    let keys = keys_v1_v2();
    let dbg = format!("{keys:?}");
    assert!(!dbg.contains("17") && !dbg.contains("34") && !dbg.contains("0x11"));
    assert!(!dbg.contains(&STANDARD.encode([0x11u8; 32])));
    assert!(dbg.contains("DataKeys"));
    let blind = BlindIndexKey::parse(&key_line(1, 0x11)).unwrap();
    let dbg = format!("{blind:?}");
    assert!(!dbg.contains(&STANDARD.encode([0x11u8; 32])));
    assert!(!dbg.contains("17"));
}

#[test]
fn encrypted_round_trip_and_debug() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Name {
        first: String,
        last: String,
    }
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    let value = Name {
        first: "Jan".into(),
        last: "Peeters".into(),
    };
    let enc = Encrypted::seal(&keys, &value, &a).unwrap();
    let dbg = format!("{enc:?}");
    assert_eq!(dbg, format!("Encrypted({} bytes)", enc.as_bytes().len()));
    let stored = Encrypted::<Name>::from_bytes(enc.as_bytes().to_vec());
    assert_eq!(stored.open(&keys, &a).unwrap(), value);
    assert!(stored.open(&keys, &aad(Uuid::new_v4())).is_err());
}

#[test]
fn encrypted_open_wrong_type_fails_without_detail() {
    let keys = keys_v1();
    let a = aad(Uuid::new_v4());
    let enc = Encrypted::seal(&keys, &"hunter2".to_string(), &a).unwrap();
    let wrong = Encrypted::<u32>::from_bytes(enc.into_bytes());
    let err = wrong.open(&keys, &a).unwrap_err();
    assert!(matches!(err, CryptoError::Deserialize));
    assert!(!err.to_string().contains("hunter2"));
}

#[test]
fn blind_index_is_deterministic_and_keyed() {
    let k1 = BlindIndexKey::parse(&key_line(1, 1)).unwrap();
    let k2 = BlindIndexKey::parse(&key_line(1, 2)).unwrap();
    assert_eq!(k1.index("jan peeters"), k1.index("jan peeters"));
    assert_ne!(k1.index("jan peeters"), k1.index("jan peeter"));
    assert_ne!(k1.index("jan peeters"), k2.index("jan peeters"));
}

#[test]
fn blind_index_key_requires_exactly_one_key() {
    assert!(BlindIndexKey::parse("").is_err());
    let two = format!("{}\n{}", key_line(1, 1), key_line(2, 2));
    assert!(matches!(
        BlindIndexKey::parse(&two),
        Err(CryptoError::InvalidKeyFile { .. })
    ));
}

#[test]
fn blind_index_known_vector() {
    // HMAC-SHA256(key = 32 x 0x00, "abc")
    let k = BlindIndexKey::parse(&key_line(1, 0)).unwrap();
    let hex: String = k.index("abc").iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex,
        "fd7adb152c05ef80dccf50a1fa4c05d5a3ec6da95575fc312ae7c5d091836351"
    );
}

#[test]
fn normalize_name_works() {
    assert_eq!(normalize_name("  Jan   PEETERS \t"), "jan peeters");
    assert_eq!(normalize_name("An\u{a0}De  Vries"), "an de vries");
    assert_eq!(normalize_name(""), "");
}

#[test]
fn key_version_byte_is_authenticated() {
    // Two versions with the same key material: only the AAD can tell the versions apart.
    let same = format!("{}\n{}\n", key_line(1, 0x33), key_line(2, 0x33));
    let keys = DataKeys::parse(&same).unwrap();
    let a = aad(Uuid::new_v4());
    let mut blob = keys.encrypt(b"secret", &a);
    assert_eq!(blob[0], 2);
    blob[0] = 1;
    assert!(matches!(keys.decrypt(&blob, &a), Err(CryptoError::Decrypt)));
}

#[cfg(unix)]
#[test]
fn check_permissions_flags_group_and_world_access() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("crypto-perm-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keys");
    std::fs::write(&path, key_line(1, 5)).unwrap();
    for (mode, ok) in [
        (0o600, true),
        (0o400, true),
        (0o640, false),
        (0o604, false),
        (0o644, false),
    ] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        let r = check_permissions(&path);
        assert_eq!(r.is_ok(), ok, "mode {mode:o}");
        if let Err(w) = r {
            assert!(w.to_string().contains(&format!("{mode:o}")));
        }
    }
    assert!(check_permissions(dir.join("missing")).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
