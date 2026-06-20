use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::error::AppResult;
use crate::session::SessionState;
use crate::store::store_hash;

/// 对话项的临时请求表示，只在构造 provider 请求时存在。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConversationItem {
    /// 用户输入消息。
    User {
        /// 用户输入正文。
        text: String,
    },
    /// 助手最终可见回复。
    Assistant {
        /// 助手回复正文。
        text: String,
    },
}

/// session 历史文件中的一行事件。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
enum SessionStoreEvent {
    /// session 元数据。
    Meta {
        /// session id。
        session_id: String,
        /// 稳定路由 key。
        session_key: String,
    },
    /// 用户消息。
    User {
        /// 消息正文。
        text: String,
    },
    /// 助手最终可见回复。
    Assistant {
        /// 回复正文。
        text: String,
    },
    /// 旧版 provider 原始 item。
    Raw {
        /// 原始 item，仅用于兼容旧文件读取。
        value: Value,
    },
    /// token 使用量。
    TokenUsage {
        /// 本轮 total tokens。
        total_tokens: u64,
    },
    /// 历史压缩检查点。
    Compaction {
        /// 压缩后 provider 应看到的历史。
        items: Vec<ConversationItem>,
    },
}

/// 加载 session 元信息，适用于 daemon 重启后按 channel key 懒恢复。
pub async fn load_session_meta(root: &Path, session_key: &str) -> AppResult<Option<SessionState>> {
    let path = session_path(root, session_key);
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let mut session = SessionState::new(session_key.to_string());
    let mut saw_meta = false;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let event: SessionStoreEvent = serde_json::from_str(line)?;
        match event {
            SessionStoreEvent::Meta {
                session_id,
                session_key: stored_key,
            } => {
                if stored_key == session_key {
                    session.id = session_id;
                    saw_meta = true;
                }
            }
            SessionStoreEvent::User { .. }
            | SessionStoreEvent::Assistant { .. }
            | SessionStoreEvent::Raw { .. }
            | SessionStoreEvent::Compaction { .. } => {}
            SessionStoreEvent::TokenUsage { total_tokens } => {
                session.used_tokens = session.used_tokens.saturating_add(total_tokens);
            }
        }
    }
    if !saw_meta {
        return Ok(None);
    }
    session.initial_context_loaded = false;
    Ok(Some(session))
}

/// 加载临时对话历史，适用于单轮 provider 请求构造。
pub async fn load_history(root: &Path, session_key: &str) -> AppResult<Vec<ConversationItem>> {
    let path = session_path(root, session_key);
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut items = Vec::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let event: SessionStoreEvent = serde_json::from_str(line)?;
        match event {
            SessionStoreEvent::User { text } => {
                items.push(ConversationItem::User { text });
            }
            SessionStoreEvent::Assistant { text } => {
                items.push(ConversationItem::Assistant { text });
            }
            SessionStoreEvent::Compaction { items: compacted } => {
                items = compacted;
            }
            SessionStoreEvent::Raw { .. }
            | SessionStoreEvent::Meta { .. }
            | SessionStoreEvent::TokenUsage { .. } => {}
        }
    }
    Ok(items)
}

/// 初始化 session 历史文件，适用于首次创建本地持久化 session。
pub async fn init_session(root: &Path, session: &SessionState) -> AppResult<()> {
    let path = session_path(root, &session.key);
    if tokio::fs::try_exists(&path).await? {
        return Ok(());
    }
    append_events(
        &path,
        &[SessionStoreEvent::Meta {
            session_id: session.id.clone(),
            session_key: session.key.clone(),
        }],
    )
    .await
}

/// 批量追加一轮成功事件，适用于减少闪存写放大。
pub async fn append_turn(
    root: &Path,
    session: &SessionState,
    user_text: &str,
    assistant_text: &str,
    total_tokens: Option<u64>,
) -> AppResult<()> {
    let path = session_path(root, &session.key);
    let needs_meta = !tokio::fs::try_exists(&path).await?;
    let mut events =
        Vec::with_capacity(usize::from(needs_meta) + 2 + usize::from(total_tokens.is_some()));
    if needs_meta {
        events.push(SessionStoreEvent::Meta {
            session_id: session.id.clone(),
            session_key: session.key.clone(),
        });
    }
    events.push(SessionStoreEvent::User {
        text: user_text.to_string(),
    });
    events.push(SessionStoreEvent::Assistant {
        text: assistant_text.to_string(),
    });
    if let Some(total_tokens) = total_tokens {
        events.push(SessionStoreEvent::TokenUsage { total_tokens });
    }
    append_events(&path, &events).await
}

/// 追加压缩检查点，适用于 provider 请求前持久化裁剪后的上下文。
pub async fn append_compaction(
    root: &Path,
    session: &SessionState,
    items: Vec<ConversationItem>,
) -> AppResult<()> {
    let path = session_path(root, &session.key);
    let needs_meta = !tokio::fs::try_exists(&path).await?;
    let mut events = Vec::with_capacity(usize::from(needs_meta) + 1);
    if needs_meta {
        events.push(SessionStoreEvent::Meta {
            session_id: session.id.clone(),
            session_key: session.key.clone(),
        });
    }
    events.push(SessionStoreEvent::Compaction { items });
    append_events(&path, &events).await
}

/// 删除 session 历史，适用于 `/reset` 后释放旧上下文。
pub async fn remove_session(root: &Path, session_key: &str) -> AppResult<()> {
    match tokio::fs::remove_file(session_path(root, session_key)).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// 批量追加事件到 JSONL，适用于一轮对话只打开一次文件。
async fn append_events(path: &Path, events: &[SessionStoreEvent]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    for event in events {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        file.write_all(&line).await?;
    }
    Ok(())
}

/// 计算 session 历史文件路径。
fn session_path(root: &Path, session_key: &str) -> PathBuf {
    root.join(format!("{}.jsonl", store_hash(session_key)))
}

#[cfg(test)]
#[path = "session_store_test.rs"]
mod session_store_test;
