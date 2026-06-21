use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::error::AppResult;
use crate::home::AppPaths;

/// daemon 顶层配置，来自 `~/.llm-loop/config.toml` 或内置默认值。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppConfig {
    /// daemon 工作目录；缺失时使用用户 home。
    #[serde(rename = "work-dir")]
    pub work_dir: Option<String>,
    /// 是否持久化 session 历史；关闭后不恢复上下文并减少闪存写入。
    #[serde(rename = "cache-session")]
    pub cache_session: bool,
    /// Codex 兼容顶层模型名；runtime merge 时灌入 `provider.model`。
    pub model: Option<String>,
    /// Codex 兼容思考等级；缺省为 high，但不启用思考摘要。
    pub model_reasoning_effort: Option<String>,
    /// Codex 兼容服务层级；`fast` 会在请求时转为 `priority`。
    pub service_tier: Option<String>,
    /// provider/profile 选择器，优先匹配 `model_providers.<key>`。
    #[serde(alias = "model_provder")]
    pub model_provider: Option<String>,
    /// 日志配置。
    pub log: LogConfig,
    /// 模型 provider 配置。
    pub provider: ProviderConfig,
    /// custom provider 选择器；兼容顶层只配置一个键的写法。
    pub custom_provider: Option<String>,
    /// custom provider 注册表，key 由 `provider.custom_provider` 引用。
    pub model_providers: HashMap<String, ModelProviderConfig>,
    /// 启动时要加载的 channel 列表。
    pub channels: Vec<ChannelConfig>,
}

/// 日志配置，支持写入单文件并在超限时清空。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LogConfig {
    /// 日志文件路径；缺失时默认写 `/tmp/llm-loop.log`。
    pub path: Option<String>,
    /// 单日志文件最大字节数；超出后清空当前文件。
    #[serde(rename = "max-size")]
    pub max_size: u64,
}

/// provider 配置，支持 Codex 与 Claude 两条请求路径。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProviderConfig {
    /// provider 类型，当前支持 `codex` 和 `claude`。
    pub kind: String,
    /// 模型名；为空时 provider 层拒绝真实请求。
    pub model: Option<String>,
    /// custom provider key；缺失或 `never` 时走 Codex auth.json。
    pub custom_provider: Option<String>,
    /// Claude API host，缺失时 Claude provider 使用官方默认 host。
    pub base_url: Option<String>,
    /// Claude API key，适用于不依赖环境变量的 daemon 部署。
    #[serde(rename = "api-key")]
    pub api_key: Option<String>,
    /// Claude API key 环境变量名。
    #[serde(rename = "api-key-env")]
    pub api_key_env: Option<String>,
    /// Claude 单次生成最大 token，Messages API 要求显式传入。
    #[serde(rename = "max-tokens")]
    pub max_tokens: Option<u32>,
    /// 模型最大上下文 token，缺失时按 provider+model registry 推断。
    #[serde(rename = "max-context-tokens")]
    pub max_context_tokens: Option<u64>,
    /// Codex Responses reasoning.effort；缺失时不发送 reasoning。
    pub model_reasoning_effort: Option<String>,
    /// Codex Responses service_tier；`fast` 兼容 Codex 配置写法。
    pub service_tier: Option<String>,
}

/// provider/profile 配置，兼容 Codex custom provider 并扩展 Claude。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ModelProviderConfig {
    /// provider 类型，缺失时按 Codex custom provider 兼容处理。
    pub kind: Option<String>,
    /// 当前 profile 的模型名。
    pub model: Option<String>,
    /// Codex custom provider key；缺失时使用当前 profile key。
    pub custom_provider: Option<String>,
    /// OpenAI-compatible base URL。
    pub base_url: Option<String>,
    /// API key 环境变量名。
    pub env_key: Option<String>,
    /// 直接写入配置的 bearer token；仅为兼容 Codex 字段。
    pub experimental_bearer_token: Option<String>,
    /// Claude API key，适用于 Anthropic 兼容 provider。
    #[serde(rename = "api-key")]
    pub api_key: Option<String>,
    /// Claude API key 环境变量名。
    #[serde(rename = "api-key-env")]
    pub api_key_env: Option<String>,
    /// Claude 单次生成最大 token。
    #[serde(rename = "max-tokens")]
    pub max_tokens: Option<u32>,
    /// 当前 profile 的模型最大上下文 token。
    #[serde(rename = "max-context-tokens")]
    pub max_context_tokens: Option<u64>,
    /// 可选 profile 级思考等级；缺失时继承顶层配置。
    pub model_reasoning_effort: Option<String>,
    /// 可选 profile 级服务层级；缺失时继承顶层配置。
    pub service_tier: Option<String>,
    /// 是否要求 OpenAI/Codex auth；custom provider 默认不要求。
    pub requires_openai_auth: bool,
}

