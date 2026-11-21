use super::*;

#[test]
fn magpie_rule_table_is_complete_and_compiles() {
    let rules = RULES.as_ref().unwrap();
    assert_eq!(rules.len(), 58);
    for (s, re) in rules {
        assert!(!re.is_match(""), "{:?}", s.category);
    }
}

fn ranges(text: &str, secrets: bool, personal: bool, extra: bool) -> Vec<&str> {
    let mut options = BTreeMap::new();
    if extra {
        for kind in [
            Category::Home,
            Category::Userhost,
            Category::Ip,
            Category::Bucket,
        ] {
            options.insert(kind, true);
        }
    }
    let mut out = Vec::new();
    scan(text, secrets, personal, &options, |(a, b)| {
        out.push(&text[a..b]);
        Ok(())
    })
    .unwrap();
    out
}

#[test]
fn personal_formats_and_contextual_exclusions() {
    // Examples are synthetic or well-known public format test vectors; not real account data.
    let number = ["139", "0000", "9931"].join("");
    for prefix in ["+86", "+86 ", "+86-", "86 ", ""] {
        let phone = format!("{prefix}{number}");
        assert!(ranges(&format!("联系手机{phone}"), false, true, false).contains(&phone.as_str()));
    }
    let card = ["4111", "1111", "1111", "1111"].join(" ");
    let id = ["110105", "19491231", "002X"].join("");
    let ssn = ["202", "17", "5060"].join("-");
    let mac = ["3e", "5e", "9f", "74", "81", "fa"].join(":");
    for (source, value) in [
        (
            "test email joe@relay.internal".to_owned(),
            "joe@relay.internal".to_owned(),
        ),
        (format!("身份证: {id}"), id),
        (format!("银行卡: {card}"), card),
        (format!("SSN: {ssn}"), ssn),
        ("Passport No. ABC543210".into(), "ABC543210".into()),
        (
            "IBAN: DE89370400440532013000".into(),
            "DE89370400440532013000".into(),
        ),
        ("DOB: 1990-05-20".into(), "1990-05-20".into()),
        (format!("MAC: {mac}"), mac),
        ("Serial Number: TQ-904959".into(), "TQ-904959".into()),
    ] {
        assert!(
            ranges(&source, false, true, false).contains(&value.as_str()),
            "unrecognized fixture: {source}"
        );
    }
    for clear in [
        format!("订单号：{number}"),
        format!("0.{number}"),
        "git@github.com:repo/project".into(),
        "fixture@example.test".into(),
        "443:443".into(),
        "0.12.2".into(),
        "a@fixture.json".into(),
        "release/bld.123".into(),
    ] {
        assert!(
            ranges(&clear, false, true, true).is_empty(),
            "false positive: {clear}"
        );
    }
}

#[test]
fn optional_infrastructure_detectors_require_opt_in() {
    let public_ip = ["74", "88", "22", "99"].join(".");
    let cases = [
        (
            "/home/cedar-agent/work".to_owned(),
            "cedar-agent".to_owned(),
        ),
        (
            "ssh qaoperator@relay-node-78".to_owned(),
            "qaoperator".to_owned(),
        ),
        (
            "ssh qaoperator@relay-node-78".to_owned(),
            "relay-node-78".to_owned(),
        ),
        (format!("public address: {public_ip}"), public_ip),
        (
            "s3://cold-archive-project-4242/objects".to_owned(),
            "cold-archive-project-4242".to_owned(),
        ),
    ];
    for (text, value) in cases {
        assert!(
            !ranges(&text, false, true, false).contains(&value.as_str()),
            "optional detector enabled by default"
        );
        assert!(
            ranges(&text, false, true, true).contains(&value.as_str()),
            "unrecognized infrastructure fixture: {text}"
        );
    }
    for s in [
        "10.0.0.5",
        "192.0.2.2",
        "8.8.8.8",
        "s3://my-bucket/objects",
        "/home/runner/work",
        "git@github.com:repo/project",
    ] {
        assert!(
            ranges(s, false, true, true).is_empty(),
            "false positive: {s}"
        );
    }
}

