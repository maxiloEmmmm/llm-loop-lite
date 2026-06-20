//! 飞书 REST API 最小客户端。

use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::message::OutboundFormat;

const RECEIVED_REACTION_EMOJI: &str = "Typing";
const DONE_REACTION_EMOJI: &str = "DONE";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_MARKDOWN_TABLES_PER_CARD: usize = 5;

/// 飞书 API 客户端。
pub struct FeishuApi {
    /// open-apis base URL。
    base_url: String,
    /// 应用 app_id。
    app_id: String,
    /// 应用 app_secret。
    app_secret: String,
    /// HTTP client。
    client: Client,
    /// tenant_access_token 缓存。
    token: Arc<Mutex<Option<CachedToken>>>,
}

impl FeishuApi {
    /// 创建飞书 API 客户端。
    pub fn new(base_url: String, app_id: String, app_secret: String) -> Self {
        Self {
            base_url,
            app_id,
            app_secret,
            client: Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .expect("飞书 HTTP client 配置必须有效"),
            token: Arc::new(Mutex::new(None)),
        }
    }

    /// 返回平台名称。
    pub fn platform_name(&self) -> &str {
        if self.base_url.contains("larksuite") {
            "lark"
        } else {
            "feishu"
        }
    }

    /// 获取飞书 WebSocket endpoint。
    pub async fn ws_endpoint(&self) -> AppResult<String> {
        let url = format!("{}/callback/ws/endpoint", self.base_url);
        crate::log_info!("feishu ws endpoint requesting");
        let response = self
            .client
            .post(url)
            .header("locale", "zh")
            .json(&serde_json::json!({
                "AppID": self.app_id,
                "AppSecret": self.app_secret,
            }))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "feishu endpoint failed with {status}: {body}"
            )));
        }
        let parsed: EndpointResponse = serde_json::from_str(&body)?;
        if parsed.code != 0 {
            return Err(AppError::Channel(format!(
                "feishu endpoint error {}: {}",
                parsed.code,
                parsed.msg.unwrap_or_default()
            )));
        }
        parsed
            .data
            .and_then(|data| data.url)
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| AppError::Channel("feishu endpoint missing URL".to_string()))
    }

    /// 发送飞书 2.0 卡片消息，优先 reply。
    pub async fn send_text(
        &self,
        recipient: FeishuRecipient<'_>,
        text: &str,
        reply_to: Option<&str>,
        reply_in_thread: bool,
        format: OutboundFormat,
    ) -> AppResult<Option<String>> {
        let rendered_text = feishu_render_outbound_text(text, format);
        log_feishu_text_shape("send_text", &rendered_text);
        if matches!(format, OutboundFormat::Text) {
            let batches =
                split_markdown_table_batches(&rendered_text, MAX_MARKDOWN_TABLES_PER_CARD);
            if batches.len() > 1 {
                return self
                    .send_text_batches(recipient, batches, reply_to, reply_in_thread)
                    .await;
            }
        }
        let card = build_markdown_card(&rendered_text);
        if let Some(message_id) = reply_to.filter(|value| !value.trim().is_empty()) {
            return self.reply_message(message_id, card, reply_in_thread).await;
        }
        self.create_message(recipient, card).await
    }

    /// 编辑飞书 2.0 卡片消息，适用于计划列表这种需要原地刷新的消息。
    pub async fn edit_text_message(
        &self,
        message_id: &str,
        text: &str,
        format: OutboundFormat,
    ) -> AppResult<Option<String>> {
        let rendered_text = feishu_render_outbound_text(text, format);
        log_feishu_text_shape("edit_text_message", &rendered_text);
        self.patch_interactive_card(message_id, build_markdown_card(&rendered_text))
            .await
    }

    /// 分批发送飞书文本卡片，适用于单条卡片 Markdown 表格数量超限。
    async fn send_text_batches(
        &self,
        recipient: FeishuRecipient<'_>,
        batches: Vec<String>,
        reply_to: Option<&str>,
        reply_in_thread: bool,
    ) -> AppResult<Option<String>> {
        crate::log_info!(
            "feishu send_text split batches={} max_tables_per_card={}",
            batches.len(),
            MAX_MARKDOWN_TABLES_PER_CARD
        );
        let mut last_message_id = None;
        for (index, batch) in batches.iter().enumerate() {
            crate::log_info!(
                "feishu send_text batch sending index={} total={} chars={} markdown_tables={}",
                index + 1,
                batches.len(),
                batch.chars().count(),
                count_markdown_tables(batch)
            );
            let card = build_markdown_card(batch);
            let message_id = if index == 0 {
                if let Some(message_id) = reply_to.filter(|value| !value.trim().is_empty()) {
                    self.reply_message(message_id, card, reply_in_thread)
                        .await?
                } else {
                    self.create_message(recipient, card).await?
                }
            } else {
                self.create_message(recipient, card).await?
            };
            last_message_id = message_id.or(last_message_id);
        }
        Ok(last_message_id)
    }

    /// 更新飞书消息卡片，适用于保持 interactive 类型原地刷新按钮状态。
    pub async fn patch_interactive_card(
        &self,
        message_id: &str,
        card: Value,
    ) -> AppResult<Option<String>> {
        let url = format!("{}/open-apis/im/v1/messages/{}", self.base_url, message_id);
        let token = self.tenant_access_token().await?;
        let body = serde_json::json!({
            "content": card.to_string(),
        });
        self.patch_message_api(url, token, body).await
    }

    /// 发送飞书交互卡片。
    pub async fn send_interactive_card(
        &self,
        recipient: FeishuRecipient<'_>,
        card: Value,
    ) -> AppResult<Option<String>> {
        let url = format!(
            "{}/open-apis/im/v1/messages?receive_id_type={}",
            self.base_url,
            recipient.id_type.query_value()
        );
        let token = self.tenant_access_token().await?;
        let body = serde_json::json!({
            "receive_id": recipient.id,
            "msg_type": "interactive",
            "content": card.to_string(),
            "uuid": uuid::Uuid::new_v4().to_string(),
        });
        self.post_message_api(url, token, body).await
    }

    /// 给收到的飞书消息添加处理中 reaction。
    pub async fn add_received_reaction(&self, message_id: &str) -> AppResult<Option<String>> {
        self.add_reaction(message_id, RECEIVED_REACTION_EMOJI).await
    }

    /// 给消息添加完成 reaction。
    pub async fn add_done_reaction(&self, message_id: &str) -> AppResult<Option<String>> {
        self.add_reaction(message_id, DONE_REACTION_EMOJI).await
    }

    /// 给指定消息添加 reaction。
    async fn add_reaction(&self, message_id: &str, emoji_type: &str) -> AppResult<Option<String>> {
        crate::log_info!(
            "feishu reaction sending for message_id={} emoji_type={}",
            message_id,
            emoji_type
        );
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reactions",
            self.base_url, message_id
        );
        let token = self.tenant_access_token().await?;
        let body = serde_json::json!({
            "reaction_type": {
                "emoji_type": emoji_type,
            },
        });
        self.post_message_api(url, token, body).await
    }

    /// 下载用户消息中的图片资源。
    pub async fn download_message_image(
        &self,
        message_id: &str,
        image_key: &str,
    ) -> AppResult<DownloadedImage> {
        let downloaded = self
            .download_message_resource(message_id, image_key, "image")
            .await?;
        Ok(DownloadedImage {
            mime_type: normalize_image_mime(&downloaded.mime_type, &downloaded.bytes),
            bytes: downloaded.bytes,
        })
    }

    /// 下载用户消息中的普通文件资源。
    pub async fn download_message_file(
        &self,
        message_id: &str,
        file_key: &str,
    ) -> AppResult<DownloadedFile> {
        self.download_message_resource(message_id, file_key, "file")
            .await
    }

    /// 获取指定消息内容，适用于读取合并转发消息中的子消息。
    pub async fn get_message_content(
        &self,
        message_id: &str,
    ) -> AppResult<Vec<FeishuMessageDetail>> {
        let url = format!("{}/open-apis/im/v1/messages/{}", self.base_url, message_id);
        let token = self.tenant_access_token().await?;
        crate::log_info!("feishu get message start url={}", redact_query(&url));
        let response = self.client.get(url).bearer_auth(token).send().await?;
        let status = response.status();
        let text = response.text().await?;
        crate::log_info!(
            "feishu get message response status={} bytes={}",
            status,
            text.len()
        );
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "feishu get message failed with {status}: {text}"
            )));
        }
        let parsed: MessageContentResponse = serde_json::from_str(&text)?;
        if parsed.code != 0 {
            return Err(AppError::Channel(format!(
                "feishu get message error {}: {}",
                parsed.code,
                parsed.msg.unwrap_or_default()
            )));
        }
        Ok(parsed.data.map(|data| data.items).unwrap_or_default())
    }

    /// 获取当前应用机器人的 open_id，适用于群聊 @ 门禁初始化。
    pub async fn bot_open_id(&self) -> AppResult<String> {
        let url = format!("{}/open-apis/bot/v3/info", self.base_url);
        let token = self.tenant_access_token().await?;
        crate::log_info!("feishu bot info requesting");
        let response = self.client.get(url).bearer_auth(token).send().await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "feishu bot info failed with {status}: {text}"
            )));
        }
        let parsed: BotInfoResponse = serde_json::from_str(&text)?;
        if parsed.code != 0 {
            return Err(AppError::Channel(format!(
                "feishu bot info error {}: {}",
                parsed.code,
                parsed.msg.unwrap_or_default()
            )));
        }
        parsed
            .bot
            .and_then(|bot| bot.open_id)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Channel("feishu bot info missing open_id".to_string()))
    }

    /// 下载用户消息中的资源。
    async fn download_message_resource(
        &self,
        message_id: &str,
        file_key: &str,
        resource_type: &str,
    ) -> AppResult<DownloadedFile> {
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/resources/{}?type={}",
            self.base_url, message_id, file_key, resource_type
        );
        let token = self.tenant_access_token().await?;
        let response = self.client.get(url).bearer_auth(token).send().await?;
        let status = response.status();
        let mime_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = response.bytes().await?.to_vec();
        if !status.is_success() {
            let text = String::from_utf8_lossy(&bytes);
            return Err(AppError::Channel(format!(
                "feishu message resource failed with {status}: {text}"
            )));
        }
        Ok(DownloadedFile { mime_type, bytes })
    }

    /// 回复飞书消息。
    async fn reply_message(
        &self,
        message_id: &str,
        card: Value,
        reply_in_thread: bool,
    ) -> AppResult<Option<String>> {
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/reply",
            self.base_url, message_id
        );
        let token = self.tenant_access_token().await?;
        let mut body = serde_json::json!({
            "msg_type": "interactive",
            "content": card.to_string(),
            "uuid": uuid::Uuid::new_v4().to_string(),
        });
        if reply_in_thread {
            // 触发条件：入站消息本身来自飞书回复链。
            // 常规 reply API 会在原消息下回复，但不会进入子话题。
            // 仅此处加飞书专属字段，避免通用 channel 层感知平台语义。
            body["reply_in_thread"] = serde_json::Value::Bool(true);
        }
        self.post_message_api(url, token, body).await
    }

    /// 创建飞书消息。
    async fn create_message(
        &self,
        recipient: FeishuRecipient<'_>,
        card: Value,
    ) -> AppResult<Option<String>> {
        let url = format!(
            "{}/open-apis/im/v1/messages?receive_id_type={}",
            self.base_url,
            recipient.id_type.query_value()
        );
        let token = self.tenant_access_token().await?;
        let body = serde_json::json!({
            "receive_id": recipient.id,
            "msg_type": "interactive",
            "content": card.to_string(),
            "uuid": uuid::Uuid::new_v4().to_string(),
        });
        self.post_message_api(url, token, body).await
    }

    /// 调用消息发送类 API。
    async fn post_message_api(
        &self,
        url: String,
        token: String,
        body: Value,
    ) -> AppResult<Option<String>> {
        crate::log_info!("feishu post api start url={}", redact_query(&url));
        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        crate::log_info!(
            "feishu post api response status={} bytes={}",
            status,
            text.len()
        );
        log_feishu_api_response("post", &text);
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "feishu message api failed with {status}: {text}"
            )));
        }
        let parsed: ApiResponse = serde_json::from_str(&text)?;
        crate::log_info!(
            "feishu post api parsed code={} msg={}",
            parsed.code,
            parsed.msg.as_deref().unwrap_or("")
        );
        if parsed.code != 0 {
            return Err(AppError::Channel(format!(
                "feishu api error {}: {}",
                parsed.code,
                parsed.msg.unwrap_or_default()
            )));
        }
        Ok(parsed
            .data
            .and_then(|data| data.message_id.or(data.reaction_id))
            .filter(|value| !value.trim().is_empty()))
    }

    /// 调用消息 PATCH API，适用于更新已发送的共享卡片。
    async fn patch_message_api(
        &self,
        url: String,
        token: String,
        body: Value,
    ) -> AppResult<Option<String>> {
        crate::log_info!("feishu patch api start url={}", redact_query(&url));
        let response = self
            .client
            .patch(url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        crate::log_info!(
            "feishu patch api response status={} bytes={}",
            status,
            text.len()
        );
        log_feishu_api_response("patch", &text);
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "feishu patch message failed with {status}: {text}"
            )));
        }
        let parsed: ApiResponse = serde_json::from_str(&text)?;
        crate::log_info!(
            "feishu patch api parsed code={} msg={}",
            parsed.code,
            parsed.msg.as_deref().unwrap_or("")
        );
        if parsed.code != 0 {
            return Err(AppError::Channel(format!(
                "feishu patch api error {}: {}",
                parsed.code,
                parsed.msg.unwrap_or_default()
            )));
        }
        Ok(parsed
            .data
            .and_then(|data| data.message_id.or(data.reaction_id))
            .filter(|value| !value.trim().is_empty()))
    }

    /// 获取 tenant_access_token。
    async fn tenant_access_token(&self) -> AppResult<String> {
        if let Some(token) = self.valid_cached_token().await {
            return Ok(token);
        }
        let url = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.base_url
        );
        let response = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "app_secret": self.app_secret,
            }))
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "feishu token failed with {status}: {text}"
            )));
        }
        let parsed: TokenResponse = serde_json::from_str(&text)?;
        if parsed.code != 0 {
            return Err(AppError::Channel(format!(
                "feishu token error {}: {}",
                parsed.code,
                parsed.msg.unwrap_or_default()
            )));
        }
        let expire = parsed.expire.unwrap_or(7200).saturating_sub(60);
        let cached = CachedToken {
            value: parsed.tenant_access_token,
            expires_at: Instant::now() + Duration::from_secs(expire),
        };
        let value = cached.value.clone();
        *self.token.lock().await = Some(cached);
        Ok(value)
    }

    /// 返回未过期 token。
    async fn valid_cached_token(&self) -> Option<String> {
        self.token
            .lock()
            .await
            .as_ref()
            .filter(|token| token.expires_at > Instant::now())
            .map(|token| token.value.clone())
    }
}

