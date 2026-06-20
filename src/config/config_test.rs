use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{AppConfig, load_config, load_merged_config};
use crate::home::AppPaths;

/// 配置文件不存在时使用内置默认值。
#[test]
fn missing_config_uses_default() {
    let config = load_config(Path::new("/tmp/llm-loop-config-that-should-not-exist.toml"))
        .expect("缺失配置应返回默认值");
    assert_eq!(config, AppConfig::default());
    assert!(config.provider.custom_provider.is_none());
    assert!(config.channels.is_empty());
    assert_eq!(config.log.path.as_deref(), Some("/tmp/llm-loop.log"));
}

/// TOML 配置能覆盖 provider 和 channel。
#[test]
fn parse_provider_and_channel_config() {
    let raw = r#"
[provider]
model = "gpt-test"
custom_provider = "local"

[model_providers.local]
base_url = "http://127.0.0.1:8080/v1"
env_key = "TEST_API_KEY"

[[channels]]
name = "main"
kind = "ws"
enabled = true

[[channels]]
name = "tg-main"
kind = "telegram"
enabled = true

[channels.telegram]
bot_token_env = "TG_TOKEN"
require_mention = true
"#;
    let config: AppConfig = toml::from_str(raw).expect("TOML 应能解析");
    assert_eq!(config.provider.custom_provider.as_deref(), Some("local"));
    assert_eq!(config.provider.model.as_deref(), Some("gpt-test"));
    assert_eq!(
        config.model_providers["local"].base_url.as_deref(),
        Some("http://127.0.0.1:8080/v1")
    );
    assert_eq!(config.channels[0].kind, "ws");
    assert_eq!(
        config.channels[1].telegram.bot_token_env.as_deref(),
        Some("TG_TOKEN")
    );
    assert!(config.channels[1].telegram.require_mention);
}

/// TOML 配置能关闭 session 缓存并配置单文件日志。
#[test]
fn parse_cache_session_and_log_config() {
    let raw = r#"
cache-session = false

[log]
path = "/tmp/llm-loop.log"
max-size = 1024
"#;
    let config: AppConfig = toml::from_str(raw).expect("TOML 应能解析");

    assert!(!config.cache_session);
    assert_eq!(config.log.path.as_deref(), Some("/tmp/llm-loop.log"));
    assert_eq!(config.log.max_size, 1024);
}

/// 合并配置时复用 `~/.codex/config.toml` 的 custom provider。
#[test]
fn merged_config_reads_codex_model_providers() {
    let root = temp_home("merged_config_reads_codex_model_providers");
    let paths = AppPaths::from_home(&root);
    std::fs::create_dir_all(paths.codex_config_path.parent().expect("应有父目录"))
        .expect("应能创建 Codex 配置目录");
    std::fs::create_dir_all(paths.config_path.parent().expect("应有父目录"))
        .expect("应能创建 llm-loop 配置目录");

    std::fs::write(
        &paths.codex_config_path,
        r#"
[model_providers.shared]
base_url = "http://codex.example/v1"
env_key = "CODEX_KEY"
"#,
    )
    .expect("应能写入 Codex 配置");
    std::fs::write(
        &paths.config_path,
        r#"
custom_provider = "shared"

[provider]
model = "gpt-test"
"#,
    )
    .expect("应能写入 llm-loop 配置");

    let config = load_merged_config(&paths).expect("应能合并配置");
    assert_eq!(config.provider.custom_provider.as_deref(), Some("shared"));
    assert_eq!(
        config.model_providers["shared"].base_url.as_deref(),
        Some("http://codex.example/v1")
    );
}

/// llm-loop 的 custom provider 同 key 覆盖 Codex 配置。
#[test]
fn merged_config_prefers_llm_loop_provider_on_duplicate_key() {
    let root = temp_home("merged_config_prefers_llm_loop_provider_on_duplicate_key");
    let paths = AppPaths::from_home(&root);
    std::fs::create_dir_all(paths.codex_config_path.parent().expect("应有父目录"))
        .expect("应能创建 Codex 配置目录");
    std::fs::create_dir_all(paths.config_path.parent().expect("应有父目录"))
        .expect("应能创建 llm-loop 配置目录");

    std::fs::write(
        &paths.codex_config_path,
        r#"
[model_providers.shared]
base_url = "http://codex.example/v1"
env_key = "CODEX_KEY"
"#,
    )
    .expect("应能写入 Codex 配置");
    std::fs::write(
        &paths.config_path,
        r#"
[provider]
model = "gpt-test"
custom_provider = "shared"

[model_providers.shared]
base_url = "http://loop.example/v1"
env_key = "LOOP_KEY"
"#,
    )
    .expect("应能写入 llm-loop 配置");

    let config = load_merged_config(&paths).expect("应能合并配置");
    assert_eq!(
        config.model_providers["shared"].base_url.as_deref(),
        Some("http://loop.example/v1")
    );
    assert_eq!(
        config.model_providers["shared"].env_key.as_deref(),
        Some("LOOP_KEY")
    );
}

/// `model_provider` 指向 Claude profile 时会展开成 Claude provider。
#[test]
fn merged_config_uses_model_provider_as_claude_profile() {
    let root = temp_home("merged_config_uses_model_provider_as_claude_profile");
    let paths = AppPaths::from_home(&root);
    std::fs::create_dir_all(paths.config_path.parent().expect("应有父目录"))
        .expect("应能创建 llm-loop 配置目录");

    std::fs::write(
        &paths.config_path,
        r#"
    model_provider = "fake_claude"

    [model_providers.fake_claude]
kind = "claude"
model = "fake-claude-model"
base_url = "https://fake-provider.example/api/anthropic"
api-key = "fake-api-key"
max-tokens = 4096
"#,
    )
    .expect("应能写入 llm-loop 配置");

    let config = load_merged_config(&paths).expect("应能合并配置");
    assert_eq!(config.provider.kind, "claude");
    assert_eq!(config.provider.model.as_deref(), Some("fake-claude-model"));
    assert_eq!(
        config.provider.base_url.as_deref(),
        Some("https://fake-provider.example/api/anthropic")
    );
    assert_eq!(config.provider.api_key.as_deref(), Some("fake-api-key"));
    assert_eq!(config.provider.max_tokens, Some(4096));
}

/// `model_provider` 指向无 kind profile 时保持 Codex custom provider 兼容。
#[test]
fn merged_config_uses_model_provider_as_codex_custom_provider() {
    let root = temp_home("merged_config_uses_model_provider_as_codex_custom_provider");
    let paths = AppPaths::from_home(&root);
    std::fs::create_dir_all(paths.config_path.parent().expect("应有父目录"))
        .expect("应能创建 llm-loop 配置目录");

    std::fs::write(
        &paths.config_path,
        r#"
    model_provider = "fake_custom"
model = "gpt-test"

    [model_providers.fake_custom]
base_url = "http://loop.example/v1"
experimental_bearer_token = "fake-bearer-token"
"#,
    )
    .expect("应能写入 llm-loop 配置");

    let config = load_merged_config(&paths).expect("应能合并配置");
    assert_eq!(config.provider.kind, "codex");
    assert_eq!(config.provider.model.as_deref(), Some("gpt-test"));
    assert_eq!(
        config.provider.custom_provider.as_deref(),
        Some("fake_custom")
    );
}

/// 创建唯一临时 home，适用于不依赖第三方临时目录库的测试。
fn temp_home(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("llm-loop-{name}-{nanos}"))
}
