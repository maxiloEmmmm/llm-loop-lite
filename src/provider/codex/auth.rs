use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

use crate::config::{AppConfig, ModelProviderConfig};
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;

/// provider 路由，描述本轮请求应该发到哪里以及如何认证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRoute {
    /// provider 类型。
    pub kind: ProviderRouteKind,
    /// 请求 base URL。
    pub base_url: String,
    /// bearer token。
    pub bearer_token: String,
    /// ChatGPT account id，仅 OAuth 路径需要。
    pub account_id: Option<String>,
}

/// provider 路由类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRouteKind {
    /// Codex OAuth auth.json。
    CodexOauth,
    /// `model_providers.<key>` custom provider。
    Custom,
}

/// Codex auth.json 的最小兼容结构。
#[derive(Debug, Deserialize)]
struct CodexAuthJson {
    /// ChatGPT/OAuth token 组。
    tokens: CodexTokens,
    /// Codex 兼容字段，旧 auth.json 可能带这个 key。
    #[serde(rename = "OPENAI_API_KEY")]
    #[allow(dead_code)]
    openai_api_key: Option<String>,
}

/// Codex auth.json tokens 子结构。
#[derive(Debug, Deserialize)]
struct CodexTokens {
    /// access token。
    access_token: String,
    /// ChatGPT account id；旧版本可能缺失，需要从 id_token 补。
    account_id: Option<String>,
    /// id token，用于补 account id。
    id_token: Option<String>,
}

impl ProviderRoute {
    /// 根据配置解析 provider 路由。
    pub fn resolve(config: &AppConfig, paths: &AppPaths) -> AppResult<Self> {
        match selected_custom_provider_key(config.provider.custom_provider.as_deref()) {
            Some(key) => {
                let provider = config.model_providers.get(key).ok_or_else(|| {
                    AppError::Provider(format!("custom provider `{key}` is not configured"))
                })?;
                Self::from_custom_provider(provider)
            }
            None => Self::from_codex_auth(paths),
        }
    }

    /// 从 Codex auth.json 创建 OAuth 路由。
    fn from_codex_auth(paths: &AppPaths) -> AppResult<Self> {
        let raw = std::fs::read_to_string(&paths.auth_path).map_err(|err| {
            AppError::Provider(format!(
                "read Codex auth {} failed: {err}",
                paths.auth_path.display()
            ))
        })?;
        let auth: CodexAuthJson = serde_json::from_str(&raw)?;
        let account_id = auth.tokens.account_id.or_else(|| {
            auth.tokens
                .id_token
                .as_deref()
                .and_then(account_id_from_id_token)
        });
        let account_id = account_id.ok_or_else(|| {
            AppError::Provider("Codex auth.json has no ChatGPT account id".to_string())
        })?;

        Ok(Self {
            kind: ProviderRouteKind::CodexOauth,
            base_url: "https://chatgpt.com/backend-api/codex".to_string(),
            bearer_token: auth.tokens.access_token,
            account_id: Some(account_id),
        })
    }

    /// 从 custom provider 创建路由。
    fn from_custom_provider(provider: &ModelProviderConfig) -> AppResult<Self> {
        let base_url = provider.base_url.clone().ok_or_else(|| {
            AppError::Provider("custom provider base_url is required".to_string())
        })?;
        let bearer_token = if let Some(token) = &provider.experimental_bearer_token {
            token.clone()
        } else if let Some(env_key) = &provider.env_key {
            std::env::var(env_key).map_err(|_| {
                AppError::Provider(format!("environment variable `{env_key}` is not set"))
            })?
        } else {
            return Err(AppError::Provider(
                "custom provider env_key or experimental_bearer_token is required".to_string(),
            ));
        };

        Ok(Self {
            kind: ProviderRouteKind::Custom,
            base_url,
            bearer_token,
            account_id: None,
        })
    }
}

/// `custom_provider` 缺失或 `never` 时不启用 custom provider。
fn selected_custom_provider_key(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| *value != "never")
}

/// 从 JWT id_token 中解析 ChatGPT account id。
fn account_id_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(ToOwned::to_owned)
}

/// 测试专用：暴露 account id 解析，避免测试碰私有结构。
#[cfg(test)]
pub(super) fn account_id_from_test_id_token(id_token: &str) -> Option<String> {
    account_id_from_id_token(id_token)
}

/// 测试专用：验证旧 auth.json 字段仍可反序列化。
#[cfg(test)]
pub(super) fn has_openai_api_key_for_test(raw: &str) -> bool {
    serde_json::from_str::<CodexAuthJson>(raw)
        .ok()
        .and_then(|auth| auth.openai_api_key)
        .is_some()
}
