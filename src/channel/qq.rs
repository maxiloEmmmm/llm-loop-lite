//! QQ 官方机器人 channel 实现。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{Sink, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::channel::{Channel, ChannelAckCapability, ChannelAckKind, ChannelCapabilities};
use crate::config::{ChannelConfig, QqChannelConfig};
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::message::{
    InboundMessage, MessageSource, MessageUpdate, OutboundMessage, OutboundRecipient, SendResult,
    UserInputRequest, UserInputResponse,
};
use crate::resource::{ResourceUsage, estimate_answers_bytes, estimate_user_input_request_bytes};

const OP_DISPATCH: i64 = 0;
const OP_HEARTBEAT: i64 = 1;
const OP_IDENTIFY: i64 = 2;
const OP_RECONNECT: i64 = 7;
const OP_INVALID_SESSION: i64 = 9;
const OP_HELLO: i64 = 10;
const OP_HEARTBEAT_ACK: i64 = 11;
const QQ_USER_PREFIX: &str = "user:";
const QQ_GROUP_PREFIX: &str = "group:";
const QQ_CHANNEL_PREFIX: &str = "channel:";

/// QQ 官方机器人 channel，负责长连接收消息和 REST 被动回复。
pub struct QqChannel {
    /// channel 实例名。
    name: String,
    /// QQ REST API。
    api: Arc<QqApi>,
    /// WebSocket intents 位图。
    intents: u64,
    /// 消息去重状态。
    seen: Arc<Mutex<QqDedupCache>>,
    /// 等待中的用户输入请求。
    user_inputs: Arc<Mutex<QqPendingUserInputs>>,
    /// 已发送消息目标缓存。
    sent_messages: Arc<Mutex<QqSentMessageTargets>>,
    /// WebSocket 断线重连间隔。
    reconnect_delay: Duration,
    /// 后台长连接任务。
    task: Option<JoinHandle<()>>,
}

/// QQ channel 的轻量工具句柄。
#[derive(Clone)]
pub struct QqChannelHandle {
    /// channel 实例名。
    name: String,
    /// QQ REST API。
    api: Arc<QqApi>,
    /// 消息去重状态。
    seen: Arc<Mutex<QqDedupCache>>,
    /// 等待中的用户输入请求。
    user_inputs: Arc<Mutex<QqPendingUserInputs>>,
    /// 已发送消息目标缓存。
    sent_messages: Arc<Mutex<QqSentMessageTargets>>,
}

impl QqChannel {
    /// 从 channel 配置创建 QQ channel。
    pub fn from_config(config: &ChannelConfig) -> AppResult<Self> {
        let settings = ResolvedQqConfig::resolve(&config.qq)?;
        Ok(Self {
            name: config.name.clone(),
            api: Arc::new(QqApi::new(
                settings.auth_url,
                settings.api_base_url,
                settings.app_id,
                settings.app_secret,
            )),
            intents: settings.intents,
            seen: Arc::new(Mutex::new(QqDedupCache::new(
                config.qq.dedup_cache_size.max(1),
            ))),
            user_inputs: Arc::new(Mutex::new(QqPendingUserInputs::default())),
            sent_messages: Arc::new(Mutex::new(QqSentMessageTargets::new(
                config.qq.dedup_cache_size.max(1),
            ))),
            reconnect_delay: Duration::from_secs(config.qq.reconnect_delay_seconds.max(1)),
            task: None,
        })
    }

    /// 返回轻量工具句柄，适用于 tool 调用时发送消息。
    pub fn tool_handle(&self) -> QqChannelHandle {
        QqChannelHandle {
            name: self.name.clone(),
            api: Arc::clone(&self.api),
            seen: Arc::clone(&self.seen),
            user_inputs: Arc::clone(&self.user_inputs),
            sent_messages: Arc::clone(&self.sent_messages),
        }
    }

    /// 返回平台名称，适用于 cron 构造虚拟来源。
    pub fn platform_name(&self) -> &str {
        self.api.platform_name()
    }
}

impl QqChannelHandle {
    /// 返回 channel 名称。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 返回平台名称，适用于 cron 构造虚拟来源。
    pub fn platform_name(&self) -> &str {
        self.api.platform_name()
    }

    /// 返回 QQ 确认能力，适用于轻量句柄分派。
    pub fn ack_capability(&self, _kind: ChannelAckKind) -> ChannelAckCapability {
        ChannelAckCapability::TextReply
    }