#[test]
fn luhn_group_prefix_and_full_length_account_numbers_are_checked() {
    let leaked = "4111111111111111 123";
    let hits = ranges(leaked, false, true, false);
    assert_eq!(hits, vec!["4111111111111111"]);
    let prefix = ["4111", "1111", "1111", "1111", "11"].join("");
    let full = (0..=9)
        .map(|digit| format!("{prefix}{digit}"))
        .find(|value| ranges(value, false, true, false).contains(&value.as_str()));
    assert!(
        full.is_some(),
        "valid complete 19-digit card vector was not recognized"
    );
}

#[test]
fn numeric_arrays_are_indexed_once_and_preserved() {
    let values = vec!["13800138000"; 2048].join(",");
    let input = format!("{{\"phone_numbers\":[{values}]}}");
    assert!(
        ranges(&input, false, true, false).is_empty(),
        "numbers inside a JSON numeric array are values, not text phones"
    );
    assert_eq!(super::guards::numeric_arrays(&input).len(), 1);
    let nested = format!("[[],[{values}],[{values}]]");
    assert!(ranges(&nested, false, true, false).is_empty());
    assert_eq!(super::guards::numeric_arrays(&nested).len(), 3);
}

#[test]
fn repaired_iban_match_resumes_before_the_unconsumed_suffix() {
    let iban = "DE89 3704 0044 0532 0130 00";
    let input = format!("{iban} {iban}");
    assert_eq!(
        ranges(&input, false, true, false)
            .iter()
            .filter(|&&hit| hit == iban)
            .count(),
        2
    );
}

#[test]
fn credential_families_and_checksum_guards() {
    let vendor: Vec<String> = [
        ("sk-proj-", 22),
        ("ark-", 21),
        ("oc_sk_", 22),
        ("ghp_", 36),
        ("github_pat_", 46),
        ("glpat-", 22),
        ("AIza", 35),
        ("xoxb-", 24),
        ("rk_live_", 24),
        ("hf_", 30),
        ("gsk_", 40),
        ("xai-", 40),
        ("npm_", 36),
        ("pypi-", 50),
        ("cfut_", 40),
        ("cfat_", 40),
        ("cfk_", 40),
        ("cfoat_", 40),
        ("eyJhIjoi", 44),
    ]
    .into_iter()
    .map(|(prefix, n)| format!("{prefix}{}", "xQ12Z".repeat(10).get(..n).unwrap()))
    .collect();
    for value in vendor {
        assert!(
            ranges(&value, true, false, false).contains(&value.as_str()),
            "unrecognized built-in credential family {}",
            &value[..5]
        );
    }
    let derive = ed25519_dalek::SigningKey::from_bytes(&[0x53; 32]).to_keypair_bytes();
    let wif = bs58::encode([&[0x80][..], &[0x34; 32], &[1]].concat())
        .with_check()
        .into_string();
    let invalid = format!(
        "{}{}",
        &wif[..wif.len() - 1],
        if wif.ends_with('A') { 'B' } else { 'A' }
    );
    assert!(ranges(&wif, true, false, false).contains(&wif.as_str()));
    assert!(ranges(&invalid, true, false, false).is_empty());
    let solana = bs58::encode(derive).into_string();
    assert!(ranges(&solana, true, false, false).contains(&solana.as_str()));
    let key_array = serde_json::to_string(derive.as_slice()).unwrap();
    assert!(ranges(&key_array, true, false, false).contains(&key_array.as_str()));
    let key_hex = "0x".to_owned() + &"b5".repeat(32);
    assert!(
        ranges(&format!("signing key {key_hex}"), true, false, false).contains(&key_hex.as_str())
    );
    assert!(ranges(&format!("wallet tx hash {key_hex}"), true, false, false).is_empty());
    let pem = "-----BEGIN PRIVATE KEY-----\nfixture-only-data\n-----END PRIVATE KEY-----";
    assert!(ranges(pem, true, false, false).contains(&pem));
    assert!(ranges("密码：Se9a1ch", true, false, false).contains(&"Se9a1ch"));
    let words =
        "abandon ability able about above absent absorb abstract absurd abuse access accident";
    assert!(ranges(&format!("mnemonic: {words}"), true, false, false).contains(&words));
}
