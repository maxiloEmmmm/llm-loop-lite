use crate::config::ProviderConfig;
use crate::provider::claude::client::{build_compact_request_body, build_request_body};
use crate::session::SessionState;
use crate::session_store::ConversationItem;

/// 构造 Claude 测试配置，适用于请求体字段断言。
fn claude_config() -> ProviderConfig {
    ProviderConfig {
        kind: "claude".to_string(),
        model: Some("claude-sonnet-4-5".to_string()),
        max_tokens: Some(4096),
        ..ProviderConfig::default()
    }
}

/// 普通 Claude 请求默认写入 block 级 prompt cache。
#[test]
fn claude_request_body_marks_prompt_cache_blocks() {
    let mut session = SessionState::new("test-session".to_string());
    session.instructions = "system prompt".to_string();
    let body = build_request_body(&claude_config(), &session, &[], "你好", &[], &[], &[])
        .expect("Claude 请求体应该能构造");

    assert!(body.get("cache_control").is_none());
    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["text"], "system prompt");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
}

/// Claude compact 请求默认写入 block 级 prompt cache。
#[test]
fn claude_compact_body_marks_prompt_cache_blocks() {
    let mut session = SessionState::new("test-session".to_string());
    session.instructions = "system prompt".to_string();
    let history = vec![ConversationItem::User {
        text: "需要摘要的历史".to_string(),
    }];
    let body = build_compact_request_body(&claude_config(), &session, &history)
        .expect("Claude compact 请求体应该能构造");

    assert!(body.get("cache_control").is_none());
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(
        body["messages"][1]["content"][0]["cache_control"]["type"],
        "ephemeral"
    );
}

/// Claude 未配置 max-tokens 时使用模型 registry。
#[test]
fn claude_request_body_uses_registry_max_tokens() {
    let mut config = claude_config();
    config.model = Some("claude-sonnet-4-6".to_string());
    config.max_tokens = None;
    let session = SessionState::new("test-session".to_string());

    let body = build_request_body(&config, &session, &[], "你好", &[], &[], &[])
        .expect("Claude 请求体应该能构造");

    assert_eq!(body["max_tokens"], 64_000);
}
