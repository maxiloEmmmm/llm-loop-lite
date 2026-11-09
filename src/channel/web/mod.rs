use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::channel::user_input::{PendingInputMetadata, PendingUserInputs, wait_for_user_input};
use crate::channel::{
    Channel, ChannelAckCapability, ChannelAckKind, ChannelCapabilities, resolve_channel_secret,
};
use crate::config::ChannelConfig;
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::message::{
    InboundMessage, MessageSource, MessageUpdate, OutboundFormat, OutboundMessage, SendResult,
    UserInputRequest, UserInputResponse,
};
use crate::query::QueryService;
use crate::resource::ResourceUsage;
use crate::tools::registry::ToolChannel;

use clients::ClientRegistry;
use protocol::ServerEvent;
use server::WebServerState;

mod clients;
mod protocol;
mod server;

/// Web 结构化输入附加状态，适用于把工具问题路由回原浏览器会话。
pub(super) struct WebInputMetadata {
    /// 浏览器 conversation ID。
    chat_id: String,
}

impl PendingInputMetadata for WebInputMetadata {
    /// 返回 conversation ID 动态容量，例如 `chat-1 -> 6 bytes`。
    fn resource_bytes(&self) -> usize {
        self.chat_id.capacity()
    }
}

/// Web 浏览器 channel，负责 HTTP 生命周期并复用 daemon 对话主链。
pub struct WebChannel {
    /// channel 实例名。
    name: String,
    /// HTTP 监听地址。
    bind: SocketAddr,
    /// React 构建产物目录。
    static_dir: PathBuf,
    /// 可选浏览器连接令牌。
    auth_token: Option<String>,
    /// 单条入站正文最大字节数。
    max_message_bytes: usize,
    /// 无状态独立查询服务。
    query_service: QueryService,
    /// 在线浏览器连接表。
    clients: ClientRegistry,
    /// 等待中的结构化输入请求。
    pending_inputs: Arc<Mutex<PendingUserInputs<WebInputMetadata>>>,
    /// HTTP 服务退出令牌。
    shutdown: CancellationToken,
    /// HTTP 服务任务。
    server_task: Option<JoinHandle<()>>,
}

/// Web channel 轻量句柄，供 daemon 工具和回复链共享。
#[derive(Clone)]
pub struct WebChannelHandle {
    /// channel 实例名。
    name: String,
    /// 在线浏览器连接表。
    clients: ClientRegistry,
    /// 等待中的结构化输入请求。
    pending_inputs: Arc<Mutex<PendingUserInputs<WebInputMetadata>>>,
}

impl WebChannel {
    /// 从 channel 配置构造 Web 服务，例如 `kind=web -> WebChannel`。
    pub fn from_config(config: &ChannelConfig, query_service: QueryService) -> AppResult<Self> {
        let bind = config.web.bind.parse::<SocketAddr>().map_err(|err| {
            AppError::Channel(format!("web bind `{}` is invalid: {err}", config.web.bind))
        })?;
        let name = if config.name.trim().is_empty() {
            "web".to_string()
        } else {
            config.name.trim().to_string()
        };
        Ok(Self {
            name,
            bind,
            static_dir: PathBuf::from(config.web.static_dir.trim()),
            auth_token: resolve_channel_secret(
                config.web.auth_token.as_deref(),
                config.web.auth_token_env.as_deref(),
            ),
            max_message_bytes: config.web.max_message_bytes.max(1),
            query_service,
            clients: ClientRegistry::default(),
            pending_inputs: Arc::new(Mutex::new(PendingUserInputs::new())),
            shutdown: CancellationToken::new(),
            server_task: None,
        })
    }

    /// 返回轻量发送句柄，例如 `channel -> handle`。
    pub fn tool_handle(&self) -> WebChannelHandle {
        WebChannelHandle {
            name: self.name.clone(),
            clients: self.clients.clone(),
            pending_inputs: Arc::clone(&self.pending_inputs),
        }
    }
}

