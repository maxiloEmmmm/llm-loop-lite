use super::{estimate_request_tokens, prepare_context_window, prepare_context_window_with_summary};
use crate::session::SessionState;
use crate::session_store::ConversationItem;

/// 构造测试消息，适用于生成超过压缩阈值的长历史。
fn user_text(text: &str) -> ConversationItem {
    ConversationItem::User {
        text: text.to_string(),
    }
}

/// 构造测试回复，适用于生成超过压缩阈值的长历史。
fn assistant_text(text: &str) -> ConversationItem {
    ConversationItem::Assistant {
        text: text.to_string(),
    }
}

/// 未超过 Codex 风格阈值时不压缩历史。
#[test]
fn prepare_context_window_keeps_small_history() {
    let session = SessionState::new("agent:main:test".to_string());
    let history = vec![user_text("hi"), assistant_text("ok")];

    let plan = prepare_context_window(&session, &history, "继续");

    assert!(!plan.compacted);
    assert_eq!(plan.history, history);
}

/// 超过 Codex 风格阈值时保留近期历史并插入 handoff。
#[test]
fn prepare_context_window_compacts_large_history() {
    let mut session = SessionState::new("agent:main:test".to_string());
    session.max_context_tokens = Some(20);
    let history = vec![
        user_text(&"旧请求".repeat(30_000)),
        assistant_text(&"旧回复".repeat(30_000)),
        user_text("最近问题"),
        assistant_text("最近回答"),
    ];

    let plan = prepare_context_window(&session, &history, "继续");

    assert!(plan.compacted);
    assert!(plan.dropped_items > 0);
    assert!(matches!(
        &plan.history[0],
        ConversationItem::User { text } if text.contains("CONTEXT COMPACTION")
    ));
}

/// 模型摘要成功时会作为压缩 handoff 写入历史。
#[test]
fn prepare_context_window_uses_model_summary() {
    let mut session = SessionState::new("agent:main:test".to_string());
    session.max_context_tokens = Some(20);
    let history = vec![
        user_text(&"旧请求".repeat(30_000)),
        assistant_text(&"旧回复".repeat(30_000)),
        user_text("最近问题"),
        assistant_text("最近回答"),
    ];

    let plan = prepare_context_window_with_summary(
        &session,
        &history,
        "继续",
        "模型生成的摘要".to_string(),
    );

    assert!(plan.compacted);
    assert!(matches!(
        &plan.history[0],
        ConversationItem::User { text } if text.contains("模型生成的摘要")
    ));
}

/// 估算请求 token 时跳过与当前输入重复的末尾用户消息。
#[test]
fn estimate_request_tokens_skips_duplicate_last_user() {
    let session = SessionState::new("agent:main:test".to_string());
    let without_duplicate = vec![assistant_text("ok")];
    let with_duplicate = vec![assistant_text("ok"), user_text("继续")];

    let expected = estimate_request_tokens(&session, &without_duplicate, "继续");
    let actual = estimate_request_tokens(&session, &with_duplicate, "继续");

    assert_eq!(actual, expected);
}
