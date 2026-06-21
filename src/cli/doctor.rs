use crate::config::{AppConfig, ChannelConfig, ProviderConfig};
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;

/// 体检项等级，适用于 doctor 输出和退出码判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckLevel {
    /// 通过。
    Ok,
    /// 可运行但建议处理。
    Warn,
    /// 配置缺失或无法安全启动。
    Fail,
}

/// 单条体检结果。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Check {
    /// 体检等级。
    level: CheckLevel,
    /// 检查对象。
    name: String,
    /// 人读描述。
    message: String,
}

/// 执行 CLI doctor，适用于启动前检查本机配置。
pub async fn run_doctor(config: &AppConfig, paths: &AppPaths) -> AppResult<()> {
    let mut checks = Vec::new();
    check_paths(paths, &mut checks);
    check_provider(&config.provider, config, paths, &mut checks);
    check_channels(&config.channels, &mut checks);
    print_checks(&checks);
    if checks.iter().any(|check| check.level == CheckLevel::Fail) {
        return Err(AppError::Cli("doctor found failed checks".to_string()));
    }
    Ok(())
}

/// 检查关键路径，适用于发现目录权限和工作目录错误。
fn check_paths(paths: &AppPaths, checks: &mut Vec<Check>) {
    push_check(
        checks,
        paths.config_path.exists(),
        "config",
        format!("config path: {}", paths.config_path.display()),
    );
    push_check(
        checks,
        paths.work_dir.is_dir(),
        "work-dir",
        format!("work dir: {}", paths.work_dir.display()),
    );
    for (name, path) in [
        ("app-dir", &paths.app_dir),
        ("sessions", &paths.sessions_dir),
        ("channel", &paths.channel_data_dir),
        ("skills", &paths.skills_dir),
        ("mems", &paths.mems_dir),
        ("crons", &paths.crons_dir),
    ] {
        let level = if path.exists() {
            CheckLevel::Ok
        } else {
            CheckLevel::Warn
        };
        checks.push(Check {
            level,
            name: name.to_string(),
            message: format!("{}: {}", name, path.display()),
        });
    }
}

/// 检查 provider 配置，适用于请求前发现模型和凭据缺失。
fn check_provider(
    provider: &ProviderConfig,
    config: &AppConfig,
    paths: &AppPaths,
    checks: &mut Vec<Check>,
) {
    match provider.kind.as_str() {
        "codex" => check_codex_provider(provider, config, paths, checks),
        "claude" => check_claude_provider(provider, checks),
        other => checks.push(Check {
            level: CheckLevel::Fail,
            name: "provider.kind".to_string(),
            message: format!("unsupported provider kind: {other}"),
        }),
    }
    push_check(
        checks,
        provider
            .model
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty()),
        "provider.model",
        format!(
            "model: {}",
            provider.model.as_deref().unwrap_or("<missing>")
        ),
    );
}

/// 检查 Codex provider，适用于 OAuth 或 OpenAI-compatible profile。
fn check_codex_provider(
    provider: &ProviderConfig,
    config: &AppConfig,
    paths: &AppPaths,
    checks: &mut Vec<Check>,
) {
    if let Some(profile_key) = provider.custom_provider.as_deref() {
        let Some(profile) = config.model_providers.get(profile_key) else {
            checks.push(Check {
                level: CheckLevel::Fail,
                name: "provider.custom_provider".to_string(),
                message: format!("profile `{profile_key}` is missing"),
            });
            return;
        };
        push_check(
            checks,
            profile
                .base_url
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty()),
            "provider.base_url",
            format!("profile `{profile_key}` base_url configured"),
        );
        if let Some(env_key) = profile.env_key.as_deref() {
            push_env_check(checks, env_key, "provider.env_key");
        } else if profile.experimental_bearer_token.is_none() && !profile.requires_openai_auth {
            checks.push(Check {
                level: CheckLevel::Warn,
                name: "provider.env_key".to_string(),
                message: format!("profile `{profile_key}` has no env_key or bearer token"),
            });
        }
        return;
    }
    push_check(
        checks,
        paths.auth_path.is_file(),
        "codex.auth",
        format!("auth path: {}", paths.auth_path.display()),
    );
}

