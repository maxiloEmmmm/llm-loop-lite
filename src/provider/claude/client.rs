use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::{Value, json};

use crate::config::{AppConfig, ProviderConfig};
use crate::error::{AppError, AppResult};
use crate::message::InboundAttachment;
use crate::session::SessionState;
use crate::session_store::ConversationItem;
use crate::tools::registry::{ToolCall, ToolInput};
use crate::tools::spec::{JsonSchema, JsonSchemaPrimitiveType, JsonSchemaType, ToolSpec};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Claude 压缩摘要提示词，适用于独立 compact 请求。
const COMPACTION_PROMPT: &str = "Summarize the previous conversation into a compact handoff. Keep only durable facts, decisions, user preferences, file paths, commands/results that matter, current blockers, and unresolved asks. Treat old tasks as historical reference only. Do not answer those tasks. Return only the summary.";

/// Claude 请求路由，包含 host 与认证 key。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeRoute {
    /// Claude API host，可能是官方或 Anthropic 兼容代理。
    pub base_url: String,
    /// API key，按 Anthropic `x-api-key` 头发送。
    api_key: String,
}

impl ClaudeRoute {
    /// 从配置解析 Claude 路由，适用于每轮请求前拿到最新 runtime merge 结果。
    pub fn resolve(config: &AppConfig) -> AppResult<Self> {
        let base_url = config
            .provider
            .base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let api_key = resolve_api_key(&config.provider)?;
        Ok(Self { base_url, api_key })
    }
}

/// Claude 响应的最小抽取结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeResponse {
    /// assistant 文本。
    pub text: String,
    /// assistant content blocks 原文。
    pub content: Vec<Value>,
    /// 待执行工具调用。
    pub tool_calls: Vec<ToolCall>,
    /// 本轮 token 用量。
    pub total_tokens: Option<u64>,
    /// Claude stop_reason。
    pub stop_reason: Option<String>,
}

/// 构造 Claude Messages API 请求体。
pub fn build_request_body(
    config: &ProviderConfig,
    session: &SessionState,
    history: &[ConversationItem],
    user_input: &str,
    attachments: &[InboundAttachment],
    tool_specs: &[ToolSpec],
    extra_messages: &[Value],
) -> AppResult<Value> {
    let model = config
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Provider("provider.model is required".to_string()))?;
    let max_tokens = config.max_tokens.ok_or_else(|| {
        AppError::Provider("provider.max-tokens is required for claude".to_string())
    })?;
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": build_messages(history, user_input, attachments, extra_messages)?,
        "tools": create_tools_json_for_claude(tool_specs)?,
        "tool_choice": {
            "type": "auto",
            "disable_parallel_tool_use": true,
        },
        "metadata": {
            "user_id": session.key,
        },
    });
    if !session.instructions.trim().is_empty() {
        body["system"] = Value::String(session.instructions.clone());
    }
    Ok(body)
}

/// 构造 Claude 压缩请求体，适用于不启用工具的摘要请求。
pub fn build_compact_request_body(
    config: &ProviderConfig,
    session: &SessionState,
    history: &[ConversationItem],
) -> AppResult<Value> {
    let model = config
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Provider("provider.model is required".to_string()))?;
    let max_tokens = config.max_tokens.ok_or_else(|| {
        AppError::Provider("provider.max-tokens is required for claude".to_string())
    })?;
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": build_messages(history, COMPACTION_PROMPT, &[], &[])?,
        "metadata": {
            "user_id": session.key,
        },
    });
    if !session.instructions.trim().is_empty() {
        body["system"] = Value::String(session.instructions.clone());
    }
    Ok(body)
}

