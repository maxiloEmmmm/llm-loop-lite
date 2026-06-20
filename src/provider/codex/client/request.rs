use reqwest::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;

use crate::config::ProviderConfig;
use crate::error::{AppError, AppResult};
use crate::message::InboundAttachment;
use crate::provider::codex::auth::{ProviderRoute, ProviderRouteKind};
use crate::provider::codex::telemetry::{
    CodexRequestMetadata, X_CODEX_INSTALLATION_ID_HEADER, X_CODEX_TURN_METADATA_HEADER,
    X_CODEX_WINDOW_ID_HEADER,
};
use crate::session::SessionState;
use crate::session_store::ConversationItem;
use crate::tools::spec::{ToolSpec, create_tools_json_for_responses_api};

/// Codex 压缩摘要提示词，适用于独立 compact 请求。
const COMPACTION_PROMPT: &str = "Summarize the previous conversation into a compact handoff. Keep only durable facts, decisions, user preferences, file paths, commands/results that matter, current blockers, and unresolved asks. Treat old tasks as historical reference only. Do not answer those tasks. Return only the summary.";

/// 构造 Codex Responses 请求体。
pub fn build_request_body(
    config: &ProviderConfig,
    session: &SessionState,
    history: &[ConversationItem],
    user_input: &str,
    attachments: &[InboundAttachment],
    metadata: &CodexRequestMetadata,
    tool_specs: &[ToolSpec],
    extra_input: &[Value],
) -> AppResult<Value> {
    let model = config
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Provider("provider.model is required".to_string()))?;
    let mut input = Vec::new();
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
                input.push(serde_json::json!({"role": "user", "content": text}));
            }
            ConversationItem::Assistant { text } => {
                input.push(serde_json::json!({"role": "assistant", "content": text}));
            }
        }
    }
    input.push(user_message_item(user_input, attachments)?);
    input.extend(extra_input.iter().cloned());
    let tools = create_tools_json_for_responses_api(tool_specs)?;

    let reasoning = reasoning_effort_for_request(config).map(|effort| {
        serde_json::json!({
            "effort": effort,
        })
    });
    let include = if reasoning.is_some() {
        vec!["reasoning.encrypted_content"]
    } else {
        Vec::new()
    };
    let mut body = serde_json::json!({
        "model": model,
        "instructions": &session.instructions,
        "input": input,
        "stream": true,
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "include": include,
        "prompt_cache_key": &session.id,
        "metadata": {
            "llm_loop_session_id": session.id,
            "llm_loop_session_key": session.key,
        },
        "client_metadata": metadata.client_metadata()?,
    });
    if let Some(reasoning) = reasoning {
        body["reasoning"] = reasoning;
    }
    if let Some(service_tier) = service_tier_for_request(config) {
        body["service_tier"] = Value::String(service_tier);
    }
    Ok(body)
}

/// 构造 Codex Responses 压缩请求体，适用于不启用工具的摘要请求。
pub fn build_compact_request_body(
    config: &ProviderConfig,
    session: &SessionState,
    history: &[ConversationItem],
    metadata: &CodexRequestMetadata,
) -> AppResult<Value> {
    let model = config
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Provider("provider.model is required".to_string()))?;
    let mut input = Vec::new();
    for item in history {
        match item {
            ConversationItem::User { text } => {
                input.push(serde_json::json!({"role": "user", "content": text}));
            }
            ConversationItem::Assistant { text } => {
                input.push(serde_json::json!({"role": "assistant", "content": text}));
            }
        }
    }
    input.push(serde_json::json!({"role": "user", "content": COMPACTION_PROMPT}));

    let reasoning = reasoning_effort_for_request(config).map(|effort| {
        serde_json::json!({
            "effort": effort,
        })
    });
    let include = if reasoning.is_some() {
        vec!["reasoning.encrypted_content"]
    } else {
        Vec::new()
    };
    let mut body = serde_json::json!({
        "model": model,
        "instructions": &session.instructions,
        "input": input,
        "stream": true,
        "store": false,
        "include": include,
        "prompt_cache_key": &session.id,
        "metadata": {
            "llm_loop_session_id": session.id,
            "llm_loop_session_key": session.key,
            "llm_loop_request_kind": "compaction",
        },
        "client_metadata": metadata.client_metadata()?,
    });
    if let Some(reasoning) = reasoning {
        body["reasoning"] = reasoning;
    }
    Ok(body)
}

