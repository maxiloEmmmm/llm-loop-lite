use std::collections::BTreeMap;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

const MAX_SLEEP_DURATION_MS: u64 = 3_600_000;

/// sleep 参数。
#[derive(Debug, Clone, Deserialize)]
struct SleepArgs {
    /// 暂停毫秒数。
    duration_ms: u64,
}

/// 暂停执行工具。
pub struct SleepHandler;

#[async_trait]
impl ToolHandler for SleepHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "sleep"
    }

    /// 返回 Codex 风格 sleep spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([(
            "duration_ms".to_string(),
            JsonSchema::number(Some(format!(
                "How long to sleep in milliseconds. Must be between 1 and {MAX_SLEEP_DURATION_MS}."
            ))),
        )]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description:
                "Pause execution for a specified duration. Returns the elapsed wall-clock time."
                    .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["duration_ms".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 执行真实 sleep。
    async fn execute(&self, call: ToolCall, _context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "sleep requires function arguments".to_string(),
            ));
        };
        let args: SleepArgs = serde_json::from_str(arguments)?;
        if !(1..=MAX_SLEEP_DURATION_MS).contains(&args.duration_ms) {
            return Err(AppError::Tool(format!(
                "duration_ms must be between 1 and {MAX_SLEEP_DURATION_MS}"
            )));
        }
        let started = Instant::now();
        tokio::time::sleep(std::time::Duration::from_millis(args.duration_ms)).await;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: serde_json::Value::String(format!(
                "Wall time: {:.4} seconds\nSleep completed.",
                started.elapsed().as_secs_f64()
            )),
        })
    }
}
