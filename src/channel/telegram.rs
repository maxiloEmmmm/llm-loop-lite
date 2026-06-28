//! Telegram Bot API channel 实现。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::channel::attachments::{DownloadedAttachment, store_inbound_attachment};
use crate::channel::{Channel, ChannelAckCapability, ChannelAckKind, ChannelCapabilities};
use crate::config::{ChannelConfig, TelegramChannelConfig};
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::message::{
    InboundAttachment, InboundMessage, MessageSource, MessageUpdate, OutboundMessage, SendResult,
    UserInputRequest, UserInputResponse,
};
use crate::resource::{ResourceUsage, estimate_answers_bytes, estimate_user_input_request_bytes};

/// TG 收到消息后的确认 reaction，使用普通 emoji 避免自定义表情权限限制。
const TELEGRAM_RECEIVED_REACTION_EMOJI: &str = "\u{1F440}";
/// TG 命令完成后的确认 reaction，复用已确认可用的普通 emoji。
const TELEGRAM_DONE_REACTION_EMOJI: &str = "\u{1F440}";

/// Telegram Bot API channel，负责 polling 收消息和 REST 发消息。
pub struct TelegramChannel {
    /// channel 实例名。
    name: String,
    /// Telegram REST API。
    api: Arc<TelegramApi>,
    /// Telegram bot username，不带 @。
    bot_username: Arc<Mutex<Option<String>>>,
    /// polling 配置。
    polling: TelegramPollingConfig,
    /// 消息处理配置。
    behavior: TelegramBehaviorConfig,
    /// 等待中的用户输入请求。
    user_inputs: Arc<Mutex<TelegramPendingUserInputs>>,
    /// 后台 polling 任务。
    task: Option<JoinHandle<()>>,
}

/// Telegram channel 的轻量工具句柄。
#[derive(Clone)]
pub struct TelegramChannelHandle {
    /// channel 实例名。
    name: String,
    /// Telegram REST API。
    api: Arc<TelegramApi>,
    /// 等待中的用户输入请求。
    user_inputs: Arc<Mutex<TelegramPendingUserInputs>>,
    /// 是否启用收到消息确认。
    received_ack_enabled: bool,
    /// 是否下载入站附件。
    download_attachments: bool,
}

/// Telegram polling 配置。
#[derive(Debug, Clone)]
struct TelegramPollingConfig {
    /// 启动 polling 前是否删除 webhook。
    delete_webhook_on_start: bool,
    /// long polling 等待秒数。
    poll_timeout_seconds: u64,
    /// 单次 getUpdates 最大条数。
    poll_limit: u32,
}

/// Telegram 消息行为配置。
#[derive(Debug, Clone)]
struct TelegramBehaviorConfig {
    /// 群聊是否要求 @ 机器人。
    require_mention: bool,
    /// 是否在收到消息后发送 typing。
    send_typing: bool,
    /// 是否下载入站附件。
    download_attachments: bool,
    /// 附件最大下载字节数。
    max_download_bytes: u64,
}

/// 解析后的 Telegram 配置。
struct ResolvedTelegramConfig {
    /// BotFather 下发的 bot token。
    bot_token: String,
    /// Telegram Bot API 基础地址。
    api_base_url: String,
    /// Telegram 文件下载基础地址。
    file_base_url: String,
}

/// Telegram REST API 封装。
struct TelegramApi {
    /// BotFather 下发的 bot token。
    bot_token: String,
    /// Telegram Bot API 基础地址。
    api_base_url: String,
    /// Telegram 文件下载基础地址。
    file_base_url: String,
    /// HTTP 客户端。
    client: reqwest::Client,
}

/// Telegram 等待中的用户输入。
#[derive(Default)]
struct TelegramPendingUserInputs {
    /// 请求 id 到等待项。
    requests: HashMap<String, TelegramPendingUserInput>,
}

/// Telegram 单个等待中的用户输入。
struct TelegramPendingUserInput {
    /// 原始问题。
    request: UserInputRequest,
    /// 已收集答案。
    answers: HashMap<String, Vec<String>>,
    /// 完成通知。
    sender: Option<oneshot::Sender<UserInputResponse>>,
}

/// Telegram Bot API 响应包。
#[derive(Debug, Deserialize)]
struct TelegramApiResponse<T> {
    /// 请求是否成功。
    ok: bool,
    /// 成功结果。
    result: Option<T>,
    /// 失败说明。
    description: Option<String>,
    /// 失败错误码。
    error_code: Option<i64>,
}

/// Telegram getMe 用户。
#[derive(Debug, Clone, Deserialize)]
struct TelegramUser {
    /// 用户 id。
    id: i64,
    /// 是否为 bot。
    is_bot: bool,
    /// 名字。
    first_name: String,
    /// username，不带 @。
    username: Option<String>,
    /// bot 是否可加入群。
    can_join_groups: Option<bool>,
    /// bot 是否可读所有群消息。
    can_read_all_group_messages: Option<bool>,
}

