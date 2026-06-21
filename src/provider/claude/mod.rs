mod client;

use std::collections::HashMap;
use std::sync::Arc;

use reqwest::Client;
use serde_json::{Value, json};

use crate::config::AppConfig;
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::message::{InboundAttachment, MessageSource};
use crate::provider::{AssistantReply, CompactionReply, Provider};
use crate::session::SessionState;
use crate::session_store::ConversationItem;
use crate::tools::ToolRegistry;
use crate::tools::registry::{ToolCall, ToolChannel, ToolInput};
use crate::tools::spec::ToolSpec;

use client::{ClaudeRoute, build_compact_request_body, build_request_body, send_claude_request};

#[cfg(test)]
mod client_test;

/// Claude provider 实例，适用于 Anthropic Messages API 兼容后端。
#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    /// 应用配置，包含 Claude model、host、key 和 max_tokens。
    config: AppConfig,
    /// 应用路径，提供已解析的工作目录。
    paths: AppPaths,
    /// 复用的 HTTP client，降低 daemon 常驻连接成本。
    client: Client,
}

impl ClaudeProvider {
    /// 从配置创建 Claude provider，适用于 daemon 启动。
    pub fn new(config: AppConfig, paths: AppPaths) -> Self {
        Self {
            config,
            paths,
            client: Client::new(),
        }
    }
}

impl Provider for ClaudeProvider {
    /// 生成 Claude 回复，按 Messages API 的 tool_use/tool_result 循环请求。
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
        let route = ClaudeRoute::resolve(&self.config)?;
        let tool_specs = tools.specs();
        let tool_kinds = tool_kinds(&tool_specs);
        crate::log_info!(
            "claude provider tools count={} names={}",
            tool_specs.len(),
            tool_specs
                .iter()
                .map(ToolSpec::name)
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut extra_messages = Vec::<Value>::new();
        let mut raw_items = Vec::<Value>::new();
        let mut total_tokens = None;
        let mut reset_session = false;
        crate::log_info!(
            "claude provider route resolved base_url={} model={}",
            route.base_url,
            self.config.provider.model.as_deref().unwrap_or("")
        );

        let final_text = loop {
            let body = build_request_body(
                &self.config.provider,
                session,
                history,
                user_input,
                attachments,
                &tool_specs,
                &extra_messages,
            )?;
            crate::log_info!(
                "claude provider request sending session_key={} session_id={} history_items={} extra_messages={}",
                session.key,
                session.id,
                history.len(),
                extra_messages.len()
            );
            let response = send_claude_request(&self.client, &route, session, body).await?;
            crate::log_info!(
                "claude provider response parsed text_chars={} content_blocks={} tool_calls={} total_tokens={:?} stop_reason={}",
                response.text.chars().count(),
                response.content.len(),
                response.tool_calls.len(),
                response.total_tokens,
                response.stop_reason.as_deref().unwrap_or("")
            );
            total_tokens = response.total_tokens.or(total_tokens);
            let assistant_item = claude_raw_message("assistant", response.content.clone());
            raw_items.push(assistant_item.clone());
            if response.tool_calls.is_empty() {
                if response.text.is_empty() {
                    return Err(AppError::Provider(
                        "claude response completed without assistant text".to_string(),
                    ));
                }
                break response.text;
            }

            extra_messages.push(strip_provider_marker(&assistant_item));
            let context = tools.context_with_channel(
                session.clone(),
                source.clone(),
                &self.paths.work_dir,
                Arc::clone(&channel),
            )?;
            for call in response.tool_calls {
                crate::log_info!(
                    "claude provider tool executing name={} call_id={}",
                    call.name,
                    call.call_id
                );
                if call.name == "new_context" {
                    reset_session = true;
                }
                let call = normalize_tool_input(call, &tool_kinds)?;
                let result = match tools.execute(call.clone(), context.clone()).await {
                    Ok(output) => {
                        crate::log_info!(
                            "claude provider tool finished name={} call_id={}",
                            call.name,
                            call.call_id
                        );
                        claude_tool_result_item(
                            &output.call_id,
                            tool_output_text(&output.output),
                            false,
                        )
                    }
                    Err(err) => {
                        let message = err.to_string();
                        crate::log_info!(
                            "claude provider tool respond_to_model name={} call_id={} error={}",
                            call.name,
                            call.call_id,
                            message
                        );
                        claude_tool_result_item(&call.call_id, message, true)
                    }
                };
                extra_messages.push(strip_provider_marker(&result));
                raw_items.push(result);
            }
        };

        Ok(AssistantReply {
            text: final_text,
            raw_items,
            total_tokens,
            reset_session,
        })
    }

