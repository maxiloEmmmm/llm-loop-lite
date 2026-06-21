//! 飞书/Lark channel 实现。

mod api;
mod event;
mod ws_frame;

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use prost::Message;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::channel::{Channel, ChannelAckCapability, ChannelAckKind, ChannelCapabilities};
use crate::config::{ChannelConfig, FeishuChannelConfig};
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::message::{
    InboundAttachment, InboundMessage, MessageSource, MessageUpdate, OutboundMessage,
    OutboundRecipient, SendResult, UserInputRequest, UserInputResponse,
};
use crate::resource::{
    ResourceUsage, estimate_answers_bytes, estimate_message_source_bytes,
    estimate_user_input_request_bytes,
};
use crate::session::build_message_key;
use crate::store::store_hash;

use api::{FeishuApi, FeishuMessageDetail, FeishuReceiveIdType, FeishuRecipient};
use event::{FeishuEventEnvelope, FeishuReceiveMessageEvent};
use ws_frame::{FeishuFrame, FeishuHeader};

const FRAME_TYPE_CONTROL: i32 = 0;
const FRAME_TYPE_DATA: i32 = 1;
const HEADER_TYPE: &str = "type";
const HEADER_SUM: &str = "sum";
const HEADER_SEQ: &str = "seq";
const MESSAGE_TYPE_EVENT: &str = "event";
const MESSAGE_TYPE_PING: &str = "ping";
const MESSAGE_TYPE_PONG: &str = "pong";

/// 飞书 channel，负责长连接收消息和 REST 回消息。
pub struct FeishuChannel {
    /// channel 实例名。
    name: String,
    /// 飞书 REST API。
    api: Arc<FeishuApi>,
    /// 机器人名称，用于缺少 open_id 时兜底。
    bot_name: Option<String>,
    /// 群聊是否要求 @ 机器人。
    require_mention: bool,
    /// 消息去重状态。
    seen: Arc<Mutex<DedupCache>>,
    /// 需要以飞书子话题回复的入站消息 id。
    thread_replies: Arc<Mutex<ThreadReplyCache>>,
    /// WebSocket ping 间隔。
    ping_interval: Duration,
    /// 等待飞书卡片按钮回调的用户输入请求。
    user_inputs: Arc<Mutex<PendingUserInputs>>,
    /// 后台长连接任务。
    task: Option<JoinHandle<()>>,
}

/// 飞书 channel 的轻量工具句柄。
#[derive(Clone)]
pub struct FeishuChannelHandle {
    /// channel 实例名。
    name: String,
    /// 飞书 REST API。
    api: Arc<FeishuApi>,
    /// 消息去重状态。
    seen: Arc<Mutex<DedupCache>>,
    /// 需要以飞书子话题回复的入站消息 id。
    thread_replies: Arc<Mutex<ThreadReplyCache>>,
    /// 等待飞书卡片按钮回调的用户输入请求。
    user_inputs: Arc<Mutex<PendingUserInputs>>,
}

impl FeishuChannel {
    /// 从 channel 配置创建飞书 channel。
    pub fn from_config(config: &ChannelConfig) -> AppResult<Self> {
        let settings = ResolvedFeishuConfig::resolve(&config.feishu)?;
        Ok(Self {
            name: config.name.clone(),
            api: Arc::new(FeishuApi::new(
                settings.base_url,
                settings.app_id,
                settings.app_secret,
            )),
            bot_name: config.feishu.bot_name.clone(),
            require_mention: config.feishu.require_mention,
            seen: Arc::new(Mutex::new(DedupCache::new(config.feishu.dedup_cache_size))),
            thread_replies: Arc::new(Mutex::new(ThreadReplyCache::new(
                config.feishu.dedup_cache_size,
            ))),
            ping_interval: Duration::from_secs(config.feishu.ping_interval_seconds.max(1)),
            user_inputs: Arc::new(Mutex::new(PendingUserInputs::default())),
            task: None,
        })
    }

    /// 返回轻量工具句柄，适用于 tool 发送和更新消息。
    pub fn tool_handle(&self) -> FeishuChannelHandle {
        FeishuChannelHandle {
            name: self.name.clone(),
            api: Arc::clone(&self.api),
            seen: Arc::clone(&self.seen),
            thread_replies: Arc::clone(&self.thread_replies),
            user_inputs: Arc::clone(&self.user_inputs),
        }
    }

    /// 处理单个飞书 WebSocket binary frame。
    async fn handle_frame(
        frame: FeishuFrame,
        api: Arc<FeishuApi>,
        tx: mpsc::Sender<InboundMessage>,
        name: String,
        gate: MentionGate,
        seen: Arc<Mutex<DedupCache>>,
        thread_replies: Arc<Mutex<ThreadReplyCache>>,
        user_inputs: Arc<Mutex<PendingUserInputs>>,
        store_root: PathBuf,
    ) -> AppResult<Option<FeishuFrame>> {
        let header_type = header_value(&frame.headers, HEADER_TYPE);
        if frame.method == FRAME_TYPE_CONTROL {
            return Ok((header_type.as_deref() == Some(MESSAGE_TYPE_PONG)).then_some(frame));
        }
        if frame.method != FRAME_TYPE_DATA || header_type.as_deref() != Some(MESSAGE_TYPE_EVENT) {
            return Ok(None);
        }

        let payload = combine_payload_if_needed(&frame)?;
        let envelope: FeishuEventEnvelope = serde_json::from_slice(&payload)?;
        if envelope.header.event_type == "card.action.trigger" {
            if let Some(event_value) = envelope.event {
                handle_card_action(event_value, Arc::clone(&api), user_inputs).await;
            }
            return Ok(Some(response_frame(frame, 200, None)?));
        }
        if envelope.header.event_type != "im.message.receive_v1" {
            return Ok(Some(response_frame(frame, 200, None)?));
        }
        let Some(event_value) = envelope.event else {
            crate::log_info!("feishu event ignored reason=missing_event_body");
            return Ok(Some(response_frame(frame, 200, None)?));
        };
        let event: FeishuReceiveMessageEvent = serde_json::from_value(event_value)?;
        if should_drop_event(&event, &gate, seen).await {
            return Ok(Some(response_frame(frame, 200, None)?));
        }
        if let Some(mut message) =
            event_to_inbound_message(&api, &event, name, api.platform_name(), &store_root).await?
        {
            attach_images(&api, &event, &mut message).await;
            attach_files(&api, &event, &mut message, &store_root).await;
            let user_input_result = if message.attachments.is_empty()
                && !message.is_reset_command()
                && !message.is_stop_command()
            {
                user_inputs
                    .lock()
                    .await
                    .answer_next_by_source(&message.source, message.text.clone())
            } else {
                UserInputAnswerResult::Missing
            };
            if let UserInputAnswerResult::Accepted { update, finished } = user_input_result {
                if let Some(update) = update
                    && let Err(err) = api
                        .patch_interactive_card(&update.message_id, update.card)
                        .await
                {
                    crate::log_info!("feishu user input card patch failed: {err}");
                }
                if finished
                    && let Some(message_id) = message.message_id.as_deref()
                    && let Err(err) = api.add_done_reaction(message_id).await
                {
                    crate::log_info!("feishu user input text reaction failed: {err}");
                }
                crate::log_info!(
                    "feishu inbound consumed by user_input channel={} chat_id={} user_id={} text={}",
                    message.source.channel_name,
                    message.source.chat_id,
                    message.source.user_id.as_deref().unwrap_or(""),
                    log_preview(&message.text)
                );
                return Ok(Some(response_frame(frame, 200, None)?));
            }
            if event.message.is_reply_context()
                && let Some(message_id) = message.message_id.as_deref()
            {
                thread_replies.lock().await.insert(message_id);
            }
            crate::log_info!(
                "feishu inbound accepted channel={} platform={} chat_type={} chat_id={} thread_id={} user_id={} message_id={} text={}",
                message.source.channel_name,
                message.source.platform,
                message.source.chat_type,
                message.source.chat_id,
                message.source.thread_id.as_deref().unwrap_or(""),
                message.source.user_id.as_deref().unwrap_or(""),
                message.message_id.as_deref().unwrap_or(""),
                log_preview(&message.text)
            );
            if tx.send(message).await.is_err() {
                crate::log_info!("feishu inbound enqueue failed reason=daemon_queue_closed");
            } else {
                crate::log_info!("feishu inbound enqueued");
            }
        }
        Ok(Some(response_frame(frame, 200, None)?))
    }
}