/// Telegram Update。
#[derive(Debug, Clone, Deserialize)]
struct TelegramUpdate {
    /// update id。
    update_id: i64,
    /// 普通新消息。
    message: Option<TelegramMessage>,
    /// 编辑后的普通消息。
    edited_message: Option<TelegramMessage>,
    /// channel 新消息。
    channel_post: Option<TelegramMessage>,
    /// 编辑后的 channel 消息。
    edited_channel_post: Option<TelegramMessage>,
    /// inline keyboard 回调。
    callback_query: Option<TelegramCallbackQuery>,
    /// 其他字段，保留日志观测。
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

/// Telegram Message。
#[derive(Debug, Clone, Deserialize)]
struct TelegramMessage {
    /// 消息 id。
    message_id: i64,
    /// forum topic id。
    message_thread_id: Option<i64>,
    /// 发送者。
    from: Option<TelegramUser>,
    /// 代表某个 chat 发送的身份。
    sender_chat: Option<TelegramChat>,
    /// 所在 chat。
    chat: TelegramChat,
    /// 文本。
    text: Option<String>,
    /// 媒体 caption。
    caption: Option<String>,
    /// 图片尺寸列表。
    photo: Option<Vec<TelegramPhotoSize>>,
    /// 通用文件。
    document: Option<TelegramFileMeta>,
    /// 音频。
    audio: Option<TelegramFileMeta>,
    /// 视频。
    video: Option<TelegramFileMeta>,
    /// 语音。
    voice: Option<TelegramFileMeta>,
    /// 动图。
    animation: Option<TelegramFileMeta>,
}

/// Telegram Chat。
#[derive(Debug, Clone, Deserialize)]
struct TelegramChat {
    /// chat id。
    id: i64,
    /// chat 类型。
    #[serde(rename = "type")]
    kind: String,
    /// 群或频道标题。
    #[serde(rename = "title")]
    _title: Option<String>,
    /// username，不带 @。
    #[serde(rename = "username")]
    _username: Option<String>,
}

/// Telegram 文件元数据。
#[derive(Debug, Clone, Deserialize)]
struct TelegramFileMeta {
    /// 可下载文件 id。
    file_id: String,
    /// 跨 bot 稳定但不可下载的 id。
    #[serde(rename = "file_unique_id")]
    _file_unique_id: String,
    /// 文件名。
    file_name: Option<String>,
    /// MIME 类型。
    mime_type: Option<String>,
    /// 文件大小。
    #[serde(rename = "file_size")]
    _file_size: Option<u64>,
}

/// Telegram 图片尺寸。
#[derive(Debug, Clone, Deserialize)]
struct TelegramPhotoSize {
    /// 可下载文件 id。
    file_id: String,
    /// 跨 bot 稳定但不可下载的 id。
    #[serde(rename = "file_unique_id")]
    _file_unique_id: String,
    /// 宽度。
    width: u32,
    /// 高度。
    height: u32,
    /// 文件大小。
    #[serde(rename = "file_size")]
    _file_size: Option<u64>,
}

/// Telegram getFile 结果。
#[derive(Debug, Clone, Deserialize)]
struct TelegramFile {
    /// 可下载文件 id。
    #[serde(rename = "file_id")]
    _file_id: String,
    /// 跨 bot 稳定但不可下载的 id。
    file_unique_id: String,
    /// 文件大小。
    file_size: Option<u64>,
    /// 文件路径。
    file_path: Option<String>,
}

/// Telegram callback query。
#[derive(Debug, Clone, Deserialize)]
struct TelegramCallbackQuery {
    /// callback query id。
    id: String,
    /// 点击用户。
    from: TelegramUser,
    /// 按钮数据。
    data: Option<String>,
    /// 关联消息。
    message: Option<TelegramMessage>,
}

/// Telegram sendMessage 返回消息。
#[derive(Debug, Clone, Deserialize)]
struct TelegramSentMessage {
    /// 消息 id。
    message_id: i64,
}

impl TelegramChannel {
    /// 从 channel 配置创建 Telegram channel。
    pub fn from_config(config: &ChannelConfig) -> AppResult<Self> {
        let settings = ResolvedTelegramConfig::resolve(&config.telegram)?;
        let api = Arc::new(TelegramApi::new(
            settings.bot_token,
            settings.api_base_url,
            settings.file_base_url,
        ));
        Ok(Self {
            name: config.name.clone(),
            api,
            bot_username: Arc::new(Mutex::new(None)),
            polling: TelegramPollingConfig {
                delete_webhook_on_start: config.telegram.delete_webhook_on_start,
                poll_timeout_seconds: config.telegram.poll_timeout_seconds.max(1),
                poll_limit: config.telegram.poll_limit.clamp(1, 100),
            },
            behavior: TelegramBehaviorConfig {
                require_mention: config.telegram.require_mention,
                send_typing: config.telegram.send_typing,
                download_attachments: config.telegram.download_attachments,
                max_download_bytes: config.telegram.max_download_bytes,
            },
            user_inputs: Arc::new(Mutex::new(TelegramPendingUserInputs::default())),
            task: None,
        })
    }

    /// 返回轻量工具句柄，适用于 tool 调用时发送消息。
    pub fn tool_handle(&self) -> TelegramChannelHandle {
        TelegramChannelHandle {
            name: self.name.clone(),
            api: Arc::clone(&self.api),
            user_inputs: Arc::clone(&self.user_inputs),
            received_ack_enabled: self.behavior.send_typing,
            download_attachments: self.behavior.download_attachments,
        }
    }

    /// 返回平台名称，适用于 cron 构造虚拟来源。
    pub fn platform_name(&self) -> &str {
        self.api.platform_name()
    }

