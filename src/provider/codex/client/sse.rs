use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::tools::registry::{ToolCall, ToolInput};

/// Codex SSE 提取后的回复。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedResponse {
    /// assistant 文本。
    pub text: String,
    /// Responses API response id。
    pub response_id: Option<String>,
    /// 完成的原始 output items。
    pub output_items: Vec<Value>,
    /// 待执行工具调用。
    pub tool_calls: Vec<ToolCall>,
    /// 本轮 token 用量。
    pub total_tokens: Option<u64>,
}

/// 从 Responses SSE 文本中提取最终 assistant 文本。
pub fn extract_response_from_sse(raw: &str) -> AppResult<ExtractedResponse> {
    let mut text = String::new();
    let mut response_id = None;
    let mut output_items = Vec::new();
    let mut tool_calls = Vec::new();
    let mut total_tokens = None;

    for event in raw.split("\n\n") {
        let data = event
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap_or("");
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let value: Value = serde_json::from_str(data)?;
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        if let Some(id) = value
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(Value::as_str)
        {
            response_id = Some(id.to_string());
        }
        if kind == "response.completed" {
            total_tokens = value
                .get("response")
                .and_then(|response| response.get("usage"))
                .and_then(|usage| usage.get("total_tokens"))
                .and_then(Value::as_u64);
        }
        if kind == "response.output_item.done"
            && let Some(item) = value.get("item")
        {
            output_items.push(item.clone());
            if let Some(call) = extract_tool_call(item)? {
                tool_calls.push(call);
            }
        }
        if let Some(delta) = extract_delta(kind, &value) {
            text.push_str(delta);
        }
    }

    if text.is_empty() && tool_calls.is_empty() {
        return Err(AppError::Provider(
            "codex SSE did not contain assistant text".to_string(),
        ));
    }
    Ok(ExtractedResponse {
        text,
        response_id,
        output_items,
        tool_calls,
        total_tokens,
    })
}

/// 从单个 SSE JSON 事件中提取文本增量。
fn extract_delta<'a>(kind: &str, value: &'a Value) -> Option<&'a str> {
    match kind {
        "response.output_text.delta" | "response.refusal.delta" => {
            value.get("delta").and_then(Value::as_str)
        }
        // 触发条件：部分 custom provider 会把文本放在简化 delta/text 字段。
        // 常规 Responses output_item.done 不能走这里，因为它是完整文本快照。
        // 这样避免 delta 已拼接后又把完整 assistant 文本追加一次。
        "" => value
            .get("delta")
            .and_then(Value::as_str)
            .or_else(|| value.get("text").and_then(Value::as_str)),
        _ => None,
    }
}

/// 从 output item 中提取工具调用。
fn extract_tool_call(item: &Value) -> AppResult<Option<ToolCall>> {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::Provider("function_call missing name".to_string()))?;
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::Provider("function_call missing call_id".to_string()))?;
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string();
            Ok(Some(ToolCall {
                call_id: call_id.to_string(),
                name: name.to_string(),
                input: ToolInput::Function { arguments },
            }))
        }
        Some("custom_tool_call") => {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::Provider("custom_tool_call missing name".to_string()))?;
            let call_id = item.get("call_id").and_then(Value::as_str).ok_or_else(|| {
                AppError::Provider("custom_tool_call missing call_id".to_string())
            })?;
            let input = item
                .get("input")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(Some(ToolCall {
                call_id: call_id.to_string(),
                name: name.to_string(),
                input: ToolInput::Custom { input },
            }))
        }
        _ => Ok(None),
    }
}