/// 检查 Claude provider，适用于 Anthropic Messages 请求前。
fn check_claude_provider(provider: &ProviderConfig, checks: &mut Vec<Check>) {
    let has_key = provider
        .api_key
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || provider
            .api_key_env
            .as_deref()
            .is_some_and(|env| env_is_set(env));
    push_check(
        checks,
        has_key,
        "claude.api-key",
        "Claude API key configured",
    );
    push_check(
        checks,
        provider.max_tokens.is_some(),
        "claude.max-tokens",
        "Claude max-tokens configured",
    );
}

/// 检查 channel 配置，适用于启动前发现凭据缺失。
fn check_channels(channels: &[ChannelConfig], checks: &mut Vec<Check>) {
    let enabled = channels
        .iter()
        .filter(|channel| channel.enabled)
        .collect::<Vec<_>>();
    if enabled.is_empty() {
        checks.push(Check {
            level: CheckLevel::Warn,
            name: "channels".to_string(),
            message: "no enabled channels".to_string(),
        });
        return;
    }
    for channel in enabled {
        match channel.kind.as_str() {
            "feishu" | "lark" => {
                push_secret_check(
                    checks,
                    &channel.feishu.app_id,
                    &channel.feishu.app_id_env,
                    &format!("channel.{}.app_id", channel.name),
                );
                push_secret_check(
                    checks,
                    &channel.feishu.app_secret,
                    &channel.feishu.app_secret_env,
                    &format!("channel.{}.app_secret", channel.name),
                );
            }
            "telegram" | "tg" => push_secret_check(
                checks,
                &channel.telegram.bot_token,
                &channel.telegram.bot_token_env,
                &format!("channel.{}.bot_token", channel.name),
            ),
            "qq" | "qqbot" => {
                push_secret_check(
                    checks,
                    &channel.qq.app_id,
                    &channel.qq.app_id_env,
                    &format!("channel.{}.app_id", channel.name),
                );
                push_secret_check(
                    checks,
                    &channel.qq.app_secret,
                    &channel.qq.app_secret_env,
                    &format!("channel.{}.app_secret", channel.name),
                );
            }
            other => checks.push(Check {
                level: CheckLevel::Fail,
                name: format!("channel.{}", channel.name),
                message: format!("unsupported channel kind: {other}"),
            }),
        }
    }
}

/// 检查明文或 env secret，适用于 channel 凭据。
fn push_secret_check(
    checks: &mut Vec<Check>,
    direct: &Option<String>,
    env_name: &Option<String>,
    name: &str,
) {
    if direct
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        checks.push(Check {
            level: CheckLevel::Ok,
            name: name.to_string(),
            message: "configured directly".to_string(),
        });
        return;
    }
    if let Some(env_name) = env_name.as_deref() {
        push_env_check(checks, env_name, name);
        return;
    }
    checks.push(Check {
        level: CheckLevel::Fail,
        name: name.to_string(),
        message: "missing direct value and env name".to_string(),
    });
}

/// 检查环境变量是否存在。
fn push_env_check(checks: &mut Vec<Check>, env_name: &str, name: &str) {
    push_check(
        checks,
        env_is_set(env_name),
        name,
        format!("env {env_name} is set"),
    );
}

/// 返回环境变量是否设置且非空。
fn env_is_set(env_name: &str) -> bool {
    std::env::var(env_name).is_ok_and(|value| !value.trim().is_empty())
}

/// 添加二值检查结果。
fn push_check(
    checks: &mut Vec<Check>,
    ok: bool,
    name: impl Into<String>,
    message: impl Into<String>,
) {
    checks.push(Check {
        level: if ok { CheckLevel::Ok } else { CheckLevel::Fail },
        name: name.into(),
        message: message.into(),
    });
}

/// 打印 doctor 结果。
fn print_checks(checks: &[Check]) {
    for check in checks {
        let mark = match check.level {
            CheckLevel::Ok => "ok",
            CheckLevel::Warn => "warn",
            CheckLevel::Fail => "fail",
        };
        println!("{mark:>4}  {:<28} {}", check.name, check.message);
    }
}
