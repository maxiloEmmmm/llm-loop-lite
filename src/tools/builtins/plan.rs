use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{AppError, AppResult};
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

/// 计划条目。
#[derive(Debug, Clone, Deserialize)]
struct PlanItem {
    /// 任务文本。
    step: String,
    /// 当前状态。
    status: String,
}

/// update_plan 参数。
#[derive(Debug, Clone, Deserialize)]
struct UpdatePlanArgs {
    /// 可选说明。
    explanation: Option<String>,
    /// 计划列表。
    plan: Vec<PlanItem>,
}

/// 更新内存计划的工具。
pub struct UpdatePlanHandler;

#[async_trait]
impl ToolHandler for UpdatePlanHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "update_plan"
    }

    /// 返回 Codex 风格 update_plan spec。
    fn spec(&self) -> ToolSpec {
        let plan_item_properties = BTreeMap::from([
            (
                "step".to_string(),
                JsonSchema::string(Some("Task step text.".to_string())),
            ),
            (
                "status".to_string(),
                JsonSchema::string_enum(
                    vec![json!("pending"), json!("in_progress"), json!("completed")],
                    Some("Step status.".to_string()),
                ),
            ),
        ]);
        let properties = BTreeMap::from([
            (
                "explanation".to_string(),
                JsonSchema::string(Some(
                    "Optional explanation for this plan update.".to_string(),
                )),
            ),
            (
                "plan".to_string(),
                JsonSchema::array(
                    JsonSchema::object(
                        plan_item_properties,
                        Some(vec!["step".to_string(), "status".to_string()]),
                        Some(false.into()),
                    ),
                    Some("The list of steps".to_string()),
                ),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Updates the task plan.\nProvide an optional explanation and a list of plan items, each with a step and status.\nAt most one step can be in_progress at a time.\n".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, Some(vec!["plan".to_string()]), Some(false.into())),
            output_schema: None,
        })
    }

    /// 校验并接受计划更新。
    async fn execute(&self, call: ToolCall, _context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "update_plan requires function arguments".to_string(),
            ));
        };
        let args: UpdatePlanArgs = serde_json::from_str(arguments)?;
        let in_progress = args
            .plan
            .iter()
            .filter(|item| item.status == "in_progress")
            .count();
        if in_progress > 1 {
            return Err(AppError::Tool(
                "update_plan allows at most one in_progress step".to_string(),
            ));
        }
        let invalid = args.plan.iter().find(|item| {
            !matches!(
                item.status.as_str(),
                "pending" | "in_progress" | "completed"
            )
        });
        if let Some(item) = invalid {
            return Err(AppError::Tool(format!(
                "invalid plan status `{}` for step `{}`",
                item.status, item.step
            )));
        }
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: Value::String(
                args.explanation
                    .unwrap_or_else(|| "Plan updated.".to_string()),
            ),
        })
    }
}