impl FeishuChannelHandle {
    /// 返回 channel 名称。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 返回飞书平台名称，适用于构造虚拟入站来源。
    pub fn platform_name(&self) -> &str {
        self.api.platform_name()
    }

    /// 返回飞书确认能力，适用于轻量句柄分派。
    pub fn ack_capability(&self, kind: ChannelAckKind) -> ChannelAckCapability {
        match kind {
            ChannelAckKind::StopDone => ChannelAckCapability::TextReply,
            ChannelAckKind::Received | ChannelAckKind::ResetDone => ChannelAckCapability::Reaction,
        }
    }

    /// 返回飞书能力集合，适用于轻量句柄暴露运行态能力。
    pub fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            patch_message: true,
            append_update: false,
            request_user_input: true,
            reaction_ack: true,
            text_ack: true,
            chat_action: false,
            reply_threading: true,
            inbound_attachments: true,
            outbound_attachments: false,
        }
    }

    /// 返回飞书句柄持有的缓存资源估算。
    pub async fn resource_usage(&self) -> Vec<ResourceUsage> {
        vec![
            self.seen
                .lock()
                .await
                .resource_usage("channel.feishu.dedup"),
            self.thread_replies
                .lock()
                .await
                .resource_usage("channel.feishu.thread_replies"),
            self.user_inputs
                .lock()
                .await
                .resource_usage("channel.feishu.pending_inputs"),
        ]
    }

    /// 按统一语义执行飞书确认反馈。
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

    /// 发送飞书文本消息。
    pub async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        let reply_in_thread = match message.reply_to.as_deref() {
            Some(message_id) => self.thread_replies.lock().await.remove(message_id),
            None => false,
        };
        let message_id = self
            .api
            .send_text(
                feishu_recipient_from_outbound(&message)?,
                &message.text,
                message.reply_to.as_deref(),
                reply_in_thread,
                message.format,
            )
            .await?;
        crate::log_info!(
            "feishu outbound sent chat_id={} reply_to={} message_id={}",
            message.chat_id,
            message.reply_to.as_deref().unwrap_or(""),
            message_id.as_deref().unwrap_or("")
        );
        Ok(SendResult {
            success: true,
            message_id,
        })
    }

    /// 更新飞书计划消息。
    pub async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        self.api
            .edit_text_message(&message.message_id, &message.text, message.format)
            .await?;
        Ok(SendResult {
            success: true,
            message_id: Some(message.message_id),
        })
    }

    /// 发送飞书交互卡片并等待用户点击。
    pub async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        let request_id = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.user_inputs.lock().await;
            pending.insert(request_id.clone(), request.clone(), tx);
            pending.bind_source(&request_id, source.clone());
        }
        let card = build_user_input_card(&request_id, &request);
        match self
            .api
            .send_interactive_card(feishu_recipient_from_source(source)?, card)
            .await
        {
            Ok(message_id) => {
                if let Some(message_id) = &message_id {
                    self.user_inputs
                        .lock()
                        .await
                        .bind_card_message(&request_id, message_id.clone());
                }
                crate::log_info!(
                    "feishu user input card sent request_id={} message_id={}",
                    request_id,
                    message_id.as_deref().unwrap_or("")
                );
            }
            Err(err) => {
                self.user_inputs.lock().await.remove(&request_id);
                return Err(err);
            }
        }
        let output = if let Some(timeout_ms) = request.auto_resolution_ms {
            match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
                Ok(Ok(response)) => response,
                Ok(Err(_)) => {
                    return Err(AppError::Channel(
                        "feishu user input waiter was closed".to_string(),
                    ));
                }
                Err(_) => self.user_inputs.lock().await.auto_resolve(&request_id)?,
            }
        } else {
            rx.await
                .map_err(|_| AppError::Channel("feishu user input waiter was closed".to_string()))?
        };
        Ok(output)
    }

    /// 给入站消息添加处理中 reaction。
    pub async fn acknowledge_received(&self, message: &InboundMessage) -> AppResult<()> {
        let Some(message_id) = message.message_id.as_deref() else {
            return Ok(());
        };
        self.api.add_received_reaction(message_id).await?;
        Ok(())
    }

    /// reset 后只添加完成 reaction，不额外回复文本。
    pub async fn acknowledge_reset(&self, message: &InboundMessage) -> AppResult<()> {
        self.seen.lock().await.clear();
        self.thread_replies.lock().await.clear();
        self.user_inputs.lock().await.clear();
        if let Some(message_id) = message.message_id.as_deref() {
            self.api.add_done_reaction(message_id).await?;
        }
        Ok(())
    }

    /// stop 完成后回复 done，适用于用户确认取消已经落地。
    pub async fn acknowledge_stop(&self, message: &InboundMessage) -> AppResult<()> {
        let (recipient, chat_id) = crate::message::outbound_target_from_source(&message.source);
        let reply = OutboundMessage {
            channel_name: self.name.clone(),
            chat_id,
            recipient,
            text: "done".to_string(),
            reply_to: message.message_id.clone(),
            format: crate::message::OutboundFormat::Text,
        };
        self.send(reply).await?;
        Ok(())
    }
}

/// 将飞书事件转换为入站消息，适用于普通消息和合并转发消息。
async fn event_to_inbound_message(
    api: &Arc<FeishuApi>,
    event: &FeishuReceiveMessageEvent,
    channel_name: String,
    platform: &str,
    store_root: &Path,
) -> AppResult<Option<InboundMessage>> {
    if event.message.message_type != "merge_forward" {
        let Some(mut message) = event.to_inbound(channel_name, platform) else {
            return Ok(None);
        };
        prepend_merge_forward_root_context(api, event, &mut message, store_root).await;
        return Ok(Some(message));
    }
    let mut message = event.to_inbound_with_text(channel_name, platform, String::new());
    let mut visited = HashSet::new();
    let merge = merge_forward_content(
        api,
        &event.message.message_id,
        &message.source,
        store_root,
        &mut visited,
    )
    .await?
    .unwrap_or_else(|| {
        MergeForwardContent::text_only("[合并转发消息]\n未读取到可解析的子消息".to_string())
    });
    message.text = merge.text;
    message.attachments.extend(merge.attachments);
    Ok(Some(message))
}

