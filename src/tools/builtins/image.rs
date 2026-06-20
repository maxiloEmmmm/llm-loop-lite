use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

/// view_image 参数。
#[derive(Debug, Clone, Deserialize)]
struct ViewImageArgs {
    /// 本地图片路径。
    path: String,
    /// 图片细节级别。
    detail: Option<String>,
}

/// 本地图片查看工具。
pub struct ViewImageHandler;

#[async_trait]
impl ToolHandler for ViewImageHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "view_image"
    }

    /// 返回 Codex 风格 view_image spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "path".to_string(),
                JsonSchema::string(Some("Local filesystem path to an image file.".to_string())),
            ),
            (
                "detail".to_string(),
                JsonSchema::string_enum(
                    vec![json!("high"), json!("original")],
                    Some("Image detail level. Defaults to `high`; use `original` to preserve exact resolution.".to_string()),
                ),
            ),
        ]);
        let output_schema = json!({
            "type": "object",
            "properties": {
                "image_url": {
                    "type": "string",
                    "description": "Data URL for the loaded image."
                },
                "detail": {
                    "type": "string",
                    "enum": ["high", "original"],
                    "description": "Image detail hint returned by view_image."
                }
            },
            "required": ["image_url", "detail"],
            "additionalProperties": false
        });
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "View a local image file from the filesystem when visual inspection is needed. Use this for images already available on disk.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, Some(vec!["path".to_string()]), Some(false.into())),
            output_schema: Some(output_schema),
        })
    }

    /// 读取图片并返回 data URL。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "view_image requires function arguments".to_string(),
            ));
        };
        let args: ViewImageArgs = serde_json::from_str(arguments)?;
        let detail = match args.detail.as_deref() {
            None | Some("high") => "high",
            Some("original") => "original",
            Some(value) => {
                return Err(AppError::Tool(format!(
                    "view_image.detail only supports `high` or `original`, got `{value}`"
                )));
            }
        };
        let path = resolve_path(&context.cwd, &args.path);
        let metadata = tokio::fs::metadata(&path).await?;
        if !metadata.is_file() {
            return Err(AppError::Tool(format!(
                "image path `{}` is not a file",
                path.display()
            )));
        }
        let bytes = tokio::fs::read(&path).await?;
        let mime = mime_guess::from_path(&path)
            .first_raw()
            .unwrap_or("application/octet-stream");
        let image_url = format!("data:{mime};base64,{}", STANDARD.encode(bytes));
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: json!({
                "image_url": image_url,
                "detail": detail,
            }),
        })
    }
}

/// 解析工具传入路径，绝对路径原样使用，相对路径按 cwd 解析。
fn resolve_path(cwd: &std::path::Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