    /// 返回 channel 名称。
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl TelegramChannelHandle {
    /// 返回 channel 名称。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 返回平台名称，适用于 cron 构造虚拟来源。
    pub fn platform_name(&self) -> &str {
        self.api.platform_name()
    }

    /// 返回 Telegram 确认能力，适用于轻量句柄分派。
    pub fn ack_capability(&self, kind: ChannelAckKind) -> ChannelAckCapability {
        match kind {
            ChannelAckKind::Received if self.received_ack_enabled => ChannelAckCapability::Reaction,
            ChannelAckKind::Received => ChannelAckCapability::None,
            ChannelAckKind::ResetDone => ChannelAckCapability::Reaction,
            ChannelAckKind::StopDone => ChannelAckCapability::Reaction,
        }
    }

    /// 返回 Telegram 能力集合，适用于轻量句柄暴露运行态能力。
    pub fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            patch_message: true,
            append_update: false,
            request_user_input: true,
            reaction_ack: true,
            text_ack: true,
            chat_action: self.received_ack_enabled,
            reply_threading: true,
            inbound_attachments: self.download_attachments,
            outbound_attachments: false,
        }
    }

    /// 返回 Telegram 句柄持有的缓存资源估算。
    pub async fn resource_usage(&self) -> Vec<ResourceUsage> {
        vec![
            self.user_inputs
                .lock()
                .await
                .resource_usage("channel.telegram.pending_inputs"),
        ]
    }

    /// 按统一语义执行 Telegram 确认反馈。
    pub async fn acknowledge(
        &self,
        message: &InboundMessage,
        kind: ChannelAckKind,
    ) -> AppResult<()> {
        match kind {
            ChannelAckKind::Received if self.received_ack_enabled => {
                self.acknowledge_received(message).await
            }
            ChannelAckKind::Received => Ok(()),
            ChannelAckKind::ResetDone => self.acknowledge_reset(message).await,
            ChannelAckKind::StopDone => self.acknowledge_stop(message).await,
        }
    }

    /// 发送 Telegram 消息。
    pub async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        let OutboundMessage {
            channel_name: _,
            chat_id,
            recipient: _,
            text,
            reply_to,
            thread_id,
            format: _,
        } = message;
        let message_thread_id = thread_id.as_deref().and_then(parse_i64);
        let sent = self
            .api
            .send_message(
                &chat_id,
                &text,
                reply_to.as_deref(),
                message_thread_id,
                None,
            )
            .await?;
        let message_id = sent.message_id.to_string();
        Ok(SendResult {
            success: true,
            message_id: Some(message_id),
        })
    }

    /// 更新 Telegram 消息。
    pub async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        let chat_id = message.chat_id.as_deref().ok_or_else(|| {
            AppError::Channel(format!(
                "telegram update chat_id is missing for message_id={}",
                message.message_id
            ))
        })?;
        let message_id = parse_message_id(&message.message_id)?;
        self.api
            .edit_message_text(chat_id, message_id, &message.text)
            .await?;
        Ok(SendResult {
            success: true,
            message_id: Some(message.message_id),
        })
    }

    /// 通过 Telegram inline keyboard 请求结构化输入。
    pub async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        let request_id = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = oneshot::channel();
        {
            self.user_inputs
                .lock()
                .await
                .insert(request_id.clone(), request.clone(), tx);
        }
        let reply_markup = build_user_input_keyboard(&request_id, &request);
        if let Err(err) = self
            .api
            .send_message(
                &source.chat_id,
                &build_user_input_text(&request),
                None,
                source.thread_id.as_deref().and_then(parse_i64),
                Some(reply_markup),
            )
            .await
        {
            self.user_inputs.lock().await.remove(&request_id);
            return Err(err);
        }
        if let Some(timeout_ms) = request.auto_resolution_ms {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(_)) => Err(AppError::Channel(
                    "telegram user input waiter was closed".to_string(),
                )),
                Err(_) => self.user_inputs.lock().await.auto_resolve(&request_id),
            }
        } else {
            rx.await
                .map_err(|_| AppError::Channel("telegram user input waiter was closed".to_string()))
        }
    }

    /// 收到入站消息后优先添加 reaction，失败时退回 typing。
    pub async fn acknowledge_received(&self, message: &InboundMessage) -> AppResult<()> {
        let thread_id = message.source.thread_id.as_deref().and_then(parse_i64);
        if let Some(message_id) = message.message_id.as_deref().and_then(parse_i64) {
            match self
                .api
                .set_message_reaction(
                    &message.source.chat_id,
                    message_id,
                    TELEGRAM_RECEIVED_REACTION_EMOJI,
                    true,
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    // 触发条件：部分服务消息或群权限可能禁止 bot 添加 reaction。
                    // 不能直接失败返回：用户会失去“已收到”的即时反馈。
                    // 防止回归：保留 typing 作为 TG 的最低可用确认路径。
                    crate::log_info!("telegram reaction acknowledge failed: {err}");
                }
            }
        }
        self.api
            .send_chat_action(&message.source.chat_id, "typing", thread_id)
            .await
    }

    /// reset 后优先添加 reaction，失败时退回短文本确认。
    pub async fn acknowledge_reset(&self, message: &InboundMessage) -> AppResult<()> {
        let thread_id = message.source.thread_id.as_deref().and_then(parse_i64);
        if let Some(message_id) = message.message_id.as_deref().and_then(parse_i64) {
            match self
                .api
                .set_message_reaction(
                    &message.source.chat_id,
                    message_id,
                    TELEGRAM_RECEIVED_REACTION_EMOJI,
                    true,
                )
                .await
            {
                Ok(()) => return Ok(()),
                Err(err) => {
                    // 触发条件：某些 TG 消息或群权限不允许 bot 添加 reaction。
                    // 不能直接跳过确认：reset 是破坏性操作，用户需要即时反馈。
                    // 防止回归：reaction 不可用时仍回复短文本确认。
                    crate::log_info!("telegram reset reaction acknowledge failed: {err}");
                }
            }
        }
        self.api
            .send_message(
                &message.source.chat_id,
                "已重置。",
                message.message_id.as_deref(),
                thread_id,
                None,
            )
            .await?;
        Ok(())
    }

    /// stop 完成后添加 reaction，适用于 TG 支持消息附属表情的场景。
    pub async fn acknowledge_stop(&self, message: &InboundMessage) -> AppResult<()> {
        if let Some(message_id) = message.message_id.as_deref().and_then(parse_i64) {
            self.api
                .set_message_reaction(
                    &message.source.chat_id,
                    message_id,
                    TELEGRAM_DONE_REACTION_EMOJI,
                    true,
                )
                .await?;
        }
        Ok(())
    }
}