/// 合并转发内容，适用于同时携带文本摘要和图片附件。
struct MergeForwardContent {
    /// 渲染后的转发文本。
    text: String,
    /// 子消息中下载到的附件。
    attachments: Vec<InboundAttachment>,
}

impl MergeForwardContent {
    /// 构造仅包含文本的合并转发内容，适用于解析失败兜底。
    fn text_only(text: String) -> Self {
        Self {
            text,
            attachments: Vec::new(),
        }
    }
}

/// 拉取并渲染合并转发消息，适用于把转发会话交给 provider。
async fn merge_forward_content(
    api: &Arc<FeishuApi>,
    message_id: &str,
    source: &MessageSource,
    store_root: &Path,
    visited: &mut HashSet<String>,
) -> AppResult<Option<MergeForwardContent>> {
    if !visited.insert(message_id.to_string()) {
        return Ok(Some(MergeForwardContent::text_only(
            "[合并转发消息]\n已跳过循环引用".to_string(),
        )));
    }
    let items = api.get_message_content(message_id).await?;
    let result =
        render_merge_forward_items(api, message_id, &items, source, store_root, visited).await?;
    visited.remove(message_id);
    Ok(result)
}

/// 渲染合并转发消息列表，适用于直接转发和回复 root 复用。
async fn render_merge_forward_items(
    api: &Arc<FeishuApi>,
    message_id: &str,
    items: &[FeishuMessageDetail],
    source: &MessageSource,
    store_root: &Path,
    visited: &mut HashSet<String>,
) -> AppResult<Option<MergeForwardContent>> {
    let mut lines = Vec::new();
    let mut sender_aliases = HashMap::new();
    let mut next_sender = 1_usize;
    let mut attachments = Vec::new();
    lines.push("[合并转发消息]".to_string());
    let mut count = 0_usize;
    for item in items {
        if item.message_id == message_id {
            continue;
        }
        if item.upper_message_id.as_deref() != Some(message_id) {
            continue;
        }
        if let Some(merged) = merged_message_item_content(
            api,
            message_id,
            item,
            source,
            store_root,
            visited,
            &mut sender_aliases,
            &mut next_sender,
        )
        .await?
        {
            count += 1;
            lines.push(format!("{}. {}", count, indent_multiline(&merged.text)));
            attachments.extend(merged.attachments);
        }
    }
    crate::log_info!(
        "feishu merge_forward fetched message_id={} items={}",
        message_id,
        count
    );
    if count == 0 {
        return Ok(None);
    }
    Ok(Some(MergeForwardContent {
        text: lines.join("\n"),
        attachments,
    }))
}

/// 归一化合并转发子消息，适用于统一处理文本、附件和嵌套合并转发。
async fn merged_message_item_content(
    api: &Arc<FeishuApi>,
    resource_message_id: &str,
    item: &FeishuMessageDetail,
    source: &MessageSource,
    store_root: &Path,
    visited: &mut HashSet<String>,
    sender_aliases: &mut HashMap<String, String>,
    next_sender: &mut usize,
) -> AppResult<Option<MergeForwardContent>> {
    let sender = render_merge_sender(item, sender_aliases, next_sender);
    if item.msg_type == "merge_forward" {
        let nested = Box::pin(merge_forward_content(
            api,
            &item.message_id,
            source,
            store_root,
            visited,
        ))
        .await?;
        return Ok(nested.map(|content| MergeForwardContent {
            text: format!("{sender}: {}", content.text),
            attachments: content.attachments,
        }));
    }
    let Some(content) = item.body.as_ref().map(|body| body.content.as_str()) else {
        return Ok(None);
    };
    let Some(text) = event::message_text(&item.msg_type, content) else {
        return Ok(None);
    };
    let mut attachments = Vec::new();
    attach_images_from_content(api, resource_message_id, content, &mut attachments).await;
    attach_files_from_content(
        api,
        resource_message_id,
        content,
        source,
        store_root,
        &mut attachments,
    )
    .await;
    Ok(Some(MergeForwardContent {
        text: format!("{sender}: {text}"),
        attachments,
    }))
}

/// 给回复消息补充 root 合并转发内容，适用于用户在会话记录下追问。
async fn prepend_merge_forward_root_context(
    api: &Arc<FeishuApi>,
    event: &FeishuReceiveMessageEvent,
    message: &mut InboundMessage,
    store_root: &Path,
) {
    let Some(root_id) = reply_root_message_id(event) else {
        return;
    };
    let mut visited = HashSet::new();
    match merge_forward_content(api, root_id, &message.source, store_root, &mut visited).await {
        Ok(Some(context)) => {
            // 触发条件：用户回复飞书合并转发消息并只写追问。
            // 不能走普通 session 历史：旧版本可能未收录 root 内容，
            // 且 root 本体不是当前这条文本消息。
            // 防止回归：provider 不再只看到“理解一下”这种孤立指令。
            message.text = format!("{}\n\n[用户追问]\n{}", context.text, message.text);
            message.attachments.extend(context.attachments);
            crate::log_info!(
                "feishu merge_forward root prepended message_id={} root_id={}",
                event.message.message_id,
                root_id
            );
        }
        Ok(None) => {}
        Err(err) => {
            crate::log_info!(
                "feishu merge_forward root fetch failed message_id={} root_id={} error={}",
                event.message.message_id,
                root_id,
                err
            );
        }
    }
}

/// 返回回复链 root 消息 id，适用于尝试读取被回复的合并转发内容。
fn reply_root_message_id(event: &FeishuReceiveMessageEvent) -> Option<&str> {
    event
        .message
        .root_id
        .as_deref()
        .or(event.message.parent_id.as_deref())
        .filter(|root_id| !root_id.trim().is_empty())
        .filter(|root_id| *root_id != event.message.message_id)
}

/// 渲染合并转发发送者，适用于避免把飞书 open_id 暴露给 provider。
fn render_merge_sender(
    item: &FeishuMessageDetail,
    sender_aliases: &mut HashMap<String, String>,
    next_sender: &mut usize,
) -> String {
    let Some(sender_id) = item
        .sender
        .as_ref()
        .map(|sender| sender.id.trim())
        .filter(|value| !value.is_empty())
    else {
        return "未知用户".to_string();
    };
    if let Some(label) = sender_aliases.get(sender_id) {
        return label.clone();
    }
    let label = format!("用户{}", *next_sender);
    *next_sender += 1;
    sender_aliases.insert(sender_id.to_string(), label.clone());
    label
}

/// 缩进多行子消息，适用于嵌套合并转发保持列表可读。
fn indent_multiline(text: &str) -> String {
    text.lines().collect::<Vec<_>>().join("\n   ")
}

/// 下载并落盘飞书文件附件。
async fn attach_files(
    api: &Arc<FeishuApi>,
    event: &FeishuReceiveMessageEvent,
    message: &mut InboundMessage,
    store_root: &Path,
) {
    attach_files_from_content(
        api,
        &event.message.message_id,
        &event.message.content,
        &message.source,
        store_root,
        &mut message.attachments,
    )
    .await;
}