/// channel 配置项，当前只定义注册表入口，不实现具体平台。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ChannelConfig {
    /// channel 实例名，用于日志和出站路由。
    pub name: String,
    /// channel 类型，例如未来的 `ws`。
    pub kind: String,
    /// 是否启用该 channel。
    pub enabled: bool,
    /// 飞书 channel 配置。
    pub feishu: FeishuChannelConfig,
    /// QQ 官方机器人 channel 配置。
    pub qq: QqChannelConfig,
    /// Telegram Bot API channel 配置。
    pub telegram: TelegramChannelConfig,
}

/// 飞书 channel 配置，覆盖自建应用长连接最小闭环。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FeishuChannelConfig {
    /// 飞书/Lark 自建应用 app_id。
    pub app_id: Option<String>,
    /// 飞书/Lark 自建应用 app_secret。
    pub app_secret: Option<String>,
    /// 从环境变量读取 app_id 的变量名。
    pub app_id_env: Option<String>,
    /// 从环境变量读取 app_secret 的变量名。
    pub app_secret_env: Option<String>,
    /// 域名模式：`feishu` 或 `lark`。
    pub domain: String,
    /// 群聊是否要求 @ 机器人。
    pub require_mention: bool,
    /// 可选机器人名称，用于缺少 open_id 时兜底判断。
    pub bot_name: Option<String>,
    /// 消息去重缓存容量。
    pub dedup_cache_size: usize,
    /// WebSocket ping 间隔秒。
    pub ping_interval_seconds: u64,
}

/// QQ 官方机器人 channel 配置，覆盖 WebSocket 收消息和被动回复最小闭环。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct QqChannelConfig {
    /// QQ Bot AppID。
    pub app_id: Option<String>,
    /// QQ Bot AppSecret。
    pub app_secret: Option<String>,
    /// 从环境变量读取 AppID 的变量名。
    pub app_id_env: Option<String>,
    /// 从环境变量读取 AppSecret 的变量名。
    pub app_secret_env: Option<String>,
    /// AccessToken 获取地址。
    pub auth_url: String,
    /// OpenAPI 基础地址。
    pub api_base_url: String,
    /// WebSocket intents 位图。
    pub intents: u64,
    /// 消息去重缓存容量。
    pub dedup_cache_size: usize,
    /// WebSocket 断线重连间隔秒。
    pub reconnect_delay_seconds: u64,
}

/// Telegram Bot API channel 配置，覆盖 polling、出站和附件下载。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TelegramChannelConfig {
    /// BotFather 下发的 bot token。
    pub bot_token: Option<String>,
    /// 从环境变量读取 bot token 的变量名。
    pub bot_token_env: Option<String>,
    /// Telegram Bot API 基础地址。
    pub api_base_url: String,
    /// Telegram 文件下载基础地址。
    pub file_base_url: String,
    /// 启动 polling 前是否删除 webhook。
    pub delete_webhook_on_start: bool,
    /// long polling 等待秒数。
    pub poll_timeout_seconds: u64,
    /// 单次 getUpdates 最大条数。
    pub poll_limit: u32,
    /// 群聊是否要求 @ 机器人。
    pub require_mention: bool,
    /// 是否在收到消息后发送 typing。
    pub send_typing: bool,
    /// typing 刷新间隔秒，供后续长任务刷新使用。
    pub typing_refresh_seconds: u64,
    /// 是否下载入站附件。
    pub download_attachments: bool,
    /// 附件最大下载字节数。
    pub max_download_bytes: u64,
    /// 消息去重缓存容量。
    pub dedup_cache_size: usize,
}

