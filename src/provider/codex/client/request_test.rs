use super::request::{
    build_compact_request_body, build_headers, build_request_body, parse_error_summary, resolve_url,
};
use crate::config::ProviderConfig;
use crate::message::InboundAttachment;
use crate::provider::codex::auth::{ProviderRoute, ProviderRouteKind};
use crate::provider::codex::telemetry::CodexRequestMetadata;
use crate::session::SessionState;
use crate::session_store::ConversationItem;

/// custom provider base_url 会补 `/responses`。
#[test]
fn custom_provider_url_appends_responses() {
    let route = ProviderRoute {
        kind: ProviderRouteKind::Custom,
        base_url: "http://127.0.0.1:8080/v1".to_string(),
        bearer_token: "token".to_string(),
        account_id: None,
    };

    assert_eq!(resolve_url(&route), "http://127.0.0.1:8080/v1/responses");
}

/// 失败响应摘要会提取 response.failed 错误码，适用于 502 日志排查。
#[test]
fn parse_error_summary_reads_response_failed_event() {
    let body = r#"{"error":{"message":"Upstream request failed","type":"upstream_error"}}event: response.failed
data: {"type":"response.failed","response":{"error":{"code":"upstream_error","message":"Upstream request failed"}}}"#;

    assert_eq!(
        parse_error_summary(body),
        "code=upstream_error message=Upstream request failed"
    );
}

/// 初始指令会进入顶层 instructions，不污染 Responses input 对话流。
#[test]
fn request_body_puts_initial_context_in_instructions() {
    let mut session = SessionState::new("key".to_string());
    session.instructions = "skill list".to_string();
    session.initial_context_loaded = true;
    let history = vec![ConversationItem::User {
        text: "hello".to_string(),
    }];
    let metadata = CodexRequestMetadata::for_test();
    let body = build_request_body(
        &ProviderConfig {
            model: Some("gpt-5-codex".to_string()),
            model_reasoning_effort: Some("high".to_string()),
            custom_provider: None,
            ..ProviderConfig::default()
        },
        &session,
        &history,
        "hello",
        &[],
        &metadata,
        &[],
        &[],
    )
    .expect("请求体应能构造");
    let input = body
        .get("input")
        .and_then(|value| value.as_array())
        .expect("input 应为数组");

    assert_eq!(body["instructions"], "skill list");
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[0]["content"], "hello");
    assert_eq!(body["store"], false);
    assert_eq!(body["include"][0], "reasoning.encrypted_content");
    assert_eq!(body["prompt_cache_key"], session.id);
}

/// Codex headers 对齐官方 Responses HTTP 路径。
#[test]
fn request_headers_use_session_headers_without_old_beta() {
    let session = SessionState::new("key".to_string());
    let metadata = CodexRequestMetadata::for_test();
    let route = ProviderRoute {
        kind: ProviderRouteKind::Custom,
        base_url: "http://127.0.0.1:8080/v1".to_string(),
        bearer_token: "token".to_string(),
        account_id: None,
    };

    let headers = build_headers(&route, &session, &metadata).expect("headers 应能构造");

    assert!(headers.get("OpenAI-Beta").is_none());
    assert_eq!(headers["session-id"], metadata.session_id);
    assert_eq!(headers["thread-id"], metadata.thread_id);
}

/// 当前用户消息带图片时会发送 Responses input_image content。
#[test]
fn request_body_includes_current_user_images() {
    let session = SessionState::new("key".to_string());
    let history = vec![ConversationItem::User {
        text: "看图".to_string(),
    }];
    let metadata = CodexRequestMetadata::for_test();
    let body = build_request_body(
        &ProviderConfig {
            model: Some("gpt-5-codex".to_string()),
            custom_provider: None,
            ..ProviderConfig::default()
        },
        &session,
        &history,
        "看图",
        &[InboundAttachment::Image {
            mime_type: "image/png".to_string(),
            bytes: vec![1, 2, 3],
        }],
        &metadata,
        &[],
        &[],
    )
    .expect("请求体应能构造");
    let input = body
        .get("input")
        .and_then(|value| value.as_array())
        .expect("input 应为数组");
    let content = input[0]["content"].as_array().expect("content 应为数组");

    assert_eq!(input.len(), 1);
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "看图");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "data:image/png;base64,AQID");
}

/// 普通文件只把本地路径和元信息提供给模型。
#[test]
fn request_body_describes_stored_files_as_text() {
    let session = SessionState::new("key".to_string());
    let history = vec![ConversationItem::User {
        text: "看文件".to_string(),
    }];
    let metadata = CodexRequestMetadata::for_test();
    let body = build_request_body(
        &ProviderConfig {
            model: Some("gpt-5-codex".to_string()),
            custom_provider: None,
            ..ProviderConfig::default()
        },
        &session,
        &history,
        "看文件",
        &[InboundAttachment::StoredFile {
            path: std::path::PathBuf::from("/tmp/a.pdf"),
            filename: "a.pdf".to_string(),
            mime_type: "application/pdf".to_string(),
            size: 12,
        }],
        &metadata,
        &[],
        &[],
    )
    .expect("请求体应能构造");
    let input = body["input"].as_array().expect("input 应为数组");
    let content = input[0]["content"].as_array().expect("content 应为数组");
    let file_text = content[1]["text"].as_str().expect("文件说明应为文本");

    assert!(file_text.contains("path: /tmp/a.pdf"));
    assert!(file_text.contains("name: a.pdf"));
    assert!(file_text.contains("mime: application/pdf"));
}

/// 压缩请求不携带工具，适用于避免摘要流程递归进入 tool loop。
#[test]
fn compact_request_body_omits_tools() {
    let session = SessionState::new("key".to_string());
    let history = vec![
        ConversationItem::User {
            text: "旧问题".to_string(),
        },
        ConversationItem::Assistant {
            text: "旧回答".to_string(),
        },
    ];
    let metadata = CodexRequestMetadata::for_test();
    let body = build_compact_request_body(
        &ProviderConfig {
            model: Some("gpt-5-codex".to_string()),
            custom_provider: None,
            ..ProviderConfig::default()
        },
        &session,
        &history,
        &metadata,
    )
    .expect("压缩请求体应能构造");
    let input = body["input"].as_array().expect("input 应为数组");

    assert!(body.get("tools").is_none());
    assert_eq!(body["store"], false);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[2]["role"], "user");
}
