use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{AppError, AppResult};
use crate::message::MessageSource;
use crate::scheduler::CronSchedule;
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

const CRON_FILE_NAME: &str = "cron.md";
const TASK_PREFIX: &str = "task-";
const TASK_SUFFIX: &str = ".md";
const FORBIDDEN_AUXILIARY_PATHS: &[&str] = &[
    "~/.llm-loop/cron",
    "$HOME/.llm-loop/cron",
    "/root/.llm-loop/cron",
    "~/.llm-loop/scripts",
    "$HOME/.llm-loop/scripts",
    "/root/.llm-loop/scripts",
];

/// cron 工具文件存储。
#[derive(Debug, Clone)]
pub struct CronStore {
    /// cron 根目录。
    root: PathBuf,
}

impl CronStore {
    /// 创建 cron 存储，适用于工具按当前来源读写任务。
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 返回当前来源对应的目录。
    fn target_dir(&self, source: &MessageSource) -> AppResult<PathBuf> {
        Ok(self.root.join(target_dir_name(source)?))
    }
}

/// cron 工具处理器。
pub struct CronHandler;

#[async_trait]
impl ToolHandler for CronHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "__cron"
    }

    /// 返回 cron 工具 spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "type".to_string(),
                JsonSchema::string_enum(
                    vec![
                        json!("list"),
                        json!("add"),
                        json!("remove"),
                        json!("edit"),
                    ],
                    Some("Operation type.".to_string()),
                ),
            ),
            (
                "key".to_string(),
                JsonSchema::string(Some(
                    "Stable task key. Auxiliary files for this task must be in the same cron directory as task-<key>.md and named with this key, for example task-<key>.py, task-<key>-fetch.py, or task-<key>_fetch.py. The tool rejects prompts that mention ~/.llm-loop/cron or global scripts paths."
                        .to_string(),
                )),
            ),
            (
                "time_step".to_string(),
                JsonSchema::array(
                    JsonSchema::string(Some("One cron field.".to_string())),
                    Some("Five standard crontab fields, required for add and edit.".to_string()),
                ),
            ),
            (
                "prompt".to_string(),
                JsonSchema::string(Some(
                    "Task prompt. The first line is a title without spaces, followed by the task description. If the scheduled task needs extra scripts or data files, create them next to task-<key>.md in the same cron directory. Never use ~/.llm-loop/cron, ~/.llm-loop/scripts, /root/.llm-loop/cron, /root/.llm-loop/scripts, or other global script locations. Name auxiliary files with the task key so remove can clean them."
                        .to_string(),
                )),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description:
                "List, add, edit, or remove local scheduled tasks for the current channel scope. Add/edit store the prompt as task-<key>.md. If extra script files are needed, they must be created in the same directory as task-<key>.md and named task-<key>.<ext>, task-<key>-<name>.<ext>, or task-<key>_<name>.<ext>. Do not use the singular ~/.llm-loop/cron directory or global script directories. Remove deletes the cron entry, task prompt, and those same-directory auxiliary files."
                    .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["type".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 执行 cron 工具调用。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "__cron requires function arguments".to_string(),
            ));
        };
        let args: CronArgs = serde_json::from_str(arguments)?;
        let output = match args.action {
            CronAction::List => list_tasks(&context.shared.crons, &context.source).await?,
            CronAction::Add => {
                add_task(&context.shared.crons, &context.source, args).await?;
                Value::String(String::new())
            }
            CronAction::Remove => {
                remove_task(&context.shared.crons, &context.source, args).await?;
                Value::String(String::new())
            }
            CronAction::Edit => {
                edit_task(&context.shared.crons, &context.source, args).await?;
                Value::String(String::new())
            }
        };
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output,
        })
    }
}

/// cron 工具参数。
#[derive(Debug, Clone, Deserialize)]
struct CronArgs {
    /// 操作类型。
    #[serde(rename = "type")]
    action: CronAction,
    /// 任务 key。
    key: Option<String>,
    /// 标准 5 段 cron 字段。
    time_step: Option<Vec<String>>,
    /// 任务提示词，第一行为 title。
    prompt: Option<String>,
}

