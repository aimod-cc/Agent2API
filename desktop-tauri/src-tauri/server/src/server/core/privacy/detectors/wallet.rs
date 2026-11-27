//! Key shape/checksum verification based on Magpie crypto.go (MIT; licenses/magpie-MIT).
use ed25519_dalek::SigningKey;
use std::{collections::HashSet, sync::LazyLock};

pub(super) fn wif(v: &str) -> bool {
    if !matches!(v.len(), 51 | 52) {
        return false;
    }
    bs58::decode(v)
        .with_check(None)
        .into_vec()
        .is_ok_and(|p| p.len() == 33 || p.len() == 34 && p[33] == 1)
}

pub(super) fn bip32(v: &str) -> bool {
    bs58::decode(v)
        .with_check(None)
        .into_vec()
        .is_ok_and(|p| p.len() == 78)
}

fn solana_bytes(data: &[u8]) -> bool {
    let Ok(bytes) = <&[u8; 64]>::try_from(data) else {
        return false;
    };
    SigningKey::from_keypair_bytes(bytes).is_ok()
}

pub(super) fn solana(v: &str) -> bool {
    (85..=88).contains(&v.len()) && bs58::decode(v).into_vec().is_ok_and(|p| solana_bytes(&p))
}

pub(super) fn solana_array(v: &str) -> bool {
    v.trim_matches(|c: char| c == '[' || c == ']' || c.is_ascii_whitespace())
        .split(',')
        .map(|part| part.trim().parse::<u8>())
        .collect::<Result<Vec<_>, _>>()
        .is_ok_and(|p| solana_bytes(&p))
}

pub(super) fn stellar(v: &str) -> bool {
    let Ok(bin) = data_encoding::BASE32_NOPAD.decode(v.as_bytes()) else {
        return false;
    };
    if bin.len() != 35 || bin[0] != (18 << 3) {
        return false;
    }
    let crc = bin[..33].iter().fold(0_u16, |mut state, byte| {
        state ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            state = (state << 1) ^ if state & 0x8000 != 0 { 0x1021 } else { 0 };
        }
        state
    });
    bin[33..] == crc.to_le_bytes()
}

pub(super) fn wallet_context(text: &str, start: usize) -> bool {
    let start_line = text[..start].rfind('\n').map_or(0, |n| n + 1);
    let from = super::guards::before(text, start, 48).max(start_line);
    let prefix = text[from..start].to_ascii_lowercase();
    let cue = [
        "priv",
        "secret",
        "signer",
        "signing",
        "deployer",
        "wallet",
        "pk",
        "私钥",
        "私鑰",
        "秘密鍵",
        "개인키",
        "비밀키",
    ]
    .into_iter()
    .flat_map(|cue| {
        let prefix = prefix.as_str();
        prefix.match_indices(cue).filter(move |&(a, _)| {
            cue != "pk"
                || !super::guards::adjacent(
                    prefix,
                    a,
                    a + 2,
                    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
                )
        })
    })
    .max_by_key(|&(a, _)| a);
    let Some((a, cue)) = cue else { return false };
    let middle = &prefix[a + cue.len()..];
    if [
        "hash",
        "sha",
        "digest",
        "sum",
        "pub",
        "addr",
        "tx",
        "fingerprint",
    ]
    .iter()
    .any(|mark| middle.contains(mark))
    {
        return false;
    }
    !middle
        .trim_start_matches(['_', '-', ' '])
        .strip_prefix("id")
        .is_some_and(|suffix| {
            suffix
                .as_bytes()
                .first()
                .is_none_or(|b| !b.is_ascii_alphabetic())
        })
}

static WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("bip39english.txt")
        .split_ascii_whitespace()
        .collect()
});

pub(super) fn phrases(text: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let (mut run_start, mut run_end, mut words) = (start, start, 0);
    let mut at = start;
    while at < end {
        while at < end && matches!(text.as_bytes()[at], b' ' | b'\t') {
            at += 1;
        }
        if at == end {
            break;
        }
        let stop = text[at..end].find([' ', '\t']).map_or(end, |n| at + n);
        if WORDS.contains(text[at..stop].to_ascii_lowercase().as_str()) {
            if words == 0 {
                run_start = at;
            }
            words += 1;
            run_end = stop;
        } else {
            if words >= 12 {
                found.push((run_start, run_end));
            }
            words = 0;
        }
        at = stop;
    }
    if words >= 12 {
        found.push((run_start, run_end));
    }
    found
}