    /// 返回 QQ 能力集合，适用于轻量句柄暴露运行态能力。
    pub fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            patch_message: false,
            append_update: true,
            request_user_input: true,
            reaction_ack: false,
            text_ack: true,
            chat_action: false,
            reply_threading: true,
            inbound_attachments: false,
            outbound_attachments: false,
        }
    }

    /// 返回 QQ 句柄持有的缓存资源估算。
    pub async fn resource_usage(&self) -> Vec<ResourceUsage> {
        vec![
            self.seen.lock().await.resource_usage("channel.qq.dedup"),
            self.user_inputs
                .lock()
                .await
                .resource_usage("channel.qq.pending_inputs"),
            self.sent_messages
                .lock()
                .await
                .resource_usage("channel.qq.sent_messages"),
        ]
    }

    /// 按统一语义执行 QQ 确认反馈。
    pub async fn acknowledge(
        &self,
        message: &InboundMessage,
        kind: ChannelAckKind,
    ) -> AppResult<()> {
        match kind {
            ChannelAckKind::Received => self.acknowledge_received(message).await,
            ChannelAckKind::ResetDone => self.acknowledge_reset(message).await,
            ChannelAckKind::StopDone => self.acknowledge_stop(message).await,
        }
    }

    /// 发送 QQ 消息。
    pub async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        let OutboundMessage {
            channel_name: _,
            chat_id,
            recipient,
            text,
            reply_to,
            thread_id: _,
            format: _,
        } = message;
        let cached_target = if let Some(reply_to) = reply_to.as_deref() {
            self.sent_messages.lock().await.next_target(reply_to)
        } else {
            None
        };
        let (chat_id, recipient, reply_to, msg_seq) = if let Some(target) = cached_target {
            (
                target.chat_id,
                target.recipient,
                target.reply_to,
                Some(target.msg_seq),
            )
        } else {
            (chat_id, recipient, reply_to.clone(), None)
        };
        let message_id = self
            .api
            .send_text(&chat_id, recipient, &text, reply_to.clone(), msg_seq)
            .await?;
        if let Some(message_id) = message_id.as_ref() {
            self.sent_messages.lock().await.insert(
                message_id.clone(),
                QqSentMessageTarget {
                    chat_id,
                    recipient,
                    reply_to,
                    next_msg_seq: msg_seq.unwrap_or(1).saturating_add(1),
                },
            );
        }
        Ok(SendResult {
            success: true,
            message_id,
        })
    }

    /// 降级更新 QQ 消息，适用于计划列表全量追加新回复。
    pub async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        let target = self
            .sent_messages
            .lock()
            .await
            .next_target(&message.message_id)
            .ok_or_else(|| {
                AppError::Channel(format!(
                    "qq update target is missing for message_id={}",
                    message.message_id
                ))
            })?;
        let message_id = self
            .api
            .send_text(
                &target.chat_id,
                target.recipient,
                &message.text,
                target.reply_to.clone(),
                target.reply_to.as_ref().map(|_| target.msg_seq),
            )
            .await?;
        if let Some(message_id) = message_id.as_ref() {
            self.sent_messages.lock().await.insert(
                message_id.clone(),
                QqSentMessageTarget {
                    chat_id: target.chat_id,
                    recipient: target.recipient,
                    reply_to: target.reply_to,
                    next_msg_seq: target.msg_seq.saturating_add(1),
                },
            );
        }
        Ok(SendResult {
            success: true,
            message_id,
        })
    }

    /// 发送 QQ 按钮消息并等待用户点击。
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
        if let Err(err) = self
            .api
            .send_user_input(source, &request_id, &request)
            .await
        {
            self.user_inputs.lock().await.remove(&request_id);
            return Err(err);
        }
        if let Some(timeout_ms) = request.auto_resolution_ms {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(_)) => Err(AppError::Channel(
                    "qq user input waiter was closed".to_string(),
                )),
                Err(_) => self.user_inputs.lock().await.auto_resolve(&request_id),
            }
        } else {
            rx.await
                .map_err(|_| AppError::Channel("qq user input waiter was closed".to_string()))
        }
    }

    /// 收到入站消息后发送短确认，适用于 QQ 无通用 reaction 的场景。
    pub async fn acknowledge_received(&self, message: &InboundMessage) -> AppResult<()> {
        let Some(reply_to) = message
            .message_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(());
        };
        let (recipient, chat_id) = crate::message::outbound_target_from_source(&message.source);
        crate::log_info!(
            "qq acknowledge received sending chat_id={} reply_to={}",
            chat_id,
            reply_to
        );
        let message_id = self
            .api
            .send_text(&chat_id, recipient, "ing...", Some(reply_to.clone()), None)
            .await?;
        let mut sent_messages = self.sent_messages.lock().await;
        let target = QqSentMessageTarget {
            chat_id,
            recipient,
            reply_to: Some(reply_to.clone()),
            next_msg_seq: 2,
        };
        // 触发条件：QQ 没有通用附属表情，收到消息后要立刻给用户反馈。
        // 常规发送路径只记录机器人消息 id，后续最终回复同一原消息会复用 msg_seq=1。
        // 同时按原消息 id 建索引，防止最终回复被平台当作重复被动回复吞掉。
        sent_messages.insert(reply_to.clone(), target.clone());
        if let Some(message_id) = message_id {
            sent_messages.insert(message_id, target);
        }
        Ok(())
    }

    /// reset 后发送短文本确认，适用于 QQ 无通用附属表情的场景。
    pub async fn acknowledge_reset(&self, message: &InboundMessage) -> AppResult<()> {
        self.user_inputs.lock().await.clear();
        self.sent_messages.lock().await.clear();
        let (recipient, chat_id) = crate::message::outbound_target_from_source(&message.source);
        let reply = OutboundMessage {
            channel_name: self.name.clone(),
            chat_id,
            recipient,
            text: "已重置。".to_string(),
            reply_to: message.message_id.clone(),
            thread_id: message.source.thread_id.clone(),
            format: crate::message::OutboundFormat::Text,
        };
        self.send(reply).await?;
        Ok(())
    }

    /// stop 完成后发送短文本确认，适用于 QQ 无通用附属表情的场景。
    pub async fn acknowledge_stop(&self, message: &InboundMessage) -> AppResult<()> {
        let (recipient, chat_id) = crate::message::outbound_target_from_source(&message.source);
        let reply = OutboundMessage {
            channel_name: self.name.clone(),
            chat_id,
            recipient,
            text: "done".to_string(),
            reply_to: message.message_id.clone(),
            thread_id: message.source.thread_id.clone(),
            format: crate::message::OutboundFormat::Text,
        };
        self.send(reply).await?;
        Ok(())
    }
}

impl Channel for QqChannel {
    /// 返回 channel 名称。
    fn name(&self) -> &str {
        &self.name
    }

    /// 启动 QQ WebSocket 长连接。
    async fn start(
        &mut self,
        tx: mpsc::Sender<InboundMessage>,
        _paths: &AppPaths,
    ) -> AppResult<()> {
        let api = Arc::clone(&self.api);
        let name = self.name.clone();
        let intents = self.intents;
        let seen = Arc::clone(&self.seen);
        let user_inputs = Arc::clone(&self.user_inputs);
        let reconnect_delay = self.reconnect_delay;
        self.task = Some(tokio::spawn(async move {
            loop {
                match run_qq_ws(
                    Arc::clone(&api),
                    tx.clone(),
                    name.clone(),
                    intents,
                    Arc::clone(&seen),
                    Arc::clone(&user_inputs),
                )
                .await
                {
                    Ok(()) => {
                        crate::log_info!(
                            "qq channel disconnected, reconnecting after {:?}",
                            reconnect_delay
                        );
                    }
                    Err(err) => {
                        crate::log_info!(
                            "qq channel stopped: {err}; reconnecting after {:?}",
                            reconnect_delay
                        );
                    }
                }
                tokio::time::sleep(reconnect_delay).await;
            }
        }));
        Ok(())
    }

