use super::{
    ConversationItem, append_compaction, append_turn, init_session, load_history, load_session_meta,
};
use crate::session::SessionState;

/// 测试 session 历史落盘和恢复，适用于 daemon 重启后的同 key 续聊。
#[tokio::test]
async fn load_session_restores_user_assistant_and_token_usage() {
    let temp = std::env::temp_dir().join(format!("llm-loop-test-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp)
        .await
        .expect("临时目录必须创建成功");
    let session = SessionState::new("agent:main:test".to_string());

    init_session(&temp, &session)
        .await
        .expect("初始化 session 历史必须成功");
    append_turn(&temp, &session, "继续", "ok", Some(12))
        .await
        .expect("批量追加一轮事件必须成功");

    let restored = load_session_meta(&temp, &session.key)
        .await
        .expect("加载 session 元信息必须成功")
        .expect("session 元信息必须存在");
    let history = load_history(&temp, &session.key)
        .await
        .expect("加载临时历史必须成功");

    assert_eq!(restored.id, session.id);
    assert_eq!(restored.used_tokens, 12);
    assert!(!restored.initial_context_loaded);
    assert_eq!(history.len(), 2);
    assert!(matches!(history[0], ConversationItem::User { .. }));
    assert!(matches!(history[1], ConversationItem::Assistant { .. }));
    tokio::fs::remove_dir_all(&temp)
        .await
        .expect("临时目录必须清理成功");
}

/// 压缩检查点会覆盖旧历史，适用于 append-only session 文件恢复。
#[tokio::test]
async fn load_history_uses_latest_compaction_checkpoint() {
    let temp = std::env::temp_dir().join(format!("llm-loop-test-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp)
        .await
        .expect("临时目录必须创建成功");
    let session = SessionState::new("agent:main:test".to_string());

    append_turn(&temp, &session, "旧问题", "旧回答", None)
        .await
        .expect("旧历史必须追加成功");
    append_compaction(
        &temp,
        &session,
        vec![ConversationItem::User {
            text: "压缩摘要".to_string(),
        }],
    )
    .await
    .expect("压缩检查点必须追加成功");
    append_turn(&temp, &session, "新问题", "新回答", None)
        .await
        .expect("新历史必须追加成功");

    let history = load_history(&temp, &session.key)
        .await
        .expect("加载临时历史必须成功");

    assert_eq!(history.len(), 3);
    assert!(matches!(
        &history[0],
        ConversationItem::User { text } if text == "压缩摘要"
    ));
    tokio::fs::remove_dir_all(&temp)
        .await
        .expect("临时目录必须清理成功");
}
