use std::path::Path;

use chrono::{DateTime, Utc};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::{AppError, AppResult};
use crate::home::AppPaths;

const DEVICE_CODE_URL: &str = "https://auth.openai.com/oauth/device/code";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "codex-cli";

/// device code 响应。
#[derive(Debug, Deserialize)]
pub(super) struct UserCodeResp {
    /// device auth id。
    pub device_auth_id: String,
    /// 用户输入 code。
    pub user_code: String,
    /// 轮询间隔秒数。
    #[serde(deserialize_with = "deserialize_interval")]
    pub interval: u64,
}

/// token 响应。
#[derive(Debug, Deserialize)]
pub(super) struct TokenResponse {
    /// access token。
    access_token: String,
    /// refresh token。
    refresh_token: String,
    /// id token。
    id_token: String,
}

/// Codex auth.json 兼容结构。
#[derive(Debug, Serialize)]
struct AuthJson {
    /// auth 模式。
    auth_mode: &'static str,
    /// Codex 兼容字段。
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    /// token 组。
    tokens: AuthTokens,
    /// 最后刷新时间。
    last_refresh: DateTime<Utc>,
}

/// Codex auth.json tokens 子结构。
#[derive(Debug, Serialize)]
struct AuthTokens {
    /// access token。
    access_token: String,
    /// refresh token。
    refresh_token: String,
    /// id token。
    id_token: String,
    /// ChatGPT account id。
    account_id: Option<String>,
}

/// 执行 OAuth device code 登录并保存 auth.json。
pub async fn run_oauth_login(paths: &AppPaths) -> AppResult<()> {
    let client = Client::new();
    let user_code = request_device_code(&client).await?;
    println!(
        "Open https://chatgpt.com/activate and enter {}",
        user_code.user_code
    );
    let token = poll_token(&client, &user_code).await?;
    save_auth_json(paths, token, Utc::now()).await?;
    println!("OAuth login saved to {}", paths.auth_path.display());
    Ok(())
}

/// 请求 device code。
async fn request_device_code(client: &Client) -> AppResult<UserCodeResp> {
    let response = client
        .post(DEVICE_CODE_URL)
        .form(&[("client_id", CLIENT_ID), ("scope", "openid profile email")])
        .send()
        .await?;
    Ok(response.json::<UserCodeResp>().await?)
}

/// 轮询 token 直到登录完成。
async fn poll_token(client: &Client, user_code: &UserCodeResp) -> AppResult<TokenResponse> {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(user_code.interval.max(1))).await;
        let response = client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", user_code.device_auth_id.as_str()),
            ])
            .send()
            .await?;
        if response.status() == StatusCode::OK {
            return Ok(response.json::<TokenResponse>().await?);
        }
        if response.status() != StatusCode::BAD_REQUEST {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::Provider(format!(
                "OAuth token poll failed with status {status}: {body}"
            )));
        }
    }
}

/// 保存 Codex 兼容 auth.json。
async fn save_auth_json(
    paths: &AppPaths,
    token: TokenResponse,
    now: DateTime<Utc>,
) -> AppResult<()> {
    if let Some(parent) = paths.auth_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    write_private_json(&paths.auth_path, &build_auth_json(token, now)).await
}

/// 写入 JSON 文件。
async fn write_private_json(path: &Path, auth: &AuthJson) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(auth)?;
    let mut file = tokio::fs::File::create(path).await?;
    file.write_all(&bytes).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

/// 构造 Codex auth.json。
fn build_auth_json(token: TokenResponse, now: DateTime<Utc>) -> AuthJson {
    let account_id = account_id_from_id_token(&token.id_token);
    AuthJson {
        auth_mode: "chatgpt",
        openai_api_key: None,
        tokens: AuthTokens {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            id_token: token.id_token,
            account_id,
        },
        last_refresh: now,
    }
}

/// interval 兼容字符串和数字。
fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("interval must be u64")),
        serde_json::Value::String(text) => text.parse().map_err(serde::de::Error::custom),
        _ => Err(serde::de::Error::custom(
            "interval must be string or number",
        )),
    }
}

/// 从 id_token 中解析 account id。
fn account_id_from_id_token(id_token: &str) -> Option<String> {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let payload = id_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(ToOwned::to_owned)
}

/// 测试专用：构造 token response。
#[cfg(test)]
pub(super) fn token_response_for_test(
    id_token: String,
    access_token: String,
    refresh_token: String,
) -> TokenResponse {
    TokenResponse {
        access_token,
        refresh_token,
        id_token,
    }
}

/// 测试专用：构造 auth.json 的 JSON 值。
#[cfg(test)]
pub(super) fn build_auth_json_for_test(
    token: TokenResponse,
    now: DateTime<Utc>,
) -> serde_json::Value {
    serde_json::to_value(build_auth_json(token, now)).expect("测试 auth 应能序列化")
}
