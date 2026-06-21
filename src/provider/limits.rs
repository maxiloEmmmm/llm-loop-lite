use crate::config::ProviderConfig;

/// 未知模型默认上下文窗口，适用于 registry 未命中的保守路径。
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;
/// 未知模型默认输出上限，适用于 registry 未命中的保守路径。
pub const DEFAULT_MAX_TOKENS: u32 = 16_384;

const CODEX_CONTEXT_WINDOW: u64 = 272_000;
const CODEX_MAX_TOKENS: u32 = 128_000;
const GPT5_CONTEXT_WINDOW: u64 = 400_000;
const GPT5_LARGE_CONTEXT_WINDOW: u64 = 1_050_000;
const CLAUDE_DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
const CLAUDE_LONG_CONTEXT_WINDOW: u64 = 1_000_000;
const CLAUDE_KIMI_CONTEXT_WINDOW: u64 = 262_144;
const CLAUDE_KIMI_MAX_TOKENS: u32 = 32_768;

/// 模型窗口限制，适用于 provider 请求和 session 上下文预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLimits {
    /// 可提交给模型的最大上下文 token。
    pub context_window: u64,
    /// 单次生成的最大输出 token。
    pub max_tokens: u32,
}

impl ModelLimits {
    /// 构造模型限制，适用于静态 registry 条目。
    const fn new(context_window: u64, max_tokens: u32) -> Self {
        Self {
            context_window,
            max_tokens,
        }
    }
}

impl Default for ModelLimits {
    /// 返回未知模型 fallback，适用于 provider+model 未命中时。
    fn default() -> Self {
        Self::new(DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_TOKENS)
    }
}

/// 解析最终模型限制，适用于配置覆盖和内置 registry 合并。
pub fn resolve_model_limits(config: &ProviderConfig) -> ModelLimits {
    let mut limits = lookup_provider_model_limits(&config.kind, config.model.as_deref())
        .unwrap_or_else(ModelLimits::default);
    if is_claude_kimi_host(config) {
        limits = ModelLimits::new(CLAUDE_KIMI_CONTEXT_WINDOW, CLAUDE_KIMI_MAX_TOKENS);
    }
    if let Some(context_window) = config.max_context_tokens.filter(|value| *value > 0) {
        limits.context_window = context_window;
    }
    if let Some(max_tokens) = config.max_tokens.filter(|value| *value > 0) {
        limits.max_tokens = max_tokens;
    }
    limits
}

/// 判断 Claude host 是否为 Kimi 兼容端点，适用于覆盖 Anthropic 默认窗口。
fn is_claude_kimi_host(config: &ProviderConfig) -> bool {
    if config.kind.trim().to_ascii_lowercase() != "claude" {
        return false;
    }
    config
        .base_url
        .as_deref()
        .map(|base_url| base_url.to_ascii_lowercase().contains("api.kimi.com"))
        .unwrap_or(false)
}

/// 按 provider+model 查找限制，适用于内置已知模型。
pub fn lookup_provider_model_limits(provider: &str, model: Option<&str>) -> Option<ModelLimits> {
    let model = normalize_model_id(model?);
    if model.is_empty() {
        return None;
    }
    match provider.trim().to_ascii_lowercase().as_str() {
        "codex" => lookup_codex_model_limits(&model).or_else(|| lookup_openai_model_limits(&model)),
        "claude" => lookup_claude_model_limits(&model),
        _ => None,
    }
}

/// 规范化模型名，适用于兼容 `provider/model` 与日期后缀。
fn normalize_model_id(model: &str) -> String {
    let mut model = model.trim().to_ascii_lowercase();
    if let Some((_, suffix)) = model.rsplit_once('/') {
        model = suffix.to_string();
    }
    model
}

/// 查找 Codex OAuth 模型限制，适用于 ChatGPT backend-api 实测窗口。
fn lookup_codex_model_limits(model: &str) -> Option<ModelLimits> {
    match model {
        "gpt-5.2"
        | "gpt-5.3-codex"
        | "gpt-5.3-codex-spark"
        | "gpt-5.4"
        | "gpt-5.4-mini"
        | "gpt-5.5" => Some(ModelLimits::new(CODEX_CONTEXT_WINDOW, CODEX_MAX_TOKENS)),
        _ => None,
    }
}

/// 查找 OpenAI 模型限制，适用于 Codex provider 走 OpenAI-compatible profile。
fn lookup_openai_model_limits(model: &str) -> Option<ModelLimits> {
    match model {
        "gpt-5-chat-latest" | "gpt-5.1-chat" | "gpt-5.2-chat" | "gpt-5.3-chat" => {
            Some(ModelLimits::new(DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_TOKENS))
        }
        "gpt-5-nano" => Some(ModelLimits::new(GPT5_CONTEXT_WINDOW, 4_096)),
        "gpt-5.1-codex-mini" => Some(ModelLimits::new(GPT5_CONTEXT_WINDOW, 100_000)),
        "gpt-5.4" | "gpt-5.4-pro" | "gpt-5.5" | "gpt-5.5-pro" => {
            Some(ModelLimits::new(GPT5_LARGE_CONTEXT_WINDOW, 128_000))
        }
        model if model.starts_with("gpt-5") => {
            Some(ModelLimits::new(GPT5_CONTEXT_WINDOW, CODEX_MAX_TOKENS))
        }
        _ => None,
    }
}

/// 查找 Claude 模型限制，适用于 Anthropic Messages API。
fn lookup_claude_model_limits(model: &str) -> Option<ModelLimits> {
    if model.starts_with("claude-fable-5")
        || model.starts_with("claude-mythos-5")
        || model.starts_with("claude-opus-4-6")
        || model.starts_with("claude-opus-4.6")
        || model.starts_with("claude-opus-4-7")
        || model.starts_with("claude-opus-4.7")
        || model.starts_with("claude-opus-4-8")
        || model.starts_with("claude-opus-4.8")
    {
        return Some(ModelLimits::new(CLAUDE_LONG_CONTEXT_WINDOW, 128_000));
    }
    if model.starts_with("claude-sonnet-4-6") || model.starts_with("claude-sonnet-4.6") {
        return Some(ModelLimits::new(CLAUDE_LONG_CONTEXT_WINDOW, 64_000));
    }
    if model.starts_with("claude-haiku-4-5") || model.starts_with("claude-haiku-4.5") {
        return Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 64_000));
    }
    if model.starts_with("claude-opus-4-5") || model.starts_with("claude-opus-4.5") {
        return Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 64_000));
    }
    if model.starts_with("claude-sonnet-4-5") || model.starts_with("claude-sonnet-4.5") {
        return Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 64_000));
    }
    if model.starts_with("claude-3-5-sonnet")
        || model.starts_with("claude-3.5-sonnet")
        || model.starts_with("claude-3-5-haiku")
        || model.starts_with("claude-3.5-haiku")
    {
        return Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 8_192));
    }
    if model.starts_with("claude-3-7-sonnet") || model.starts_with("claude-3.7-sonnet") {
        return Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 64_000));
    }

    match model {
        "claude-opus-4" | "claude-opus-4-1" | "claude-opus-4.1" => {
            Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 32_000))
        }
        "claude-sonnet-4" => Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 64_000)),
        "claude-3-haiku" => Some(ModelLimits::new(CLAUDE_DEFAULT_CONTEXT_WINDOW, 4_096)),
        _ => None,
    }
}