/// 发送 Claude 请求并解析 JSON 响应。
pub async fn send_claude_request(
    client: &Client,
    route: &ClaudeRoute,
    session: &SessionState,
    body: Value,
) -> AppResult<ClaudeResponse> {
    let url = resolve_url(route);
    crate::log_info!(
        "claude http request start url={} session_id={}",
        url,
        session.id
    );
    let response = client
        .post(url)
        .headers(build_headers(route)?)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    crate::log_info!(
        "claude http response status={} session_id={} bytes={}",
        status,
        session.id,
        text.len()
    );
    if !status.is_success() {
        crate::log_info!(
            "claude http non_success session_id={} status={} body_preview={} parsed_error={}",
            session.id,
            status,
            log_preview(&text, 2048),
            parse_error_summary(&text)
        );
        return Err(AppError::Provider(format!(
            "claude request failed with status {status}: {text}"
        )));
    }
    extract_response(&text)
}

/// 解析 Claude JSON 响应。
fn extract_response(raw: &str) -> AppResult<ClaudeResponse> {
    let value: Value = serde_json::from_str(raw)?;
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in &content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(delta) = block.get("text").and_then(Value::as_str) {
                    text.push_str(delta);
                }
            }
            Some("tool_use") => {
                tool_calls.push(extract_tool_call(block)?);
            }
            _ => {}
        }
    }
    let total_tokens = value.get("usage").and_then(total_usage_tokens);
    let stop_reason = value
        .get("stop_reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    if text.is_empty() && tool_calls.is_empty() {
        return Err(AppError::Provider(
            "claude response did not contain text or tool_use".to_string(),
        ));
    }
    Ok(ClaudeResponse {
        text,
        content,
        tool_calls,
        total_tokens,
        stop_reason,
    })
}

/// 从 tool_use block 提取本地工具调用。
fn extract_tool_call(block: &Value) -> AppResult<ToolCall> {
    let call_id = block
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Provider("claude tool_use missing id".to_string()))?;
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Provider("claude tool_use missing name".to_string()))?;
    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
    Ok(ToolCall {
        call_id: call_id.to_string(),
        name: name.to_string(),
        input: ToolInput::Function {
            arguments: input.to_string(),
        },
    })
}

/// 构造 Claude messages 数组。
fn build_messages(
    history: &[ConversationItem],
    user_input: &str,
    attachments: &[InboundAttachment],
    extra_messages: &[Value],
) -> AppResult<Vec<Value>> {
    let mut messages = Vec::new();
    let skip_last_user = matches!(
        history.last(),
        Some(ConversationItem::User { text }) if text == user_input
    );
    let last_index = history.len().saturating_sub(1);
    for (index, item) in history.iter().enumerate() {
        if skip_last_user && index == last_index {
            continue;
        }
        match item {
            ConversationItem::User { text } => {
                messages.push(json!({"role": "user", "content": text}));
            }
            ConversationItem::Assistant { text } => {
                messages.push(json!({"role": "assistant", "content": text}));
            }
        }
    }
    messages.push(user_message_item(user_input, attachments)?);
    messages.extend(extra_messages.iter().cloned());
    Ok(messages)
}

/// 构造用户消息，适用于文本、图片和落盘文件混合输入。
fn user_message_item(user_input: &str, attachments: &[InboundAttachment]) -> AppResult<Value> {
    if attachments.is_empty() {
        return Ok(json!({"role": "user", "content": user_input}));
    }
    let mut content = Vec::new();
    if !user_input.trim().is_empty() {
        content.push(json!({
            "type": "text",
            "text": user_input,
        }));
    }
    for attachment in attachments {
        match attachment {
            InboundAttachment::Image { mime_type, bytes } => {
                content.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": claude_image_media_type(mime_type),
                        "data": STANDARD.encode(bytes),
                    },
                }));
            }
            InboundAttachment::StoredFile {
                path,
                filename,
                mime_type,
                size,
            } => {
                content.push(json!({
                    "type": "text",
                    "text": format!(
                        "用户上传了文件：\n- path: {}\n- name: {}\n- mime: {}\n- size: {} bytes",
                        path.display(),
                        filename,
                        mime_type,
                        size
                    ),
                }));
            }
        }
    }
    if content.is_empty() {
        return Err(AppError::Provider(
            "user message has no text or attachment content".to_string(),
        ));
    }
    Ok(json!({
        "role": "user",
        "content": content,
    }))
}