    /// 停止 QQ WebSocket 长连接。
    async fn stop(&mut self) -> AppResult<()> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.user_inputs.lock().await.clear();
        self.sent_messages.lock().await.clear();
        Ok(())
    }

    /// 发送 QQ 出站消息。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        self.tool_handle().send(message).await
    }

    /// QQ 当前不支持通用原地消息更新。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        self.tool_handle().update_message(message).await
    }

    /// 通过 QQ Markdown keyboard 请求结构化输入。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        self.tool_handle().request_user_input(source, request).await
    }

    /// 返回 QQ 确认能力，适用于 daemon 统一确认语义。
    fn ack_capability(&self, _kind: ChannelAckKind) -> ChannelAckCapability {
        ChannelAckCapability::TextReply
    }

    /// 返回 QQ 能力集合，适用于上层按平台能力选择接口。
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            patch_message: false,
            append_update: true,
            request_user_input: true,
            reaction_ack: false,
            text_ack: true,
            chat_action: false,
            reply_threading: true,
            inbound_attachments: false,
            outbound_attachments: false,
        }
    }

    /// 按统一语义执行 QQ 确认反馈。
    async fn acknowledge(&self, message: &InboundMessage, kind: ChannelAckKind) -> AppResult<()> {
        match kind {
            ChannelAckKind::Received => self.tool_handle().acknowledge_received(message).await,
            ChannelAckKind::ResetDone => self.tool_handle().acknowledge_reset(message).await,
            ChannelAckKind::StopDone => self.tool_handle().acknowledge_stop(message).await,
        }
    }
}

/// QQ REST API 封装。
struct QqApi {
    /// AccessToken 获取地址。
    auth_url: String,
    /// OpenAPI 基础地址。
    api_base_url: String,
    /// QQ Bot AppID。
    app_id: String,
    /// QQ Bot AppSecret。
    app_secret: String,
    /// HTTP 客户端。
    client: reqwest::Client,
    /// AccessToken 缓存。
    token: Mutex<Option<QqCachedToken>>,
}

impl QqApi {
    /// 创建 QQ API 客户端。
    fn new(auth_url: String, api_base_url: String, app_id: String, app_secret: String) -> Self {
        Self {
            auth_url,
            api_base_url,
            app_id,
            app_secret,
            client: reqwest::Client::new(),
            token: Mutex::new(None),
        }
    }

    /// 返回平台名称。
    fn platform_name(&self) -> &str {
        "qq"
    }

    /// 获取 QQ AccessToken。
    async fn access_token(&self) -> AppResult<String> {
        if let Some(token) = self.cached_token().await {
            return Ok(token);
        }
        let body = json!({
            "appId": self.app_id,
            "clientSecret": self.app_secret,
        });
        crate::log_info!("qq token requesting");
        let response = self.client.post(&self.auth_url).json(&body).send().await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "qq token failed with {status}: {text}"
            )));
        }
        let parsed: QqTokenResponse = serde_json::from_str(&text)?;
        let expires_in = parse_expires_in(&parsed.expires_in).unwrap_or(7200);
        let token = parsed.access_token;
        let cached = QqCachedToken {
            value: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in.saturating_sub(60)),
        };
        *self.token.lock().await = Some(cached);
        Ok(token)
    }

    /// 从缓存中读取未过期 AccessToken。
    async fn cached_token(&self) -> Option<String> {
        self.token
            .lock()
            .await
            .as_ref()
            .filter(|token| token.expires_at > Instant::now())
            .map(|token| token.value.clone())
    }

    /// 获取 QQ WebSocket gateway 地址。
    async fn gateway(&self) -> AppResult<QqGateway> {
        let token = self.access_token().await?;
        let url = format!("{}/gateway/bot", self.api_base_url);
        crate::log_info!("qq gateway requesting");
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("QQBot {token}"))
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "qq gateway failed with {status}: {text}"
            )));
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// 发送 QQ 消息，适用于私聊、群聊和频道被动回复。
    async fn send_text(
        &self,
        chat_id: &str,
        recipient: OutboundRecipient,
        text: &str,
        reply_to: Option<String>,
        msg_seq: Option<u64>,
    ) -> AppResult<Option<String>> {
        let target = QqMessageTarget::from_outbound(chat_id, recipient)?;
        let token = self.access_token().await?;
        let url = match target {
            QqMessageTarget::User(id) => format!("{}/v2/users/{id}/messages", self.api_base_url),
            QqMessageTarget::Group(id) => {
                format!("{}/v2/groups/{id}/messages", self.api_base_url)
            }
            QqMessageTarget::Channel(id) => format!("{}/channels/{id}/messages", self.api_base_url),
        };
        let text = qq_outbound_markdown(text);
        let body = match target {
            QqMessageTarget::User(_) | QqMessageTarget::Group(_) => {
                make_markdown_message_body(&text, reply_to.as_deref(), msg_seq)
            }
            QqMessageTarget::Channel(_) => {
                make_text_message_body(&text, reply_to.as_deref(), msg_seq)
            }
        };
        self.post_message_api(url, token, body).await
    }

    /// 发送 QQ 结构化输入消息，适用于 request_user_input。
    async fn send_user_input(
        &self,
        source: &MessageSource,
        request_id: &str,
        request: &UserInputRequest,
    ) -> AppResult<Option<String>> {
        let target = QqMessageTarget::from_source(source)?;
        let token = self.access_token().await?;
        let url = match target {
            QqMessageTarget::User(id) => format!("{}/v2/users/{id}/messages", self.api_base_url),
            QqMessageTarget::Group(id) => {
                format!("{}/v2/groups/{id}/messages", self.api_base_url)
            }
            QqMessageTarget::Channel(id) => format!("{}/channels/{id}/messages", self.api_base_url),
        };
        let markdown = build_qq_user_input_markdown(request);
        let body = json!({
            "content": markdown,
            "msg_type": 2,
            "markdown": {
                "content": markdown,
            },
            "keyboard": build_qq_user_input_keyboard(&self.app_id, request_id, request)?,
        });
        self.post_message_api(url, token, body).await
    }

    /// 回应 QQ interaction，适用于清理客户端按钮 loading。
    async fn acknowledge_interaction(&self, interaction_id: &str, code: i64) -> AppResult<()> {
        let token = self.access_token().await?;
        let url = format!("{}/interactions/{interaction_id}", self.api_base_url);
        let response = self
            .client
            .put(url)
            .header("Authorization", format!("QQBot {token}"))
            .json(&json!({ "code": code }))
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        crate::log_info!(
            "qq interaction ack response status={} bytes={}",
            status,
            text.len()
        );
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "qq interaction ack failed with {status}: {text}"
            )));
        }
        Ok(())
    }

    /// 调用 QQ 发送消息 API。
    async fn post_message_api(
        &self,
        url: String,
        token: String,
        body: Value,
    ) -> AppResult<Option<String>> {
        crate::log_info!("qq post api start url={}", redact_query(&url));
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("QQBot {token}"))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        crate::log_info!(
            "qq post api response status={} bytes={}",
            status,
            text.len()
        );
        log_qq_send_response(&text);
        if !status.is_success() {
            return Err(AppError::Channel(format!(
                "qq message api failed with {status}: {text}"
            )));
        }
        let parsed: QqSendMessageResponse = serde_json::from_str(&text)?;
        Ok(parsed.id.filter(|value| !value.trim().is_empty()))
    }
}

