use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};

use crate::error::{AppError, AppResult};
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

/// 长命令 session 表。
#[derive(Default)]
pub struct ExecSessions {
    /// 下一个 session id。
    next_id: u64,
    /// 正在运行的进程。
    processes: HashMap<u64, RunningProcess>,
}

/// 正在运行的进程。
pub struct RunningProcess {
    /// 子进程。
    child: Child,
    /// stdin 句柄。
    stdin: Option<ChildStdin>,
    /// stdout 缓冲。
    stdout: Vec<u8>,
    /// stderr 缓冲。
    stderr: Vec<u8>,
    /// 启动时间。
    started: Instant,
}

/// exec_command 参数。
#[derive(Debug, Clone, Deserialize)]
struct ExecCommandArgs {
    /// 命令文本。
    cmd: String,
    /// 工作目录。
    workdir: Option<String>,
    /// shell 路径。
    shell: Option<String>,
    /// 是否使用 login shell。
    login: Option<bool>,
    /// 输出等待毫秒数。
    yield_time_ms: Option<u64>,
    /// 输出 token 预算。
    max_output_tokens: Option<usize>,
}

/// write_stdin 参数。
#[derive(Debug, Clone, Deserialize)]
struct WriteStdinArgs {
    /// session id。
    session_id: u64,
    /// 写入字符。
    chars: Option<String>,
    /// 输出等待毫秒数。
    yield_time_ms: Option<u64>,
    /// 输出 token 预算。
    max_output_tokens: Option<usize>,
}

/// shell_command 参数。
#[derive(Debug, Clone, Deserialize)]
struct ShellCommandArgs {
    /// shell 脚本。
    command: String,
    /// 工作目录。
    workdir: Option<String>,
    /// 超时毫秒。
    timeout_ms: Option<u64>,
    /// 是否 login shell。
    login: Option<bool>,
}

/// unified exec 工具。
pub struct ExecCommandHandler;

/// 写 stdin 工具。
pub struct WriteStdinHandler;

/// 旧版 shell_command 工具。
pub struct ShellCommandHandler;

#[async_trait]
impl ToolHandler for ExecCommandHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "exec_command"
    }

    /// 返回 Codex 风格 exec_command spec。
    fn spec(&self) -> ToolSpec {
        let mut properties = BTreeMap::from([
            (
                "cmd".to_string(),
                JsonSchema::string(Some("Shell command to execute.".to_string())),
            ),
            (
                "workdir".to_string(),
                JsonSchema::string(Some(
                    "Working directory for the command. Defaults to the turn cwd.".to_string(),
                )),
            ),
            (
                "tty".to_string(),
                JsonSchema::boolean(Some(
                    "True allocates a PTY for the command; false or omitted uses plain pipes."
                        .to_string(),
                )),
            ),
            (
                "yield_time_ms".to_string(),
                JsonSchema::number(Some(
                    "Wait before yielding output. Defaults to 10000 ms; effective range is 250-30000 ms."
                        .to_string(),
                )),
            ),
            (
                "max_output_tokens".to_string(),
                JsonSchema::number(Some(
                    "Output token budget. Defaults to 10000 tokens; larger requests may be capped by policy."
                        .to_string(),
                )),
            ),
            (
                "shell".to_string(),
                JsonSchema::string(Some(
                    "Shell binary to launch. Defaults to the user's default shell.".to_string(),
                )),
            ),
            (
                "login".to_string(),
                JsonSchema::boolean(Some(
                    "True runs the shell with -l/-i semantics; false disables them. Defaults to true."
                        .to_string(),
                )),
            ),
        ]);
        properties.extend(approval_compat_properties());
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description:
                "Runs a command in a PTY, returning output or a session ID for ongoing interaction."
                    .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["cmd".to_string()]),
                Some(false.into()),
            ),
            output_schema: Some(unified_exec_output_schema()),
        })
    }

    /// 执行 shell 命令。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "exec_command requires function arguments".to_string(),
            ));
        };
        let args: ExecCommandArgs = serde_json::from_str(arguments)?;
        let workdir = resolve_workdir(&context.cwd, args.workdir.as_deref());
        let output = run_command(
            args.shell.as_deref(),
            &args.cmd,
            workdir,
            args.login.unwrap_or(true),
            args.yield_time_ms.unwrap_or(10_000),
            args.max_output_tokens.unwrap_or(10_000),
            Some(context),
        )
        .await?;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output,
        })
    }
}

