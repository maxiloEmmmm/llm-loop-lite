mod auth;
mod client;
mod login;
mod telemetry;

use reqwest::Client;
use serde_json::{Value, json};

use crate::config::AppConfig;
use crate::error::AppResult;
use crate::home::AppPaths;
use crate::message::{InboundAttachment, MessageSource};
use crate::provider::limits::{ModelLimits, resolve_model_limits};
use crate::provider::{AssistantReply, CompactionReply, Provider};
use crate::session::SessionState;
use crate::session_store::ConversationItem;
use crate::tools::ToolRegistry;
use crate::tools::registry::{ToolChannel, ToolOutputKind};
use crate::tools::spec::{ToolSpec, create_image_generation_tool, create_web_search_tool};

use auth::ProviderRoute;
use client::{
    build_compact_request_body, build_request_body, extract_response_from_sse, send_codex_request,
};
use telemetry::CodexRequestMetadata;

pub use login::run_oauth_login;

/// Codex provider 实例，支持 OAuth 与 custom provider 两种配置模式。
#[derive(Debug, Clone)]
pub struct CodexProvider {
    /// 应用配置，包含 provider 选择器和 custom provider 注册表。
    config: AppConfig,
    /// 应用路径，用于读取 auth.json。
    paths: AppPaths,
    /// 复用的 HTTP client，避免每轮新建连接池。
    client: Client,
}

impl CodexProvider {
    /// 从配置和路径创建 provider，适用于 daemon 启动。
    pub fn new(config: AppConfig, paths: AppPaths) -> Self {
        Self {
            config,
            paths,
            client: Client::new(),
        }
    }

    /// 返回 auth.json 路径。
    pub fn auth_path(&self) -> &std::path::Path {
        &self.paths.auth_path
    }
}

impl Provider for CodexProvider {
    /// 返回 Codex 当前模型限制，适用于隐藏 registry/custom provider 细节。
    fn model_limits(&self) -> ModelLimits {
        resolve_model_limits(&self.config.provider)
    }

    /// 生成 Codex 回复，按 Codex Responses SSE 请求方式发送。
    async fn complete(
        &self,
        session: &SessionState,
        history: &[ConversationItem],
        source: &MessageSource,
        user_input: &str,
        attachments: &[InboundAttachment],
        tools: &ToolRegistry,
        channel: std::sync::Arc<dyn ToolChannel>,
    ) -> AppResult<AssistantReply> {
        let route = ProviderRoute::resolve(&self.config, &self.paths)?;
        crate::log_info!(
            "codex provider route resolved kind={:?} base_url={} model={}",
            route.kind,
            route.base_url,
            self.config.provider.model.as_deref().unwrap_or("")
        );
        let mut tool_specs = tools.specs();
        tool_specs.extend(provider_hosted_tool_specs(&route));
        crate::log_info!("codex provider tools count={}", tool_specs.len());
        let mut extra_input = Vec::<Value>::new();
        let mut raw_items = Vec::<Value>::new();
        let mut total_tokens = None;
        let mut reset_session = false;
        let final_text = loop {
            let metadata = CodexRequestMetadata::for_turn(&self.paths, session).await?;
            crate::log_info!(
                "codex provider turn build session_key={} session_id={} history_items={} extra_input={} raw_items_so_far={}",
                session.key,
                session.id,
                history.len(),
                extra_input.len(),
                raw_items.len()
            );
            let body = build_request_body(
                &self.config.provider,
                session,
                &history,
                user_input,
                attachments,
                &metadata,
                &tool_specs,
                &extra_input,
            )?;
            crate::log_info!("codex provider request sending session_id={}", session.id);
            let sse = send_codex_request(&self.client, &route, session, &metadata, body).await?;
            crate::log_info!(
                "codex provider response received session_id={} bytes={}",
                session.id,
                sse.len()
            );
            let response = extract_response_from_sse(&sse)?;
            crate::log_info!(
                "codex provider response parsed text_chars={} output_items={} tool_calls={} total_tokens={:?}",
                response.text.chars().count(),
                response.output_items.len(),
                response.tool_calls.len(),
                response.total_tokens
            );
            total_tokens = response.total_tokens.or(total_tokens);
            raw_items.extend(response.output_items.clone());
            if response.tool_calls.is_empty() {
                if response.text.is_empty() {
                    return Err(crate::error::AppError::Provider(
                        "codex response completed without assistant text".to_string(),
                    ));
                }
                break response.text;
            }
            extra_input.extend(response.output_items.clone());
            let context = tools.context_with_channel(
                session.clone(),
                source.clone(),
                &self.paths.work_dir,
                std::sync::Arc::clone(&channel),
            )?;
            for call in response.tool_calls {
                crate::log_info!(
                    "codex provider tool executing name={} call_id={}",
                    call.name,
                    call.call_id
                );
                if call.name == "new_context" {
                    reset_session = true;
                }
                match tools.execute(call.clone(), context.clone()).await {
                    Ok(output) => {
                        crate::log_info!(
                            "codex provider tool finished name={} call_id={}",
                            call.name,
                            call.call_id
                        );
                        let item = tool_call_item(&call.name, &output);
                        extra_input.push(item.clone());
                        raw_items.push(item);
                    }
                    Err(err) => {
                        let message = err.to_string();
                        crate::log_info!(
                            "codex provider tool respond_to_model name={} call_id={} error={}",
                            call.name,
                            call.call_id,
                            message
                        );
                        let item = tool_error_item(&call.name, &call.call_id, message);
                        extra_input.push(item.clone());
                        raw_items.push(item);
                    }
                }
            }
        };
        Ok(AssistantReply {
            text: final_text,
            raw_items,
            total_tokens,
            reset_session,
        })
    }

