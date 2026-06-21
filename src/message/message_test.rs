use super::{InboundMessage, MessageSource};

/// 构造测试消息，适用于 reset 命令识别覆盖。
fn text_message(text: &str) -> InboundMessage {
    InboundMessage::text(
        text,
        MessageSource::default(),
        Some("message-test".to_string()),
    )
}

/// 验证私聊 reset 命令仍然按原路径识别。
#[test]
fn reset_command_matches_plain_text() {
    assert!(text_message("/reset").is_reset_command());
}

/// 验证群聊 @ 机器人后 reset 仍走 reset 快路径。
#[test]
fn reset_command_ignores_leading_mention_placeholder() {
    assert!(text_message("@_user_1 /reset").is_reset_command());
}

/// 验证带额外正文的 reset 不会被误识别成命令。
#[test]
fn reset_command_rejects_extra_text() {
    assert!(!text_message("@_user_1 /reset now").is_reset_command());
}

/// 验证私聊 stop 命令仍然按取消快路径识别。
#[test]
fn stop_command_matches_plain_text() {
    assert!(text_message("/stop").is_stop_command());
}

/// 验证群聊 @ 机器人后 stop 仍走取消快路径。
#[test]
fn stop_command_ignores_leading_mention_placeholder() {
    assert!(text_message("@_user_1 /stop").is_stop_command());
}

/// 验证带额外正文的 stop 不会被误识别成命令。
#[test]
fn stop_command_rejects_extra_text() {
    assert!(!text_message("@_user_1 /stop now").is_stop_command());
}

/// 验证私聊 status 命令仍然按直回快路径识别。
#[test]
fn status_command_matches_plain_text() {
    assert!(text_message("/status").is_status_command());
}

/// 验证群聊 @ 机器人后 status 仍走直回快路径。
#[test]
fn status_command_ignores_leading_mention_placeholder() {
    assert!(text_message("@_user_1 /status").is_status_command());
}

/// 验证带额外正文的 status 不会被误识别成命令。
#[test]
fn status_command_rejects_extra_text() {
    assert!(!text_message("@_user_1 /status now").is_status_command());
}