#[async_trait]
impl ToolHandler for WriteStdinHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "write_stdin"
    }

    /// 返回 Codex 风格 write_stdin spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "session_id".to_string(),
                JsonSchema::number(Some(
                    "Identifier of the running unified exec session.".to_string(),
                )),
            ),
            (
                "chars".to_string(),
                JsonSchema::string(Some(
                    "Bytes to write to stdin. Defaults to empty, which polls without writing."
                        .to_string(),
                )),
            ),
            (
                "yield_time_ms".to_string(),
                JsonSchema::number(Some(
                    "Wait before yielding output. Non-empty writes default to 250 ms and cap at 30000 ms; empty polls wait 5000-300000 ms by default.".to_string(),
                )),
            ),
            (
                "max_output_tokens".to_string(),
                JsonSchema::number(Some(
                    "Output token budget. Defaults to 10000 tokens; larger requests may be capped by policy.".to_string(),
                )),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description:
                "Writes characters to an existing unified exec session and returns recent output."
                    .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["session_id".to_string()]),
                Some(false.into()),
            ),
            output_schema: Some(unified_exec_output_schema()),
        })
    }

    /// 写入或轮询长命令 session。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "write_stdin requires function arguments".to_string(),
            ));
        };
        let args: WriteStdinArgs = serde_json::from_str(arguments)?;
        let mut sessions = context.shared.exec_sessions.lock().await;
        let Some(process) = sessions.processes.get_mut(&args.session_id) else {
            return Err(AppError::Tool(format!(
                "unknown exec session {}",
                args.session_id
            )));
        };
        if let Some(chars) = args.chars.as_ref()
            && let Some(stdin) = process.stdin.as_mut()
        {
            stdin.write_all(chars.as_bytes()).await?;
            stdin.flush().await?;
        }
        tokio::time::sleep(Duration::from_millis(args.yield_time_ms.unwrap_or_else(
            || {
                if args.chars.is_some() { 250 } else { 5_000 }
            },
        )))
        .await;
        let output = collect_running_output(
            process,
            Some(args.session_id),
            args.max_output_tokens.unwrap_or(10_000),
        )
        .await?;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output,
        })
    }
}

#[async_trait]
impl ToolHandler for ShellCommandHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "shell_command"
    }

    /// 返回 Codex 风格 shell_command spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "command".to_string(),
                JsonSchema::string(Some(
                    "Shell script to run in the user's default shell.".to_string(),
                )),
            ),
            (
                "workdir".to_string(),
                JsonSchema::string(Some(
                    "Working directory for the command. Defaults to the turn cwd.".to_string(),
                )),
            ),
            (
                "timeout_ms".to_string(),
                JsonSchema::number(Some(
                    "Maximum command runtime. Defaults to 10000 ms.".to_string(),
                )),
            ),
            (
                "login".to_string(),
                JsonSchema::boolean(Some(
                    "True runs with login shell semantics; false disables them. Defaults to true."
                        .to_string(),
                )),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Runs a shell command and returns its output.\n- Always set the `workdir` param when using the shell_command function. Do not use `cd` unless absolutely necessary.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["command".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 执行旧版 shell command。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "shell_command requires function arguments".to_string(),
            ));
        };
        let args: ShellCommandArgs = serde_json::from_str(arguments)?;
        let workdir = resolve_workdir(&context.cwd, args.workdir.as_deref());
        let output = run_command(
            None,
            &args.command,
            workdir,
            args.login.unwrap_or(true),
            args.timeout_ms.unwrap_or(10_000),
            10_000,
            None,
        )
        .await?;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: Value::String(
                output
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
        })
    }
}

/// 运行命令并收集输出。
async fn run_command(
    shell: Option<&str>,
    cmd: &str,
    workdir: PathBuf,
    login: bool,
    wait_ms: u64,
    max_output_tokens: usize,
    context: Option<ToolContext>,
) -> AppResult<Value> {
    let shell = shell
        .map(str::to_string)
        .unwrap_or_else(|| default_shell().to_string());
    let mut command = Command::new(&shell);
    if login {
        command.arg("-lc");
    } else {
        command.arg("-c");
    }
    command
        .arg(cmd)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdin = child.stdin.take();
    let started = Instant::now();
    let status = tokio::time::timeout(Duration::from_millis(wait_ms), child.wait()).await;
    match status {
        Ok(status) => {
            let status = status?;
            let mut output = Vec::new();
            if let Some(mut stdout) = child.stdout.take() {
                stdout.read_to_end(&mut output).await?;
            }
            if let Some(mut stderr) = child.stderr.take() {
                stderr.read_to_end(&mut output).await?;
            }
            Ok(command_output(
                started,
                status.code(),
                None,
                output,
                max_output_tokens,
            ))
        }
        Err(_) => {
            let Some(context) = context else {
                let _ = child.kill().await;
                return Ok(json!({
                    "wall_time_seconds": started.elapsed().as_secs_f64(),
                    "exit_code": null,
                    "output": "command timed out"
                }));
            };
            let mut sessions = context.shared.exec_sessions.lock().await;
            sessions.next_id = sessions.next_id.saturating_add(1);
            let session_id = sessions.next_id;
            sessions.processes.insert(
                session_id,
                RunningProcess {
                    child,
                    stdin,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    started,
                },
            );
            Ok(json!({
                "wall_time_seconds": started.elapsed().as_secs_f64(),
                "session_id": session_id,
                "output": "Process is still running."
            }))
        }
    }
}