    /// 生成 Codex 历史摘要，不启用工具和 channel。
    async fn compact(
        &self,
        session: &SessionState,
        history: &[ConversationItem],
    ) -> AppResult<CompactionReply> {
        let route = ProviderRoute::resolve(&self.config, &self.paths)?;
        let metadata = CodexRequestMetadata::for_turn(&self.paths, session).await?;
        let body = build_compact_request_body(&self.config.provider, session, history, &metadata)?;
        crate::log_info!(
            "codex provider compact sending session_key={} session_id={} history_items={}",
            session.key,
            session.id,
            history.len()
        );
        let sse = send_codex_request(&self.client, &route, session, &metadata, body).await?;
        let response = extract_response_from_sse(&sse)?;
        if response.text.trim().is_empty() {
            return Err(crate::error::AppError::Provider(
                "codex compact completed without summary text".to_string(),
            ));
        }
        crate::log_info!(
            "codex provider compact parsed summary_chars={} total_tokens={:?}",
            response.text.chars().count(),
            response.total_tokens
        );
        Ok(CompactionReply {
            summary: response.text,
            total_tokens: response.total_tokens,
        })
    }
}

/// 返回 Codex provider 支持的 hosted tools。
fn provider_hosted_tool_specs(route: &ProviderRoute) -> Vec<ToolSpec> {
    if matches!(route.kind, auth::ProviderRouteKind::CodexOauth) {
        vec![
            create_web_search_tool(true),
            create_image_generation_tool("png"),
        ]
    } else {
        Vec::new()
    }
}

/// 把本地工具输出转成 Responses input item。
fn tool_call_item(name: &str, output: &crate::tools::ToolResult) -> Value {
    let output_text = tool_output_text(&output.output);
    match output.output_kind {
        ToolOutputKind::Function => json!({
            "type": "function_call_output",
            "call_id": output.call_id,
            "output": output_text,
        }),
        ToolOutputKind::Custom => json!({
            "type": "custom_tool_call_output",
            "call_id": output.call_id,
            "name": name,
            "output": output_text,
        }),
    }
}

/// 将工具输出压成 Responses 兼容文本。
fn tool_output_text(output: &Value) -> String {
    match output {
        Value::String(value) => value.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|error| error.to_string()),
    }
}

/// 把可恢复的工具错误回灌给模型，适用于参数错误或 channel 能力缺失。
fn tool_error_item(name: &str, call_id: &str, message: String) -> Value {
    let result = crate::tools::ToolResult {
        output_kind: ToolOutputKind::Function,
        call_id: call_id.to_string(),
        output: Value::String(message),
    };
    tool_call_item(name, &result)
}

#[cfg(test)]
mod auth_test;

#[cfg(test)]
mod login_test;

#[cfg(test)]
mod tool_output_test;
