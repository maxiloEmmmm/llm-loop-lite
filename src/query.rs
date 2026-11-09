use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::error::{AppError, AppResult};
use crate::provider::{BuiltinProvider, Provider, StreamChunk};
use crate::session::SessionState;
use crate::session_store::ConversationItem;

/// 独立查询请求，历史由浏览器持有并在每轮完整提交。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    /// 浏览器 session ID，仅用于上游缓存与请求标识。
    pub session_id: String,
    /// 当前 session 的 system 提示词。
    pub system_prompt: String,
    /// 本轮之前的对话历史。
    pub messages: Vec<QueryMessage>,
    /// 当前用户输入。
    pub content: String,
}

/// 独立查询历史消息，例如 `{role: user, content: hello}`。
#[derive(Debug, Deserialize)]
pub struct QueryMessage {
    /// 消息角色，只允许 `user` 或 `assistant`。
    pub role: String,
    /// 消息正文。
    pub content: String,
}

/// 查询失败响应，适用于 JSON 参数错误而非 SSE 运行时错误。
#[derive(Debug, Serialize)]
pub struct QueryError {
    /// 可直接展示的错误摘要。
    pub error: String,
}

/// 无状态独立查询服务，每次请求只使用请求中携带的提示词和历史。
#[derive(Clone)]
pub struct QueryService {
    /// 复用 daemon 当前 provider 配置和 HTTP 连接池。
    provider: BuiltinProvider,
}

impl QueryService {
    /// 创建查询服务，例如 `daemon provider -> QueryService`。
    pub fn new(provider: BuiltinProvider) -> Self {
        Self { provider }
    }

    /// 启动一次独立流式查询，例如 `request -> delta...done receiver`。
    pub fn stream(&self, request: QueryRequest) -> AppResult<mpsc::Receiver<StreamChunk>> {
        validate_request(&request)?;
        let history = convert_history(request.messages)?;
        let mut session = SessionState::new(format!("web-query:{}", request.session_id));
        session.id = request.session_id;
        session.instructions = request.system_prompt;
        session.initial_context_loaded = true;
        let provider = self.provider.clone();
        let content = request.content;
        let (sender, receiver) = mpsc::channel(64);
        tokio::spawn(async move {
            match provider
                .stream_query(&session, &history, &content, sender.clone())
                .await
            {
                Ok(total_tokens) => {
                    let _ = sender.send(StreamChunk::Done { total_tokens }).await;
                }
                Err(err) => {
                    let _ = sender
                        .send(StreamChunk::Error {
                            message: query_error_message(&err),
                        })
                        .await;
                }
            }
        });
        Ok(receiver)
    }
}

/// 校验独立查询必填字段，例如 `empty content -> error`。
fn validate_request(request: &QueryRequest) -> AppResult<()> {
    if request.session_id.is_empty()
        || request.session_id.len() > 128
        || !request
            .session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(AppError::Channel("query sessionId is invalid".to_string()));
    }
    if request.content.trim().is_empty() {
        return Err(AppError::Channel(
            "query content must not be empty".to_string(),
        ));
    }
    Ok(())
}

/// 把浏览器历史转换成 provider 历史，例如 `user/assistant -> ConversationItem`。
fn convert_history(messages: Vec<QueryMessage>) -> AppResult<Vec<ConversationItem>> {
    messages
        .into_iter()
        .map(|message| match message.role.as_str() {
            "user" => Ok(ConversationItem::User {
                text: message.content,
            }),
            "assistant" => Ok(ConversationItem::Assistant {
                text: message.content,
            }),
            role => Err(AppError::Channel(format!(
                "query message role `{role}` is invalid"
            ))),
        })
        .collect()
}

/// 返回适合浏览器展示的查询错误，避免泄露完整上游正文，例如 `provider 500 -> summary`。
fn query_error_message(error: &AppError) -> String {
    let message = error.to_string();
    if message.len() <= 512 {
        return message;
    }
    format!("{}…", message.chars().take(512).collect::<String>())
}