impl Channel for WebChannel {
    /// 返回 channel 实例名，例如 `web-main`。
    fn name(&self) -> &str {
        &self.name
    }

    /// 启动 HTTP、WebSocket 和磁盘静态文件服务，例如 `start -> 127.0.0.1:8787`。
    async fn start(
        &mut self,
        tx: mpsc::Sender<InboundMessage>,
        _paths: &AppPaths,
    ) -> AppResult<()> {
        if !self.static_dir.join("index.html").is_file() {
            return Err(AppError::Channel(format!(
                "web static index is missing: {}",
                self.static_dir.join("index.html").display()
            )));
        }
        let listener = TcpListener::bind(self.bind)
            .await
            .map_err(|err| AppError::Channel(format!("web bind {} failed: {err}", self.bind)))?;
        let state = WebServerState {
            channel_name: self.name.clone(),
            auth_token: self.auth_token.clone(),
            max_message_bytes: self.max_message_bytes,
            inbound_tx: tx,
            clients: self.clients.clone(),
            pending_inputs: Arc::clone(&self.pending_inputs),
            query_service: self.query_service.clone(),
        };
        let static_dir = self.static_dir.clone();
        let shutdown = self.shutdown.clone();
        let bind = self.bind;
        self.server_task = Some(tokio::spawn(async move {
            crate::log_info!("web channel listening bind={bind}");
            if let Err(err) = server::serve(listener, state, static_dir, shutdown).await {
                crate::log_info!("web channel stopped with error: {err}");
            }
        }));
        Ok(())
    }

    /// 停止 HTTP 服务并释放等待项，例如 `stop -> no listeners`。
    async fn stop(&mut self) -> AppResult<()> {
        self.shutdown.cancel();
        self.pending_inputs.lock().await.clear();
        if let Some(task) = self.server_task.take() {
            task.await
                .map_err(|err| AppError::Channel(format!("web server join failed: {err}")))?;
        }
        Ok(())
    }

    /// 向浏览器新增消息，例如 `OutboundMessage -> message event`。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        self.tool_handle().send(message).await
    }

    /// 更新浏览器中已有消息，例如 `MessageUpdate -> message_updated event`。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        self.tool_handle().update_message(message).await
    }

    /// 返回 Web 确认能力，例如 `Received -> TextReply`。
    fn ack_capability(&self, _kind: ChannelAckKind) -> ChannelAckCapability {
        ChannelAckCapability::TextReply
    }

    /// 返回 Web channel 能力，例如 `web -> patch + user input`。
    fn capabilities(&self) -> ChannelCapabilities {
        web_capabilities()
    }

    /// 把 daemon 确认状态推送给浏览器，例如 `Received -> ack`。
    async fn acknowledge(&self, message: &InboundMessage, kind: ChannelAckKind) -> AppResult<()> {
        self.tool_handle().acknowledge(message, kind).await
    }

    /// 请求浏览器完成结构化用户输入，例如 `questions -> answers`。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        self.tool_handle().request_user_input(source, request).await
    }
}

impl WebChannelHandle {
    /// 返回 channel 实例名，例如 `handle -> web-main`。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 返回平台名，例如 `handle -> web`。
    pub fn platform_name(&self) -> &str {
        "web"
    }

    /// 返回 Web 确认能力，例如 `Received -> TextReply`。
    pub fn ack_capability(&self, _kind: ChannelAckKind) -> ChannelAckCapability {
        ChannelAckCapability::TextReply
    }

    /// 返回 Web channel 能力，例如 `handle -> patch + user input`。
    pub fn capabilities(&self) -> ChannelCapabilities {
        web_capabilities()
    }

