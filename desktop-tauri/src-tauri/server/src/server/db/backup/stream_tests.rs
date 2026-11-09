use super::*;
use std::io::{Cursor, Read, Write};

fn encrypt(input: &[u8]) -> Vec<u8> {
    let mut writer = EncryptWriter::new(Vec::new(), &[7; 32], &[3; 7], b"header".to_vec()).unwrap();
    for piece in input.chunks(7919) {
        writer.write_all(piece).unwrap();
    }
    writer.finish().unwrap()
}

fn decrypt(input: &[u8], key: &[u8; 32], header: &[u8]) -> io::Result<Vec<u8>> {
    let mut reader = DecryptReader::new(Cursor::new(input), key, &[3; 7], header.to_vec())?;
    let mut output = Vec::new();
    reader.read_to_end(&mut output)?;
    reader.finish()?;
    Ok(output)
}

#[test]
fn empty_partial_exact_and_multiple_chunks_round_trip() {
    for length in [
        0,
        1,
        CHUNK_BYTES - 1,
        CHUNK_BYTES,
        CHUNK_BYTES + 1,
        CHUNK_BYTES * 2,
    ] {
        let plain = (0..length).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let encrypted = encrypt(&plain);
        assert_eq!(
            decrypt(&encrypted, &[7; 32], b"header").unwrap(),
            plain,
            "length={length}"
        );
    }
}

#[test]
fn wrong_key_or_authenticated_header_is_rejected() {
    let encrypted = encrypt(b"private account data");
    assert!(decrypt(&encrypted, &[8; 32], b"header").is_err());
    assert!(decrypt(&encrypted, &[7; 32], b"changed").is_err());
}

#[test]
fn changed_ciphertext_and_missing_final_tag_are_rejected() {
    let encrypted = encrypt(&vec![42; CHUNK_BYTES + 17]);
    for position in [0, CHUNK_BYTES, encrypted.len() - 1] {
        let mut changed = encrypted.clone();
        changed[position] ^= 1;
        assert!(
            decrypt(&changed, &[7; 32], b"header").is_err(),
            "position={position}"
        );
    }
    for length in [
        0,
        TAG_BYTES - 1,
        CHUNK_BYTES + TAG_BYTES,
        encrypted.len() - 1,
    ] {
        assert!(
            decrypt(&encrypted[..length], &[7; 32], b"header").is_err(),
            "length={length}"
        );
    }
}

#[test]
fn chunks_cannot_be_reordered_or_duplicated() {
    let encrypted = encrypt(&vec![17; CHUNK_BYTES * 2 + 5]);
    let width = CHUNK_BYTES + TAG_BYTES;
    let reordered = [
        &encrypted[width..width * 2],
        &encrypted[..width],
        &encrypted[width * 2..],
    ]
    .concat();
    let duplicated = [
        &encrypted[..width],
        &encrypted[..width],
        &encrypted[width..],
    ]
    .concat();
    assert!(decrypt(&reordered, &[7; 32], b"header").is_err());
    assert!(decrypt(&duplicated, &[7; 32], b"header").is_err());
}

#[test]
fn trailing_bytes_after_a_valid_backup_are_rejected() {
    let mut encrypted = encrypt(b"account");
    encrypted.extend_from_slice(b"extra");
    assert!(decrypt(&encrypted, &[7; 32], b"header").is_err());
}

#[test]
fn finishing_without_consuming_all_plaintext_is_rejected() {
    let encrypted = encrypt(b"account");
    let reader = DecryptReader::new(
        Cursor::new(encrypted),
        &[7; 32],
        &[3; 7],
        b"header".to_vec(),
    )
    .unwrap();
    assert!(reader.finish().is_err());
}

#[test]
fn short_reads_and_interruptions_preserve_chunk_boundaries() {
    struct Fragmented {
        input: Cursor<Vec<u8>>,
        interrupted: bool,
    }
    impl Read for Fragmented {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.interrupted = !self.interrupted;
            if self.interrupted {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = output.len().min(8191);
            self.input.read(&mut output[..count])
        }
    }
    let plain = vec![19; CHUNK_BYTES + 33];
    let source = Fragmented {
        input: Cursor::new(encrypt(&plain)),
        interrupted: false,
    };
    let mut reader = DecryptReader::new(source, &[7; 32], &[3; 7], b"header".to_vec()).unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, plain);
    reader.finish().unwrap();
}