/// 隐去 URL query，适用于日志里保留接口路径但不泄漏参数。
fn redact_query(url: &str) -> String {
    url.split('?').next().unwrap_or(url).to_string()
}

/// 记录飞书正文形态，适用于排查卡片渲染限制。
fn log_feishu_text_shape(action: &str, text: &str) {
    crate::log_info!(
        "feishu {} text_shape chars={} markdown_tables={}",
        action,
        text.chars().count(),
        count_markdown_tables(text)
    );
}

/// 统计 Markdown 管道表格，适用于提前发现飞书 card table 限制风险。
fn count_markdown_tables(text: &str) -> usize {
    split_markdown_table_batches(text, usize::MAX)
        .iter()
        .map(|batch| count_markdown_tables_in_batch(batch))
        .sum()
}

/// 按飞书卡片表格上限切分 Markdown，适用于保留原生表格样式。
fn split_markdown_table_batches(text: &str, max_tables: usize) -> Vec<String> {
    let max_tables = max_tables.max(1);
    let lines = text.lines().collect::<Vec<_>>();
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_tables = 0usize;
    let mut in_code_block = false;
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index];
        if is_markdown_fence(line) {
            in_code_block = !in_code_block;
            current.push(line.to_string());
            index += 1;
            continue;
        }
        if in_code_block {
            current.push(line.to_string());
            index += 1;
            continue;
        }
        if is_markdown_table_start(&lines, index) {
            if current_tables >= max_tables && !current.is_empty() {
                batches.push(current.join("\n"));
                current = Vec::new();
                current_tables = 0;
            }
            current.push(line.to_string());
            current.push(lines[index + 1].to_string());
            index += 2;
            while index < lines.len() && lines[index].trim().contains('|') {
                current.push(lines[index].to_string());
                index += 1;
            }
            current_tables += 1;
            continue;
        }
        current.push(line.to_string());
        index += 1;
    }

    if !current.is_empty() || text.is_empty() {
        batches.push(current.join("\n"));
    }
    batches
}