/// cron 工具操作类型。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CronAction {
    /// 列出当前来源任务。
    List,
    /// 新增任务。
    Add,
    /// 删除任务。
    Remove,
    /// 编辑任务。
    Edit,
}

/// list 返回的单个任务。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct CronTaskView {
    /// 任务 key。
    key: String,
    /// 5 段 cron 字段。
    time_step: Vec<String>,
    /// 任务标题。
    title: String,
    /// 任务描述。
    task: String,
}

/// 列出当前来源下的 cron 任务。
async fn list_tasks(store: &CronStore, source: &MessageSource) -> AppResult<Value> {
    let dir = store.target_dir(source)?;
    let cron_path = dir.join(CRON_FILE_NAME);
    if !cron_path.exists() {
        crate::log_info!(
            "cron tool list empty channel={} dir={}",
            source.channel_name,
            dir.display()
        );
        return Ok(json!([]));
    }
    let raw = tokio::fs::read_to_string(&cron_path).await?;
    let mut output = Vec::new();
    for line in parse_cron_lines(&raw, &cron_path)? {
        let task_path = dir.join(task_file_name(&line.key));
        let prompt = match tokio::fs::read_to_string(&task_path).await {
            Ok(prompt) => prompt,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                crate::log_info!(
                    "cron list skipped missing task path={}",
                    task_path.display()
                );
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        let parsed = parse_prompt(&prompt)?;
        output.push(CronTaskView {
            key: line.key,
            time_step: line.time_step,
            title: parsed.title,
            task: parsed.task,
        });
    }
    crate::log_info!(
        "cron tool list channel={} dir={} items={}",
        source.channel_name,
        dir.display(),
        output.len()
    );
    Ok(json!(output))
}

/// 新增 cron 任务。
async fn add_task(store: &CronStore, source: &MessageSource, args: CronArgs) -> AppResult<()> {
    let key = required_key(args.key.as_deref())?;
    let time_step = required_time_step(args.time_step)?;
    let prompt = required_prompt(args.prompt)?;
    validate_prompt(&prompt, &key)?;
    let dir = store.target_dir(source)?;
    tokio::fs::create_dir_all(&dir).await?;
    let task_path = dir.join(task_file_name(&key));
    if task_path.exists() {
        return Err(AppError::Tool(format!(
            "__cron task `{key}` already exists; edit it instead"
        )));
    }
    let mut lines = read_cron_lines(&dir).await?;
    if lines.iter().any(|line| line.key == key) {
        return Err(AppError::Tool(format!(
            "__cron task `{key}` already exists; edit it instead"
        )));
    }
    tokio::fs::write(&task_path, prompt).await?;
    lines.push(CronLine { time_step, key });
    write_cron_lines(&dir, &lines).await?;
    crate::log_info!(
        "cron tool add channel={} dir={} task_path={} items={}",
        source.channel_name,
        dir.display(),
        task_path.display(),
        lines.len()
    );
    Ok(())
}

/// 删除 cron 任务和任务文件。
async fn remove_task(store: &CronStore, source: &MessageSource, args: CronArgs) -> AppResult<()> {
    let key = required_key(args.key.as_deref())?;
    let dir = store.target_dir(source)?;
    let mut lines = read_cron_lines(&dir).await?;
    let before = lines.len();
    lines.retain(|line| line.key != key);
    if before == lines.len() {
        return Err(AppError::Tool(format!(
            "__cron task `{key}` does not exist"
        )));
    }
    let removed_files = remove_task_files(&dir, &key, &lines).await?;
    write_cron_lines(&dir, &lines).await?;
    crate::log_info!(
        "cron tool remove channel={} dir={} key={} removed_files={} items={}",
        source.channel_name,
        dir.display(),
        key,
        removed_files,
        lines.len()
    );
    Ok(())
}

/// 删除任务提示词及同级辅助文件，适用于 remove 时释放该任务资源。
async fn remove_task_files(dir: &Path, key: &str, remaining: &[CronLine]) -> AppResult<usize> {
    let mut removed = 0usize;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !is_task_owned_file(&file_name, key, remaining) {
            continue;
        }
        match tokio::fs::remove_file(entry.path()).await {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(removed)
}

/// 判断文件是否归属于任务 key，适用于只清理同级辅助资源。
fn is_task_owned_file(file_name: &str, key: &str, remaining: &[CronLine]) -> bool {
    if file_name == CRON_FILE_NAME {
        return false;
    }
    if remaining
        .iter()
        .any(|line| file_name == task_file_name(&line.key))
    {
        return false;
    }
    let prefix = format!("{TASK_PREFIX}{key}");
    file_name == task_file_name(key)
        || file_name
            .strip_prefix(&prefix)
            .is_some_and(|suffix| suffix.starts_with(['.', '-', '_']))
}

/// 编辑已有 cron 任务。
async fn edit_task(store: &CronStore, source: &MessageSource, args: CronArgs) -> AppResult<()> {
    let key = required_key(args.key.as_deref())?;
    let time_step = required_time_step(args.time_step)?;
    let prompt = required_prompt(args.prompt)?;
    validate_prompt(&prompt, &key)?;
    let dir = store.target_dir(source)?;
    let mut lines = read_cron_lines(&dir).await?;
    let Some(line) = lines.iter_mut().find(|line| line.key == key) else {
        return Err(AppError::Tool(format!(
            "__cron task `{key}` does not exist"
        )));
    };
    line.time_step = time_step;
    tokio::fs::write(dir.join(task_file_name(&key)), prompt).await?;
    write_cron_lines(&dir, &lines).await?;
    crate::log_info!(
        "cron tool edit channel={} dir={} key={} items={}",
        source.channel_name,
        dir.display(),
        key,
        lines.len()
    );
    Ok(())
}

/// 读取 cron.md 行，缺失时返回空列表。
async fn read_cron_lines(dir: &Path) -> AppResult<Vec<CronLine>> {
    let path = dir.join(CRON_FILE_NAME);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = tokio::fs::read_to_string(&path).await?;
    parse_cron_lines(&raw, &path)
}

/// 写回 cron.md。
async fn write_cron_lines(dir: &Path, lines: &[CronLine]) -> AppResult<()> {
    tokio::fs::create_dir_all(dir).await?;
    let output = lines
        .iter()
        .map(CronLine::render)
        .collect::<Vec<_>>()
        .join("\n");
    let content = if output.is_empty() {
        String::new()
    } else {
        format!("{output}\n")
    };
    tokio::fs::write(dir.join(CRON_FILE_NAME), content).await?;
    Ok(())
}

/// 解析 cron 配置。
fn parse_cron_lines(raw: &str, _path: &Path) -> AppResult<Vec<CronLine>> {
    let mut output = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 6 {
            return Err(AppError::Tool(format!(
                "cron 配置第 {} 行必须是 5 个时间字段加 1 个任务 key",
                index + 1
            )));
        }
        let key = key_from_task_file(parts[5])?;
        output.push(CronLine {
            time_step: parts[0..5].iter().map(|value| value.to_string()).collect(),
            key,
        });
    }
    Ok(output)
}

/// 单行 cron 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CronLine {
    /// 5 段 cron 字段。
    time_step: Vec<String>,
    /// 任务 key。
    key: String,
}

