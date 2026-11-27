use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use futures::{stream, StreamExt};
use serde_json::json;

use crate::server::core::protocol::native::WireProtocol;
use crate::server::core::upstream::native::{NativeBody, NativeResponse};
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;

use super::*;

fn full_settings() -> PrivacySettings {
    let mut s = PrivacySettings {
        enabled: true,
        reversible: true,
        action: PrivacyAction::Mask,
        secrets: true,
        personal: true,
        ..PrivacySettings::default()
    };
    for (id, _, _) in PERSONAL_CATEGORIES {
        s.categories.insert((*id).to_string(), true);
    }
    s
}

#[test]
fn secret_detection_weak_passwords_and_deterministic_roundtrip() {
    let compiled = CompiledPrivacySettings::compile(full_settings()).unwrap();
    let vault = PrivacyVault::ephemeral();
    let input = "OPENAI_API_KEY=sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd \
                 postgres://admin:SuperSecr3tPass@db.internal:5432/app \
                 password=abcdefgh passwd=12345678 \
                 密码：myPass1234 密钥为 2048 位 DB_PASS=${ENV_PASS} token=toBase64Url(x)";
    let MaskTextResult::Handled {
        text: Some(masked),
        hits,
        blocked_message: None,
    } = mask_text(input, &compiled, &vault)
    else {
        panic!("expected masked output");
    };
    assert!(!masked.contains("sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd"));
    assert!(!masked.contains("SuperSecr3tPass"));
    assert!(!masked.contains("abcdefgh"), "纯字母弱密码必须遮罩: {masked}");
    assert!(!masked.contains("12345678"), "纯数字弱密码必须遮罩: {masked}");
    assert!(!masked.contains("myPass1234"));
    // 数量词、环境变量与函数调用不误伤
    assert!(masked.contains("2048 位"));
    assert!(masked.contains("${ENV_PASS}"));
    assert!(masked.contains("toBase64Url(x)"));
    assert!(!hits.is_empty());

    // 同一进程/保险库多次铸造得到相同占位符，保护提示词缓存
    let MaskTextResult::Handled {
        text: Some(masked2),
        ..
    } = mask_text("key sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd", &compiled, &vault)
    else {
        panic!("expected second masked output");
    };
    let ph1 = masked
        .split_whitespace()
        .next()
        .unwrap()
        .strip_prefix("OPENAI_API_KEY=")
        .unwrap();
    let ph2 = masked2.split_whitespace().nth(1).unwrap();
    assert_eq!(ph1, ph2);

    let (restored, restored_stats) = vault.restore_text(&masked, false);
    assert_eq!(restored, input);
    assert!(!restored_stats.is_empty());
}

#[test]
fn personal_pii_international_and_false_positive_guards() {
    let compiled = CompiledPrivacySettings::compile(full_settings()).unwrap();
    let vault = PrivacyVault::ephemeral();
    let input = "Mail alice@acme-corp.com, ignore bob@example.com or icon@2x.png, \
                 phone 13800138000, order_id=13912345678, \
                 id 11010519491231002X, badid 110105194912310021, nonleap 110105202302290024, \
                 pub 198.51.101.25, dns 8.8.8.8, priv 192.168.1.10, \
                 path /Users/alice_dev/src and /Users/root/tmp, \
                 mac aa:bb:cc:dd:ee:01, iban DE89 3704 0044 0532 0130 00, \
                 bucket s3://prod-finance-ledger/report.csv";
    let MaskTextResult::Handled {
        text: Some(masked),
        ..
    } = mask_text(input, &compiled, &vault)
    else {
        panic!("expected pii mask");
    };
    assert!(!masked.contains("alice@acme-corp.com"));
    assert!(masked.contains("bob@example.com"), "示例域名不应误遮");
    assert!(masked.contains("icon@2x.png"));
    assert!(!masked.contains("13800138000"));
    assert!(masked.contains("13912345678"));
    assert!(!masked.contains("11010519491231002X"));
    assert!(masked.contains("110105194912310021"));
    assert!(masked.contains("110105202302290024"), "非闰年 2 月 29 日不应误判为身份证");
    assert!(!masked.contains("198.51.101.25"));
    assert!(masked.contains("8.8.8.8"), "公共 DNS 不应误遮");
    assert!(masked.contains("192.168.1.10"));
    assert!(!masked.contains("alice_dev"));
    assert!(masked.contains("/Users/root/tmp"));
    assert!(!masked.contains("aa:bb:cc:dd:ee:01"));
    assert!(!masked.contains("DE89 3704 0044 0532 0130 00"));
    assert!(!masked.contains("prod-finance-ledger"));

    let (restored, _) = vault.restore_text(&masked, false);
    assert_eq!(restored, input);
}

