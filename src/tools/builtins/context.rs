use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::AppResult;
use crate::tools::registry::{ToolCall, ToolContext, ToolHandler, ToolOutputKind, ToolResult};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

/// 新上下文工具。
pub struct NewContextHandler;

/// 查询剩余上下文工具。
pub struct GetContextRemainingHandler;

#[async_trait]
impl ToolHandler for NewContextHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "new_context"
    }

    /// 返回 Codex 风格 new_context spec。
    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Start a new context window.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(BTreeMap::new(), None, Some(false.into())),
            output_schema: None,
        })
    }

    /// 返回新上下文请求结果，由 provider loop 解释为 reset。
    async fn execute(&self, call: ToolCall, _context: ToolContext) -> AppResult<ToolResult> {
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: Value::String("New context requested.".to_string()),
        })
    }
}

#[async_trait]
impl ToolHandler for GetContextRemainingHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "get_context_remaining"
    }

    /// 返回 Codex 风格 get_context_remaining spec。
    fn spec(&self) -> ToolSpec {
        let output_schema = json!({
            "type": "object",
            "properties": {
                "tokens_left": {
                    "anyOf": [
                        { "type": "integer" },
                        { "type": "null" }
                    ],
                    "description": "Remaining tokens in the current context window, or null when unavailable."
                }
            },
            "required": ["tokens_left"],
            "additionalProperties": false
        });
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Get the remaining tokens in the current context window.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(BTreeMap::new(), None, Some(false.into())),
            output_schema: Some(output_schema),
        })
    }

    /// 使用 session 中累计 token 估算剩余上下文。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let max_context_tokens = context.session.max_context_tokens;
        let used_tokens = context.session.used_tokens;
        let tokens_left = max_context_tokens.map(|max| max.saturating_sub(used_tokens));
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: json!({ "tokens_left": tokens_left }),
        })
    }
}
