use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::message::{UserInputQuestion, UserInputRequest};
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

const MIN_AUTO_RESOLUTION_MS: u64 = 60_000;
const MAX_AUTO_RESOLUTION_MS: u64 = 240_000;

/// request_user_input 参数。
#[derive(Debug, Clone, Deserialize)]
struct RequestUserInputArgs {
    /// 问题列表。
    questions: Vec<UserInputQuestion>,
    /// 自动解析超时毫秒数。
    #[serde(rename = "autoResolutionMs")]
    auto_resolution_ms: Option<u64>,
}

/// 请求用户输入工具。
pub struct RequestUserInputHandler;

#[async_trait]
impl ToolHandler for RequestUserInputHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "request_user_input"
    }

    /// 返回 Codex 风格 request_user_input spec。
    fn spec(&self) -> ToolSpec {
        let option_props = BTreeMap::from([
            (
                "label".to_string(),
                JsonSchema::string(Some("User-facing label (1-5 words).".to_string())),
            ),
            (
                "description".to_string(),
                JsonSchema::string(Some(
                    "One short sentence explaining impact/tradeoff if selected.".to_string(),
                )),
            ),
        ]);
        let options_schema = JsonSchema::array(
            JsonSchema::object(
                option_props,
                Some(vec!["label".to_string(), "description".to_string()]),
                Some(false.into()),
            ),
            Some("Provide 2-3 mutually exclusive choices. Put the recommended option first and suffix its label with \"(Recommended)\". Do not include an \"Other\" option in this list; the client will add a free-form \"Other\" option automatically.".to_string()),
        );
        let question_props = BTreeMap::from([
            (
                "id".to_string(),
                JsonSchema::string(Some(
                    "Stable identifier for mapping answers (snake_case).".to_string(),
                )),
            ),
            (
                "header".to_string(),
                JsonSchema::string(Some(
                    "Short header label shown in the UI (12 or fewer chars).".to_string(),
                )),
            ),
            (
                "question".to_string(),
                JsonSchema::string(Some(
                    "Single-sentence prompt shown to the user.".to_string(),
                )),
            ),
            ("options".to_string(), options_schema),
        ]);
        let questions_schema = JsonSchema::array(
            JsonSchema::object(
                question_props,
                Some(vec![
                    "id".to_string(),
                    "header".to_string(),
                    "question".to_string(),
                    "options".to_string(),
                ]),
                Some(false.into()),
            ),
            Some("Questions to show the user. Prefer exactly 1; use multiple questions only when one choice cannot unblock the task.".to_string()),
        );
        let properties = BTreeMap::from([
            ("questions".to_string(), questions_schema),
            (
                "autoResolutionMs".to_string(),
                JsonSchema::number(Some(format!(
                    "Optional auto-resolution window in milliseconds, from {MIN_AUTO_RESOLUTION_MS} to {MAX_AUTO_RESOLUTION_MS}."
                ))),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: format!(
                "Request user input for one to three short questions and wait for the response. Prefer exactly one high-value question. If multiple questions are sent, the channel may continue after the first answer and choose recommended defaults for the remaining questions. Set autoResolutionMs, from {MIN_AUTO_RESOLUTION_MS} to {MAX_AUTO_RESOLUTION_MS} milliseconds, only when the question is useful but non-blocking and continuing with best judgment is acceptable if the user does not answer; omit it when explicit user input is required."
            ),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["questions".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 通过 channel 回调请求用户输入。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "request_user_input requires function arguments".to_string(),
            ));
        };
        let args: RequestUserInputArgs = serde_json::from_str(arguments)?;
        if args.questions.is_empty() || args.questions.len() > 3 {
            return Err(AppError::Tool(
                "request_user_input requires one to three questions".to_string(),
            ));
        }
        if let Some(value) = args.auto_resolution_ms
            && !(MIN_AUTO_RESOLUTION_MS..=MAX_AUTO_RESOLUTION_MS).contains(&value)
        {
            return Err(AppError::Tool(format!(
                "autoResolutionMs must be between {MIN_AUTO_RESOLUTION_MS} and {MAX_AUTO_RESOLUTION_MS}"
            )));
        }
        let Some(requester) = context.user_input.as_ref() else {
            return Err(AppError::Tool(
                "request_user_input channel callback is not connected".to_string(),
            ));
        };
        let response = requester
            .request_user_input(
                &context.source,
                UserInputRequest {
                    questions: args.questions,
                    auto_resolution_ms: args.auto_resolution_ms,
                },
            )
            .await?;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: json!(response),
        })
    }
}
