use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::config::LogConfig;
use crate::error::AppResult;

static LOGGER: OnceLock<Logger> = OnceLock::new();
const DEFAULT_LOG_PATH: &str = "/tmp/llm-loop.log";

/// 初始化全局日志器，适用于 main 加载配置后尽早调用。
pub fn init(config: &LogConfig) -> AppResult<()> {
    let path = config
        .path
        .as_deref()
        .and_then(resolve_log_path)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LOG_PATH));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let _ = LOGGER.set(Logger::file(path, file, config.max_size));
    Ok(())
}

/// 写入 info 日志，适用于普通 daemon 流程日志。
pub fn info(message: impl AsRef<str>) {
    write_line("INFO", message.as_ref());
}

/// 写入 error 日志，适用于错误和异常分支。
pub fn error(message: impl AsRef<str>) {
    write_line("ERROR", message.as_ref());
}

/// 写 info 日志的格式化宏，适用于替代直接 stderr 打印。
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::logger::info(format!($($arg)*))
    };
}

/// 写 error 日志的格式化宏，适用于异常分支。
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::logger::error(format!($($arg)*))
    };
}

/// 写入日志行，适用于 daemon 所有运行期日志。
fn write_line(level: &str, message: &str) {
    let line = format!(
        "{} {} {}\n",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
        level,
        message
    );
    if let Some(logger) = LOGGER.get() {
        logger.write(&line);
    }
}

/// 解析日志路径，适用于支持 `~` 配置。
fn resolve_log_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    Some(PathBuf::from(trimmed))
}

/// 全局日志输出目标。
struct Logger {
    /// 输出目标。
    target: Mutex<LogTarget>,
}

impl Logger {
    /// 创建文件日志器。
    fn file(path: PathBuf, file: std::fs::File, max_size: u64) -> Self {
        Self {
            target: Mutex::new(LogTarget::File {
                path,
                file,
                max_size,
            }),
        }
    }

    /// 写入一行日志，适用于单文件超限清空。
    fn write(&self, line: &str) {
        let Ok(mut target) = self.target.lock() else {
            return;
        };
        match &mut *target {
            LogTarget::File {
                path,
                file,
                max_size,
            } => {
                if should_truncate(file, *max_size) {
                    if let Ok(new_file) = truncate_log_file(path) {
                        *file = new_file;
                    }
                }
                let _ = file.write_all(line.as_bytes());
                let _ = file.flush();
            }
        }
    }
}

/// 日志输出目标。
enum LogTarget {
    /// 单文件输出。
    File {
        /// 日志文件路径。
        path: PathBuf,
        /// 当前打开的文件。
        file: std::fs::File,
        /// 最大文件大小。
        max_size: u64,
    },
}

/// 判断是否需要清空日志文件。
fn should_truncate(file: &std::fs::File, max_size: u64) -> bool {
    max_size > 0
        && file
            .metadata()
            .is_ok_and(|metadata| metadata.len() >= max_size)
}

/// 清空日志文件并返回新的 append 句柄。
fn truncate_log_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}
