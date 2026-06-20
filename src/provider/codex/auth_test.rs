use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::auth::{account_id_from_test_id_token, has_openai_api_key_for_test};

/// Codex id_token 可以解析出 ChatGPT account id。
#[test]
fn parse_account_id_from_codex_id_token() {
    let payload = r#"{
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "account-123"
        }
    }"#;
    let jwt = format!(
        "header.{}.signature",
        URL_SAFE_NO_PAD.encode(payload.as_bytes())
    );

    assert_eq!(
        account_id_from_test_id_token(&jwt).as_deref(),
        Some("account-123")
    );
}

/// 未使用的 API key 字段不会影响 OAuth token 读取。
#[test]
fn deserialize_codex_auth_accepts_openai_api_key_field() {
    let raw = r#"{
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": "sk-unused",
        "tokens": {
            "access_token": "access",
            "account_id": "account"
        }
    }"#;

    assert!(has_openai_api_key_for_test(raw));
}
