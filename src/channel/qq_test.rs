use serde_json::json;

use crate::channel::qq::{
    QqMessageTarget, QqTokenResponse, clean_qq_text, qq_event_to_inbound, qq_outbound_markdown,
};
use crate::message::OutboundRecipient;

/// 测试 QQ 文本清理，适用于群聊 @ 机器人后的正文归一化。
#[test]
fn clean_qq_text_removes_mentions() {
    assert_eq!(clean_qq_text("<@!123> 你好 QQ"), "你好 QQ");
}

/// 测试 QQ 出站 Markdown 修正，避免展示 Feishu 专用转义。
#[test]
fn outbound_markdown_restores_ordered_list_markers() {
    assert_eq!(
        qq_outbound_markdown("[id]\n1\\. 验证数据\n  2\\. 创建任务\n正文 3\\. 不改"),
        "[id]\n1. 验证数据\n  2. 创建任务\n正文 3\\. 不改"
    );
}

/// 测试 QQ 目标解析，防止群聊和私聊出站 API 混用。
#[test]
fn message_target_uses_chat_id_prefix() {
    assert!(matches!(
        QqMessageTarget::from_outbound("user:u1", OutboundRecipient::Chat).unwrap(),
        QqMessageTarget::User("u1")
    ));
    assert!(matches!(
        QqMessageTarget::from_outbound("group:g1", OutboundRecipient::Chat).unwrap(),
        QqMessageTarget::Group("g1")
    ));
    assert!(matches!(
        QqMessageTarget::from_outbound("channel:c1", OutboundRecipient::Chat).unwrap(),
        QqMessageTarget::Channel("c1")
    ));
}

/// 测试 QQ 群事件转换，保证 daemon 收到可回复的来源信息。
#[test]
fn group_event_keeps_reply_target() {
    let message = qq_event_to_inbound(
        "qq-main",
        "GROUP_AT_MESSAGE_CREATE",
        json!({
            "id": "m1",
            "content": "<@!bot> hi",
            "group_openid": "g1",
            "author": {
                "member_openid": "member1"
            }
        }),
    )
    .unwrap()
    .unwrap();

    assert_eq!(message.text, "hi");
    assert_eq!(message.source.channel_name, "qq-main");
    assert_eq!(message.source.platform, "qq");
    assert_eq!(message.source.chat_id, "group:g1");
    assert_eq!(message.source.chat_type, "group");
    assert_eq!(message.source.user_id.as_deref(), Some("member1"));
    assert_eq!(message.message_id.as_deref(), Some("m1"));
}

/// 测试 AccessToken 字段兼容，适用于 QQ 返回驼峰字段的场景。
#[test]
fn token_response_accepts_camel_case() {
    let parsed: QqTokenResponse = serde_json::from_value(json!({
        "accessToken": "token",
        "expiresIn": "7200"
    }))
    .unwrap();

    assert_eq!(parsed.access_token, "token");
    assert_eq!(parsed.expires_in.as_str(), Some("7200"));
}