/// 提取 Responses reasoning.effort，适用于用户显式配置思考等级的场景。
fn reasoning_effort_for_request(config: &ProviderConfig) -> Option<String> {
    config
        .model_reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// 转换 Codex service_tier 配置，适用于保持 `fast` 配置写法兼容。
fn service_tier_for_request(config: &ProviderConfig) -> Option<String> {
    let tier = config.service_tier.as_deref()?.trim();
    match tier {
        "" | "default" => None,
        "fast" => Some("priority".to_string()),
        value => Some(value.to_string()),
    }
}

/// 构造用户消息 item，适用于把文本和图片按 Responses content 数组发送。
fn user_message_item(user_input: &str, attachments: &[InboundAttachment]) -> AppResult<Value> {
    if attachments.is_empty() {
        return Ok(serde_json::json!({"role": "user", "content": user_input}));
    }
    let mut content = Vec::new();
    if !user_input.trim().is_empty() {
        content.push(serde_json::json!({
            "type": "input_text",
            "text": user_input,
        }));
    }
    for attachment in attachments {
        match attachment {
            InboundAttachment::Image { mime_type, bytes } => {
                content.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": image_data_url(mime_type, bytes),
                }));
            }
            InboundAttachment::StoredFile {
                path,
                filename,
                mime_type,
                size,
            } => {
                content.push(serde_json::json!({
                    "type": "input_text",
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
    Ok(serde_json::json!({
        "role": "user",
        "content": content,
    }))
}

/// 把图片 bytes 编码为 Responses API 支持的 data URL。
fn image_data_url(mime_type: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    let mime_type = if mime_type.trim().is_empty() {
        "image/png"
    } else {
        mime_type.trim()
    };
    format!("data:{mime_type};base64,{}", STANDARD.encode(bytes))
}

/// 发送 Codex 请求并返回 SSE 文本。
pub async fn send_codex_request(
    client: &Client,
    route: &ProviderRoute,
    session: &SessionState,
    metadata: &CodexRequestMetadata,
    body: Value,
) -> AppResult<String> {
    let url = resolve_url(route);
    crate::log_info!(
        "codex http request start url={} session_id={}",
        url,
        session.id
    );
    let response = client
        .post(url)
        .headers(build_headers(route, session, metadata)?)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    crate::log_info!(
        "codex http response status={} session_id={} bytes={}",
        status,
        session.id,
        text.len()
    );
    if !status.is_success() {
        crate::log_info!(
            "codex http non_success session_id={} status={} body_preview={} parsed_error={}",
            session.id,
            status,
            log_preview(&text, 2048),
            parse_error_summary(&text)
        );
        return Err(AppError::Provider(format!(
            "codex request failed with status {status}: {text}"
        )));
    }
    Ok(text)
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
pub(crate) fn parse_error_summary(body: &str) -> String {
    let Some(line) = body
        .lines()
        .find(|line| line.trim_start().starts_with("data: "))
        .and_then(|line| line.trim_start().strip_prefix("data: "))
        .or_else(|| Some(body.trim()))
    else {
        return "none".to_string();
    };
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return "unparsed".to_string();
    };
    let direct = value.get("error");
    let nested = value
        .get("response")
        .and_then(|response| response.get("error"));
    let error = direct.or(nested);
    let code = error
        .and_then(|error| error.get("code").or_else(|| error.get("type")))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if code.is_empty() && message.is_empty() {
        "none".to_string()
    } else {
        format!("code={code} message={}", log_preview(message, 512))
    }
}

/// 解析 provider 对应的请求 URL。
pub(super) fn resolve_url(route: &ProviderRoute) -> String {
    match route.kind {
        ProviderRouteKind::CodexOauth => {
            format!("{}/responses", route.base_url.trim_end_matches('/'))
        }
        ProviderRouteKind::Custom => {
            let base = route.base_url.trim_end_matches('/');
            if base.ends_with("/responses") {
                base.to_string()
            } else {
                format!("{base}/responses")
            }
        }
    }
}

/// 构造请求头，Codex OAuth 路径补 ChatGPT account header。
pub(super) fn build_headers(
    route: &ProviderRoute,
    session: &SessionState,
    metadata: &CodexRequestMetadata,
) -> AppResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static("llm-loop/0.1"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", route.bearer_token))
            .map_err(|err| AppError::Provider(format!("invalid bearer token header: {err}")))?,
    );
    headers.insert(
        "X-LLM-Loop-Session-Id",
        HeaderValue::from_str(&session.id)
            .map_err(|err| AppError::Provider(format!("invalid session header: {err}")))?,
    );
    headers.insert(
        "session-id",
        HeaderValue::from_str(&metadata.session_id)
            .map_err(|err| AppError::Provider(format!("invalid session-id header: {err}")))?,
    );
    headers.insert(
        "thread-id",
        HeaderValue::from_str(&metadata.thread_id)
            .map_err(|err| AppError::Provider(format!("invalid thread-id header: {err}")))?,
    );
    headers.insert(
        X_CODEX_INSTALLATION_ID_HEADER,
        HeaderValue::from_str(&metadata.installation_id).map_err(|err| {
            AppError::Provider(format!("invalid Codex installation id header: {err}"))
        })?,
    );
    headers.insert(
        X_CODEX_WINDOW_ID_HEADER,
        HeaderValue::from_str(&metadata.window_id)
            .map_err(|err| AppError::Provider(format!("invalid Codex window id header: {err}")))?,
    );
    headers.insert(
        X_CODEX_TURN_METADATA_HEADER,
        HeaderValue::from_str(&metadata.turn_metadata_json()?).map_err(|err| {
            AppError::Provider(format!("invalid Codex turn metadata header: {err}"))
        })?,
    );
    if matches!(route.kind, ProviderRouteKind::CodexOauth) {
        if let Some(account_id) = &route.account_id {
            headers.insert(
                "ChatGPT-Account-ID",
                HeaderValue::from_str(account_id).map_err(|err| {
                    AppError::Provider(format!("invalid account id header: {err}"))
                })?,
            );
        }
    }
    Ok(headers)
}