impl Channel for TelegramChannel {
    /// 返回 channel 名称。
    fn name(&self) -> &str {
        &self.name
    }

    /// 启动 Telegram polling 后台任务。
    async fn start(&mut self, tx: mpsc::Sender<InboundMessage>, paths: &AppPaths) -> AppResult<()> {
        let api = Arc::clone(&self.api);
        let name = self.name.clone();
        let polling = self.polling.clone();
        let behavior = self.behavior.clone();
        let user_inputs = Arc::clone(&self.user_inputs);
        let bot_username = Arc::clone(&self.bot_username);
        let offset_path = telegram_offset_path(paths, &name);
        let store_root = paths.channel_store_dir.clone();
        self.task = Some(tokio::spawn(async move {
            loop {
                match run_telegram_polling(
                    Arc::clone(&api),
                    tx.clone(),
                    name.clone(),
                    polling.clone(),
                    behavior.clone(),
                    Arc::clone(&user_inputs),
                    Arc::clone(&bot_username),
                    offset_path.clone(),
                    store_root.clone(),
                )
                .await
                {
                    Ok(()) => crate::log_info!("telegram polling stopped, restarting"),
                    Err(err) => crate::log_info!("telegram polling error: {err}; restarting"),
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }));
        Ok(())
    }

    /// 停止 Telegram polling 后台任务。
    async fn stop(&mut self) -> AppResult<()> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.user_inputs.lock().await.clear();
        Ok(())
    }

    /// 发送 Telegram 出站消息。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        self.tool_handle().send(message).await
    }

    /// 更新 Telegram 出站消息。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        self.tool_handle().update_message(message).await
    }

    /// 返回 Telegram 确认能力，适用于 daemon 统一确认语义。
    fn ack_capability(&self, kind: ChannelAckKind) -> ChannelAckCapability {
        match kind {
            ChannelAckKind::Received if self.behavior.send_typing => ChannelAckCapability::Reaction,
            ChannelAckKind::Received => ChannelAckCapability::None,
            ChannelAckKind::ResetDone => ChannelAckCapability::Reaction,
            ChannelAckKind::StopDone => ChannelAckCapability::Reaction,
        }
    }

    /// 返回 Telegram 能力集合，适用于上层按平台能力选择接口。
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            patch_message: true,
            append_update: false,
            request_user_input: true,
            reaction_ack: true,
            text_ack: true,
            chat_action: self.behavior.send_typing,
            reply_threading: true,
            inbound_attachments: self.behavior.download_attachments,
            outbound_attachments: false,
        }
    }

    /// 按统一语义执行 Telegram 确认反馈。
    async fn acknowledge(&self, message: &InboundMessage, kind: ChannelAckKind) -> AppResult<()> {
        match kind {
            ChannelAckKind::Received if self.behavior.send_typing => {
                self.tool_handle().acknowledge_received(message).await
            }
            ChannelAckKind::Received => Ok(()),
            ChannelAckKind::ResetDone => self.tool_handle().acknowledge_reset(message).await,
            ChannelAckKind::StopDone => self.tool_handle().acknowledge_stop(message).await,
        }
    }

    /// 通过 Telegram inline keyboard 请求结构化输入。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        self.tool_handle().request_user_input(source, request).await
    }
}

impl ResolvedTelegramConfig {
    /// 解析 Telegram 配置，适用于启动前读取 token。
    fn resolve(config: &TelegramChannelConfig) -> AppResult<Self> {
        let bot_token = config
            .bot_token
            .clone()
            .or_else(|| {
                config
                    .bot_token_env
                    .as_ref()
                    .and_then(|key| std::env::var(key).ok())
            })
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Channel("telegram bot token is required".to_string()))?;
        Ok(Self {
            bot_token,
            api_base_url: config.api_base_url.clone(),
            file_base_url: config.file_base_url.clone(),
        })
    }
}

impl TelegramApi {
    /// 创建 Telegram API 客户端。
    fn new(bot_token: String, api_base_url: String, file_base_url: String) -> Self {
        Self {
            bot_token,
            api_base_url,
            file_base_url,
            client: reqwest::Client::new(),
        }
    }

    /// 返回平台名称。
    fn platform_name(&self) -> &str {
        "telegram"
    }

    /// 调用 getMe。
    async fn get_me(&self) -> AppResult<TelegramUser> {
        self.post("getMe", json!({})).await
    }

    /// 删除 webhook，适用于切换到 polling。
    async fn delete_webhook(&self) -> AppResult<bool> {
        self.post("deleteWebhook", json!({ "drop_pending_updates": false }))
            .await
    }

