use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::AppResult;
use crate::home::AppPaths;
use crate::session::SessionState;

/// Codex client metadata key：installation id。
pub const X_CODEX_INSTALLATION_ID_HEADER: &str = "x-codex-installation-id";
/// Codex client metadata key：turn metadata JSON。
pub const X_CODEX_TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
/// Codex client metadata key：window id。
pub const X_CODEX_WINDOW_ID_HEADER: &str = "x-codex-window-id";

/// 单次 Codex 请求的遥测上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRequestMetadata {
    /// installation id，持久化到 `~/.llm-loop/installation_id`。
    pub installation_id: String,
    /// session id，轻量版本中等同 llm-loop session id。
    pub session_id: String,
    /// thread id，轻量版本中等同 llm-loop session id。
    pub thread_id: String,
    /// turn id，每次用户请求生成一个 UUID。
    pub turn_id: String,
    /// window id，按 Codex 形状使用 `<thread_id>:0`。
    pub window_id: String,
    /// 请求类型，当前普通消息固定为 `turn`。
    pub request_kind: String,
    /// turn 开始时间，Unix 毫秒。
    pub turn_started_at_unix_ms: i64,
}

/// `x-codex-turn-metadata` 内部 JSON payload。
#[derive(Debug, Serialize)]
struct TurnMetadataPayload<'a> {
    /// installation id。
    installation_id: &'a str,
    /// session id。
    session_id: &'a str,
    /// thread id。
    thread_id: &'a str,
    /// turn id。
    turn_id: &'a str,
    /// window id。
    window_id: &'a str,
    /// 请求类型。
    request_kind: &'a str,
    /// turn 开始时间，Unix 毫秒。
    turn_started_at_unix_ms: i64,
}

impl CodexRequestMetadata {
    /// 为普通用户 turn 构造 Codex 风格遥测上下文。
    pub async fn for_turn(paths: &AppPaths, session: &SessionState) -> AppResult<Self> {
        let installation_id = resolve_installation_id(&paths.installation_id_path).await?;
        let turn_id = Uuid::new_v4().to_string();
        let turn_started_at_unix_ms = chrono::Utc::now().timestamp_millis();
        Ok(Self {
            installation_id,
            session_id: session.id.clone(),
            thread_id: session.id.clone(),
            turn_id,
            window_id: format!("{}:0", session.id),
            request_kind: "turn".to_string(),
            turn_started_at_unix_ms,
        })
    }

    /// 构造 Responses API 的 `client_metadata` 字段。
    pub fn client_metadata(&self) -> AppResult<BTreeMap<String, String>> {
        let mut metadata = BTreeMap::new();
        metadata.insert(
            X_CODEX_INSTALLATION_ID_HEADER.to_string(),
            self.installation_id.clone(),
        );
        metadata.insert("session_id".to_string(), self.session_id.clone());
        metadata.insert("thread_id".to_string(), self.thread_id.clone());
        metadata.insert("turn_id".to_string(), self.turn_id.clone());
        metadata.insert(X_CODEX_WINDOW_ID_HEADER.to_string(), self.window_id.clone());
        metadata.insert(
            X_CODEX_TURN_METADATA_HEADER.to_string(),
            self.turn_metadata_json()?,
        );
        Ok(metadata)
    }

    /// 构造兼容 header 使用的 turn metadata JSON。
    pub fn turn_metadata_json(&self) -> AppResult<String> {
        Ok(serde_json::to_string(&TurnMetadataPayload {
            installation_id: &self.installation_id,
            session_id: &self.session_id,
            thread_id: &self.thread_id,
            turn_id: &self.turn_id,
            window_id: &self.window_id,
            request_kind: &self.request_kind,
            turn_started_at_unix_ms: self.turn_started_at_unix_ms,
        })?)
    }

    /// 构造固定测试遥测，避免请求体单测访问真实文件。
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            installation_id: "00000000-0000-4000-8000-000000000000".to_string(),
            session_id: "session-test".to_string(),
            thread_id: "session-test".to_string(),
            turn_id: "00000000-0000-4000-8000-000000000001".to_string(),
            window_id: "session-test:0".to_string(),
            request_kind: "turn".to_string(),
            turn_started_at_unix_ms: 0,
        }
    }
}

/// 读取或创建持久化 installation id。
async fn resolve_installation_id(path: &Path) -> AppResult<String> {
    if let Ok(raw) = tokio::fs::read_to_string(path).await {
        let trimmed = raw.trim();
        if Uuid::parse_str(trimmed).is_ok() {
            return Ok(trimmed.to_string());
        }
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let installation_id = Uuid::new_v4().to_string();
    let mut file = tokio::fs::File::create(path).await?;
    file.write_all(installation_id.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(installation_id)
}

#[cfg(test)]
#[path = "telemetry_test.rs"]
mod telemetry_test;