    /// 推送 daemon 确认状态，例如 `message + Received -> ack`。
    pub async fn acknowledge(
        &self,
        message: &InboundMessage,
        kind: ChannelAckKind,
    ) -> AppResult<()> {
        let kind = match kind {
            ChannelAckKind::Received => "received",
            ChannelAckKind::ResetDone => "reset_done",
            ChannelAckKind::StopDone => "stop_done",
        };
        self.clients
            .send(
                &message.source.chat_id,
                ServerEvent::Ack {
                    message_id: message.message_id.clone(),
                    kind: kind.to_string(),
                },
            )
            .await;
        Ok(())
    }

    /// 返回连接表与输入等待表资源估算，例如 `handle -> 2 ResourceUsage`。
    pub async fn resource_usage(&self) -> Vec<ResourceUsage> {
        let clients = self
            .clients
            .resource_usage(&format!("channel.{}.clients", self.name))
            .await;
        let pending = self
            .pending_inputs
            .lock()
            .await
            .resource_usage(&format!("channel.{}.pending_inputs", self.name));
        vec![clients, pending]
    }
}

#[async_trait]
impl ToolChannel for WebChannelHandle {
    /// 向浏览器新增消息，例如 `OutboundMessage -> message event`。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        let message_id = uuid::Uuid::new_v4().simple().to_string();
        let delivered = self
            .clients
            .send(
                &message.chat_id,
                ServerEvent::Message {
                    id: message_id.clone(),
                    text: message.text,
                    reply_to: message.reply_to,
                    format: format_name(message.format).to_string(),
                },
            )
            .await;
        if delivered == 0 {
            return Err(AppError::Channel(format!(
                "web conversation `{}` has no online client",
                message.chat_id
            )));
        }
        Ok(SendResult {
            success: true,
            message_id: Some(message_id),
        })
    }

    /// 更新浏览器中已有消息，例如 `MessageUpdate -> message_updated event`。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        let chat_id = message.chat_id.unwrap_or_default();
        let delivered = self
            .clients
            .send(
                &chat_id,
                ServerEvent::MessageUpdated {
                    id: message.message_id.clone(),
                    text: message.text,
                    format: format_name(message.format).to_string(),
                },
            )
            .await;
        if delivered == 0 {
            return Err(AppError::Channel(format!(
                "web conversation `{chat_id}` has no online client"
            )));
        }
        Ok(SendResult {
            success: true,
            message_id: Some(message.message_id),
        })
    }

    /// 请求浏览器完成结构化用户输入，例如 `questions -> answers`。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        let timeout_ms = request.auto_resolution_ms;
        let waiter = self.pending_inputs.lock().await.start(
            request.clone(),
            WebInputMetadata {
                chat_id: source.chat_id.clone(),
            },
        );
        let request_id = waiter.request_id().to_string();
        let delivered = self
            .clients
            .send(
                &source.chat_id,
                ServerEvent::InputRequest {
                    request_id: request_id.clone(),
                    questions: request.questions,
                },
            )
            .await;
        if delivered == 0 {
            self.pending_inputs.lock().await.remove(&request_id);
            return Err(AppError::Channel(format!(
                "web conversation `{}` disconnected before user input",
                source.chat_id
            )));
        }
        wait_for_user_input(&self.pending_inputs, waiter, timeout_ms, &self.name).await
    }
}

/// 返回 Web channel 固定能力集合，例如 `web -> ChannelCapabilities`。
fn web_capabilities() -> ChannelCapabilities {
    ChannelCapabilities {
        patch_message: true,
        append_update: false,
        request_user_input: true,
        reaction_ack: false,
        text_ack: true,
        chat_action: false,
        reply_threading: true,
        inbound_attachments: false,
        outbound_attachments: false,
    }
}

/// 把统一消息格式转换成浏览器展示名，例如 `Plan -> plan`。
fn format_name(format: OutboundFormat) -> &'static str {
    match format {
        OutboundFormat::Text => "text",
        OutboundFormat::Plan => "plan",
    }
}