/// 统计单个分批中的表格数量，适用于分批日志。
fn count_markdown_tables_in_batch(text: &str) -> usize {
    let lines = text.lines().collect::<Vec<_>>();
    let mut count = 0usize;
    let mut in_code_block = false;
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if is_markdown_fence(line) {
            in_code_block = !in_code_block;
            index += 1;
            continue;
        }
        if !in_code_block && is_markdown_table_start(&lines, index) {
            count += 1;
            index += 2;
            while index < lines.len() && lines[index].trim().contains('|') {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    count
}

/// 判断当前位置是否为 Markdown 管道表格起点。
fn is_markdown_table_start(lines: &[&str], index: usize) -> bool {
    index + 1 < lines.len()
        && lines[index].contains('|')
        && is_markdown_table_separator(lines[index + 1])
}

/// 判断 Markdown 代码块围栏，适用于避免误判代码块内表格。
fn is_markdown_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// 判断 Markdown 表格分隔行，适用于粗略识别 pipe table。
fn is_markdown_table_separator(line: &str) -> bool {
    let trimmed = line.trim().trim_matches('|');
    if trimmed.is_empty() || !trimmed.contains('|') {
        return false;
    }
    trimmed.split('|').all(|cell| {
        let cell = cell.trim();
        !cell.is_empty()
            && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ' | '\t'))
            && cell.chars().any(|ch| ch == '-')
    })
}

/// 记录飞书 API 响应摘要，适用于定位 HTTP 成功但业务失败。
fn log_feishu_api_response(action: &str, text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        crate::log_info!(
            "feishu {} api body unparsed preview={}",
            action,
            log_preview(text)
        );
        return;
    };
    let code = value.get("code").map(Value::to_string).unwrap_or_default();
    let msg = value.get("msg").and_then(Value::as_str).unwrap_or("");
    let error_field = find_feishu_detail(&value, "ErrorField")
        .or_else(|| find_feishu_key_string(&value, "error_field"))
        .unwrap_or_default();
    let error_value = find_feishu_detail(&value, "ErrorValue")
        .or_else(|| find_feishu_key_string(&value, "error_value"))
        .unwrap_or_default();
    crate::log_info!(
        "feishu {} api parsed code={} msg={} error_field={} error_value={} preview={}",
        action,
        code,
        msg,
        error_field,
        error_value,
        log_preview(&value.to_string())
    );
}

/// 从飞书 details 数组中读取 key/value，适用于结构化错误体。
fn find_feishu_detail(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            let matched = map
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|item| item.eq_ignore_ascii_case(key));
            if matched {
                return map.get("value").and_then(Value::as_str).map(str::to_string);
            }
            map.values().find_map(|item| find_feishu_detail(item, key))
        }
        Value::Array(items) => items.iter().find_map(|item| find_feishu_detail(item, key)),
        _ => None,
    }
}

