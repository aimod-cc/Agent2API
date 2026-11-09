use super::*;

#[test]
fn encryption_round_trip_uses_fresh_nonces_and_separate_key_domains() {
    let first = seal(b"account-secret", SecretScope::Accounts).unwrap();
    let second = seal(b"account-secret", SecretScope::Accounts).unwrap();
    assert_ne!(first, second);
    assert_eq!(
        open(&first, SecretScope::Accounts).unwrap(),
        b"account-secret"
    );
    assert!(open(&first, SecretScope::UpdateToken).is_err());
}

#[test]
fn malformed_envelopes_and_changed_ciphertext_are_rejected() {
    for input in ["plain", "enc1:not base64!", "enc1:AA=="] {
        assert!(open_with_key(input, &[7; 32]).is_err());
    }
    let encrypted = seal_with_key(b"account-secret", &[7; 32]).unwrap();
    let mut bytes = BASE64
        .decode(encrypted.strip_prefix(ENVELOPE_PREFIX).unwrap())
        .unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    assert!(open_with_key(
        &format!("{ENVELOPE_PREFIX}{}", BASE64.encode(bytes)),
        &[7; 32]
    )
    .is_err());
    assert!(open_with_key(&encrypted, &[8; 32]).is_err());
}

#[test]
fn fingerprint_does_not_create_a_key_and_corrupt_key_is_not_overwritten() {
    crate::test_support::isolated(
        concat!(
            module_path!(),
            "::fingerprint_does_not_create_a_key_and_corrupt_key_is_not_overwritten"
        ),
        || {
            let path = key_file_path(SecretScope::Accounts);
            assert!(key_fingerprint(SecretScope::Accounts).is_none());
            assert!(!path.exists());
            std::fs::write(&path, b"invalid-key").unwrap();
            assert!(seal(b"secret", SecretScope::Accounts).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"invalid-key");
        },
    );
}

#[test]
fn environment_key_is_used_without_creating_a_key_file() {
    crate::test_support::isolated(
        concat!(
            module_path!(),
            "::environment_key_is_used_without_creating_a_key_file"
        ),
        || {
            std::env::set_var(ACCOUNT_KEY_ENV, "07".repeat(32));
            let encrypted = seal(b"secret", SecretScope::Accounts).unwrap();
            assert_eq!(open_with_key(&encrypted, &[7; 32]).unwrap(), b"secret");
            assert_eq!(existing_key_bytes(SecretScope::Accounts), Some([7; 32]));
            assert!(!key_file_path(SecretScope::Accounts).exists());
        },
    );
}

#[test]
fn hex_keys_reject_bad_length_and_characters() {
    for value in ["", "00", &"0".repeat(63), &"z".repeat(64)] {
        assert!(decode_hex_key(value).is_none());
    }
    assert_eq!(
        decode_hex_key(&format!(" {} ", "aB".repeat(32))),
        Some([0xab; 32])
    );
}