/// 记录 QQ 发消息响应摘要，适用于排查 HTTP 200 但群内不可见。
fn log_qq_send_response(text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        crate::log_info!(
            "qq post api response body unparsed preview={}",
            log_preview(text)
        );
        return;
    };
    let id = value.get("id").and_then(Value::as_str).unwrap_or("");
    let code = value
        .get("code")
        .or_else(|| value.get("ret"))
        .map(Value::to_string)
        .unwrap_or_default();
    let message = value
        .get("message")
        .or_else(|| value.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("");
    crate::log_info!(
        "qq post api parsed id={} code={} message={} preview={}",
        id,
        code,
        message,
        log_preview(&value.to_string())
    );
}

/// 运行单次 QQ WebSocket 连接。
async fn run_qq_ws(
    api: Arc<QqApi>,
    tx: mpsc::Sender<InboundMessage>,
    channel_name: String,
    intents: u64,
    seen: Arc<Mutex<QqDedupCache>>,
    user_inputs: Arc<Mutex<QqPendingUserInputs>>,
) -> AppResult<()> {
    let gateway = api.gateway().await?;
    let (stream, _) = connect_async(&gateway.url)
        .await
        .map_err(|err| AppError::Channel(format!("qq websocket connect failed: {err}")))?;
    crate::log_info!(
        "qq websocket connected shards={}",
        gateway.shards.unwrap_or(1)
    );
    let (writer, mut reader) = stream.split();
    let writer = Arc::new(Mutex::new(writer));
    let seq = Arc::new(Mutex::new(None::<u64>));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(45));

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                send_heartbeat(&writer, &seq).await?;
            }
            maybe_message = reader.next() => {
                let Some(message) = maybe_message else {
                    return Ok(());
                };
                let message = message.map_err(|err| AppError::Channel(format!("qq ws read: {err}")))?;
                let text = match message {
                    WsMessage::Text(text) => text.to_string(),
                    WsMessage::Binary(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    WsMessage::Close(frame) => {
                        return Err(AppError::Channel(format!("qq websocket closed: {frame:?}")));
                    }
                    _ => continue,
                };
                handle_qq_payload(
                    &api,
                    &writer,
                    &seq,
                    &tx,
                    &channel_name,
                    intents,
                    &seen,
                    &user_inputs,
                    &mut heartbeat,
                    &text,
                ).await?;
            }
        }
    }
}

/// 处理 QQ WebSocket payload。
async fn handle_qq_payload<S>(
    api: &Arc<QqApi>,
    writer: &Arc<Mutex<S>>,
    seq: &Arc<Mutex<Option<u64>>>,
    tx: &mpsc::Sender<InboundMessage>,
    channel_name: &str,
    intents: u64,
    seen: &Arc<Mutex<QqDedupCache>>,
    user_inputs: &Arc<Mutex<QqPendingUserInputs>>,
    heartbeat: &mut tokio::time::Interval,
    raw: &str,
) -> AppResult<()>
where
    S: Sink<WsMessage> + Unpin,
    <S as Sink<WsMessage>>::Error: std::fmt::Display,
{
    let payload: QqPayload = serde_json::from_str(raw)?;
    if let Some(next_seq) = payload.s {
        *seq.lock().await = Some(next_seq);
    }
    match payload.op {
        OP_HELLO => {
            if let Some(interval) = payload.d.get("heartbeat_interval").and_then(Value::as_u64) {
                *heartbeat = tokio::time::interval(Duration::from_millis(interval.max(1000)));
            }
            identify(api, writer, intents).await?;
        }
        OP_DISPATCH => {
            handle_qq_dispatch(api, tx, channel_name, seen, user_inputs, payload).await?;
        }
        OP_HEARTBEAT => {
            send_heartbeat(writer, seq).await?;
        }
        OP_RECONNECT => {
            return Err(AppError::Channel(
                "qq gateway requested reconnect".to_string(),
            ));
        }
        OP_INVALID_SESSION => {
            return Err(AppError::Channel("qq gateway invalid session".to_string()));
        }
        OP_HEARTBEAT_ACK => {}
        other => {
            crate::log_info!("qq websocket ignored op={}", other);
        }
    }
    Ok(())
}

/// 发送 QQ Identify payload。
async fn identify<S>(api: &QqApi, writer: &Arc<Mutex<S>>, intents: u64) -> AppResult<()>
where
    S: Sink<WsMessage> + Unpin,
    <S as Sink<WsMessage>>::Error: std::fmt::Display,
{
    let token = api.access_token().await?;
    let payload = json!({
        "op": OP_IDENTIFY,
        "d": {
            "token": format!("QQBot {token}"),
            "intents": intents,
            "shard": [0, 1],
            "properties": {
                "$os": "linux",
                "$browser": "llm-loop",
                "$device": "llm-loop",
            },
        },
    });
    send_ws_json(writer, payload).await
}

/// 发送 QQ 心跳 payload。
async fn send_heartbeat<S>(writer: &Arc<Mutex<S>>, seq: &Arc<Mutex<Option<u64>>>) -> AppResult<()>
where
    S: Sink<WsMessage> + Unpin,
    <S as Sink<WsMessage>>::Error: std::fmt::Display,
{
    let seq = *seq.lock().await;
    send_ws_json(
        writer,
        json!({
            "op": OP_HEARTBEAT,
            "d": seq,
        }),
    )
    .await
}

/// 发送 QQ WebSocket JSON。
async fn send_ws_json<S>(writer: &Arc<Mutex<S>>, payload: Value) -> AppResult<()>
where
    S: Sink<WsMessage> + Unpin,
    <S as Sink<WsMessage>>::Error: std::fmt::Display,
{
    writer
        .lock()
        .await
        .send(WsMessage::Text(payload.to_string().into()))
        .await
        .map_err(|err| AppError::Channel(format!("qq ws write: {err}")))
}

