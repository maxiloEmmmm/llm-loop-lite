use super::{SessionRegistry, build_message_key};
use crate::message::MessageSource;

/// 相同来源会生成稳定 message key。
#[test]
fn message_key_is_stable_for_same_source() {
    let source = MessageSource {
        channel_name: "main".to_string(),
        platform: "ws".to_string(),
        chat_id: "room-1".to_string(),
        chat_type: "dm".to_string(),
        user_id: Some("user-1".to_string()),
        thread_id: None,
    };

    assert_eq!(build_message_key(&source), build_message_key(&source));
}

/// reset 会保留 key 但替换 session id。
#[test]
fn reset_rebinds_key_to_new_session_id() {
    let source = MessageSource {
        channel_name: "main".to_string(),
        platform: "ws".to_string(),
        chat_id: "room-1".to_string(),
        chat_type: "dm".to_string(),
        user_id: Some("user-1".to_string()),
        thread_id: None,
    };
    let mut registry = SessionRegistry::new();
    let first = registry.get_or_create(&source).clone();
    let second = registry.reset(&source).clone();

    assert_eq!(first.key, second.key);
    assert_ne!(first.id, second.id);
}

/// 普通 session 元信息不持有历史，适用于低内存常驻。
#[test]
fn session_state_does_not_store_history() {
    let source = MessageSource {
        channel_name: "main".to_string(),
        platform: "ws".to_string(),
        chat_id: "room-1".to_string(),
        chat_type: "dm".to_string(),
        user_id: Some("user-1".to_string()),
        thread_id: None,
    };
    let mut registry = SessionRegistry::new();
    let session = registry.get_or_create(&source);

    assert_eq!(session.used_tokens, 0);
    assert!(session.instructions.is_empty());
}
