pub mod claude;
pub mod codex;
pub mod limits;

pub use claude::ClaudeProvider;
pub use codex::CodexProvider;

#[cfg(test)]
mod limits_test;

use crate::config::AppConfig;
use crate::error::AppResult;
use crate::home::AppPaths;
use crate::message::{InboundAttachment, MessageSource};
use crate::session::SessionState;
use crate::session_store::ConversationItem;
use crate::tools::ToolRegistry;
use crate::tools::registry::ToolChannel;
use std::sync::Arc;

/// 创建配置指定的 provider，适用于 daemon 启动时固定请求后端。
pub fn build_provider(config: AppConfig, paths: AppPaths) -> AppResult<BuiltinProvider> {
    match config.provider.kind.as_str() {
        "codex" => Ok(BuiltinProvider::Codex(CodexProvider::new(config, paths))),
        "claude" => Ok(BuiltinProvider::Claude(ClaudeProvider::new(config, paths))),
        other => Err(crate::error::AppError::Provider(format!(
            "unsupported provider kind: {other}"
        ))),
    }
}

/// 内置 provider 枚举，避免 async trait object 带来的 vtable 限制。
#[derive(Debug, Clone)]
pub enum BuiltinProvider {
    /// Codex Responses provider。
    Codex(CodexProvider),
    /// Claude Messages provider。
    Claude(ClaudeProvider),
}

impl Provider for BuiltinProvider {
    /// 按枚举分支静态分发 provider 请求。
    async fn complete(
        &self,
        session: &SessionState,
        history: &[ConversationItem],
        source: &MessageSource,
        user_input: &str,
        attachments: &[InboundAttachment],
        tools: &ToolRegistry,
        channel: Arc<dyn ToolChannel>,
    ) -> AppResult<AssistantReply> {
        match self {
            Self::Codex(provider) => {
                provider
                    .complete(
                        session,
                        history,
                        source,
                        user_input,
                        attachments,
                        tools,
                        channel,
                    )
                    .await
            }
            Self::Claude(provider) => {
                provider
                    .complete(
                        session,
                        history,
                        source,
                        user_input,
                        attachments,
                        tools,
                        channel,
                    )
                    .await
            }
        }
    }

    /// 按枚举分支静态分发上下文压缩请求。
    async fn compact(
        &self,
        session: &SessionState,
        history: &[ConversationItem],
    ) -> AppResult<CompactionReply> {
        match self {
            Self::Codex(provider) => provider.compact(session, history).await,
            Self::Claude(provider) => provider.compact(session, history).await,
        }
    }
}

/// provider 抽象，负责把 session + user input 转成模型回复。
#[allow(async_fn_in_trait)]
pub trait Provider: Send + Sync {
    /// 生成回复，适用于 daemon 收到普通文本消息后调用。
    async fn complete(
        &self,
        session: &SessionState,
        history: &[ConversationItem],
        source: &MessageSource,
        user_input: &str,
        attachments: &[InboundAttachment],
        tools: &ToolRegistry,
        channel: Arc<dyn ToolChannel>,
    ) -> AppResult<AssistantReply>;

    /// 压缩历史上下文，适用于不启用工具和 channel 的摘要请求。
    async fn compact(
        &self,
        session: &SessionState,
        history: &[ConversationItem],
    ) -> AppResult<CompactionReply>;
}

/// 助手回复结构，后续可扩展 tool call 和 usage。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantReply {
    /// 回复文本。
    pub text: String,
    /// 本轮 provider 原始项，仅用于当前 tool loop 和调试日志。
    pub raw_items: Vec<serde_json::Value>,
    /// 本轮总 token 用量。
    pub total_tokens: Option<u64>,
    /// 是否请求重置当前 session。
    pub reset_session: bool,
}

/// 上下文压缩回复，只包含可持久化摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReply {
    /// 压缩摘要文本。
    pub summary: String,
    /// 本次摘要请求 token 用量。
    pub total_tokens: Option<u64>,
}