/// 处理 QQ Dispatch 事件。
async fn handle_qq_dispatch(
    api: &Arc<QqApi>,
    tx: &mpsc::Sender<InboundMessage>,
    channel_name: &str,
    seen: &Arc<Mutex<QqDedupCache>>,
    user_inputs: &Arc<Mutex<QqPendingUserInputs>>,
    payload: QqPayload,
) -> AppResult<()> {
    let event_type = payload.t.as_deref().unwrap_or("");
    if event_type == "READY" {
        crate::log_info!("qq gateway ready");
        return Ok(());
    }
    if event_type == "INTERACTION_CREATE" {
        handle_qq_interaction(api, user_inputs, payload.d).await;
        return Ok(());
    }
    let Some(message) = qq_event_to_inbound(channel_name, event_type, payload.d)? else {
        return Ok(());
    };
    let Some(message_id) = message.message_id.as_deref() else {
        return Ok(());
    };
    if seen.lock().await.seen_before(message_id) {
        crate::log_info!(
            "qq event dropped reason=duplicate message_id={}",
            message_id
        );
        return Ok(());
    }
    crate::log_info!(
        "qq inbound accepted chat_type={} chat_id={} user_id={} message_id={} text={}",
        message.source.chat_type,
        message.source.chat_id,
        message.source.user_id.as_deref().unwrap_or(""),
        message_id,
        log_preview(&message.text)
    );
    if tx.send(message).await.is_err() {
        return Err(AppError::Channel("qq inbound receiver closed".to_string()));
    }
    Ok(())
}

/// 将 QQ 事件转换为 daemon 入站消息。
pub(super) fn qq_event_to_inbound(
    channel_name: &str,
    event_type: &str,
    data: Value,
) -> AppResult<Option<InboundMessage>> {
    match event_type {
        "C2C_MESSAGE_CREATE" => c2c_event_to_inbound(channel_name, data),
        "GROUP_AT_MESSAGE_CREATE" => group_event_to_inbound(channel_name, data),
        "AT_MESSAGE_CREATE" => channel_event_to_inbound(channel_name, data),
        _ => Ok(None),
    }
}

/// 将 QQ 单聊事件转换为 daemon 入站消息。
fn c2c_event_to_inbound(channel_name: &str, data: Value) -> AppResult<Option<InboundMessage>> {
    let event: QqMessageEvent = serde_json::from_value(data)?;
    let Some(user_openid) = event.author.and_then(|author| author.user_openid) else {
        return Ok(None);
    };
    let text = clean_qq_text(&event.content);
    if text.is_empty() {
        return Ok(None);
    }
    let source = MessageSource {
        channel_name: channel_name.to_string(),
        platform: "qq".to_string(),
        chat_id: format!("{QQ_USER_PREFIX}{user_openid}"),
        chat_type: "dm".to_string(),
        user_id: Some(user_openid),
        thread_id: None,
    };
    Ok(Some(InboundMessage::text(text, source, Some(event.id))))
}

/// 将 QQ 群聊 @ 事件转换为 daemon 入站消息。
fn group_event_to_inbound(channel_name: &str, data: Value) -> AppResult<Option<InboundMessage>> {
    let event: QqMessageEvent = serde_json::from_value(data)?;
    let Some(group_openid) = event.group_openid else {
        return Ok(None);
    };
    let user_id = event
        .author
        .and_then(|author| author.member_openid.or(author.user_openid));
    let text = clean_qq_text(&event.content);
    if text.is_empty() {
        return Ok(None);
    }
    let source = MessageSource {
        channel_name: channel_name.to_string(),
        platform: "qq".to_string(),
        chat_id: format!("{QQ_GROUP_PREFIX}{group_openid}"),
        chat_type: "group".to_string(),
        user_id,
        thread_id: None,
    };
    Ok(Some(InboundMessage::text(text, source, Some(event.id))))
}

/// 将 QQ 频道 @ 事件转换为 daemon 入站消息。
fn channel_event_to_inbound(channel_name: &str, data: Value) -> AppResult<Option<InboundMessage>> {
    let event: QqMessageEvent = serde_json::from_value(data)?;
    let Some(channel_id) = event.channel_id else {
        return Ok(None);
    };
    let user_id = event
        .author
        .and_then(|author| author.id.or(author.user_openid).or(author.member_openid));
    let text = clean_qq_text(&event.content);
    if text.is_empty() {
        return Ok(None);
    }
    let source = MessageSource {
        channel_name: channel_name.to_string(),
        platform: "qq".to_string(),
        chat_id: format!("{QQ_CHANNEL_PREFIX}{channel_id}"),
        chat_type: "group".to_string(),
        user_id,
        thread_id: None,
    };
    Ok(Some(InboundMessage::text(text, source, Some(event.id))))
}

/// 清理 QQ 文本里的机器人 mention 标记。
pub(super) fn clean_qq_text(text: &str) -> String {
    text.split_whitespace()
        .filter(|part| !part.starts_with("<@") || !part.ends_with('>'))
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// 生成 QQ 出站 Markdown，适用于修正其他平台留下的转义。
pub(super) fn qq_outbound_markdown(text: &str) -> String {
    restore_ordered_list_markers(text)
}

/// 还原有序列表标记。
///
/// 触发条件：Feishu 侧为了避免 Markdown 自动列表，会输出 `1\.`。
/// 不能直接复用常规文本：QQ Markdown 会把反斜杠展示出来。
/// 防止回归：只处理行首数字列表，避免误改正文里的转义点号。
fn restore_ordered_list_markers(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index > 0 && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&restore_ordered_list_marker_line(line));
    }
    output
}

