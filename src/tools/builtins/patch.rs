use async_trait::async_trait;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{FreeformTool, FreeformToolFormat, ToolSpec};

const APPLY_PATCH_GRAMMAR: &str = r#"start: begin_patch hunk+ end_patch
begin_patch: "*** Begin Patch" LF
end_patch: "*** End Patch" LF?

hunk: add_hunk | delete_hunk | update_hunk
add_hunk: "*** Add File: " filename LF add_line+
delete_hunk: "*** Delete File: " filename LF
update_hunk: "*** Update File: " filename LF change_move? change?

filename: /(.+)/
add_line: "+" /(.*)/ LF -> line

change_move: "*** Move to: " filename LF
change: (change_context | change_line)+ eof_line?
change_context: ("@@" | "@@ " /(.+)/) LF
change_line: ("+" | "-" | " ") /(.*)/ LF
eof_line: "*** End of File" LF

%import common.LF
"#;

/// apply_patch 工具。
pub struct ApplyPatchHandler;

/// 单个 patch 操作。
#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchOp {
    /// 新增文件。
    Add { path: String, lines: Vec<String> },
    /// 删除文件。
    Delete { path: String },
    /// 更新文件。
    Update {
        path: String,
        move_to: Option<String>,
        chunks: Vec<PatchChunk>,
    },
}

/// update patch 的单个 chunk。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchChunk {
    /// 原始行。
    old_lines: Vec<String>,
    /// 新行。
    new_lines: Vec<String>,
}

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    /// 返回 Codex 风格 apply_patch freeform spec。
    fn spec(&self) -> ToolSpec {
        ToolSpec::Freeform(FreeformTool {
            name: self.name().to_string(),
            description: "Use the `apply_patch` tool to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.".to_string(),
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: APPLY_PATCH_GRAMMAR.to_string(),
            },
        })
    }

    /// 应用 patch 到本地文件系统。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Custom { input } = &call.input else {
            return Err(AppError::Tool(
                "apply_patch requires custom input".to_string(),
            ));
        };
        let ops = parse_patch(input)?;
        let mut changed = Vec::new();
        for op in ops {
            match op {
                PatchOp::Add { path, lines } => {
                    let path = context.cwd.join(path);
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&path, lines.join("\n")).await?;
                    changed.push(format!("added {}", path.display()));
                }
                PatchOp::Delete { path } => {
                    let path = context.cwd.join(path);
                    tokio::fs::remove_file(&path).await?;
                    changed.push(format!("deleted {}", path.display()));
                }
                PatchOp::Update {
                    path,
                    move_to,
                    chunks,
                } => {
                    let source = context.cwd.join(&path);
                    let original = tokio::fs::read_to_string(&source).await?;
                    let updated = apply_chunks(&original, &chunks)?;
                    let target = move_to
                        .as_ref()
                        .map(|value| context.cwd.join(value))
                        .unwrap_or_else(|| source.clone());
                    if let Some(parent) = target.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(&target, updated).await?;
                    if target != source {
                        tokio::fs::remove_file(&source).await?;
                    }
                    changed.push(format!("updated {}", target.display()));
                }
            }
        }
        Ok(ToolResult {
            output_kind: ToolOutputKind::Custom,
            call_id: call.call_id,
            output: Value::String(changed.join("\n")),
        })
    }
}

/// 解析 apply_patch 文本。
fn parse_patch(input: &str) -> AppResult<Vec<PatchOp>> {
    let lines: Vec<&str> = input.lines().collect();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Err(AppError::Tool(
            "apply_patch must start with *** Begin Patch and end with *** End Patch".to_string(),
        ));
    }
    let mut index = 1;
    let mut ops = Vec::new();
    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut add_lines = Vec::new();
            while index < lines.len() && !lines[index].starts_with("*** ") {
                let Some(value) = lines[index].strip_prefix('+') else {
                    return Err(AppError::Tool(
                        "add file lines must start with +".to_string(),
                    ));
                };
                add_lines.push(value.to_string());
                index += 1;
            }
            ops.push(PatchOp::Add {
                path: path.to_string(),
                lines: add_lines,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(PatchOp::Delete {
                path: path.to_string(),
            });
            index += 1;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_to = None;
            if index < lines.len()
                && let Some(target) = lines[index].strip_prefix("*** Move to: ")
            {
                move_to = Some(target.to_string());
                index += 1;
            }
            let mut chunks = Vec::new();
            while index < lines.len() && !lines[index].starts_with("*** ") {
                if lines[index].starts_with("@@") {
                    index += 1;
                }
                let mut old_lines = Vec::new();
                let mut new_lines = Vec::new();
                while index < lines.len()
                    && !lines[index].starts_with("@@")
                    && !lines[index].starts_with("*** ")
                {
                    let current = lines[index];
                    if current == "*** End of File" {
                        index += 1;
                        continue;
                    }
                    let (marker, body) = current.split_at(1);
                    match marker {
                        " " => {
                            old_lines.push(body.to_string());
                            new_lines.push(body.to_string());
                        }
                        "-" => old_lines.push(body.to_string()),
                        "+" => new_lines.push(body.to_string()),
                        _ => {
                            return Err(AppError::Tool(format!(
                                "invalid update line marker `{marker}`"
                            )));
                        }
                    }
                    index += 1;
                }
                if !old_lines.is_empty() || !new_lines.is_empty() {
                    chunks.push(PatchChunk {
                        old_lines,
                        new_lines,
                    });
                }
            }
            ops.push(PatchOp::Update {
                path: path.to_string(),
                move_to,
                chunks,
            });
        } else {
            return Err(AppError::Tool(format!("unknown patch hunk: {line}")));
        }
    }
    Ok(ops)
}

/// 应用 update chunks。
fn apply_chunks(original: &str, chunks: &[PatchChunk]) -> AppResult<String> {
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let had_trailing_newline = original.ends_with('\n');
    let mut cursor = 0;
    for chunk in chunks {
        let Some(position) = find_subsequence(&lines, &chunk.old_lines, cursor) else {
            return Err(AppError::Tool(
                "apply_patch update context did not match file".to_string(),
            ));
        };
        lines.splice(
            position..position + chunk.old_lines.len(),
            chunk.new_lines.clone(),
        );
        cursor = position + chunk.new_lines.len();
    }
    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }
    Ok(output)
}

/// 查找行子序列。
fn find_subsequence(haystack: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start.min(haystack.len()));
    }
    (start..=haystack.len().saturating_sub(needle.len()))
        .find(|&index| haystack[index..index + needle.len()] == *needle)
}