/// 收集长任务输出。
async fn collect_running_output(
    process: &mut RunningProcess,
    session_id: Option<u64>,
    max_output_tokens: usize,
) -> AppResult<Value> {
    if let Some(stdout) = process.child.stdout.as_mut() {
        let mut buf = vec![0_u8; 8192];
        if let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(10), stdout.read(&mut buf)).await
        {
            process.stdout.extend_from_slice(&buf[..n]);
        }
    }
    if let Some(stderr) = process.child.stderr.as_mut() {
        let mut buf = vec![0_u8; 8192];
        if let Ok(Ok(n)) =
            tokio::time::timeout(Duration::from_millis(10), stderr.read(&mut buf)).await
        {
            process.stderr.extend_from_slice(&buf[..n]);
        }
    }
    let status = process.child.try_wait()?;
    let mut output = process.stdout.clone();
    output.extend_from_slice(&process.stderr);
    Ok(command_output(
        process.started,
        status.and_then(|status| status.code()),
        status.is_none().then_some(session_id).flatten(),
        output,
        max_output_tokens,
    ))
}

/// 构造命令输出 JSON。
fn command_output(
    started: Instant,
    exit_code: Option<i32>,
    session_id: Option<u64>,
    bytes: Vec<u8>,
    max_output_tokens: usize,
) -> Value {
    let text = String::from_utf8_lossy(&bytes).to_string();
    let original_token_count = estimate_tokens(&text);
    let output = truncate_by_tokens(&text, max_output_tokens);
    json!({
        "wall_time_seconds": started.elapsed().as_secs_f64(),
        "exit_code": exit_code,
        "session_id": session_id,
        "original_token_count": original_token_count,
        "output": output,
    })
}

/// 估算 token 数。
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// 按估算 token 截断输出。
fn truncate_by_tokens(text: &str, max_tokens: usize) -> String {
    let max_bytes = max_tokens.saturating_mul(4);
    if text.len() <= max_bytes {
        return text.to_string();
    }
    format!("{}...[truncated]", &text[..safe_boundary(text, max_bytes)])
}

/// 找到 UTF-8 安全截断边界。
fn safe_boundary(text: &str, max: usize) -> usize {
    let mut index = max.min(text.len());
    while !text.is_char_boundary(index) {
        index = index.saturating_sub(1);
    }
    index
}

/// 默认 shell。
fn default_shell() -> String {
    if cfg!(windows) {
        return "cmd".to_string();
    }
    if let Ok(shell) = std::env::var("SHELL")
        && !shell.trim().is_empty()
        && std::path::Path::new(&shell).is_file()
    {
        return shell;
    }
    for shell in ["/bin/bash", "/usr/bin/bash", "/bin/sh", "/usr/bin/sh"] {
        if std::path::Path::new(shell).is_file() {
            return shell.to_string();
        }
    }
    "sh".to_string()
}

/// 解析工作目录。
fn resolve_workdir(cwd: &std::path::Path, workdir: Option<&str>) -> PathBuf {
    match workdir.filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        }
        None => cwd.to_path_buf(),
    }
}

/// 兼容 Codex approval 参数，但 full any 模式下忽略。
fn approval_compat_properties() -> BTreeMap<String, JsonSchema> {
    BTreeMap::from([
        (
            "sandbox_permissions".to_string(),
            JsonSchema::string_enum(
                vec![json!("use_default"), json!("require_escalated")],
                Some("Per-command sandbox override. Defaults to `use_default`; use `require_escalated` for unsandboxed execution.".to_string()),
            ),
        ),
        (
            "justification".to_string(),
            JsonSchema::string(Some(
                "User-facing approval question for `require_escalated`; omit otherwise.".to_string(),
            )),
        ),
        (
            "prefix_rule".to_string(),
            JsonSchema::array(
                JsonSchema::string(None),
                Some("Reusable approval prefix for `cmd`, only with `sandbox_permissions: \"require_escalated\"`.".to_string()),
            ),
        ),
    ])
}

/// unified exec 输出 schema。
fn unified_exec_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "chunk_id": { "type": "string" },
            "wall_time_seconds": { "type": "number" },
            "exit_code": { "type": "number" },
            "session_id": { "type": "number" },
            "original_token_count": { "type": "number" },
            "output": { "type": "string" }
        },
        "required": ["wall_time_seconds", "output"],
        "additionalProperties": false
    })
}