    /// 调用 getUpdates。
    async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout: u64,
        limit: u32,
    ) -> AppResult<Vec<TelegramUpdate>> {
        let mut body = json!({
            "timeout": timeout,
            "limit": limit,
            "allowed_updates": telegram_allowed_updates(),
        });
        if let Some(offset) = offset {
            body["offset"] = Value::Number(offset.into());
        }
        self.post("getUpdates", body).await
    }

    /// 发送文本消息。
    async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<&str>,
        message_thread_id: Option<i64>,
        reply_markup: Option<Value>,
    ) -> AppResult<TelegramSentMessage> {
        let chunks = split_telegram_text(text);
        let mut last = None;
        for (index, chunk) in chunks.iter().enumerate() {
            let mut body = json!({
                "chat_id": chat_id,
                "text": chunk,
                "disable_web_page_preview": true,
            });
            if let Some(message_thread_id) = message_thread_id {
                body["message_thread_id"] = Value::Number(message_thread_id.into());
            }
            if index == 0
                && let Some(reply_to) = reply_to.and_then(parse_i64)
            {
                body["reply_parameters"] = json!({ "message_id": reply_to });
            }
            if index == 0
                && let Some(reply_markup) = reply_markup.clone()
            {
                body["reply_markup"] = reply_markup;
            }
            last = Some(self.post("sendMessage", body).await?);
        }
        last.ok_or_else(|| {
            AppError::Channel("telegram sendMessage produced no message".to_string())
        })
    }

    /// 编辑文本消息。
    async fn edit_message_text(&self, chat_id: &str, message_id: i64, text: &str) -> AppResult<()> {
        let chunk = split_telegram_text(text)
            .into_iter()
            .next()
            .unwrap_or_else(|| " ".to_string());
        let _: Value = self
            .post(
                "editMessageText",
                json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "text": chunk,
                    "disable_web_page_preview": true,
                }),
            )
            .await?;
        Ok(())
    }

    /// 发送 chat action。
    async fn send_chat_action(
        &self,
        chat_id: &str,
        action: &str,
        message_thread_id: Option<i64>,
    ) -> AppResult<()> {
        let mut body = json!({
            "chat_id": chat_id,
            "action": action,
        });
        if let Some(message_thread_id) = message_thread_id {
            body["message_thread_id"] = Value::Number(message_thread_id.into());
        }
        let _: bool = self.post("sendChatAction", body).await?;
        Ok(())
    }

    /// 给消息设置 reaction，适用于收到消息后的即时确认。
    async fn set_message_reaction(
        &self,
        chat_id: &str,
        message_id: i64,
        emoji: &str,
        is_big: bool,
    ) -> AppResult<()> {
        let _: bool = self
            .post(
                "setMessageReaction",
                build_message_reaction_body(chat_id, message_id, emoji, is_big),
            )
            .await?;
        Ok(())
    }

    /// 回应 callback query。
    async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> AppResult<()> {
        let mut body = json!({ "callback_query_id": callback_query_id });
        if let Some(text) = text {
            body["text"] = Value::String(text.to_string());
        }
        let _: bool = self.post("answerCallbackQuery", body).await?;
        Ok(())
    }

    /// 获取文件元数据。
    async fn get_file(&self, file_id: &str) -> AppResult<TelegramFile> {
        self.post("getFile", json!({ "file_id": file_id })).await
    }

    /// 下载文件 bytes。
    async fn download_file(&self, file_path: &str, max_bytes: u64) -> AppResult<Vec<u8>> {
        let url = format!(
            "{}/bot{}/{}",
            self.file_base_url.trim_end_matches('/'),
            self.bot_token,
            file_path.trim_start_matches('/')
        );
        let response = self.client.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::Channel(format!(
                "telegram file download failed with {status}: {text}"
            )));
        }
        let bytes = response.bytes().await?;
        if bytes.len() as u64 > max_bytes {
            return Err(AppError::Channel(format!(
                "telegram file exceeds max_download_bytes: {} > {}",
                bytes.len(),
                max_bytes
            )));
        }
        Ok(bytes.to_vec())
    }

    /// 调用 Telegram Bot API。
    async fn post<T>(&self, method: &str, body: Value) -> AppResult<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let url = format!(
            "{}/bot{}/{}",
            self.api_base_url.trim_end_matches('/'),
            self.bot_token,
            method
        );
        crate::log_info!("telegram api start method={}", method);
        let response = self.client.post(url).json(&body).send().await?;
        let status = response.status();
        let text = response.text().await?;
        crate::log_info!(
            "telegram api response method={} status={} bytes={}",
            method,
            status,
            text.len()
        );
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "telegram api {method} failed with {status}: {text}"
            )));
        }
        let parsed: TelegramApiResponse<T> = serde_json::from_str(&text)?;
        if !parsed.ok {
            return Err(AppError::Channel(format!(
                "telegram api {method} error code={:?} desc={}",
                parsed.error_code,
                parsed.description.unwrap_or_default()
            )));
        }
        parsed
            .result
            .ok_or_else(|| AppError::Channel(format!("telegram api {method} missing result")))
    }
}

impl TelegramPendingUserInputs {
    /// 插入等待中的用户输入请求。
    fn insert(
        &mut self,
        request_id: String,
        request: UserInputRequest,
        sender: oneshot::Sender<UserInputResponse>,
    ) {
        self.requests.insert(
            request_id,
            TelegramPendingUserInput {
                request,
                answers: HashMap::new(),
                sender: Some(sender),
            },
        );
    }

    /// 移除等待中的用户输入请求。
    fn remove(&mut self, request_id: &str) {
        self.requests.remove(request_id);
    }

    /// 清空等待中的用户输入请求。
    fn clear(&mut self) {
        self.requests.clear();
    }