/// 转换图片 MIME，适用于 Claude 只接受固定图片 media_type 的限制。
fn claude_image_media_type(mime_type: &str) -> &str {
    match mime_type {
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" => mime_type,
        _ => "image/png",
    }
}

/// 将本地 tools 转成 Claude `tools` 数组。
fn create_tools_json_for_claude(tools: &[ToolSpec]) -> AppResult<Vec<Value>> {
    let mut output = Vec::new();
    for tool in tools {
        match tool {
            ToolSpec::Function(function) => output.push(json!({
                "name": function.name,
                "description": function.description,
                "input_schema": serde_json::to_value(&function.parameters)?,
            })),
            ToolSpec::Freeform(freeform) => output.push(json!({
                "name": freeform.name,
                "description": freeform.description,
                "input_schema": freeform_input_schema(),
            })),
            ToolSpec::ImageGeneration { .. } | ToolSpec::WebSearch { .. } => {}
        }
    }
    Ok(output)
}

/// 构造 freeform 桥接 schema，适用于 Claude 只能返回 object input 的场景。
fn freeform_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "input": {
                "type": "string",
                "description": "Freeform input body for this custom tool.",
            },
        },
        "required": ["input"],
    })
}

/// 解析 Claude API key，配置 key 优先，其次配置 env，最后对齐官方 SDK 默认 env。
fn resolve_api_key(config: &ProviderConfig) -> AppResult<String> {
    if let Some(key) = config.api_key.as_deref().filter(|value| !value.is_empty()) {
        return Ok(key.to_string());
    }
    let env_name = config.api_key_env.as_deref().unwrap_or(DEFAULT_API_KEY_ENV);
    std::env::var(env_name).map_err(|_| {
        AppError::Provider(format!(
            "claude api key is required: set provider.api-key or env {env_name}"
        ))
    })
}

/// 解析 Claude 请求 URL。
fn resolve_url(route: &ClaudeRoute) -> String {
    let base = route.base_url.trim_end_matches('/');
    if base.ends_with("/v1/messages") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

/// 构造 Claude 请求头。
fn build_headers(route: &ClaudeRoute) -> AppResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static("llm-loop/0.1"));
    headers.insert(
        "anthropic-version",
        HeaderValue::from_static(ANTHROPIC_VERSION),
    );
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(&route.api_key)
            .map_err(|err| AppError::Provider(format!("invalid claude api key header: {err}")))?,
    );
    Ok(headers)
}

/// 汇总 Claude usage token，适用于 session token 累加。
fn total_usage_tokens(usage: &Value) -> Option<u64> {
    let input = usage.get("input_tokens").and_then(Value::as_u64)?;
    let output = usage.get("output_tokens").and_then(Value::as_u64)?;
    let cache_create = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(
        input
            .saturating_add(output)
            .saturating_add(cache_create)
            .saturating_add(cache_read),
    )
}

/// 截断日志正文，适用于记录 provider 失败响应但避免日志文件暴涨。
fn log_preview(value: &str, max_chars: usize) -> String {
    let mut preview = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        preview.push_str("...(truncated)");
    }
    preview.replace('\n', "\\n")
}

/// 提取失败响应摘要，适用于快速 grep 上游错误类型。
fn parse_error_summary(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return "unparsed".to_string();
    };
    let error = value.get("error");
    let kind = error
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if kind.is_empty() && message.is_empty() {
        "none".to_string()
    } else {
        format!("type={kind} message={}", log_preview(message, 512))
    }
}

/// 保留 BTreeMap 引入，适用于 serde 生成的 schema 中稳定字段顺序。
#[allow(dead_code)]
fn _schema_object(properties: BTreeMap<String, JsonSchema>) -> JsonSchema {
    JsonSchema::object(properties, None, Some(false.into()))
}

/// 保留 schema 类型引用，适用于避免后续裁剪误删 Claude schema 兼容依赖。
#[allow(dead_code)]
fn _schema_type(schema: JsonSchemaPrimitiveType) -> JsonSchemaType {
    JsonSchemaType::Single(schema)
}