#[test]
fn wallet_seed_phrase_and_date_variants_and_overlap_merge() {
    let mut s = full_settings();
    s.words = vec!["1990-05-20".to_string(), "lp-example".to_string()];
    let compiled = CompiledPrivacySettings::compile(s).unwrap();
    let vault = PrivacyVault::ephemeral();

    let key = concat!("sk-", "lp-example", "ABC123def456GHI789");
    let mnemonic = "abandon ability able about above absent absorb abstract absurd abuse access accident";
    let text = format!(
        "recovery phrase: {mnemonic}\nbirth: 1990年5月20日 and May 20, 1990 and 19900520\nkey: {key}"
    );
    let MaskTextResult::Handled {
        text: Some(masked),
        ..
    } = mask_text(&text, &compiled, &vault)
    else {
        panic!("expected wallet & date variants masked");
    };
    assert!(!masked.contains("abandon ability"));
    assert!(!masked.contains("1990年5月20日"));
    assert!(!masked.contains("May 20, 1990"));
    assert!(!masked.contains("19900520"));
    // 重叠命中必须合并区间，不得暴露密钥剩余部分
    assert!(!masked.contains("ABC123def456GHI789"));
    assert!(!masked.contains("lp-example"));

    let (restored, _) = vault.restore_text(&masked, false);
    assert_eq!(restored, text);
}

#[test]
fn tool_json_secret_fields_and_structured_json_output_restore() {
    let mut s = full_settings();
    s.words = vec!["A\"B\nC".to_string()];
    let compiled = CompiledPrivacySettings::compile(s).unwrap();
    let vault = PrivacyVault::ephemeral();

    let req = json!({
        "model": "gpt-4.1",
        "response_format": { "type": "json_object" },
        "messages": [
            {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "login",
                        "arguments": "{\"password\":\"abc\",\"max_tokens\":100,\"public_key\":\"pk_pub_visible\",\"note\":\"A\\\"B\\nC\"}"
                    }
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"api_key\":\"tiny\",\"page_token\":\"0123456789abcdef0123456789abcdef\"}"
            }
        ]
    });

    assert!(request_expects_json_output(&req, WireProtocol::Chat));
    let PrivacyBodyOutcome::Masked { body: masked, .. } = mask_chat_body(&req, &compiled, &vault)
    else {
        panic!("expected tool json masked");
    };
    let args_str = masked["messages"][0]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    let args_json: serde_json::Value = serde_json::from_str(args_str).unwrap();
    assert_ne!(args_json["password"], "abc", "短密码字段必须整值遮罩");
    assert_eq!(args_json["max_tokens"], 100);
    assert_eq!(args_json["public_key"], "pk_pub_visible", "public_key 不应误遮");

    let tool_out = masked["messages"][1]["content"].as_str().unwrap();
    let tool_json: serde_json::Value = serde_json::from_str(tool_out).unwrap();
    assert_ne!(tool_json["api_key"], "tiny", "短 api_key 字段必须整值遮罩");
    assert_eq!(
        tool_json["page_token"], "0123456789abcdef0123456789abcdef",
        "分页游标不应误遮"
    );

    // 模拟模型在 JSON 输出模式下返回带占位符的 JSON，原值含双引号与换行，还原后仍须是合法 JSON
    let note_ph = args_json["note"].as_str().unwrap();
    let mut resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": format!("{{\"secret\":{}}}", serde_json::to_string(note_ph).unwrap())
            }
        }]
    });
    let restored_stats = unmask_json_response_with_mode(&mut resp, WireProtocol::Chat, &vault, true);
    assert!(!restored_stats.is_empty());
    let content = resp["choices"][0]["message"]["content"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(content).expect("还原后必须仍是合法 JSON");
    assert_eq!(parsed["secret"], "A\"B\nC");
}