/// 还原单行行首有序列表标记。
fn restore_ordered_list_marker_line(line: &str) -> String {
    let line_end = if line.ends_with('\n') { "\n" } else { "" };
    let content = line.strip_suffix('\n').unwrap_or(line);
    let trimmed = content.trim_start();
    let prefix_len = content.len() - trimmed.len();
    let digit_count = trimmed
        .chars()
        .take_while(|item| item.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    if digit_count == 0 || !trimmed[digit_count..].starts_with("\\.") {
        return line.to_string();
    }
    format!(
        "{}{}.{}{}",
        &content[..prefix_len],
        &trimmed[..digit_count],
        &trimmed[digit_count + 2..],
        line_end
    )
}

/// 构造 QQ Markdown 消息请求体。
fn make_markdown_message_body(text: &str, reply_to: Option<&str>, msg_seq: Option<u64>) -> Value {
    let mut body = json!({
        "content": text,
        "msg_type": 2,
        "markdown": {
            "content": text,
        },
    });
    attach_reply_fields(&mut body, reply_to, msg_seq);
    body
}

/// 构造 QQ 纯文本消息请求体。
fn make_text_message_body(text: &str, reply_to: Option<&str>, msg_seq: Option<u64>) -> Value {
    let mut body = json!({
        "content": text,
        "msg_type": 0,
    });
    attach_reply_fields(&mut body, reply_to, msg_seq);
    body
}

/// 附加 QQ 被动回复字段。
fn attach_reply_fields(body: &mut Value, reply_to: Option<&str>, msg_seq: Option<u64>) {
    if let Some(reply_to) = reply_to.filter(|value| !value.trim().is_empty()) {
        body["msg_id"] = Value::String(reply_to.to_string());
        body["msg_seq"] = Value::Number(msg_seq.unwrap_or(1).into());
    }
}

/// 构造 QQ 用户输入 Markdown 正文。
fn build_qq_user_input_markdown(request: &UserInputRequest) -> String {
    let mut lines = vec!["**需要你确认**".to_string(), String::new()];
    for (index, question) in request.questions.iter().enumerate() {
        lines.push(format!(
            "{}. **{}** {}",
            index + 1,
            question.header,
            question.question
        ));
        let options = question
            .options
            .iter()
            .map(|option| format!("`{}`", option.label))
            .collect::<Vec<_>>()
            .join(" / ");
        if !options.is_empty() {
            lines.push(format!("可选：{options}"));
        }
        lines.push(String::new());
    }
    lines.push("请选择下面的按钮。".to_string());
    lines.join("\n")
}

/// 构造 QQ 用户输入 keyboard。
fn build_qq_user_input_keyboard(
    app_id: &str,
    request_id: &str,
    request: &UserInputRequest,
) -> AppResult<Value> {
    let mut rows = Vec::new();
    for question in &request.questions {
        let mut buttons = Vec::new();
        for option in &question.options {
            let data = serde_json::to_string(&QqUserInputButtonData {
                llm_loop: "request_user_input".to_string(),
                request_id: request_id.to_string(),
                question_id: question.id.clone(),
                answer: option.label.clone(),
            })?;
            buttons.push(json!({
                "id": format!("{}:{}", question.id, option.label),
                "render_data": {
                    "label": option.label,
                    "visited_label": format!("已选 {}", option.label),
                    "style": 1,
                },
                "action": {
                    "type": 1,
                    "permission": {
                        "type": 2,
                    },
                    "data": data,
                    "unsupport_tips": "当前客户端不支持按钮，请直接回复文字。",
                },
            }));
        }
        if !buttons.is_empty() {
            rows.push(json!({ "buttons": buttons }));
        }
    }
    Ok(json!({
        "content": {
            "rows": rows,
            "bot_appid": app_id.parse::<u64>().unwrap_or_default(),
        },
    }))
}

/// 处理 QQ interaction 事件，适用于唤醒 request_user_input。
async fn handle_qq_interaction(
    api: &Arc<QqApi>,
    user_inputs: &Arc<Mutex<QqPendingUserInputs>>,
    data: Value,
) {
    let event: QqInteractionEvent = match serde_json::from_value(data) {
        Ok(event) => event,
        Err(err) => {
            crate::log_info!("qq interaction ignored reason=parse_failed err={err}");
            return;
        }
    };
    let Some(resolved) = event.data.and_then(|data| data.resolved) else {
        crate::log_info!("qq interaction ignored reason=missing_resolved");
        acknowledge_qq_interaction(api, &event.id, 1).await;
        return;
    };
    let button_data = match serde_json::from_str::<QqUserInputButtonData>(&resolved.button_data) {
        Ok(button_data) => button_data,
        Err(err) => {
            crate::log_info!("qq interaction ignored reason=bad_button_data err={err}");
            acknowledge_qq_interaction(api, &event.id, 1).await;
            return;
        }
    };
    if button_data.llm_loop != "request_user_input" {
        crate::log_info!("qq interaction ignored reason=foreign_button");
        acknowledge_qq_interaction(api, &event.id, 1).await;
        return;
    }
    let accepted = user_inputs.lock().await.answer(
        &button_data.request_id,
        &button_data.question_id,
        button_data.answer.clone(),
    );
    acknowledge_qq_interaction(api, &event.id, if accepted { 0 } else { 3 }).await;
    crate::log_info!(
        "qq interaction handled request_id={} question_id={} accepted={}",
        button_data.request_id,
        button_data.question_id,
        accepted
    );
}

/// 回应 QQ interaction，失败只打日志，避免影响 WebSocket 主循环。
async fn acknowledge_qq_interaction(api: &QqApi, interaction_id: &str, code: i64) {
    if let Err(err) = api.acknowledge_interaction(interaction_id, code).await {
        crate::log_info!("qq interaction ack failed: {err}");
    }
}

/// 解析 QQ 出站目标。
pub(super) enum QqMessageTarget<'a> {
    /// QQ 单聊目标。
    User(&'a str),
    /// QQ 群聊目标。
    Group(&'a str),
    /// QQ 频道目标。
    Channel(&'a str),
}

impl<'a> QqMessageTarget<'a> {
    /// 从通用出站消息推导 QQ API 目标。
    pub(super) fn from_outbound(chat_id: &'a str, recipient: OutboundRecipient) -> AppResult<Self> {
        if let Some(id) = chat_id.strip_prefix(QQ_USER_PREFIX) {
            return Ok(Self::User(id));
        }
        if let Some(id) = chat_id.strip_prefix(QQ_GROUP_PREFIX) {
            return Ok(Self::Group(id));
        }
        if let Some(id) = chat_id.strip_prefix(QQ_CHANNEL_PREFIX) {
            return Ok(Self::Channel(id));
        }
        match recipient {
            OutboundRecipient::User => Ok(Self::User(chat_id)),
            OutboundRecipient::Chat => Err(AppError::Channel(
                "qq chat_id must start with user:, group:, or channel:".to_string(),
            )),
        }
    }

    /// 从消息来源推导 QQ API 目标，适用于结构化输入请求。
    fn from_source(source: &'a MessageSource) -> AppResult<Self> {
        if let Some(id) = source.chat_id.strip_prefix(QQ_USER_PREFIX) {
            return Ok(Self::User(id));
        }
        if let Some(id) = source.chat_id.strip_prefix(QQ_GROUP_PREFIX) {
            return Ok(Self::Group(id));
        }
        if let Some(id) = source.chat_id.strip_prefix(QQ_CHANNEL_PREFIX) {
            return Ok(Self::Channel(id));
        }
        if source.chat_type == "dm"
            && let Some(user_id) = source
                .user_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        {
            return Ok(Self::User(user_id));
        }
        Err(AppError::Channel(
            "qq source chat_id must start with user:, group:, or channel:".to_string(),
        ))
    }
}

/// 解析后的 QQ channel 配置。
struct ResolvedQqConfig {
    /// AccessToken 获取地址。
    auth_url: String,
    /// OpenAPI 基础地址。
    api_base_url: String,
    /// QQ Bot AppID。
    app_id: String,
    /// QQ Bot AppSecret。
    app_secret: String,
    /// WebSocket intents 位图。
    intents: u64,
}

impl ResolvedQqConfig {
    /// 解析 QQ 配置，适用于启动前校验凭据和地址。
    fn resolve(config: &QqChannelConfig) -> AppResult<Self> {
        let app_id = resolve_secret(config.app_id.as_deref(), config.app_id_env.as_deref())
            .ok_or_else(|| AppError::Channel("qq app_id is required".to_string()))?;
        let app_secret = resolve_secret(
            config.app_secret.as_deref(),
            config.app_secret_env.as_deref(),
        )
        .ok_or_else(|| AppError::Channel("qq app_secret is required".to_string()))?;
        Ok(Self {
            auth_url: config.auth_url.clone(),
            api_base_url: config.api_base_url.trim_end_matches('/').to_string(),
            app_id,
            app_secret,
            intents: config.intents,
        })
    }
}

/// QQ 消息去重缓存。
struct QqDedupCache {
    /// 最大缓存条目数。
    capacity: usize,
    /// 插入顺序。
    order: VecDeque<String>,
    /// 已见消息 id。
    ids: HashSet<String>,
}

impl QqDedupCache {
    /// 创建 QQ 去重缓存。
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    /// 判断消息是否已处理，适用于 WebSocket 重连补发事件去重。
    fn seen_before(&mut self, message_id: &str) -> bool {
        if !self.ids.insert(message_id.to_string()) {
            return true;
        }
        self.order.push_back(message_id.to_string());
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
        false
    }

    /// 返回 QQ 去重缓存资源估算。
    fn resource_usage(&self, name: &str) -> ResourceUsage {
        let order_bytes = self
            .order
            .capacity()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(self.order.iter().map(String::capacity).sum::<usize>());
        let id_bytes = self
            .ids
            .capacity()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(self.ids.iter().map(String::capacity).sum::<usize>());
        ResourceUsage::new(
            name,
            "cache",
            self.ids.len(),
            Some(self.capacity),
            order_bytes.saturating_add(id_bytes),
        )
    }
}

/// QQ 已发送消息目标缓存。
struct QqSentMessageTargets {
    /// 最大缓存条目数。
    capacity: usize,
    /// 插入顺序。
    order: VecDeque<String>,
    /// 消息 id 到目标信息。
    targets: HashMap<String, QqSentMessageTarget>,
}

/// QQ 已发送消息目标信息。
#[derive(Clone)]
struct QqSentMessageTarget {
    /// 目标会话 id。
    chat_id: String,
    /// 目标粒度。
    recipient: OutboundRecipient,
    /// 原用户消息 id，用于继续被动回复。
    reply_to: Option<String>,
    /// 下一次被动回复序号。
    next_msg_seq: u64,
}

/// QQ 更新消息时使用的发送目标。
struct QqNextSendTarget {
    /// 目标会话 id。
    chat_id: String,
    /// 目标粒度。
    recipient: OutboundRecipient,
    /// 原用户消息 id，用于继续被动回复。
    reply_to: Option<String>,
    /// 本次被动回复序号。
    msg_seq: u64,
}

impl QqSentMessageTargets {
    /// 创建已发送消息缓存。
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            order: VecDeque::new(),
            targets: HashMap::new(),
        }
    }

    /// 插入已发送消息目标，适用于后续 update 降级追加。
    fn insert(&mut self, message_id: String, target: QqSentMessageTarget) {
        if !self.targets.contains_key(&message_id) {
            self.order.push_back(message_id.clone());
        }
        self.targets.insert(message_id, target);
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.targets.remove(&old);
            }
        }
    }

    /// 取出下一次发送目标并递增序号。
    fn next_target(&mut self, message_id: &str) -> Option<QqNextSendTarget> {
        let target = self.targets.get_mut(message_id)?;
        let msg_seq = target.next_msg_seq;
        target.next_msg_seq = target.next_msg_seq.saturating_add(1);
        Some(QqNextSendTarget {
            chat_id: target.chat_id.clone(),
            recipient: target.recipient,
            reply_to: target.reply_to.clone(),
            msg_seq,
        })
    }

    /// 清空缓存，适用于 reset 或 channel 停止释放内存。
    fn clear(&mut self) {
        self.order.clear();
        self.targets.clear();
    }

    /// 返回已发送消息目标缓存资源估算。
    fn resource_usage(&self, name: &str) -> ResourceUsage {
        let order_bytes = self
            .order
            .capacity()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(self.order.iter().map(String::capacity).sum::<usize>());
        let target_bytes = self
            .targets
            .capacity()
            .saturating_mul(std::mem::size_of::<(String, QqSentMessageTarget)>())
            .saturating_add(
                self.targets
                    .iter()
                    .map(|(key, target)| {
                        key.capacity()
                            + target.chat_id.capacity()
                            + target.reply_to.as_ref().map(String::capacity).unwrap_or(0)
                    })
                    .sum::<usize>(),
            );
        ResourceUsage::new(
            name,
            "cache",
            self.targets.len(),
            Some(self.capacity),
            order_bytes.saturating_add(target_bytes),
        )
    }
}

