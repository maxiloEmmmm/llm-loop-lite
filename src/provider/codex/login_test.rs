use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};

use super::login::{build_auth_json_for_test, token_response_for_test};

/// device code interval 同时兼容字符串和数字格式。
#[test]
fn parse_device_code_interval_from_string_and_number() {
    let from_string: super::login::UserCodeResp = serde_json::from_str(
        r#"{
            "device_auth_id": "device",
            "user_code": "CODE",
            "interval": "5"
        }"#,
    )
    .expect("字符串 interval 应能解析");
    let from_number: super::login::UserCodeResp = serde_json::from_str(
        r#"{
            "device_auth_id": "device",
            "user_code": "CODE",
            "interval": 7
        }"#,
    )
    .expect("数字 interval 应能解析");

    assert_eq!(from_string.interval, 5);
    assert_eq!(from_number.interval, 7);
}

/// 构造 auth.json 时保留 Codex 兼容字段并解析 account id。
#[test]
fn build_auth_json_matches_codex_shape() {
    let payload = r#"{
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "account-123"
        }
    }"#;
    let id_token = format!(
        "header.{}.signature",
        URL_SAFE_NO_PAD.encode(payload.as_bytes())
    );
    let now = DateTime::parse_from_rfc3339("2026-06-18T00:00:00Z")
        .expect("固定时间应能解析")
        .with_timezone(&Utc);

    let value = build_auth_json_for_test(
        token_response_for_test(id_token, "access".to_string(), "refresh".to_string()),
        now,
    );

    assert_eq!(value["auth_mode"], "chatgpt");
    assert!(value["OPENAI_API_KEY"].is_null());
    assert_eq!(value["tokens"]["access_token"], "access");
    assert_eq!(value["tokens"]["refresh_token"], "refresh");
    assert_eq!(value["tokens"]["account_id"], "account-123");
    assert_eq!(value["last_refresh"], "2026-06-18T00:00:00Z");
}
