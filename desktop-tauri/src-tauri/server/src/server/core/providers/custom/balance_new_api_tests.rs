use super::*;

fn cny_display() -> QuotaDisplay {
    // 500,000 quota = $1, exchange rate 8 -> 500,000 quota = ¥8, 5,000,000 quota = ¥80
    QuotaDisplay {
        display_type: "CNY".to_string(),
        quota_per_unit: 500_000.0,
        usd_exchange_rate: 8.0,
        custom_currency_symbol: String::new(),
        custom_currency_exchange_rate: 1.0,
    }
}

#[test]
fn parses_official_new_api_subscription_with_zero_wallet() {
    let now_ms = 1_790_000_000_000i64;
    let payload = json!({
        "success": true,
        "data": {
            "billing_preference": "subscription_first",
            "subscriptions": [
                {
                    "subscription": {
                        "id": 1,
                        "amount_total": 5_000_000,
                        "amount_used": 1_505_000,
                        "status": "active",
                        "end_time": 1_800_000_000,
                        "next_reset_time": 1_790_086_400,
                        "allow_wallet_overflow": true
                    }
                }
            ],
            "all_subscriptions": [
                {
                    "subscription": {
                        "id": 99,
                        "amount_total": 99_000_000,
                        "amount_used": 0,
                        "status": "expired",
                        "end_time": 1_700_000_000
                    }
                }
            ]
        }
    });
    let summary = parse_new_api_subscription(&payload, now_ms).expect("active subscription");
    assert_eq!(summary.count, 1);
    assert!(!summary.has_unlimited);
    assert_eq!(summary.total_raw, Some(5_000_000.0));
    assert_eq!(summary.remaining_raw, Some(3_495_000.0));
    assert_eq!(summary.next_reset_ms, Some(1_790_086_400_000));

    let display = cny_display();
    let doc = new_api_document(
        0.0,
        Some(1_505_000.0),
        Some(&display),
        &json!({"data": {"quota": 0}}),
        None,
        Some(&summary),
        Some(&payload),
    );
    // 3_495_000 / 500_000 * 8 = 55.92 CNY, total = 80 CNY
    assert!((doc["available"].as_f64().unwrap() - 55.92).abs() < 1e-6);
    assert_eq!(doc["unit"].as_str(), Some("CNY"));
    let sub_wallet = &doc["wallets"][0];
    assert_eq!(sub_wallet["type"].as_str(), Some("subscription"));
    assert_eq!(sub_wallet["total"].as_f64(), Some(80.0));
    assert_eq!(sub_wallet["balanceView"].as_str(), Some("剩 55.92 / 80 CNY"));
    assert_eq!(sub_wallet["resetsAt"].as_i64(), Some(1_790_086_400_000));
    assert!(doc["balanceView"].as_str().unwrap().starts_with("下次重置 "));
}

#[test]
fn sums_multiple_active_subscriptions_and_filters_expired_array_entries() {
    let now_ms = 1_790_000_000_000i64;
    let payload = json!({
        "success": true,
        "data": [
            {
                "amount_total": 1_000_000,
                "amount_used": 250_000,
                "status": "active",
                "end_time": 1_800_000_000,
                "next_reset_time": 1_790_100_000
            },
            {
                "amount_total": 500_000,
                "amount_used": 100_000,
                "status": "active",
                "end_time": 1_805_000_000,
                "next_reset_time": 1_790_050_000
            },
            {
                "amount_total": 9_000_000,
                "amount_used": 0,
                "status": "expired",
                "end_time": 1_800_000_000
            },
            {
                "amount_total": 9_000_000,
                "amount_used": 0,
                "status": "active",
                "end_time": 1_780_000_000
            }
        ]
    });
    let summary = parse_new_api_subscription(&payload, now_ms).expect("two active");
    assert_eq!(summary.count, 2);
    assert_eq!(summary.total_raw, Some(1_500_000.0));
    assert_eq!(summary.remaining_raw, Some(1_150_000.0));
    assert_eq!(summary.next_reset_ms, Some(1_790_050_000_000));
    assert_eq!(summary.expire_at_ms, Some(1_805_000_000_000));
}

#[test]
fn empty_subscriptions_does_not_fall_back_to_all_subscriptions() {
    let payload = json!({
        "success": true,
        "data": {
            "billing_preference": "subscription_first",
            "subscriptions": [],
            "all_subscriptions": [
                {
                    "subscription": {
                        "amount_total": 5_000_000,
                        "amount_used": 0,
                        "status": "active",
                        "end_time": 1_900_000_000
                    }
                }
            ]
        }
    });
    assert!(parse_new_api_subscription(&payload, 1_790_000_000_000).is_none());
}

#[test]
fn respects_billing_preference_and_wallet_overflow() {
    let now_ms = 1_790_000_000_000i64;
    let display = cny_display();
    let make = |pref: &str, overflow: bool| {
        let payload = json!({
            "success": true,
            "data": {
                "billing_preference": pref,
                "subscriptions": [{
                    "subscription": {
                        "amount_total": 1_000_000,
                        "amount_used": 500_000,
                        "status": "active",
                        "end_time": 1_800_000_000,
                        "allow_wallet_overflow": overflow
                    }
                }]
            }
        });
        let summary = parse_new_api_subscription(&payload, now_ms).unwrap();
        new_api_document(
            1_000_000.0,
            None,
            Some(&display),
            &json!({}),
            None,
            Some(&summary),
            Some(&payload),
        )
    };

    let both = make("subscription_first", true);
    assert!((both["available"].as_f64().unwrap() - 24.0).abs() < 1e-6);
    assert!(both["wallets"][0].get("total").is_none());

    let no_overflow = make("subscription_first", false);
    assert!((no_overflow["available"].as_f64().unwrap() - 8.0).abs() < 1e-6);
    assert_eq!(no_overflow["wallets"][0]["total"].as_f64(), Some(16.0));

    let sub_only = make("subscription_only", true);
    assert!((sub_only["available"].as_f64().unwrap() - 8.0).abs() < 1e-6);

    let wallet_only = make("wallet_only", true);
    assert!((wallet_only["available"].as_f64().unwrap() - 16.0).abs() < 1e-6);
    assert!(wallet_only["wallets"][0].get("total").is_none());
}