/// 等待中的 QQ 用户输入请求集合。
#[derive(Default)]
struct QqPendingUserInputs {
    /// 按请求 id 保存等待状态。
    items: HashMap<String, QqPendingUserInput>,
}

/// 单个 QQ 用户输入等待状态。
struct QqPendingUserInput {
    /// 原始请求，用于超时自动选择。
    request: UserInputRequest,
    /// 已收集答案。
    answers: HashMap<String, Vec<String>>,
    /// 完成时唤醒 tool。
    sender: Option<oneshot::Sender<UserInputResponse>>,
}

impl QqPendingUserInputs {
    /// 插入新的等待项，适用于发送按钮前登记回调。
    fn insert(
        &mut self,
        request_id: String,
        request: UserInputRequest,
        sender: oneshot::Sender<UserInputResponse>,
    ) {
        self.items.insert(
            request_id,
            QqPendingUserInput {
                request,
                answers: HashMap::new(),
                sender: Some(sender),
            },
        );
    }

    /// 移除等待项，适用于发送失败后清理。
    fn remove(&mut self, request_id: &str) {
        self.items.remove(request_id);
    }

    /// 记录按钮选择，返回是否接收成功。
    fn answer(&mut self, request_id: &str, question_id: &str, value: String) -> bool {
        let Some(pending) = self.items.get_mut(request_id) else {
            return false;
        };
        pending.answers.insert(question_id.to_string(), vec![value]);
        if has_qq_unanswered_question(pending) {
            return true;
        }
        let Some(mut pending) = self.items.remove(request_id) else {
            return false;
        };
        if let Some(sender) = pending.sender.take() {
            let _ = sender.send(UserInputResponse {
                answers: pending.answers,
            });
        }
        true
    }

