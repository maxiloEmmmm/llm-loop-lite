use super::FeishuReceiveMessageEvent;

/// 解析飞书收消息事件，适用于 mention 门禁测试。
fn receive_event(mentions: serde_json::Value) -> FeishuReceiveMessageEvent {
    serde_json::from_value(serde_json::json!({
        "sender": {
            "sender_id": {
                "open_id": "ou_sender",
                "user_id": "user_sender",
                "union_id": "on_sender"
            }
        },
        "message": {
            "message_id": "om_test",
            "root_id": null,
            "parent_id": null,
            "thread_id": null,
            "chat_id": "oc_test",
            "chat_type": "group",
            "message_type": "text",
            "content": "{\"text\":\"@_user_1 hi\"}",
            "mentions": mentions
        }
    }))
    .expect("飞书测试事件必须能反序列化")
}

/// 验证任意用户 @ 机器人 open_id 时会命中门禁。
#[test]
fn mentions_bot_matches_open_id() {
    let event = receive_event(serde_json::json!([{
        "key": "@_user_1",
        "id": "ou_bot",
        "id_type": "open_id",
        "name": "捣蛋鬼曼波"
    }]));

    assert!(event.mentions_bot("ou_bot", None));
}

/// 验证 mention id 为对象结构时也能命中机器人。
#[test]
fn mentions_bot_matches_open_id_object() {
    let event = receive_event(serde_json::json!([{
        "key": "@_user_1",
        "id": {
            "open_id": "ou_bot",
            "user_id": "bot_user"
        },
        "name": "捣蛋鬼曼波"
    }]));

    assert!(event.mentions_bot("ou_bot", None));
}

/// 验证 @ 普通用户不会误触发机器人。
#[test]
fn mentions_bot_rejects_other_user() {
    let event = receive_event(serde_json::json!([{
        "key": "@_user_1",
        "id": "on_other",
        "id_type": "union_id",
        "name": "陈威"
    }]));

    assert!(!event.mentions_bot("ou_bot", None));
}