    /// 返回等待输入集合资源估算。
    fn resource_usage(&self, name: &str) -> ResourceUsage {
        let body_bytes = self
            .requests
            .iter()
            .map(|(key, pending)| key.capacity().saturating_add(pending.resource_bytes()))
            .sum::<usize>();
        let entry_bytes = self
            .requests
            .capacity()
            .saturating_mul(std::mem::size_of::<(String, TelegramPendingUserInput)>());
        ResourceUsage::new(
            name,
            "hashmap",
            self.requests.len(),
            Some(self.requests.capacity()),
            entry_bytes.saturating_add(body_bytes),
        )
    }

    /// 处理 callback 选择。
    fn answer_callback(
        &mut self,
        request_id: &str,
        question_index: usize,
        option_index: usize,
    ) -> AppResult<Option<UserInputResponse>> {
        let Some(pending) = self.requests.get_mut(request_id) else {
            return Ok(None);
        };
        let Some(question) = pending.request.questions.get(question_index) else {
            return Err(AppError::Channel(
                "telegram callback question index invalid".to_string(),
            ));
        };
        let Some(option) = question.options.get(option_index) else {
            return Err(AppError::Channel(
                "telegram callback option index invalid".to_string(),
            ));
        };
        pending
            .answers
            .insert(question.id.clone(), vec![option.label.clone()]);
        if pending.answers.len() < pending.request.questions.len() {
            return Ok(None);
        }
        let response = UserInputResponse {
            answers: pending.answers.clone(),
        };
        if let Some(sender) = pending.sender.take() {
            let _ = sender.send(response.clone());
        }
        self.requests.remove(request_id);
        Ok(Some(response))
    }

    /// 自动选择推荐项，适用于非阻塞用户输入超时。
    fn auto_resolve(&mut self, request_id: &str) -> AppResult<UserInputResponse> {
        let Some(pending) = self.requests.remove(request_id) else {
            return Err(AppError::Channel(
                "telegram user input request missing".to_string(),
            ));
        };
        let mut answers = pending.answers;
        for question in pending.request.questions {
            answers.entry(question.id).or_insert_with(|| {
                question
                    .options
                    .first()
                    .map(|option| vec![option.label.clone()])
                    .unwrap_or_default()
            });
        }
        Ok(UserInputResponse { answers })
    }
}

impl TelegramPendingUserInput {
    /// 估算单个等待输入项容量。
    fn resource_bytes(&self) -> usize {
        estimate_user_input_request_bytes(&self.request) + estimate_answers_bytes(&self.answers)
    }
}

/// 运行 Telegram polling 主循环。
async fn run_telegram_polling(
    api: Arc<TelegramApi>,
    tx: mpsc::Sender<InboundMessage>,
    channel_name: String,
    polling: TelegramPollingConfig,
    behavior: TelegramBehaviorConfig,
    user_inputs: Arc<Mutex<TelegramPendingUserInputs>>,
    bot_username: Arc<Mutex<Option<String>>>,
    offset_path: PathBuf,
    store_root: PathBuf,
) -> AppResult<()> {
    if polling.delete_webhook_on_start {
        let _ = api.delete_webhook().await?;
    }
    let me = api.get_me().await?;
    crate::log_info!(
        "telegram getMe id={} username={} is_bot={} can_join_groups={:?} can_read_all_group_messages={:?} first_name={}",
        me.id,
        me.username.as_deref().unwrap_or(""),
        me.is_bot,
        me.can_join_groups,
        me.can_read_all_group_messages,
        me.first_name
    );
    *bot_username.lock().await = me.username.clone();
    tokio::fs::create_dir_all(&store_root).await?;
    let mut offset = read_offset(&offset_path).await?;
    loop {
        let updates = api
            .get_updates(offset, polling.poll_timeout_seconds, polling.poll_limit)
            .await?;
        for update in updates {
            offset = Some(update.update_id.saturating_add(1));
            write_offset(&offset_path, offset).await?;
            handle_update(
                Arc::clone(&api),
                tx.clone(),
                &channel_name,
                &behavior,
                &store_root,
                &bot_username.lock().await.clone(),
                Arc::clone(&user_inputs),
                update,
            )
            .await?;
        }
    }
}

/// 处理单个 Telegram update。
async fn handle_update(
    api: Arc<TelegramApi>,
    tx: mpsc::Sender<InboundMessage>,
    channel_name: &str,
    behavior: &TelegramBehaviorConfig,
    store_root: &Path,
    bot_username: &Option<String>,
    user_inputs: Arc<Mutex<TelegramPendingUserInputs>>,
    update: TelegramUpdate,
) -> AppResult<()> {
    if let Some(callback) = update.callback_query {
        handle_callback_query(api, user_inputs, callback).await?;
        return Ok(());
    }
    let message = update
        .message
        .or(update.edited_message)
        .or(update.channel_post)
        .or(update.edited_channel_post);
    if let Some(message) = message {
        if let Some(inbound) = telegram_message_to_inbound(
            api,
            channel_name,
            behavior,
            store_root,
            bot_username,
            message,
        )
        .await?
        {
            tx.send(inbound)
                .await
                .map_err(|_| AppError::Channel("telegram inbound queue closed".to_string()))?;
        }
        return Ok(());
    }
    if !update.extra.is_empty() {
        crate::log_info!(
            "telegram update ignored update_id={} keys={}",
            update.update_id,
            update.extra.keys().cloned().collect::<Vec<_>>().join(",")
        );
    }
    Ok(())
}