    /// 生成 Claude 历史摘要，不启用工具和 channel。
    async fn compact(
        &self,
        session: &SessionState,
        history: &[ConversationItem],
    ) -> AppResult<CompactionReply> {
        let route = ClaudeRoute::resolve(&self.config)?;
        let body = build_compact_request_body(&self.config.provider, session, history)?;
        crate::log_info!(
            "claude provider compact sending session_key={} session_id={} history_items={}",
            session.key,
            session.id,
            history.len()
        );
        let response = send_claude_request(&self.client, &route, session, body).await?;
        if response.text.trim().is_empty() {
            return Err(AppError::Provider(
                "claude compact completed without summary text".to_string(),
            ));
        }
        crate::log_info!(
            "claude provider compact parsed summary_chars={} total_tokens={:?}",
            response.text.chars().count(),
            response.total_tokens
        );
        Ok(CompactionReply {
            summary: response.text,
            total_tokens: response.total_tokens,
        })
    }
}

/// Claude 工具输入形态，记录 spec 转换时是否来自 freeform 工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeToolKind {
    /// 普通 JSON function 工具。
    Function,
    /// Codex freeform 工具，被桥接为 `{ input: string }`。
    Freeform,
}

/// 构建工具类型索引，适用于 Claude tool_use 回包还原本地 ToolInput。
fn tool_kinds(specs: &[ToolSpec]) -> HashMap<String, ClaudeToolKind> {
    specs
        .iter()
        .map(|spec| {
            let kind = match spec {
                ToolSpec::Freeform(_) => ClaudeToolKind::Freeform,
                ToolSpec::Function(_)
                | ToolSpec::ImageGeneration { .. }
                | ToolSpec::WebSearch { .. } => ClaudeToolKind::Function,
            };
            (spec.name().to_string(), kind)
        })
        .collect()
}

/// 还原本地工具输入，适用于 Claude 只返回 object input 的限制。
fn normalize_tool_input(
    mut call: ToolCall,
    kinds: &HashMap<String, ClaudeToolKind>,
) -> AppResult<ToolCall> {
    if !matches!(kinds.get(&call.name), Some(ClaudeToolKind::Freeform)) {
        return Ok(call);
    }
    let ToolInput::Function { arguments } = &call.input else {
        return Ok(call);
    };
    let value: Value = serde_json::from_str(arguments)?;
    let input = value
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string());
    call.input = ToolInput::Custom { input };
    Ok(call)
}

/// 构造持久化 Claude message，适用于 session history 复用。
fn claude_raw_message(role: &str, content: Vec<Value>) -> Value {
    json!({
        "provider": "claude",
        "role": role,
        "content": content,
    })
}

/// 构造 Claude tool_result 消息。
fn claude_tool_result_item(tool_use_id: &str, content: String, is_error: bool) -> Value {
    claude_raw_message(
        "user",
        vec![json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        })],
    )
}

/// 移除内部 provider 标记，适用于发送 Claude Messages API。
fn strip_provider_marker(value: &Value) -> Value {
    let mut output = value.clone();
    if let Some(object) = output.as_object_mut() {
        object.remove("provider");
    }
    output
}

/// 将工具输出压成 Claude tool_result 文本。
fn tool_output_text(output: &Value) -> String {
    match output {
        Value::String(value) => value.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|error| error.to_string()),
    }
}