/// 从嵌套 JSON 中读取字符串字段，适用于兼容不同错误字段命名。
fn find_feishu_key_string(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                map.values()
                    .find_map(|item| find_feishu_key_string(item, key))
            }),
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_feishu_key_string(item, key)),
        _ => None,
    }
}

/// 生成日志预览，避免飞书错误体刷屏。
fn log_preview(text: &str) -> String {
    const MAX_CHARS: usize = 160;
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut output = compact.chars().take(MAX_CHARS).collect::<String>();
    if compact.chars().count() > MAX_CHARS {
        output.push_str("...");
    }
    output
}

/// 构造飞书 2.0 markdown 卡片，适用于所有普通正文消息。
fn build_markdown_card(text: &str) -> Value {
    let content = compact_markdown_card_content(text);
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "update_multi": true,
        },
        "body": {
            "direction": "vertical",
            "padding": "2px 12px 2px 12px",
            "vertical_spacing": "2px",
            "elements": [{
                "tag": "markdown",
                "content": content,
            }],
        },
    })
}

/// 渲染飞书出站文本，适用于隔离飞书 markdown 的平台差异。
fn feishu_render_outbound_text(text: &str, format: OutboundFormat) -> String {
    if matches!(format, OutboundFormat::Plan) {
        return escape_feishu_plan_ordered_markers(text);
    }
    text.to_string()
}

