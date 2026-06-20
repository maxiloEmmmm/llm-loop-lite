use uuid::Uuid;

use super::{CodexRequestMetadata, X_CODEX_TURN_METADATA_HEADER};
use crate::home::AppPaths;
use crate::session::SessionState;

/// turn metadata JSON 对齐 Codex 字段。
#[tokio::test]
async fn codex_turn_metadata_contains_core_fields() {
    let paths = AppPaths::from_home(temp_home("codex_turn_metadata_contains_core_fields"));
    let session = SessionState::new("key-a".to_string());

    let metadata = CodexRequestMetadata::for_turn(&paths, &session)
        .await
        .expect("应能构造 metadata");
    let value: serde_json::Value =
        serde_json::from_str(&metadata.turn_metadata_json().expect("应能序列化"))
            .expect("turn metadata 应是 JSON");

    assert!(Uuid::parse_str(&metadata.installation_id).is_ok());
    assert!(Uuid::parse_str(&metadata.turn_id).is_ok());
    assert_eq!(value["session_id"], session.id);
    assert_eq!(value["thread_id"], session.id);
    assert_eq!(value["turn_id"], metadata.turn_id);
    assert_eq!(value["window_id"], format!("{}:0", session.id));
    assert_eq!(value["request_kind"], "turn");
    assert!(value["turn_started_at_unix_ms"].as_i64().unwrap_or(0) > 0);
}

/// client_metadata 包含 Codex turn metadata 字符串。
#[tokio::test]
async fn client_metadata_contains_turn_metadata_blob() {
    let paths = AppPaths::from_home(temp_home("client_metadata_contains_turn_metadata_blob"));
    let session = SessionState::new("key-b".to_string());
    let metadata = CodexRequestMetadata::for_turn(&paths, &session)
        .await
        .expect("应能构造 metadata");

    let client_metadata = metadata
        .client_metadata()
        .expect("应能构造 client metadata");

    assert!(client_metadata.contains_key(X_CODEX_TURN_METADATA_HEADER));
}

/// installation id 会持久化复用。
#[tokio::test]
async fn installation_id_is_persisted() {
    let paths = AppPaths::from_home(temp_home("installation_id_is_persisted"));
    let session = SessionState::new("key-c".to_string());

    let first = CodexRequestMetadata::for_turn(&paths, &session)
        .await
        .expect("首次应能构造 metadata");
    let second = CodexRequestMetadata::for_turn(&paths, &session)
        .await
        .expect("再次应能构造 metadata");

    assert_eq!(first.installation_id, second.installation_id);
}

/// 创建唯一临时 home，适用于不依赖第三方临时目录库的测试。
fn temp_home(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("llm-loop-{name}-{nanos}"))
}