/// 从消息 content 下载并落盘文件，适用于普通消息和合并转发子消息。
async fn attach_files_from_content(
    api: &Arc<FeishuApi>,
    message_id: &str,
    content: &str,
    source: &MessageSource,
    store_root: &Path,
    attachments: &mut Vec<InboundAttachment>,
) {
    let resources = event::file_resources_from_content(content);
    if resources.is_empty() {
        return;
    }
    let session_key = build_message_key(source);
    let session_dir = store_root.join(store_hash(&session_key));
    if let Err(err) = tokio::fs::create_dir_all(&session_dir).await {
        crate::log_info!(
            "feishu file store create failed dir={} error={}",
            session_dir.display(),
            err
        );
        return;
    }
    for resource in resources {
        match api
            .download_message_file(message_id, &resource.file_key)
            .await
        {
            Ok(file) => {
                let filename = resource
                    .filename
                    .clone()
                    .unwrap_or_else(|| resource.file_key.clone());
                let path = session_dir.join(format!(
                    "{}_{}",
                    store_hash(&resource.file_key),
                    sanitize_filename(&filename)
                ));
                if let Err(err) = tokio::fs::write(&path, &file.bytes).await {
                    crate::log_info!(
                        "feishu file store write failed path={} error={}",
                        path.display(),
                        err
                    );
                    continue;
                }
                crate::log_info!(
                    "feishu file stored message_id={} file_key={} path={} mime_type={} bytes={}",
                    message_id,
                    resource.file_key,
                    path.display(),
                    file.mime_type,
                    file.bytes.len()
                );
                attachments.push(InboundAttachment::StoredFile {
                    path,
                    filename,
                    mime_type: file.mime_type,
                    size: file.bytes.len() as u64,
                });
            }
            Err(err) => {
                crate::log_info!(
                    "feishu file attach failed message_id={} file_key={} error={}",
                    message_id,
                    resource.file_key,
                    err
                );
            }
        }
    }
}

/// 下载并挂载飞书图片附件。
async fn attach_images(
    api: &Arc<FeishuApi>,
    event: &FeishuReceiveMessageEvent,
    message: &mut InboundMessage,
) {
    attach_images_from_content(
        api,
        &event.message.message_id,
        &event.message.content,
        &mut message.attachments,
    )
    .await;
}

/// 从消息 content 下载图片，适用于普通消息和合并转发子消息。
async fn attach_images_from_content(
    api: &Arc<FeishuApi>,
    message_id: &str,
    content: &str,
    attachments: &mut Vec<InboundAttachment>,
) {
    let image_keys = event::image_keys_from_content(content);
    if image_keys.is_empty() {
        return;
    }
    for image_key in image_keys {
        match api.download_message_image(message_id, &image_key).await {
            Ok(image) => {
                crate::log_info!(
                    "feishu image attached message_id={} image_key={} mime_type={} bytes={}",
                    message_id,
                    image_key,
                    image.mime_type,
                    image.bytes.len()
                );
                attachments.push(InboundAttachment::Image {
                    mime_type: image.mime_type,
                    bytes: image.bytes,
                });
            }
            Err(err) => {
                crate::log_info!(
                    "feishu image attach failed message_id={} image_key={} error={}",
                    message_id,
                    image_key,
                    err
                );
            }
        }
    }
}

/// 从通用出站消息构造飞书收件人，适用于发送最终回复和工具消息。
fn feishu_recipient_from_outbound(message: &OutboundMessage) -> AppResult<FeishuRecipient<'_>> {
    let id = message.chat_id.trim();
    if id.is_empty() {
        return Err(AppError::Channel(
            "feishu outbound recipient id is empty".to_string(),
        ));
    }
    let id_type = match message.recipient {
        OutboundRecipient::Chat => FeishuReceiveIdType::ChatId,
        OutboundRecipient::User => {
            // 触发条件：cron 私聊任务没有可回复的原始消息。
            // 飞书常规 chat_id 路径不能主动定位用户，且用户 key 可能是 union_id。
            // 按 id 前缀选择 receive_id_type，避免 on_ 被误当 open_id 触发跨应用错误。
            feishu_user_receive_id_type(id)
        }
    };
    Ok(FeishuRecipient { id, id_type })
}

/// 从消息来源构造飞书收件人，适用于 request_user_input 主动发卡片。
fn feishu_recipient_from_source(source: &MessageSource) -> AppResult<FeishuRecipient<'_>> {
    if source.chat_type == "dm" && source.chat_id.trim().is_empty() {
        let Some(id) = source
            .user_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(AppError::Channel(
                "feishu user recipient id is empty".to_string(),
            ));
        };
        // 触发条件：cron 私聊任务需要主动发卡片。
        // 常规 request_user_input 只掌握 chat_id，会导致私聊计划无法通知。
        // 按 id 前缀选择用户 id 类型，防止 union_id 被误发成 open_id。
        return Ok(FeishuRecipient {
            id,
            id_type: feishu_user_receive_id_type(id),
        });
    }
    let id = source.chat_id.trim();
    if id.is_empty() {
        return Err(AppError::Channel(
            "feishu chat recipient id is empty".to_string(),
        ));
    }
    Ok(FeishuRecipient {
        id,
        id_type: FeishuReceiveIdType::ChatId,
    })
}

/// 从飞书用户 key 推导 receive_id_type，适用于主动私聊发送。
fn feishu_user_receive_id_type(id: &str) -> FeishuReceiveIdType {
    if id.starts_with("on_") {
        FeishuReceiveIdType::UnionId
    } else if id.starts_with("ou_") {
        FeishuReceiveIdType::OpenId
    } else {
        FeishuReceiveIdType::UserId
    }
}

impl Channel for FeishuChannel {
    /// 返回 channel 名称。
    fn name(&self) -> &str {
        &self.name
    }

    /// 启动飞书长连接。
    async fn start(&mut self, tx: mpsc::Sender<InboundMessage>, paths: &AppPaths) -> AppResult<()> {
        let api = Arc::clone(&self.api);
        let name = self.name.clone();
        let seen = Arc::clone(&self.seen);
        let thread_replies = Arc::clone(&self.thread_replies);
        let user_inputs = Arc::clone(&self.user_inputs);
        let store_root = paths.channel_store_dir.clone();
        let bot_cache_path = bot_open_id_cache_path(&paths.channel_data_dir, &name);
        let bot_open_id = resolve_bot_open_id(&api, &bot_cache_path, self.require_mention).await?;
        let ping_interval = self.ping_interval;
        let gate = MentionGate {
            require_mention: self.require_mention,
            bot_open_id,
            bot_name: self.bot_name.clone(),
        };
        self.task = Some(tokio::spawn(async move {
            let mut retry_delay = Duration::from_secs(3);
            loop {
                if tx.is_closed() {
                    crate::log_info!(
                        "feishu channel supervisor stopped reason=daemon_queue_closed"
                    );
                    break;
                }
                match run_ws_loop(
                    Arc::clone(&api),
                    tx.clone(),
                    name.clone(),
                    gate.clone(),
                    Arc::clone(&seen),
                    Arc::clone(&thread_replies),
                    Arc::clone(&user_inputs),
                    store_root.clone(),
                    ping_interval,
                )
                .await
                {
                    Ok(()) => {
                        crate::log_info!(
                            "feishu channel disconnected, reconnecting after {:?}",
                            retry_delay
                        );
                    }
                    Err(err) => {
                        crate::log_info!(
                            "feishu channel stopped: {err}; reconnecting after {:?}",
                            retry_delay
                        );
                    }
                }
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
            }
        }));
        Ok(())
    }