#[test]
fn handles_unlimited_subscription_zero_amount_total() {
    let now_ms = 1_790_000_000_000i64;
    let payload = json!({
        "success": true,
        "data": {
            "billing_preference": "subscription_first",
            "subscriptions": [{
                "subscription": {
                    "amount_total": 0,
                    "amount_used": 12345,
                    "status": "active",
                    "end_time": 1_800_000_000
                }
            }]
        }
    });
    let summary = parse_new_api_subscription(&payload, now_ms).expect("unlimited");
    assert!(summary.has_unlimited);
    let doc = new_api_document(
        0.0,
        None,
        Some(&cny_display()),
        &json!({}),
        None,
        Some(&summary),
        Some(&payload),
    );
    assert_eq!(doc["unlimited"].as_bool(), Some(true));
    assert!(doc["available"].is_null());
    assert_eq!(doc["availableView"].as_str(), Some("不限"));
}

#[tokio::test]
async fn query_new_api_strips_v1_and_reads_subscription_over_http() {
    use axum::{http::StatusCode, response::Html, routing::get, Json, Router};
    use crate::server::core::account_store::custom_accounts::CustomBalanceQuery;

    let app = Router::new()
        .route(
            "/api/user/self",
            get(|| async { Json(json!({"success": true, "data": {"quota": 0, "used_quota": 1_505_000}})) }),
        )
        .route(
            "/api/status",
            get(|| async {
                Json(json!({
                    "success": true,
                    "data": {
                        "quota_display_type": "CNY",
                        "quota_per_unit": 500_000,
                        "usd_exchange_rate": 8
                    }
                }))
            }),
        )
        .route(
            "/api/subscription/self",
            get(|| async {
                Json(json!({
                    "success": true,
                    "data": {
                        "billing_preference": "subscription_first",
                        "subscriptions": [{
                            "subscription": {
                                "amount_total": 5_000_000,
                                "amount_used": 1_505_000,
                                "status": "active",
                                "end_time": 4_102_444_800i64,
                                "next_reset_time": 4_102_358_400i64,
                                "allow_wallet_overflow": true
                            }
                        }]
                    }
                }))
            }),
        )
        .route(
            "/wallet-only/api/user/self",
            get(|| async { Json(json!({"success": true, "data": {"quota": 1_000_000, "used_quota": 0}})) }),
        )
        .route(
            "/wallet-only/api/status",
            get(|| async {
                Json(json!({
                    "success": true,
                    "data": {
                        "quota_display_type": "USD",
                        "quota_per_unit": 500_000
                    }
                }))
            }),
        )
        .route(
            "/wallet-only/api/subscription/self",
            get(|| async { StatusCode::NOT_FOUND }),
        )
        .route(
            "/spa/api/user/self",
            get(|| async { Html("<!doctype html><html><body>SPA</body></html>") }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let query = CustomBalanceQuery {
        user_id: "1001".to_string(),
        access_token: "test-token".to_string(),
        username: String::new(),
        password: String::new(),
        token: String::new(),
        sensenova_jwt: String::new(),
        sensenova_jwt_expires_at: 0,
    };

    // 1. Base URL 带 /v1 + 钱包 0 + 订阅 ¥55.92 / ¥80
    let report = super::super::balance::query_new_api(&format!("http://{addr}/v1"), &query, None, "NewAPI")
        .await
        .expect("query_new_api succeeds with /v1 base");
    assert!((report["available"].as_f64().unwrap() - 55.92).abs() < 1e-6);
    assert_eq!(report["unit"].as_str(), Some("CNY"));
    assert_eq!(report["wallets"][0]["type"].as_str(), Some("subscription"));
    assert_eq!(report["wallets"][0]["total"].as_f64(), Some(80.0));
    assert_eq!(report["subscription"]["resetsAt"].as_i64(), Some(4_102_358_400_000));

    // 2. 旧站点无 /api/subscription/self（404）时回退钱包余额 $2
    let fallback = super::super::balance::query_new_api(
        &format!("http://{addr}/wallet-only/V1"),
        &query,
        None,
        "OldNewAPI",
    )
    .await
    .expect("fallback to wallet quota when subscription endpoint is 404");
    assert!((fallback["available"].as_f64().unwrap() - 2.0).abs() < 1e-6);
    assert_eq!(fallback["unit"].as_str(), Some("USD"));
    assert!(fallback["subscription"].is_null());

    // 3. 上游返回 HTML 200 时给出清晰提示并带请求 URL
    let html_err = super::super::balance::query_new_api(
        &format!("http://{addr}/spa/v1"),
        &query,
        None,
        "SpaSite",
    )
    .await
    .expect_err("HTML 200 should fail with clear diagnostic");
    assert!(html_err.message.contains("HTML"));
    assert!(html_err.message.contains("/spa/api/user/self"));

    server.abort();
}