    /// 超时后选择每题第一个选项，适用于模型允许自动决策时继续执行。
    fn auto_resolve(&mut self, request_id: &str) -> AppResult<UserInputResponse> {
        let Some(pending) = self.items.remove(request_id) else {
            return Err(AppError::Channel(
                "qq user input request is missing".to_string(),
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

    /// 清空所有等待项，适用于 channel 停止释放内存。
    fn clear(&mut self) {
        self.items.clear();
    }

    /// 返回等待输入集合资源估算。
    fn resource_usage(&self, name: &str) -> ResourceUsage {
        let body_bytes = self
            .items
            .iter()
            .map(|(key, pending)| key.capacity().saturating_add(pending.resource_bytes()))
            .sum::<usize>();
        let entry_bytes = self
            .items
            .capacity()
            .saturating_mul(std::mem::size_of::<(String, QqPendingUserInput)>());
        ResourceUsage::new(
            name,
            "hashmap",
            self.items.len(),
            Some(self.items.capacity()),
            entry_bytes.saturating_add(body_bytes),
        )
    }
}

impl QqPendingUserInput {
    /// 估算单个等待输入项容量。
    fn resource_bytes(&self) -> usize {
        estimate_user_input_request_bytes(&self.request) + estimate_answers_bytes(&self.answers)
    }
}

/// 判断 QQ 用户输入是否还有未回答问题。
fn has_qq_unanswered_question(pending: &QqPendingUserInput) -> bool {
    pending
        .request
        .questions
        .iter()
        .any(|question| !pending.answers.contains_key(&question.id))
}

/// QQ AccessToken 缓存。
struct QqCachedToken {
    /// AccessToken 值。
    value: String,
    /// 过期时间。
    expires_at: Instant,
}

/// QQ AccessToken 响应。
#[derive(Deserialize)]
pub(super) struct QqTokenResponse {
    /// AccessToken 值。
    #[serde(alias = "accessToken")]
    pub(super) access_token: String,
    /// 过期秒数，QQ 文档示例为字符串。
    #[serde(alias = "expiresIn")]
    pub(super) expires_in: Value,
}

/// QQ gateway 响应。
#[derive(Deserialize)]
struct QqGateway {
    /// WebSocket URL。
    url: String,
    /// 建议分片数。
    shards: Option<u64>,
}

/// QQ WebSocket payload。
#[derive(Deserialize)]
struct QqPayload {
    /// opcode。
    op: i64,
    /// 事件数据。
    #[serde(default)]
    d: Value,
    /// 消息序号。
    s: Option<u64>,
    /// 事件类型。
    t: Option<String>,
}

/// QQ 消息事件。
#[derive(Deserialize)]
struct QqMessageEvent {
    /// 平台消息 id。
    id: String,
    /// 文本内容。
    #[serde(default)]
    content: String,
    /// 群 openid。
    group_openid: Option<String>,
    /// 频道 id。
    channel_id: Option<String>,
    /// 发送者。
    author: Option<QqAuthor>,
}

/// QQ 消息发送者。
#[derive(Deserialize)]
struct QqAuthor {
    /// 频道用户 id。
    id: Option<String>,
    /// 单聊用户 openid。
    user_openid: Option<String>,
    /// 群成员 openid。
    member_openid: Option<String>,
}

/// QQ 发消息响应。
#[derive(Deserialize)]
struct QqSendMessageResponse {
    /// 平台消息 id。
    id: Option<String>,
}

/// QQ interaction 事件。
#[derive(Deserialize)]
struct QqInteractionEvent {
    /// interaction id，用于回 ACK。
    id: String,
    /// interaction 数据。
    data: Option<QqInteractionData>,
}

/// QQ interaction 数据。
#[derive(Deserialize)]
struct QqInteractionData {
    /// 解析后的按钮数据。
    #[serde(alias = "resoloved")]
    resolved: Option<QqInteractionResolved>,
}

/// QQ interaction resolved 数据。
#[derive(Deserialize)]
struct QqInteractionResolved {
    /// 按钮 data 字段。
    button_data: String,
}

/// QQ 用户输入按钮 data。
#[derive(Deserialize, Serialize)]
struct QqUserInputButtonData {
    /// llm-loop 事件标记。
    llm_loop: String,
    /// 用户输入请求 id。
    request_id: String,
    /// 问题 id。
    question_id: String,
    /// 选择答案。
    answer: String,
}

/// 从字符串或数字 expires_in 中解析秒数。
fn parse_expires_in(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
}

/// 解析明文或环境变量 secret。
fn resolve_secret(value: Option<&str>, env_key: Option<&str>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .or_else(|| {
            env_key
                .and_then(|key| std::env::var(key).ok())
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
        })
}

/// 隐去 URL query，适用于日志保留路径但不泄漏参数。
fn redact_query(url: &str) -> String {
    url.split('?').next().unwrap_or(url).to_string()
}

/// 生成日志预览，避免长消息刷屏。
fn log_preview(text: &str) -> String {
    const MAX_CHARS: usize = 120;
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut output = compact.chars().take(MAX_CHARS).collect::<String>();
    if compact.chars().count() > MAX_CHARS {
        output.push_str("...");
    }
    output
}