impl CronLine {
    /// 渲染为 cron.md 一行。
    fn render(&self) -> String {
        format!("{} {}", self.time_step.join(" "), task_file_name(&self.key))
    }
}

/// prompt 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedPrompt {
    /// 第一行标题。
    title: String,
    /// 后续任务描述。
    task: String,
}

/// 解析 prompt 第一行 title 和任务描述。
fn parse_prompt(prompt: &str) -> AppResult<ParsedPrompt> {
    let mut lines = prompt.lines();
    let title = lines
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Tool("__cron prompt title is required".to_string()))?;
    if title.split_whitespace().count() != 1 {
        return Err(AppError::Tool(
            "__cron prompt title must not contain spaces".to_string(),
        ));
    }
    Ok(ParsedPrompt {
        title: title.to_string(),
        task: lines.collect::<Vec<_>>().join("\n").trim().to_string(),
    })
}

/// 校验 prompt 格式和辅助文件路径，适用于 add/edit 写入前拦截错误目录。
fn validate_prompt(prompt: &str, key: &str) -> AppResult<()> {
    parse_prompt(prompt)?;
    validate_prompt_auxiliary_paths(prompt, key)
}

/// 校验辅助文件路径，避免任务把脚本写到全局旧目录。
fn validate_prompt_auxiliary_paths(prompt: &str, key: &str) -> AppResult<()> {
    let normalized = prompt.replace('\\', "/");
    for path in FORBIDDEN_AUXILIARY_PATHS {
        if contains_forbidden_path(&normalized, path) {
            return Err(AppError::Tool(format!(
                "__cron auxiliary files for `{key}` must be next to task-{key}.md; forbidden path `{path}`"
            )));
        }
    }
    Ok(())
}