    /// 停止飞书长连接后台任务。
    async fn stop(&mut self) -> AppResult<()> {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.user_inputs.lock().await.clear();
        Ok(())
    }

    /// 发送飞书文本消息。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        let reply_in_thread = match message.reply_to.as_deref() {
            Some(message_id) => self.thread_replies.lock().await.remove(message_id),
            None => false,
        };
        crate::log_info!(
            "feishu outbound sending chat_id={} reply_to={} reply_in_thread={}",
            message.chat_id,
            message.reply_to.as_deref().unwrap_or(""),
            reply_in_thread
        );
        let message_id = self
            .api
            .send_text(
                feishu_recipient_from_outbound(&message)?,
                &message.text,
                message.reply_to.as_deref(),
                reply_in_thread,
                message.format,
            )
            .await?;
        Ok(SendResult {
            success: true,
            message_id,
        })
    }

    /// 更新飞书消息。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        self.tool_handle().update_message(message).await
    }

    /// 返回飞书确认能力，适用于 daemon 统一确认语义。
    fn ack_capability(&self, kind: ChannelAckKind) -> ChannelAckCapability {
        match kind {
            ChannelAckKind::StopDone => ChannelAckCapability::TextReply,
            ChannelAckKind::Received | ChannelAckKind::ResetDone => ChannelAckCapability::Reaction,
        }
    }

    /// 返回飞书能力集合，适用于上层按平台能力选择接口。
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            patch_message: true,
            append_update: false,
            request_user_input: true,
            reaction_ack: true,
            text_ack: true,
            chat_action: false,
            reply_threading: true,
            inbound_attachments: true,
            outbound_attachments: false,
        }
    }

    /// 按统一语义执行飞书确认反馈。
    async fn acknowledge(&self, message: &InboundMessage, kind: ChannelAckKind) -> AppResult<()> {
        match kind {
            ChannelAckKind::Received => self.tool_handle().acknowledge_received(message).await,
            ChannelAckKind::ResetDone => self.tool_handle().acknowledge_reset(message).await,
            ChannelAckKind::StopDone => self.tool_handle().acknowledge_stop(message).await,
        }
    }

    /// 发送飞书交互卡片并等待按钮选择。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        self.tool_handle().request_user_input(source, request).await
    }
}

/// 飞书长连接配置解析结果。
struct ResolvedFeishuConfig {
    /// open-apis base URL。
    base_url: String,
    /// 应用 app_id。
    app_id: String,
    /// 应用 app_secret。
    app_secret: String,
}

impl ResolvedFeishuConfig {
    /// 解析飞书配置，适用于 channel 启动前检查凭据。
    fn resolve(config: &FeishuChannelConfig) -> AppResult<Self> {
        let app_id = resolve_secret(config.app_id.as_deref(), config.app_id_env.as_deref())
            .ok_or_else(|| AppError::Channel("feishu app_id is required".to_string()))?;
        let app_secret = resolve_secret(
            config.app_secret.as_deref(),
            config.app_secret_env.as_deref(),
        )
        .ok_or_else(|| AppError::Channel("feishu app_secret is required".to_string()))?;
        let base_url = match config.domain.as_str() {
            "lark" => "https://open.larksuite.com".to_string(),
            "feishu" | "" => "https://open.feishu.cn".to_string(),
            other => {
                return Err(AppError::Channel(format!(
                    "unsupported feishu domain: {other}"
                )));
            }
        };
        Ok(Self {
            base_url,
            app_id,
            app_secret,
        })
    }
}

/// 构造机器人 open_id 缓存路径，适用于避开附件 TTL 清理。
fn bot_open_id_cache_path(channel_data_dir: &Path, channel_name: &str) -> PathBuf {
    channel_data_dir
        .join("feishu")
        .join(sanitize_filename(channel_name))
        .join("bot_open_id")
}

/// 解析机器人 open_id，适用于群聊 @ 门禁启动前初始化。
async fn resolve_bot_open_id(
    api: &Arc<FeishuApi>,
    cache_path: &Path,
    require_mention: bool,
) -> AppResult<String> {
    if !require_mention {
        return Ok(String::new());
    }
    if let Some(open_id) = read_cached_bot_open_id(cache_path).await? {
        crate::log_info!(
            "feishu bot open_id loaded from cache path={}",
            cache_path.display()
        );
        return Ok(open_id);
    }
    // 触发条件：群聊启用 @ 门禁但本地没有机器人 open_id 缓存。
    // 不能直接用 app_id：飞书消息 mention 返回的是 open_id。
    // 防止回归：群聊 @ 机器人时不再被误判为 missing_bot_mention。
    let open_id = api.bot_open_id().await?;
    write_cached_bot_open_id(cache_path, &open_id).await?;
    crate::log_info!("feishu bot open_id cached path={}", cache_path.display());
    Ok(open_id)
}

/// 读取机器人 open_id 缓存，适用于 daemon 重启后减少飞书 API 调用。
async fn read_cached_bot_open_id(path: &Path) -> AppResult<Option<String>> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    Ok(content
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned))
}

/// 写入机器人 open_id 缓存，适用于首次启动或缓存丢失后复用。
async fn write_cached_bot_open_id(path: &Path, open_id: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, format!("{}\n", open_id.trim())).await?;
    Ok(())
}

/// 群聊 @ 判断配置。
#[derive(Clone)]
struct MentionGate {
    /// 群聊是否要求 @。
    require_mention: bool,
    /// 机器人 open_id，用于识别群聊 mention。
    bot_open_id: String,
    /// 机器人名称，用于缺少 open_id 时兜底。
    bot_name: Option<String>,
}

/// 消息去重缓存。
struct DedupCache {
    /// 最大缓存数量。
    capacity: usize,
    /// 最近消息 id。
    order: VecDeque<String>,
}

/// 飞书子话题回复缓存。
struct ThreadReplyCache {
    /// 最大缓存数量。
    capacity: usize,
    /// 最近需要子话题回复的消息 id。
    order: VecDeque<String>,
    /// 快速查找集合。
    ids: HashSet<String>,
}

/// 等待中的飞书用户输入请求集合。
#[derive(Default)]
struct PendingUserInputs {
    /// 按请求 id 保存等待状态。
    items: HashMap<String, PendingUserInput>,
}

/// 单个飞书用户输入等待状态。
struct PendingUserInput {
    /// 触发请求的来源。
    source: Option<MessageSource>,
    /// 原始请求，用于超时自动选择。
    request: UserInputRequest,
    /// 已收集答案。
    answers: HashMap<String, Vec<String>>,
    /// 完成时唤醒 tool。
    sender: Option<oneshot::Sender<UserInputResponse>>,
    /// 飞书卡片消息 id，用于原地更新。
    card_message_id: Option<String>,
}

/// 用户输入推进结果。
enum UserInputAnswerResult {
    /// 没有匹配的等待项。
    Missing,
    /// 已接收答案，可选继续发送下一题。
    Accepted {
        /// 卡片更新请求；为空表示无法原地更新。
        update: Option<UserInputMessageUpdate>,
        /// 是否已经收齐所有问题。
        finished: bool,
    },
}

/// 用户输入消息更新请求。
struct UserInputMessageUpdate {
    /// 飞书卡片消息 id。
    message_id: String,
    /// 更新后的卡片 JSON。
    card: serde_json::Value,
}

