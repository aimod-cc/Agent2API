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
    s.categories.insert("public_ip".to_string(), true);
    s.categories.insert("home_path".to_string(), true);
    s
}

#[test]
fn secret_detection_and_deterministic_roundtrip() {
    let compiled = CompiledPrivacySettings::compile(full_settings()).unwrap();
    let vault = PrivacyVault::ephemeral();
    let input = "OPENAI_API_KEY=sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd \
                 postgres://admin:SuperSecr3tPass@db.internal:5432/app \
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

    // 还原应与原值一致，且大小写或花括号内空格变体也容错还原
    let (restored, restored_stats) = vault.restore_text(&masked, false);
    assert_eq!(restored, input);
    assert!(!restored_stats.is_empty());
}

#[test]
fn personal_pii_and_false_positive_guards() {
    let compiled = CompiledPrivacySettings::compile(full_settings()).unwrap();
    let vault = PrivacyVault::ephemeral();
    let input = "Mail alice@example.com or icon@2x.png, phone 13800138000, \
                 order_id=13912345678, id 11010519491231002X, badid 110105194912310021, \
                 pub 8.8.8.8, priv 192.168.1.10, path /Users/alice_dev/src and /Users/root/tmp";
    let MaskTextResult::Handled {
        text: Some(masked),
        ..
    } = mask_text(input, &compiled, &vault)
    else {
        panic!("expected pii mask");
    };
    assert!(!masked.contains("alice@example.com"));
    assert!(masked.contains("icon@2x.png"));
    assert!(!masked.contains("13800138000"));
    assert!(masked.contains("13912345678")); // 订单号上下文跳过
    assert!(!masked.contains("11010519491231002X")); // 合法校验位身份证
    assert!(masked.contains("110105194912310021")); // 非法校验位跳过
    assert!(!masked.contains("8.8.8.8"));
    assert!(masked.contains("192.168.1.10")); // 私网 IP 跳过
    assert!(!masked.contains("alice_dev"));
    assert!(masked.contains("/Users/root/tmp")); // 通用系统名跳过

    let (restored, _) = vault.restore_text(&masked, false);
    assert_eq!(restored, input);
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
fn native_anthropic_preserves_signed_thinking_blocks() {
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
    // 签名思考块必须一字不动
    assert_eq!(
        next["messages"][0]["content"][0]["thinking"],
        "sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd"
    );
    assert_ne!(
        next["messages"][0]["content"][1]["text"],
        "Use sk-proj-8fK2mQx7Lp4Rt9Vw3Yz6Ab1Cd"
    );
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
async fn unmask_outcome_restores_chat_and_anthropic_sse() {
    let vault = PrivacyVault::global();
    let ph = vault.mint("EMAIL", "sse_test_user@example.com");
    let mid = ph.len() / 2;
    let c1 = format!(
        "data: {{\"id\":\"1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"hi {}\"}}}}]}}\n\n",
        &ph[..mid]
    );
    let c2 = format!(
        "data: {{\"id\":\"1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{}!\"}}}}]}}\n\n",
        &ph[mid..]
    );
    let c3 = "data: [DONE]\n\n".to_string();

    let telemetry = Arc::new(RequestTelemetry::new());
    let s = stream::iter(vec![
        Ok(Bytes::from(c1)),
        Ok(Bytes::from(c2)),
        Ok(Bytes::from(c3)),
    ]);
    let outcome = unmask_outcome(
        ForwardOutcome::Stream {
            status: 200,
            stream: Box::new(s),
        },
        telemetry.clone(),
    );
    let ForwardOutcome::Stream { mut stream, .. } = outcome else {
        panic!("expected stream");
    };
    let mut joined = String::new();
    while let Some(item) = stream.next().await {
        joined.push_str(std::str::from_utf8(&item.unwrap()).unwrap());
    }
    assert!(joined.contains("sse_test_user@example.com!"));
    assert!(!joined.contains(&ph));
    let snap = telemetry.snapshot();
    assert_eq!(snap.privacy_hits.first().map(|h| h.restored), Some(1));

    // Anthropic 原生 SSE 流（含 input_json_delta 转义与 content_block_stop 冲刷）
    let ph_key = vault.mint("API_KEY", "sk-line\"1\n2");
    let kmid = ph_key.len() / 2;
    let a1 = format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{{\\\"k\\\":\\\"{}\"}}}}\n\n",
        &ph_key[..kmid]
    );
    let a2 = format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{}\\\"}}\"}}}}\n\n",
        &ph_key[kmid..]
    );
    let a3 = "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n".to_string();
    let a4 = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string();

    let t2 = Arc::new(RequestTelemetry::new());
    let native_outcome = unmask_outcome(
        ForwardOutcome::Native(NativeResponse {
            protocol: WireProtocol::Anthropic,
            status: 200,
            headers: Default::default(),
            body: NativeBody::Stream(Box::new(stream::iter(vec![
                Ok(Bytes::from(a1)),
                Ok(Bytes::from(a2)),
                Ok(Bytes::from(a3)),
                Ok(Bytes::from(a4)),
            ]))),
        }),
        t2,
    );
    let ForwardOutcome::Native(NativeResponse {
        body: NativeBody::Stream(mut nstream),
        ..
    }) = native_outcome
    else {
        panic!("expected native stream");
    };
    let mut nj = String::new();
    let mut partial_json = String::new();
    while let Some(item) = nstream.next().await {
        let chunk = std::str::from_utf8(&item.unwrap()).unwrap().to_string();
        for line in chunk.lines() {
            if let Some(raw) = line.strip_prefix("data: ") {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
                    if let Some(p) = val
                        .pointer("/delta/partial_json")
                        .and_then(serde_json::Value::as_str)
                    {
                        partial_json.push_str(p);
                    }
                }
            }
        }
        nj.push_str(&chunk);
    }
    let parsed: serde_json::Value = serde_json::from_str(&partial_json).unwrap();
    assert_eq!(parsed["k"], "sk-line\"1\n2");
    assert!(!nj.contains(&ph_key));
}