/// 转义飞书计划编号，避免飞书 markdown 接管嵌套计划排版。
fn escape_feishu_plan_ordered_markers(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        output.push_str(&escape_feishu_plan_ordered_marker_line(line));
    }
    if !text.ends_with('\n') {
        return output;
    }
    output
}

/// 转义单行计划编号，适用于 `1.` 或 `1.2.` 开头的计划行。
fn escape_feishu_plan_ordered_marker_line(line: &str) -> String {
    let trimmed = line.trim_start_matches(' ');
    let indent_len = line.len().saturating_sub(trimmed.len());
    let Some(marker_len) = ordered_marker_len(trimmed) else {
        return line.to_string();
    };
    let marker = &trimmed[..marker_len];
    let rest = &trimmed[marker_len..];
    format!(
        "{}{}\\.{}",
        &line[..indent_len],
        &marker[..marker.len() - 1],
        rest
    )
}

/// 识别计划行有序编号长度，适用于飞书计划渲染前处理。
fn ordered_marker_len(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut saw_digit = false;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            saw_digit = true;
            index += 1;
            continue;
        }
        if bytes[index] == b'.' && saw_digit {
            if index + 1 < bytes.len() && bytes[index + 1] == b' ' {
                return Some(index + 1);
            }
            saw_digit = false;
            index += 1;
            continue;
        }
        return None;
    }
    None
}