impl Default for AppConfig {
    /// 返回缺失配置文件时使用的内置配置。
    fn default() -> Self {
        Self {
            work_dir: None,
            cache_session: true,
            model: None,
            model_reasoning_effort: Some("high".to_string()),
            service_tier: None,
            model_provider: None,
            log: LogConfig::default(),
            provider: ProviderConfig::default(),
            custom_provider: None,
            model_providers: HashMap::new(),
            channels: Vec::new(),
        }
    }
}

impl Default for LogConfig {
    /// 返回内存盘日志路径，避免默认写闪存。
    fn default() -> Self {
        Self {
            path: Some("/tmp/llm-loop.log".to_string()),
            max_size: 4 * 1024 * 1024,
        }
    }
}

impl Default for ProviderConfig {
    /// 返回 Codex OAuth 模式的 provider 默认值，但不猜测模型名。
    fn default() -> Self {
        Self {
            kind: "codex".to_string(),
            model: None,
            custom_provider: None,
            base_url: None,
            api_key: None,
            api_key_env: None,
            max_tokens: None,
            max_context_tokens: None,
            model_reasoning_effort: None,
            service_tier: None,
        }
    }
}

impl Default for ModelProviderConfig {
    /// 返回 custom provider 默认配置。
    fn default() -> Self {
        Self {
            kind: None,
            model: None,
            custom_provider: None,
            base_url: None,
            env_key: None,
            experimental_bearer_token: None,
            api_key: None,
            api_key_env: None,
            max_tokens: None,
            max_context_tokens: None,
            model_reasoning_effort: None,
            service_tier: None,
            requires_openai_auth: false,
        }
    }
}

impl Default for ChannelConfig {
    /// 返回禁用的空 channel 配置占位，仅用于反序列化缺省字段。
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: String::new(),
            enabled: true,
            feishu: FeishuChannelConfig::default(),
            qq: QqChannelConfig::default(),
            telegram: TelegramChannelConfig::default(),
        }
    }
}

impl Default for FeishuChannelConfig {
    /// 返回飞书 channel 默认配置，不猜测凭据。
    fn default() -> Self {
        Self {
            app_id: None,
            app_secret: None,
            app_id_env: Some("FEISHU_APP_ID".to_string()),
            app_secret_env: Some("FEISHU_APP_SECRET".to_string()),
            domain: "feishu".to_string(),
            require_mention: true,
            bot_name: None,
            dedup_cache_size: 2048,
            ping_interval_seconds: 120,
        }
    }
}

impl Default for TelegramChannelConfig {
    /// 返回 Telegram channel 默认配置，不猜测凭据。
    fn default() -> Self {
        Self {
            bot_token: None,
            bot_token_env: Some("TELEGRAM_BOT_TOKEN".to_string()),
            api_base_url: "https://api.telegram.org".to_string(),
            file_base_url: "https://api.telegram.org/file".to_string(),
            delete_webhook_on_start: true,
            poll_timeout_seconds: 50,
            poll_limit: 100,
            require_mention: false,
            send_typing: true,
            typing_refresh_seconds: 4,
            download_attachments: true,
            max_download_bytes: 20 * 1024 * 1024,
            dedup_cache_size: 4096,
        }
    }
}

impl Default for QqChannelConfig {
    /// 返回 QQ channel 默认配置，不猜测凭据。
    fn default() -> Self {
        Self {
            app_id: None,
            app_secret: None,
            app_id_env: Some("QQBOT_APP_ID".to_string()),
            app_secret_env: Some("QQBOT_APP_SECRET".to_string()),
            auth_url: "https://bots.qq.com/app/getAppAccessToken".to_string(),
            api_base_url: "https://api.sgroup.qq.com".to_string(),
            intents: (1 << 25) | (1 << 26),
            dedup_cache_size: 2048,
            reconnect_delay_seconds: 3,
        }
    }
}