/// 处理 Telegram callback query。
async fn handle_callback_query(
    api: Arc<TelegramApi>,
    user_inputs: Arc<Mutex<TelegramPendingUserInputs>>,
    callback: TelegramCallbackQuery,
) -> AppResult<()> {
    let Some(data) = callback.data.as_deref() else {
        api.answer_callback_query(&callback.id, None).await?;
        return Ok(());
    };
    if let Some((request_id, question_index, option_index)) = parse_user_input_callback(data) {
        let response =
            user_inputs
                .lock()
                .await
                .answer_callback(request_id, question_index, option_index)?;
        let text = if response.is_some() {
            "已选择"
        } else {
            "已记录"
        };
        api.answer_callback_query(&callback.id, Some(text)).await?;
        return Ok(());
    }
    api.answer_callback_query(&callback.id, None).await?;
    crate::log_info!(
        "telegram callback ignored id={} user_id={} has_message={} data={}",
        callback.id,
        callback.from.id,
        callback.message.is_some(),
        data
    );
    Ok(())
}

/// 将 Telegram message 转成标准入站消息。
async fn telegram_message_to_inbound(
    api: Arc<TelegramApi>,
    channel_name: &str,
    behavior: &TelegramBehaviorConfig,
    store_root: &Path,
    bot_username: &Option<String>,
    message: TelegramMessage,
) -> AppResult<Option<InboundMessage>> {
    let mut text = message
        .text
        .clone()
        .or_else(|| message.caption.clone())
        .unwrap_or_default();
    if is_group_chat(&message.chat.kind) && behavior.require_mention {
        let Some(username) = bot_username.as_deref() else {
            return Ok(None);
        };
        let Some(cleaned) = strip_bot_mention(&text, username) else {
            return Ok(None);
        };
        text = cleaned;
    }
    let source = telegram_source(channel_name, &message);
    let mut inbound = InboundMessage::text(
        if text.trim().is_empty() {
            "用户发送了附件".to_string()
        } else {
            text
        },
        source,
        Some(message.message_id.to_string()),
    );
    if behavior.download_attachments {
        attach_telegram_files(
            api,
            store_root,
            behavior.max_download_bytes,
            &message,
            &mut inbound,
        )
        .await?;
    }
    if inbound.text.trim().is_empty() && inbound.attachments.is_empty() {
        return Ok(None);
    }
    crate::log_info!(
        "telegram inbound accepted chat_id={} chat_type={} user_id={} message_id={} attachments={}",
        inbound.source.chat_id,
        inbound.source.chat_type,
        inbound.source.user_id.as_deref().unwrap_or(""),
        inbound.message_id.as_deref().unwrap_or(""),
        inbound.attachments.len()
    );
    Ok(Some(inbound))
}

/// 从 Telegram message 构造标准来源。
fn telegram_source(channel_name: &str, message: &TelegramMessage) -> MessageSource {
    let user_id = message
        .from
        .as_ref()
        .map(|user| user.id.to_string())
        .or_else(|| message.sender_chat.as_ref().map(|chat| chat.id.to_string()));
    MessageSource {
        channel_name: channel_name.to_string(),
        platform: "telegram".to_string(),
        chat_id: message.chat.id.to_string(),
        chat_type: telegram_chat_type(&message.chat.kind).to_string(),
        user_id,
        thread_id: message.message_thread_id.map(|value| value.to_string()),
    }
}

/// 下载并挂载 Telegram 附件。
async fn attach_telegram_files(
    api: Arc<TelegramApi>,
    store_root: &Path,
    max_download_bytes: u64,
    message: &TelegramMessage,
    inbound: &mut InboundMessage,
) -> AppResult<()> {
    if let Some(photo) = message.photo.as_ref().and_then(|photos| best_photo(photos)) {
        let bytes =
            download_telegram_file(api.as_ref(), &photo.file_id, max_download_bytes).await?;
        let filename = format!("{}.jpg", photo._file_unique_id);
        match store_inbound_attachment(
            store_root,
            &inbound.source,
            DownloadedAttachment {
                filename: &filename,
                mime_type: "image/jpeg",
                bytes: &bytes,
            },
        )
        .await
        {
            Ok(stored) => inbound.attachments.push(stored),
            Err(err) => {
                crate::log_info!(
                    "telegram photo store failed file_id={} error={}",
                    photo.file_id,
                    err
                );
            }
        }
    }
    for meta in [
        message.document.as_ref(),
        message.audio.as_ref(),
        message.video.as_ref(),
        message.voice.as_ref(),
        message.animation.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(size) = meta._file_size
            && size > max_download_bytes
        {
            inbound.attachments.push(InboundAttachment::StoredFile {
                path: PathBuf::from("<telegram-file-too-large>"),
                filename: meta
                    .file_name
                    .clone()
                    .unwrap_or_else(|| meta._file_unique_id.clone()),
                mime_type: meta
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                size,
            });
            continue;
        }
        let file = api.get_file(&meta.file_id).await?;
        let Some(file_path) = file.file_path.as_deref() else {
            continue;
        };
        let bytes = api.download_file(file_path, max_download_bytes).await?;
        let filename = meta
            .file_name
            .clone()
            .unwrap_or_else(|| telegram_download_filename(&file, file_path));
        let mime_type = meta
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        match store_inbound_attachment(
            store_root,
            &inbound.source,
            DownloadedAttachment {
                filename: &filename,
                mime_type: &mime_type,
                bytes: &bytes,
            },
        )
        .await
        {
            Ok(stored) => inbound.attachments.push(stored),
            Err(err) => {
                crate::log_info!(
                    "telegram file store failed file_id={} filename={} error={}",
                    meta.file_id,
                    filename,
                    err
                );
            }
        }
    }
    Ok(())
}