#[test]
fn ephemeral_opaque_vault_isolates_requests_and_hides_kind() {
    let mut s = full_settings();
    s.ephemeral_placeholders = true;
    let compiled = CompiledPrivacySettings::compile(s).unwrap();
    let v1 = PrivacyVault::ephemeral_opaque();
    let v2 = PrivacyVault::ephemeral_opaque();

    let input = "mail alice@acme-corp.com";
    let MaskTextResult::Handled {
        text: Some(m1), ..
    } = mask_text(input, &compiled, &v1)
    else {
        panic!("expected m1");
    };
    let MaskTextResult::Handled {
        text: Some(m2), ..
    } = mask_text(input, &compiled, &v2)
    else {
        panic!("expected m2");
    };
    assert!(m1.contains("{{A2A_"));
    assert!(!m1.contains("EMAIL"), "不透明占位符不得暴露类型");
    assert_ne!(m1, m2, "不同请求占位符必须隔离");
    assert_eq!(v1.restore_text(&m1, false).0, input);
    assert_eq!(v2.restore_text(&m1, false).0, m1, "跨请求保险库不得还原旧请求占位符");
}

#[test]
fn words_custom_rules_allowlist_and_block_mode() {
    let mut s = PrivacySettings {
        enabled: true,
        reversible: true,
        action: PrivacyAction::Mask,
        secrets: true,
        personal: true,
        words: vec!["Nightjar".to_string(), "ABC".to_string()],
        words_ignore_case: true,
        words_whole_word: true,
        allowlist: vec!["public@demo.org".to_string()],
        custom_rules: vec![PrivacyCustomRule {
            id: "gw-key".to_string(),
            name: "网关密钥".to_string(),
            kind: "GW_KEY".to_string(),
            enabled: true,
            match_type: CustomRuleMatchType::Prefix,
            pattern: "oc_sk_".to_string(),
            action: Some(PrivacyAction::Block),
        }],
        ..PrivacySettings::default()
    };
    s.kind_actions
        .insert("API_KEY".to_string(), PrivacyAction::Mask);
    let compiled = CompiledPrivacySettings::compile(s).unwrap();
    let vault = PrivacyVault::ephemeral();

    let text = "deploy NIGHTJAR and ABC but keep XABCD and public@demo.org";
    let MaskTextResult::Handled {
        text: Some(masked),
        blocked_message: None,
        ..
    } = mask_text(text, &compiled, &vault)
    else {
        panic!("expected words masked");
    };
    assert!(!masked.contains("NIGHTJAR"));
    assert!(!masked.contains(" ABC "));
    assert!(masked.contains("XABCD"));
    assert!(masked.contains("public@demo.org"));

    let blocked = mask_text("secret oc_sk_1234567890", &compiled, &vault);
    let MaskTextResult::Handled {
        blocked_message: Some(msg),
        ..
    } = blocked
    else {
        panic!("expected block");
    };
    assert!(!msg.contains("1234567890"));
}

#[test]
fn native_anthropic_preserves_signed_thinking_blocks_and_debug_redaction() {
    let compiled = CompiledPrivacySettings::compile(full_settings()).unwrap();
    let vault = PrivacyVault::ephemeral();
    let body = json!({
        "model": "claude-sonnet-4-5",
        "system": "Key is sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd",
        "messages": [
            {
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd", "signature": "sig_123" },
                    { "type": "text", "text": "Use sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd" }
                ]
            }
        ]
    });
    let PrivacyBodyOutcome::Masked { body: next, .. } =
        mask_native_body(&body, WireProtocol::Anthropic, &compiled, &vault)
    else {
        panic!("expected native body masked");
    };
    assert_eq!(
        next["messages"][0]["content"][0]["thinking"],
        "sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd"
    );
    assert_ne!(
        next["messages"][0]["content"][1]["text"],
        "Use sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd"
    );

    let raw_bytes = serde_json::to_vec(&json!({
        "messages": [{ "role": "user", "content": "password=mysecretpass" }]
    }))
    .unwrap();
    let safe_bytes = redact_debug_body(&raw_bytes, "/v1/chat/completions", &compiled);
    let safe_text = String::from_utf8(safe_bytes).unwrap();
    assert!(!safe_text.contains("mysecretpass"), "调试落盘不得保留明文密码");
}