impl PendingUserInputs {
    /// 插入新的等待项，适用于发送卡片前登记回调。
    fn insert(
        &mut self,
        request_id: String,
        request: UserInputRequest,
        sender: oneshot::Sender<UserInputResponse>,
    ) {
        self.items.insert(
            request_id,
            PendingUserInput {
                source: None,
                request,
                answers: HashMap::new(),
                sender: Some(sender),
                card_message_id: None,
            },
        );
    }

    /// 移除等待项，适用于发送卡片失败或 reset 清理。
    fn remove(&mut self, request_id: &str) {
        self.items.remove(request_id);
    }

    /// 记录一次按钮选择，适用于原地更新飞书卡片。
    fn answer(
        &mut self,
        request_id: &str,
        question_id: &str,
        value: String,
    ) -> UserInputAnswerResult {
        let Some(pending) = self.items.get_mut(request_id) else {
            return UserInputAnswerResult::Missing;
        };
        pending.answers.insert(question_id.to_string(), vec![value]);
        let finished = !has_unanswered_question(pending);
        let update = pending
            .card_message_id
            .as_ref()
            .map(|message_id| UserInputMessageUpdate {
                message_id: message_id.clone(),
                card: build_user_input_status_card(
                    request_id,
                    &pending.request,
                    &pending.answers,
                    finished,
                ),
            });
        if !finished {
            return UserInputAnswerResult::Accepted {
                update,
                finished: false,
            };
        }
        let Some(mut pending) = self.items.remove(request_id) else {
            return UserInputAnswerResult::Missing;
        };
        if let Some(sender) = pending.sender.take() {
            let _ = sender.send(UserInputResponse {
                answers: pending.answers,
            });
        }
        UserInputAnswerResult::Accepted {
            update,
            finished: true,
        }
    }

    /// 绑定请求来源，适用于普通文本作为自定义答案。
    fn bind_source(&mut self, request_id: &str, source: MessageSource) {
        if let Some(pending) = self.items.get_mut(request_id) {
            pending.source = Some(source);
        }
    }

    /// 绑定飞书卡片消息 id，适用于后续原地更新。
    fn bind_card_message(&mut self, request_id: &str, message_id: String) {
        if let Some(pending) = self.items.get_mut(request_id) {
            pending.card_message_id = Some(message_id);
        }
    }

    /// 将普通文本填入同来源的下一个未回答问题。
    fn answer_next_by_source(
        &mut self,
        source: &MessageSource,
        value: String,
    ) -> UserInputAnswerResult {
        let Some(request_id) = self.items.iter().find_map(|(request_id, pending)| {
            pending
                .source
                .as_ref()
                .is_some_and(|pending_source| same_user_input_source(pending_source, source))
                .then(|| request_id.clone())
        }) else {
            return UserInputAnswerResult::Missing;
        };
        let Some(question_id) = self
            .items
            .get(&request_id)
            .and_then(next_unanswered_question_id)
        else {
            return UserInputAnswerResult::Missing;
        };
        self.answer(&request_id, &question_id, value)
    }

    /// 超时后选择每题第一个选项，适用于模型允许自动决策时继续执行。
    fn auto_resolve(&mut self, request_id: &str) -> AppResult<UserInputResponse> {
        let Some(pending) = self.items.remove(request_id) else {
            return Err(AppError::Channel(
                "feishu user input request is missing".to_string(),
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

    /// 清空所有等待项，适用于 reset 或 channel 停止释放内存。
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
            .saturating_mul(std::mem::size_of::<(String, PendingUserInput)>());
        ResourceUsage::new(
            name,
            "hashmap",
            self.items.len(),
            Some(self.items.capacity()),
            entry_bytes.saturating_add(body_bytes),
        )
    }
}

impl PendingUserInput {
    /// 估算单个等待输入项容量。
    fn resource_bytes(&self) -> usize {
        self.source
            .as_ref()
            .map(estimate_message_source_bytes)
            .unwrap_or(0)
            + estimate_user_input_request_bytes(&self.request)
            + estimate_answers_bytes(&self.answers)
            + self
                .card_message_id
                .as_ref()
                .map(String::capacity)
                .unwrap_or(0)
    }
}

/// 构造首个飞书选择卡片，适用于 request_user_input 工具。
fn build_user_input_card(request_id: &str, request: &UserInputRequest) -> serde_json::Value {
    build_user_input_status_card(request_id, request, &HashMap::new(), false)
}

/// 构造飞书选择状态卡片，适用于原地展示已选项和最终状态。
fn build_user_input_status_card(
    request_id: &str,
    request: &UserInputRequest,
    answers: &HashMap<String, Vec<String>>,
    finished: bool,
) -> serde_json::Value {
    let mut elements = Vec::new();
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": if finished {
            "**已记录，处理中。**"
        } else {
            "**需要你确认**  没有合适选项时，直接在聊天里回复文字即可。"
        },
    }));
    for (index, question) in request.questions.iter().enumerate() {
        let answered = answers.contains_key(&question.id);
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": build_user_input_question_markdown(index + 1, question, answers),
        }));
        if !answered && !finished {
            elements.push(serde_json::json!({
                "tag": "column_set",
                "flex_mode": "flow",
                "horizontal_spacing": "4px",
                "columns": build_user_input_option_columns(request_id, question),
            }));
        }
    }
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
            "elements": elements,
        },
    })
}

/// 构造单个问题的选项列，适用于 v2 卡片压缩按钮上下间距。
fn build_user_input_option_columns(
    request_id: &str,
    question: &crate::message::UserInputQuestion,
) -> Vec<serde_json::Value> {
    question
        .options
        .iter()
        .map(|option| {
            let callback_value = serde_json::json!({
                "llm_loop": "request_user_input",
                "request_id": request_id,
                "question_id": question.id,
                "answer": option.label,
            });
            serde_json::json!({
                "tag": "column",
                "width": "auto",
                "padding": "0px",
                "elements": [{
                    "tag": "button",
                    "text": {
                        "tag": "plain_text",
                        "content": option.label,
                    },
                    "type": "default",
                    "size": "tiny",
                    "value": {
                        "llm_loop": "request_user_input",
                        "request_id": request_id,
                        "question_id": question.id,
                        "answer": option.label,
                    },
                    "behaviors": [{
                        "type": "callback",
                        "value": callback_value,
                    }],
                }],
            })
        })
        .collect()
}

/// 构造单个问题 markdown，适用于展示问题和已选结果。
fn build_user_input_question_markdown(
    index: usize,
    question: &crate::message::UserInputQuestion,
    answers: &HashMap<String, Vec<String>>,
) -> String {
    let answered = answers.contains_key(&question.id);
    let suffix = if answered { " ✅" } else { "" };
    let mut content = format!(
        "**{}. {}{}** {}",
        index, question.header, suffix, question.question
    );
    if let Some(answer) = answers
        .get(&question.id)
        .and_then(|values| values.first())
        .filter(|value| !value.trim().is_empty())
    {
        let other_options = question
            .options
            .iter()
            .map(|option| option.label.trim())
            .filter(|label| !label.is_empty() && *label != answer.trim())
            .collect::<Vec<_>>();
        content.push_str(&format!("\n已选：`{answer}`"));
        if !other_options.is_empty() {
            content.push_str(&format!(
                "  <font color=\"grey\">(其他选项：{})</font>",
                other_options.join(" / ")
            ));
        }
    }
    content
}

