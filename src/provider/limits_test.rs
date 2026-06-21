use crate::config::ProviderConfig;
use crate::provider::limits::{ModelLimits, resolve_model_limits};

/// 构造 provider 配置，适用于模型限制 registry 测试。
fn provider_config(kind: &str, model: &str) -> ProviderConfig {
    ProviderConfig {
        kind: kind.to_string(),
        model: Some(model.to_string()),
        ..ProviderConfig::default()
    }
}

/// 未知模型会落到保守 fallback。
#[test]
fn unknown_model_uses_default_limits() {
    let limits = resolve_model_limits(&provider_config("codex", "unknown-model"));

    assert_eq!(
        limits,
        ModelLimits {
            context_window: 128_000,
            max_tokens: 16_384,
        }
    );
}

/// 显式配置优先覆盖内置 registry。
#[test]
fn explicit_config_overrides_registry_limits() {
    let mut config = provider_config("codex", "gpt-5.3-codex");
    config.max_context_tokens = Some(42_000);
    config.max_tokens = Some(4_200);

    let limits = resolve_model_limits(&config);

    assert_eq!(
        limits,
        ModelLimits {
            context_window: 42_000,
            max_tokens: 4_200,
        }
    );
}

/// Codex OAuth 模型使用实测上下文窗口。
#[test]
fn codex_model_uses_codex_limits() {
    let limits = resolve_model_limits(&provider_config("codex", "gpt-5.3-codex"));

    assert_eq!(
        limits,
        ModelLimits {
            context_window: 272_000,
            max_tokens: 128_000,
        }
    );
}

/// Claude 长上下文模型使用 Anthropic registry 限制。
#[test]
fn claude_model_uses_claude_limits() {
    let limits = resolve_model_limits(&provider_config("claude", "claude-sonnet-4-6"));

    assert_eq!(
        limits,
        ModelLimits {
            context_window: 1_000_000,
            max_tokens: 64_000,
        }
    );
}

/// Claude 日期后缀模型会命中同族限制。
#[test]
fn claude_dated_alias_uses_family_limits() {
    let limits = resolve_model_limits(&provider_config("claude", "claude-sonnet-4-5-20250929"));

    assert_eq!(
        limits,
        ModelLimits {
            context_window: 200_000,
            max_tokens: 64_000,
        }
    );
}

/// Claude 兼容 Kimi host 使用 Kimi 窗口限制。
#[test]
fn claude_kimi_host_uses_kimi_limits() {
    let mut config = provider_config("claude", "kimi-k2.7-code");
    config.base_url = Some("https://api.kimi.com/coding/".to_string());

    let limits = resolve_model_limits(&config);

    assert_eq!(
        limits,
        ModelLimits {
            context_window: 262_144,
            max_tokens: 32_768,
        }
    );
}

/// 显式限制仍优先于 Claude Kimi host 默认值。
#[test]
fn explicit_config_overrides_claude_kimi_host_limits() {
    let mut config = provider_config("claude", "kimi-k2.7-code");
    config.base_url = Some("https://api.kimi.com/coding/".to_string());
    config.max_context_tokens = Some(131_072);
    config.max_tokens = Some(8_192);

    let limits = resolve_model_limits(&config);

    assert_eq!(
        limits,
        ModelLimits {
            context_window: 131_072,
            max_tokens: 8_192,
        }
    );
}