/// 从指定路径加载配置；文件不存在时返回内置默认值。
pub fn load_config(path: &Path) -> AppResult<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let raw = fs::read_to_string(path)?;
    Ok(toml::from_str(&raw)?)
}

/// 加载最终配置，并合并 Codex CLI 的 custom provider 注册表。
pub fn load_merged_config(paths: &AppPaths) -> AppResult<AppConfig> {
    let mut config = load_config(&paths.config_path)?;
    let codex_config = load_config(&paths.codex_config_path)?;

    let mut merged_providers = codex_config.model_providers.clone();
    merged_providers.extend(config.model_providers);
    config.model_providers = merged_providers;

    let selected_profile = config
        .model_provider
        .clone()
        .or_else(|| config.custom_provider.clone())
        .or_else(|| codex_config.custom_provider.clone())
        .or_else(|| codex_config.model_provider.clone());

    if let Some(key) = selected_profile.as_deref()
        && let Some(profile) = config.model_providers.get(key).cloned()
    {
        let fallback_model = config
            .provider
            .model
            .clone()
            .or_else(|| config.model.clone())
            .or_else(|| codex_config.model.clone());
        let fallback_reasoning_effort = config
            .provider
            .model_reasoning_effort
            .clone()
            .or_else(|| config.model_reasoning_effort.clone())
            .or_else(|| codex_config.model_reasoning_effort.clone());
        let fallback_service_tier = config
            .provider
            .service_tier
            .clone()
            .or_else(|| config.service_tier.clone())
            .or_else(|| codex_config.service_tier.clone());
        apply_model_provider_profile(&mut config, key, &profile, fallback_model);
        config.provider.model_reasoning_effort = profile
            .model_reasoning_effort
            .clone()
            .or(fallback_reasoning_effort);
        config.provider.service_tier = profile.service_tier.clone().or(fallback_service_tier);
    } else {
        if config.provider.model.is_none() {
            config.provider.model = config.model.clone().or(codex_config.model.clone());
        }
        if config.provider.model_reasoning_effort.is_none() {
            config.provider.model_reasoning_effort = config
                .model_reasoning_effort
                .clone()
                .or(codex_config.model_reasoning_effort.clone());
        }
        if config.provider.service_tier.is_none() {
            config.provider.service_tier = config
                .service_tier
                .clone()
                .or(codex_config.service_tier.clone());
        }

        if config.provider.kind == "codex" && config.provider.custom_provider.is_none() {
            config.provider.custom_provider =
                selected_profile.or_else(|| sole_custom_provider_key(&config.model_providers));
        }
    }

    Ok(config)
}

/// 将 `model_providers.<key>` profile 应用到实际 provider 配置。
fn apply_model_provider_profile(
    config: &mut AppConfig,
    key: &str,
    profile: &ModelProviderConfig,
    fallback_model: Option<String>,
) {
    let kind = profile.kind.as_deref().unwrap_or("codex");
    config.provider.kind = kind.to_string();
    config.provider.model = profile.model.clone().or(fallback_model);
    config.provider.base_url = profile
        .base_url
        .clone()
        .or_else(|| config.provider.base_url.clone());
    config.provider.api_key = profile
        .api_key
        .clone()
        .or_else(|| config.provider.api_key.clone());
    config.provider.api_key_env = profile
        .api_key_env
        .clone()
        .or_else(|| config.provider.api_key_env.clone());
    config.provider.max_tokens = profile.max_tokens.or(config.provider.max_tokens);
    config.provider.max_context_tokens = profile
        .max_context_tokens
        .or(config.provider.max_context_tokens);
    config.provider.custom_provider = if kind == "codex" {
        profile
            .custom_provider
            .clone()
            .or_else(|| Some(key.to_string()))
    } else {
        profile.custom_provider.clone()
    };
}

/// 当 runtime 合并后只有一个 custom provider 时，自动选择它。
fn sole_custom_provider_key(providers: &HashMap<String, ModelProviderConfig>) -> Option<String> {
    let mut keys = providers.keys();
    let first = keys.next()?;
    keys.next().is_none().then(|| first.clone())
}

#[cfg(test)]
mod config_test;