/// 判断两个用户输入来源是否属于同一用户会话。
fn same_user_input_source(left: &MessageSource, right: &MessageSource) -> bool {
    left.channel_name == right.channel_name
        && left.chat_id == right.chat_id
        && left.user_id == right.user_id
}

/// 返回下一个未回答问题 id。
fn next_unanswered_question_id(pending: &PendingUserInput) -> Option<String> {
    pending
        .request
        .questions
        .iter()
        .find(|question| !pending.answers.contains_key(&question.id))
        .map(|question| question.id.clone())
}

/// 判断是否还有未回答问题，适用于多问题卡片收齐后再继续。
fn has_unanswered_question(pending: &PendingUserInput) -> bool {
    pending
        .request
        .questions
        .iter()
        .any(|question| !pending.answers.contains_key(&question.id))
}

/// 处理飞书卡片按钮回调，适用于唤醒 request_user_input。
async fn handle_card_action(
    event: serde_json::Value,
    api: Arc<FeishuApi>,
    user_inputs: Arc<Mutex<PendingUserInputs>>,
) {
    crate::log_info!(
        "feishu card action received payload={}",
        log_json_preview(&event)
    );
    let Some(value) = find_action_value(&event) else {
        crate::log_info!("feishu card action ignored reason=missing_value");
        return;
    };
    if value.get("llm_loop").and_then(serde_json::Value::as_str) != Some("request_user_input") {
        crate::log_info!("feishu card action ignored reason=foreign_value");
        return;
    }
    let Some(request_id) = value.get("request_id").and_then(serde_json::Value::as_str) else {
        crate::log_info!("feishu card action ignored reason=missing_request_id");
        return;
    };
    let Some(question_id) = value.get("question_id").and_then(serde_json::Value::as_str) else {
        crate::log_info!("feishu card action ignored reason=missing_question_id");
        return;
    };
    let Some(answer) = value.get("answer").and_then(serde_json::Value::as_str) else {
        crate::log_info!("feishu card action ignored reason=missing_answer");
        return;
    };
    let result = user_inputs
        .lock()
        .await
        .answer(request_id, question_id, answer.to_string());
    let accepted = matches!(result, UserInputAnswerResult::Accepted { .. });
    match result {
        UserInputAnswerResult::Accepted {
            update: Some(update),
            finished: _,
        } => {
            spawn_user_input_card_patch(api, update);
        }
        UserInputAnswerResult::Accepted {
            update: None,
            finished: _,
        } => {}
        UserInputAnswerResult::Missing => {}
    }
    crate::log_info!(
        "feishu card action handled request_id={} question_id={} accepted={}",
        request_id,
        question_id,
        accepted
    );
}

/// 异步更新用户输入卡片，适用于避免飞书回调 ACK 覆盖已更新卡片。
fn spawn_user_input_card_patch(api: Arc<FeishuApi>, update: UserInputMessageUpdate) {
    tokio::spawn(async move {
        // 触发条件：用户点击交互卡片后需要立即刷新按钮状态。
        // 不能直接在回调 ACK 前等待 PATCH，飞书客户端可能用空 ACK 覆盖卡片。
        // 延后到 ACK 写回后执行，防止出现“先变更再闪回”的视觉回归。
        tokio::time::sleep(Duration::from_millis(120)).await;
        if let Err(err) = api
            .patch_interactive_card(&update.message_id, update.card)
            .await
        {
            crate::log_info!("feishu user input card patch failed: {err}");
        }
    });
}

/// 从飞书卡片回调里查找按钮 value。
fn find_action_value(event: &serde_json::Value) -> Option<&serde_json::Value> {
    event
        .pointer("/action/value")
        .or_else(|| event.pointer("/action/option"))
        .or_else(|| event.get("value"))
}

/// 生成 JSON 日志预览，避免卡片事件撑爆日志。
fn log_json_preview(value: &serde_json::Value) -> String {
    let raw = value.to_string();
    let mut output = raw.chars().take(1024).collect::<String>();
    if raw.chars().count() > 1024 {
        output.push_str("...");
    }
    output
}

impl ThreadReplyCache {
    /// 创建子话题回复缓存。
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(32),
            order: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    /// 记录需要以子话题回复的入站消息 id。
    fn insert(&mut self, message_id: &str) {
        if !self.ids.insert(message_id.to_string()) {
            return;
        }
        self.order.push_back(message_id.to_string());
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
    }

    /// 消费一次子话题回复标记。
    fn remove(&mut self, message_id: &str) -> bool {
        self.ids.remove(message_id)
    }

    /// 清空子话题回复标记，适用于 reset 后释放内存占用。
    fn clear(&mut self) {
        self.order.clear();
        self.ids.clear();
    }

    /// 返回子话题回复缓存资源估算。
    fn resource_usage(&self, name: &str) -> ResourceUsage {
        let order_bytes = self
            .order
            .capacity()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(self.order.iter().map(String::capacity).sum::<usize>());
        let ids_bytes = self
            .ids
            .capacity()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(self.ids.iter().map(String::capacity).sum::<usize>());
        ResourceUsage::new(
            name,
            "cache",
            self.order.len(),
            Some(self.capacity),
            order_bytes.saturating_add(ids_bytes),
        )
    }
}

impl DedupCache {
    /// 创建去重缓存。
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(32),
            order: VecDeque::new(),
        }
    }

    /// 记录消息 id，返回是否已经见过。
    fn seen_before(&mut self, message_id: &str) -> bool {
        if self.order.iter().any(|item| item == message_id) {
            return true;
        }
        self.order.push_back(message_id.to_string());
        while self.order.len() > self.capacity {
            self.order.pop_front();
        }
        false
    }

    /// 清空去重窗口，适用于 reset 后释放内存占用。
    fn clear(&mut self) {
        self.order.clear();
    }

    /// 返回去重缓存资源估算。
    fn resource_usage(&self, name: &str) -> ResourceUsage {
        let bytes = self
            .order
            .capacity()
            .saturating_mul(std::mem::size_of::<String>())
            .saturating_add(self.order.iter().map(String::capacity).sum::<usize>());
        ResourceUsage::new(name, "cache", self.order.len(), Some(self.capacity), bytes)
    }
}