/// 压紧飞书 markdown 卡片正文，适用于飞书空白行渲染间距过大的场景。
fn compact_markdown_card_content(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return " ".to_string();
    }
    let mut output = Vec::new();
    let mut in_code_block = false;
    for line in text.lines() {
        if is_markdown_fence(line) {
            in_code_block = !in_code_block;
            output.push(line.to_string());
            continue;
        }
        if !in_code_block && line.trim().is_empty() {
            continue;
        }
        output.push(line.to_string());
    }
    output.join("\n")
}

/// 下载后的图片。
pub struct DownloadedImage {
    /// MIME 类型。
    pub mime_type: String,
    /// 图片 bytes。
    pub bytes: Vec<u8>,
}

/// 下载后的文件。
pub struct DownloadedFile {
    /// MIME 类型。
    pub mime_type: String,
    /// 文件 bytes。
    pub bytes: Vec<u8>,
}

/// 飞书消息详情，来自获取指定消息内容接口。
#[derive(Debug, Clone, Deserialize)]
pub struct FeishuMessageDetail {
    /// 消息 id。
    pub message_id: String,
    /// 消息类型。
    pub msg_type: String,
    /// 上级合并转发消息 id。
    pub upper_message_id: Option<String>,
    /// 发送者。
    pub sender: Option<FeishuMessageSender>,
    /// 消息体。
    pub body: Option<FeishuMessageBody>,
}