/// 下载 Telegram 文件。
async fn download_telegram_file(
    api: &TelegramApi,
    file_id: &str,
    max_download_bytes: u64,
) -> AppResult<Vec<u8>> {
    let file = api.get_file(file_id).await?;
    if let Some(size) = file.file_size
        && size > max_download_bytes
    {
        return Err(AppError::Channel(format!(
            "telegram file exceeds max_download_bytes: {size} > {max_download_bytes}"
        )));
    }
    let Some(file_path) = file.file_path.as_deref() else {
        return Err(AppError::Channel(
            "telegram getFile missing file_path".to_string(),
        ));
    };
    api.download_file(file_path, max_download_bytes).await
}

/// 选择最大 Telegram 图片。
fn best_photo(photos: &[TelegramPhotoSize]) -> Option<&TelegramPhotoSize> {
    photos
        .iter()
        .max_by_key(|photo| (photo.width as u64).saturating_mul(photo.height as u64))
}

/// 构造 Telegram 文件名。
fn telegram_download_filename(file: &TelegramFile, file_path: &str) -> String {
    Path::new(file_path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| file.file_unique_id.clone())
}

/// 构造 Telegram inline keyboard。
fn build_user_input_keyboard(request_id: &str, request: &UserInputRequest) -> Value {
    let keyboard = request
        .questions
        .iter()
        .enumerate()
        .flat_map(|(question_index, question)| {
            question
                .options
                .iter()
                .enumerate()
                .map(move |(option_index, option)| {
                    vec![json!({
                        "text": option.label,
                        "callback_data": format!("ui:{request_id}:{question_index}:{option_index}"),
                    })]
                })
        })
        .collect::<Vec<_>>();
    json!({ "inline_keyboard": keyboard })
}

/// 构造 Telegram 用户输入提示文本。
fn build_user_input_text(request: &UserInputRequest) -> String {
    request
        .questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let options = question
                .options
                .iter()
                .map(|option| format!("- {}：{}", option.label, option.description))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}. {}\n{}\n{}",
                index + 1,
                question.header,
                question.question,
                options
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 解析 Telegram 用户输入 callback data。
fn parse_user_input_callback(data: &str) -> Option<(&str, usize, usize)> {
    let mut parts = data.split(':');
    if parts.next()? != "ui" {
        return None;
    }
    let request_id = parts.next()?;
    let question_index = parts.next()?.parse().ok()?;
    let option_index = parts.next()?.parse().ok()?;
    Some((request_id, question_index, option_index))
}

/// 返回 Telegram 全量 update 类型。
fn telegram_allowed_updates() -> Vec<&'static str> {
    vec![
        "message",
        "edited_message",
        "channel_post",
        "edited_channel_post",
        "business_connection",
        "business_message",
        "edited_business_message",
        "deleted_business_messages",
        "guest_message",
        "message_reaction",
        "message_reaction_count",
        "inline_query",
        "chosen_inline_result",
        "callback_query",
        "shipping_query",
        "pre_checkout_query",
        "purchased_paid_media",
        "poll",
        "poll_answer",
        "my_chat_member",
        "chat_member",
        "chat_join_request",
        "chat_boost",
        "removed_chat_boost",
        "managed_bot",
    ]
}

/// 返回 Telegram offset 文件路径。
fn telegram_offset_path(paths: &AppPaths, name: &str) -> PathBuf {
    paths
        .channel_data_dir
        .join("telegram")
        .join(name)
        .join("offset")
}

/// 读取 Telegram polling offset。
async fn read_offset(path: &Path) -> AppResult<Option<i64>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(content.trim().parse().ok()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// 写入 Telegram polling offset。
async fn write_offset(path: &Path, offset: Option<i64>) -> AppResult<()> {
    let Some(offset) = offset else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, offset.to_string()).await?;
    Ok(())
}

/// 判断 Telegram chat 是否群组。
fn is_group_chat(kind: &str) -> bool {
    matches!(kind, "group" | "supergroup")
}

/// 映射 Telegram chat type。
fn telegram_chat_type(kind: &str) -> &str {
    match kind {
        "private" => "dm",
        "group" | "supergroup" => "group",
        "channel" => "channel",
        other => other,
    }
}

/// 构造 setMessageReaction 请求体，适用于离线校验 TG reaction 负载。
fn build_message_reaction_body(chat_id: &str, message_id: i64, emoji: &str, is_big: bool) -> Value {
    json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "reaction": [
            {
                "type": "emoji",
                "emoji": emoji,
            }
        ],
        "is_big": is_big,
    })
}

/// 去掉 Telegram bot mention，适用于 require_mention 群聊。
fn strip_bot_mention(text: &str, username: &str) -> Option<String> {
    let mention = format!("@{}", username.trim_start_matches('@'));
    if !text.contains(&mention) {
        return None;
    }
    Some(text.replace(&mention, "").trim().to_string())
}

/// 分割 Telegram 文本，适用于 sendMessage 4096 字符限制。
fn split_telegram_text(text: &str) -> Vec<String> {
    const LIMIT: usize = 3900;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![" ".to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in trimmed.lines() {
        let extra = usize::from(!current.is_empty());
        if current.chars().count() + line.chars().count() + extra > LIMIT && !current.is_empty() {
            chunks.push(current);
            current = String::new();
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// 解析 Telegram message_id。
fn parse_message_id(value: &str) -> AppResult<i64> {
    value
        .parse()
        .map_err(|_| AppError::Channel(format!("invalid telegram message_id={value}")))
}

/// 解析 i64。
fn parse_i64(value: &str) -> Option<i64> {
    value.parse().ok()
}

#[cfg(test)]
#[path = "telegram_test.rs"]
mod telegram_test;
