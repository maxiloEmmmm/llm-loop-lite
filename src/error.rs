use std::error::Error;
use std::fmt::{Display, Formatter};

/// 应用内统一结果类型，适用于 daemon、配置、channel、provider 的边界。
pub type AppResult<T> = Result<T, AppError>;

/// 应用错误枚举，保留错误来源同时避免在核心层引入额外错误库。
#[derive(Debug)]
pub enum AppError {
    /// 文件系统或环境路径访问失败。
    Io(std::io::Error),
    /// TOML 配置解析失败。
    Config(toml::de::Error),
    /// JSON 解析或序列化失败。
    Json(serde_json::Error),
    /// HTTP 请求失败。
    Http(reqwest::Error),
    /// HOME 缺失，无法定位 `~/.llm-loop`。
    MissingHome,
    /// 配置声明了当前二进制尚未实现的 channel。
    UnsupportedChannel(String),
    /// channel 生命周期或发送失败。
    Channel(String),
    /// provider 调用失败或尚未接线。
    Provider(String),
    /// 工具调用失败。
    Tool(String),
    /// 定时任务配置或执行失败。
    Cron(String),
    /// CLI 参数或子命令不合法。
    Cli(String),
}

impl Display for AppError {
    /// 将错误转换成人可读文本，适用于 CLI/daemon 日志输出。
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Config(err) => write!(f, "config parse error: {err}"),
            Self::Json(err) => write!(f, "json error: {err}"),
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::MissingHome => write!(f, "HOME is not set; cannot resolve ~/.llm-loop"),
            Self::UnsupportedChannel(kind) => write!(f, "unsupported channel kind: {kind}"),
            Self::Channel(message) => write!(f, "channel error: {message}"),
            Self::Provider(message) => write!(f, "provider error: {message}"),
            Self::Tool(message) => write!(f, "tool error: {message}"),
            Self::Cron(message) => write!(f, "cron error: {message}"),
            Self::Cli(message) => write!(f, "cli error: {message}"),
        }
    }
}

impl Error for AppError {
    /// 暴露底层错误来源，适用于上层日志记录。
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Config(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Http(err) => Some(err),
            Self::MissingHome
            | Self::UnsupportedChannel(_)
            | Self::Channel(_)
            | Self::Provider(_)
            | Self::Tool(_)
            | Self::Cron(_)
            | Self::Cli(_) => None,
        }
    }
}

impl From<std::io::Error> for AppError {
    /// 将标准 IO 错误映射为应用错误。
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<toml::de::Error> for AppError {
    /// 将 TOML 解析错误映射为应用错误。
    fn from(value: toml::de::Error) -> Self {
        Self::Config(value)
    }
}

impl From<serde_json::Error> for AppError {
    /// 将 JSON 错误映射为应用错误。
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<reqwest::Error> for AppError {
    /// 将 HTTP 错误映射为应用错误。
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}