/// 飞书消息发送者。
#[derive(Debug, Clone, Deserialize)]
pub struct FeishuMessageSender {
    /// 发送者 id。
    pub id: String,
    /// 发送者类型。
    #[serde(rename = "sender_type")]
    pub _sender_type: Option<String>,
}

/// 飞书消息体。
#[derive(Debug, Clone, Deserialize)]
pub struct FeishuMessageBody {
    /// 消息内容 JSON 字符串。
    pub content: String,
}

/// 飞书主动发送消息的收件人。
#[derive(Debug, Clone, Copy)]
pub struct FeishuRecipient<'a> {
    /// 收件人 id。
    pub id: &'a str,
    /// 收件人 id 类型。
    pub id_type: FeishuReceiveIdType,
}

/// 飞书发送消息接口的 receive_id_type。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeishuReceiveIdType {
    /// 群或会话 id。
    ChatId,
    /// 用户 open_id。
    OpenId,
    /// 用户 union_id。
    UnionId,
    /// 用户 user_id。
    UserId,
}

impl FeishuReceiveIdType {
    /// 返回飞书 API query 参数值。
    fn query_value(self) -> &'static str {
        match self {
            Self::ChatId => "chat_id",
            Self::OpenId => "open_id",
            Self::UnionId => "union_id",
            Self::UserId => "user_id",
        }
    }
}

/// 归一化图片 MIME 类型，飞书有时返回 octet-stream。
fn normalize_image_mime(raw: &str, bytes: &[u8]) -> String {
    let raw = raw.trim();
    if raw.starts_with("image/") {
        return raw.to_string();
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png".to_string();
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return "image/jpeg".to_string();
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif".to_string();
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp".to_string();
    }
    "image/png".to_string()
}

/// token 缓存。
struct CachedToken {
    /// token 值。
    value: String,
    /// 过期时间。
    expires_at: Instant,
}

/// WS endpoint 响应。
#[derive(Deserialize)]
struct EndpointResponse {
    /// 业务状态码。
    code: i32,
    /// 错误信息。
    msg: Option<String>,
    /// endpoint 数据。
    data: Option<EndpointData>,
}

/// WS endpoint 数据。
#[derive(Deserialize)]
struct EndpointData {
    /// WebSocket URL。
    #[serde(rename = "URL")]
    url: Option<String>,
}

/// tenant token 响应。
#[derive(Deserialize)]
struct TokenResponse {
    /// 业务状态码。
    code: i32,
    /// 错误信息。
    msg: Option<String>,
    /// tenant access token。
    tenant_access_token: String,
    /// 过期秒数。
    expire: Option<u64>,
}

/// 通用 API 响应。
#[derive(Deserialize)]
struct ApiResponse {
    /// 业务状态码。
    code: i32,
    /// 错误信息。
    msg: Option<String>,
    /// 数据。
    data: Option<MessageData>,
}

/// 获取消息内容响应。
#[derive(Deserialize)]
struct MessageContentResponse {
    /// 业务状态码。
    code: i32,
    /// 错误信息。
    msg: Option<String>,
    /// 消息详情数据。
    data: Option<MessageContentData>,
}

/// 获取消息内容数据。
#[derive(Deserialize)]
struct MessageContentData {
    /// 消息详情列表。
    #[serde(default)]
    items: Vec<FeishuMessageDetail>,
}

/// 机器人信息响应。
#[derive(Deserialize)]
struct BotInfoResponse {
    /// 业务状态码。
    code: i32,
    /// 错误信息。
    msg: Option<String>,
    /// 机器人信息。
    bot: Option<BotInfo>,
}

/// 机器人信息。
#[derive(Deserialize)]
struct BotInfo {
    /// 机器人 open_id。
    open_id: Option<String>,
}

/// 消息 API 数据。
#[derive(Deserialize)]
struct MessageData {
    /// 飞书消息 id。
    message_id: Option<String>,
    /// 飞书 reaction id。
    reaction_id: Option<String>,
}

#[cfg(test)]
#[path = "api_test.rs"]
mod api_test;