/// 运行飞书 WebSocket 循环。
async fn run_ws_loop(
    api: Arc<FeishuApi>,
    tx: mpsc::Sender<InboundMessage>,
    name: String,
    gate: MentionGate,
    seen: Arc<Mutex<DedupCache>>,
    thread_replies: Arc<Mutex<ThreadReplyCache>>,
    user_inputs: Arc<Mutex<PendingUserInputs>>,
    store_root: PathBuf,
    ping_interval: Duration,
) -> AppResult<()> {
    let endpoint = api.ws_endpoint().await?;
    let service_id = parse_service_id(&endpoint).unwrap_or_default();
    let (stream, _) = connect_async(&endpoint)
        .await
        .map_err(|err| AppError::Channel(format!("feishu websocket connect failed: {err}")))?;
    crate::log_info!("feishu websocket connected service_id={service_id}");
    let (writer, mut reader) = stream.split();
    let writer = Arc::new(Mutex::new(writer));
    let ping_writer = Arc::clone(&writer);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(ping_interval);
        loop {
            interval.tick().await;
            let frame = ping_frame(service_id);
            let bytes = frame.encode_to_vec();
            if ping_writer
                .lock()
                .await
                .send(WsMessage::Binary(bytes.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    while let Some(item) = reader.next().await {
        let message = item.map_err(|err| AppError::Channel(format!("feishu ws read: {err}")))?;
        let WsMessage::Binary(bytes) = message else {
            continue;
        };
        let frame = FeishuFrame::decode(bytes.as_ref())
            .map_err(|err| AppError::Channel(format!("feishu frame decode failed: {err}")))?;
        if let Some(response) = FeishuChannel::handle_frame(
            frame,
            Arc::clone(&api),
            tx.clone(),
            name.clone(),
            gate.clone(),
            Arc::clone(&seen),
            Arc::clone(&thread_replies),
            Arc::clone(&user_inputs),
            store_root.clone(),
        )
        .await?
        {
            writer
                .lock()
                .await
                .send(WsMessage::Binary(response.encode_to_vec().into()))
                .await
                .map_err(|err| AppError::Channel(format!("feishu ws write: {err}")))?;
        }
    }
    Ok(())
}

/// 解析配置或环境变量中的密钥。
fn resolve_secret(value: Option<&str>, env_key: Option<&str>) -> Option<String> {
    value
        .filter(|item| !item.trim().is_empty())
        .map(|item| item.trim().to_string())
        .or_else(|| {
            env_key
                .and_then(|key| std::env::var(key).ok())
                .filter(|item| !item.trim().is_empty())
        })
}

/// 判断事件是否应丢弃。
async fn should_drop_event(
    event: &FeishuReceiveMessageEvent,
    gate: &MentionGate,
    seen: Arc<Mutex<DedupCache>>,
) -> bool {
    crate::log_info!(
        "feishu event received message_id={} chat_type={} chat_id={} thread_id={} root_id={} parent_id={} sender_union_id={} sender_user_id={} sender_open_id={} message_type={} text={}",
        event.message.message_id,
        event.message.chat_type,
        event.message.chat_id,
        event.message.thread_id.as_deref().unwrap_or(""),
        event.message.root_id.as_deref().unwrap_or(""),
        event.message.parent_id.as_deref().unwrap_or(""),
        event.sender.sender_id.union_id.as_deref().unwrap_or(""),
        event.sender.sender_id.user_id.as_deref().unwrap_or(""),
        event.sender.sender_id.open_id.as_deref().unwrap_or(""),
        event.message.message_type,
        event
            .message
            .text()
            .map(|text| log_preview(&text))
            .unwrap_or_else(|| "<unsupported>".to_string())
    );
    if event.message.message_id.trim().is_empty() {
        crate::log_info!("feishu event dropped reason=empty_message_id");
        return true;
    }
    if seen.lock().await.seen_before(&event.message.message_id) {
        crate::log_info!(
            "feishu event dropped reason=duplicate message_id={}",
            event.message.message_id
        );
        return true;
    }
    if event.message.chat_type == "p2p" || !gate.require_mention {
        return false;
    }
    // 触发条件：飞书群聊启用 @ 门禁。
    // 不能只判断文本里有 @_user_1：这是飞书占位符，
    // @ 任意成员都会出现。
    // 防止回归：用户 @ 其他人时不再误触发机器人。
    let mentioned = event.mentions_bot(&gate.bot_open_id, gate.bot_name.as_deref());
    if !mentioned {
        crate::log_info!(
            "feishu event dropped reason=missing_bot_mention message_id={} mentions={}",
            event.message.message_id,
            event.mentions_summary()
        );
    }
    !mentioned
}

/// 当前 mini 版不做拆包缓存，遇到拆包直接拒绝。
fn combine_payload_if_needed(frame: &FeishuFrame) -> AppResult<Vec<u8>> {
    let sum = header_value(&frame.headers, HEADER_SUM)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    let seq = header_value(&frame.headers, HEADER_SEQ)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if sum > 1 || seq > 0 {
        return Err(AppError::Channel(
            "feishu split websocket frame is not supported yet".to_string(),
        ));
    }
    Ok(frame.payload.clone())
}

/// 构造飞书 ping frame。
fn ping_frame(service_id: i32) -> FeishuFrame {
    FeishuFrame {
        method: FRAME_TYPE_CONTROL,
        service: service_id,
        headers: vec![FeishuHeader {
            key: HEADER_TYPE.to_string(),
            value: MESSAGE_TYPE_PING.to_string(),
        }],
        ..FeishuFrame::default()
    }
}

/// 构造飞书事件 ACK frame。
fn response_frame(
    mut frame: FeishuFrame,
    code: i32,
    data: Option<serde_json::Value>,
) -> AppResult<FeishuFrame> {
    let payload = serde_json::json!({
        "code": code,
        "headers": {},
        "data": data.map(|value| value.to_string().into_bytes()).unwrap_or_default(),
    });
    frame.payload = serde_json::to_vec(&payload)?;
    Ok(frame)
}

/// 获取 frame header。
fn header_value(headers: &[FeishuHeader], key: &str) -> Option<String> {
    headers
        .iter()
        .find(|header| header.key == key)
        .map(|header| header.value.clone())
}

/// 从 endpoint query 里提取 service_id。
fn parse_service_id(endpoint: &str) -> Option<i32> {
    endpoint.split('?').nth(1)?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "service_id")
            .then(|| value.parse::<i32>().ok())
            .flatten()
    })
}

/// 生成日志预览，适用于避免长文本撑爆 daemon 日志。
fn log_preview(text: &str) -> String {
    const MAX_CHARS: usize = 120;
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut output = compact.chars().take(MAX_CHARS).collect::<String>();
    if compact.chars().count() > MAX_CHARS {
        output.push_str("...");
    }
    output
}

/// 清理平台文件名，避免路径穿越和奇怪分隔符。
fn sanitize_filename(filename: &str) -> String {
    let cleaned = filename
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed
    }
}

impl FeishuReceiveMessageEvent {
    /// 转换为 daemon 入站消息。
    fn to_inbound(&self, channel_name: String, platform: &str) -> Option<InboundMessage> {
        let text = self.message.text()?;
        Some(self.to_inbound_with_text(channel_name, platform, text))
    }

    /// 使用指定文本转换为 daemon 入站消息。
    fn to_inbound_with_text(
        &self,
        channel_name: String,
        platform: &str,
        text: String,
    ) -> InboundMessage {
        let sender = &self.sender.sender_id;
        let source = MessageSource {
            channel_name,
            platform: platform.to_string(),
            chat_id: self.message.chat_id.clone(),
            chat_type: if self.message.chat_type == "p2p" {
                "dm".to_string()
            } else {
                "group".to_string()
            },
            user_id: sender
                .union_id
                .clone()
                .or_else(|| sender.user_id.clone())
                .or_else(|| sender.open_id.clone()),
            thread_id: self
                .message
                .thread_id
                .clone()
                .or_else(|| self.message.root_id.clone())
                .or_else(|| self.message.parent_id.clone()),
        };
        InboundMessage::text(text, source, Some(self.message.message_id.clone()))
    }
}

impl event::FeishuMessage {
    /// 判断入站消息是否来自飞书回复链，适用于决定回包是否进入子话题。
    fn is_reply_context(&self) -> bool {
        self.thread_id
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            || self
                .root_id
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .parent_id
                .as_deref()
                .is_some_and(|value| !value.is_empty())
    }
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod mod_test;