#[test]
fn stream_channel_restorer_handles_split_chunks_and_json_escape() {
    let vault = PrivacyVault::ephemeral();
    let ph = vault.mint("PASSWORD", "pa\"ss\nword");
    let mut restorer = StreamChannelRestorer::new(true);
    let mut stats = HashMap::new();

    let mid = ph.len() / 2;
    let p1 = format!(r#"{{"pw":"{}"#, &ph[..mid]);
    let p2 = format!(r#"{}"}}"#, &ph[mid..]);
    let out1 = restorer.push(&p1, &vault, &mut stats);
    let out2 = restorer.push(&p2, &vault, &mut stats);
    let tail = restorer.flush();
    let combined = format!("{out1}{out2}{tail}");
    let parsed: serde_json::Value = serde_json::from_str(&combined).unwrap();
    assert_eq!(parsed["pw"], "pa\"ss\nword");
    assert_eq!(stats.get("PASSWORD"), Some(&1));
}

#[tokio::test]
async fn unmask_outcome_restores_bom_cr_opaque_and_anthropic_sse() {
    // 1. 单次随机隐藏类型占位符 + 首帧 UTF-8 BOM + 单独 \r 换行 + JSON 输出模式转义
    let vault = RequestVault::ephemeral_opaque();
    let ph = vault.mint("TERM", "A\"B\nC");
    let mid = ph.len() / 2;
    let c1 = format!(
        "\u{feff}: keepalive\r\rdata: {{\"id\":\"1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{{\\\"v\\\":\\\"{}\"}}}}]}}\r\r",
        &ph[..mid]
    );
    let c2 = format!(
        "data: {{\"id\":\"1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{}\\\"}}\"}}}}]}}\r\rdata: [DONE]\r\r",
        &ph[mid..]
    );
    let telemetry = Arc::new(RequestTelemetry::new());
    let s = stream::iter(vec![Ok(Bytes::from(c1)), Ok(Bytes::from(c2))]);
    let outcome = unmask_outcome_with_options(
        ForwardOutcome::Stream {
            status: 200,
            stream: Box::new(s),
        },
        telemetry.clone(),
        vault,
        true,
    );
    let ForwardOutcome::Stream { mut stream, .. } = outcome else {
        panic!("expected stream");
    };
    let mut joined = String::new();
    while let Some(item) = stream.next().await {
        joined.push_str(std::str::from_utf8(&item.unwrap()).unwrap());
    }
    assert!(joined.starts_with('\u{feff}'));
    assert!(joined.contains(": keepalive"));
    assert!(!joined.contains(&ph));
    // 把两个 delta.content 拼起来，验证还原后仍是合法 JSON
    let mut content_joined = String::new();
    for line in joined.split('\r') {
        if let Some(payload) = line.strip_prefix("data: ") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) {
                if let Some(piece) = v.pointer("/choices/0/delta/content").and_then(|x| x.as_str()) {
                    content_joined.push_str(piece);
                }
            }
        }
    }
    let parsed: serde_json::Value = serde_json::from_str(&content_joined).expect("流式 JSON 输出必须合法");
    assert_eq!(parsed["v"], "A\"B\nC");
    assert_eq!(telemetry.snapshot().privacy_hits.first().map(|h| h.restored), Some(1));

    // 2. Anthropic 原生 SSE 流（含 input_json_delta 转义与 content_block_stop 冲刷）
    let global_vault = RequestVault::global();
    let ph2 = global_vault.mint("API_KEY", "sk-live-secret\"value");
    let cut = ph2.len() / 2;
    let a1 = format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{{\\\"k\\\":\\\"{}\"}}}}\n\n",
        &ph2[..cut]
    );
    let a2 = format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{}\\\"}}\"}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
        &ph2[cut..]
    );
    let telemetry2 = Arc::new(RequestTelemetry::new());
    let native = ForwardOutcome::Native(NativeResponse {
        protocol: WireProtocol::Anthropic,
        status: 200,
        headers: axum::http::HeaderMap::new(),
        body: NativeBody::Stream(Box::new(stream::iter(vec![
            Ok(Bytes::from(a1)),
            Ok(Bytes::from(a2)),
        ]))),
    });
    let ForwardOutcome::Native(NativeResponse {
        body: NativeBody::Stream(mut nstream),
        ..
    }) = unmask_outcome_with_options(native, telemetry2.clone(), global_vault, false)
    else {
        panic!("expected native stream");
    };
    let mut njoined = String::new();
    while let Some(item) = nstream.next().await {
        njoined.push_str(std::str::from_utf8(&item.unwrap()).unwrap());
    }
    assert!(njoined.contains("sk-live-secret"));
    assert!(!njoined.contains(&ph2));
}