/// 判断是否包含被禁止路径，避免把合法 crons 目录误判为 cron。
fn contains_forbidden_path(value: &str, path: &str) -> bool {
    let mut offset = 0usize;
    while let Some(index) = value[offset..].find(path) {
        let end = offset + index + path.len();
        let next = value[end..].chars().next();
        if next.is_none_or(|ch| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_')) {
            return true;
        }
        offset = end;
    }
    false
}

/// 读取必填 key 并做文件名安全校验。
fn required_key(key: Option<&str>) -> AppResult<String> {
    let Some(key) = key.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(AppError::Tool("__cron key is required".to_string()));
    };
    if !key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(AppError::Tool(
            "__cron key only allows ASCII letters, numbers, '-' and '_'".to_string(),
        ));
    }
    Ok(key.to_string())
}

/// 读取并校验必填 cron 字段。
fn required_time_step(time_step: Option<Vec<String>>) -> AppResult<Vec<String>> {
    let Some(time_step) = time_step else {
        return Err(AppError::Tool("__cron time_step is required".to_string()));
    };
    if time_step.len() != 5 {
        return Err(AppError::Tool(
            "__cron time_step must contain exactly 5 fields".to_string(),
        ));
    }
    if time_step.iter().any(|field| field.trim().is_empty()) {
        return Err(AppError::Tool(
            "__cron time_step fields must not be empty".to_string(),
        ));
    }
    let normalized = time_step
        .into_iter()
        .map(|field| field.trim().to_string())
        .collect::<Vec<_>>();
    let refs = normalized.iter().map(String::as_str).collect::<Vec<_>>();
    CronSchedule::parse(&refs).map_err(|err| AppError::Tool(err.to_string()))?;
    Ok(normalized)
}

/// 读取必填 prompt。
fn required_prompt(prompt: Option<String>) -> AppResult<String> {
    prompt
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Tool("__cron prompt is required".to_string()))
}

/// 从来源生成 cron 目录名。
fn target_dir_name(source: &MessageSource) -> AppResult<String> {
    let channel = source.channel_name.trim();
    if channel.is_empty() {
        return Err(AppError::Tool("__cron channel name is empty".to_string()));
    }
    let key = if source.chat_type == "dm" {
        source
            .user_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::Tool("__cron user key is empty".to_string()))?
    } else {
        source.chat_id.trim()
    };
    if key.is_empty() {
        return Err(AppError::Tool("__cron target key is empty".to_string()));
    }
    Ok(format!(
        "{}_{}",
        sanitize_dir_part(channel),
        sanitize_dir_part(key)
    ))
}

/// 清理目录名片段，避免来源 id 破坏目录结构。
fn sanitize_dir_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// 根据 key 生成任务文件名。
fn task_file_name(key: &str) -> String {
    format!("{TASK_PREFIX}{key}{TASK_SUFFIX}")
}

/// 从任务文件名解析 key。
fn key_from_task_file(filename: &str) -> AppResult<String> {
    let Some(key) = filename
        .strip_prefix(TASK_PREFIX)
        .and_then(|value| value.strip_suffix(TASK_SUFFIX))
    else {
        return Err(AppError::Tool(
            "__cron task reference is invalid".to_string(),
        ));
    };
    required_key(Some(key))
}

#[cfg(test)]
#[path = "cron_test.rs"]
mod cron_test;
