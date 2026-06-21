use tokio::sync::mpsc;

use crate::channel::feishu::{FeishuChannel, FeishuChannelHandle};
use crate::channel::qq::{QqChannel, QqChannelHandle};
use crate::channel::telegram::{TelegramChannel, TelegramChannelHandle};
use crate::channel::{Channel, ChannelAckCapability, ChannelAckKind, ChannelCapabilities};
use crate::config::ChannelConfig;
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::message::{
    InboundMessage, MessageSource, MessageUpdate, OutboundMessage, SendResult, UserInputRequest,
    UserInputResponse,
};
use crate::resource::ResourceUsage;
use crate::tools::registry::ToolChannel;

/// 内置 channel 枚举，负责把配置里的 kind 分派到具体实现。
pub enum BuiltinChannel {
    /// 飞书/Lark 自建应用 channel。
    Feishu(FeishuChannel),
    /// QQ 官方机器人 channel。
    Qq(QqChannel),
    /// Telegram Bot API channel。
    Telegram(TelegramChannel),
}

/// 内置 channel 的轻量工具句柄。
#[derive(Clone)]
pub enum BuiltinChannelHandle {
    /// 飞书/Lark 发送与更新句柄。
    Feishu(FeishuChannelHandle),
    /// QQ 官方机器人发送句柄。
    Qq(QqChannelHandle),
    /// Telegram Bot API 发送句柄。
    Telegram(TelegramChannelHandle),
}

impl Channel for BuiltinChannel {
    /// 返回内置 channel 名称。
    fn name(&self) -> &str {
        match self {
            Self::Feishu(channel) => channel.name(),
            Self::Qq(channel) => channel.name(),
            Self::Telegram(channel) => channel.name(),
        }
    }

    /// 启动内置 channel。
    async fn start(&mut self, tx: mpsc::Sender<InboundMessage>, paths: &AppPaths) -> AppResult<()> {
        match self {
            Self::Feishu(channel) => channel.start(tx, paths).await,
            Self::Qq(channel) => channel.start(tx, paths).await,
            Self::Telegram(channel) => channel.start(tx, paths).await,
        }
    }

    /// 停止内置 channel。
    async fn stop(&mut self) -> AppResult<()> {
        match self {
            Self::Feishu(channel) => channel.stop().await,
            Self::Qq(channel) => channel.stop().await,
            Self::Telegram(channel) => channel.stop().await,
        }
    }

    /// 发送内置 channel 消息。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        match self {
            Self::Feishu(channel) => channel.send(message).await,
            Self::Qq(channel) => channel.send(message).await,
            Self::Telegram(channel) => channel.send(message).await,
        }
    }

    /// 更新内置 channel 消息。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        match self {
            Self::Feishu(channel) => channel.update_message(message).await,
            Self::Qq(channel) => channel.update_message(message).await,
            Self::Telegram(channel) => channel.update_message(message).await,
        }
    }

    /// 分派确认能力查询到具体内置 channel。
    fn ack_capability(&self, kind: ChannelAckKind) -> ChannelAckCapability {
        match self {
            Self::Feishu(channel) => channel.ack_capability(kind),
            Self::Qq(channel) => channel.ack_capability(kind),
            Self::Telegram(channel) => channel.ack_capability(kind),
        }
    }

    /// 分派能力集合查询到具体内置 channel。
    fn capabilities(&self) -> ChannelCapabilities {
        match self {
            Self::Feishu(channel) => channel.capabilities(),
            Self::Qq(channel) => channel.capabilities(),
            Self::Telegram(channel) => channel.capabilities(),
        }
    }

    /// 分派确认反馈到具体内置 channel。
    async fn acknowledge(&self, message: &InboundMessage, kind: ChannelAckKind) -> AppResult<()> {
        match self {
            Self::Feishu(channel) => channel.acknowledge(message, kind).await,
            Self::Qq(channel) => channel.acknowledge(message, kind).await,
            Self::Telegram(channel) => channel.acknowledge(message, kind).await,
        }
    }

    /// 分派用户输入请求到具体内置 channel。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        match self {
            Self::Feishu(channel) => channel.request_user_input(source, request).await,
            Self::Qq(channel) => channel.request_user_input(source, request).await,
            Self::Telegram(channel) => channel.request_user_input(source, request).await,
        }
    }
}

impl BuiltinChannelHandle {
    /// 返回内置 channel 句柄名称。
    pub fn name(&self) -> &str {
        match self {
            Self::Feishu(channel) => channel.name(),
            Self::Qq(channel) => channel.name(),
            Self::Telegram(channel) => channel.name(),
        }
    }

    /// 返回平台名称，适用于 cron 构造虚拟入站来源。
    pub fn platform_name(&self) -> &str {
        match self {
            Self::Feishu(channel) => channel.platform_name(),
            Self::Qq(channel) => channel.platform_name(),
            Self::Telegram(channel) => channel.platform_name(),
        }
    }

    /// 返回内置 channel 确认能力。
    pub fn ack_capability(&self, kind: ChannelAckKind) -> ChannelAckCapability {
        match self {
            Self::Feishu(channel) => channel.ack_capability(kind),
            Self::Qq(channel) => channel.ack_capability(kind),
            Self::Telegram(channel) => channel.ack_capability(kind),
        }
    }

    /// 返回内置 channel 能力集合。
    pub fn capabilities(&self) -> ChannelCapabilities {
        match self {
            Self::Feishu(channel) => channel.capabilities(),
            Self::Qq(channel) => channel.capabilities(),
            Self::Telegram(channel) => channel.capabilities(),
        }
    }

    /// 返回内置 channel 句柄缓存资源估算。
    pub async fn resource_usage(&self) -> Vec<ResourceUsage> {
        match self {
            Self::Feishu(channel) => channel.resource_usage().await,
            Self::Qq(channel) => channel.resource_usage().await,
            Self::Telegram(channel) => channel.resource_usage().await,
        }
    }

    /// 按统一语义给入站消息添加确认反馈。
    pub async fn acknowledge(
        &self,
        message: &InboundMessage,
        kind: ChannelAckKind,
    ) -> AppResult<()> {
        match self {
            Self::Feishu(channel) => channel.acknowledge(message, kind).await,
            Self::Qq(channel) => channel.acknowledge(message, kind).await,
            Self::Telegram(channel) => channel.acknowledge(message, kind).await,
        }
    }
}

#[async_trait::async_trait]
impl ToolChannel for BuiltinChannelHandle {
    /// 通过内置 channel 发送消息。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult> {
        match self {
            Self::Feishu(channel) => channel.send(message).await,
            Self::Qq(channel) => channel.send(message).await,
            Self::Telegram(channel) => channel.send(message).await,
        }
    }

    /// 通过内置 channel 更新消息。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        match self {
            Self::Feishu(channel) => channel.update_message(message).await,
            Self::Qq(channel) => channel.update_message(message).await,
            Self::Telegram(channel) => channel.update_message(message).await,
        }
    }

    /// 通过内置 channel 请求用户输入。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        match self {
            Self::Feishu(channel) => channel.request_user_input(source, request).await,
            Self::Qq(channel) => channel.request_user_input(source, request).await,
            Self::Telegram(channel) => channel.request_user_input(source, request).await,
        }
    }
}

impl BuiltinChannel {
    /// 返回轻量工具句柄，适用于本轮 tool 调用发消息。
    pub fn tool_handle(&self) -> BuiltinChannelHandle {
        match self {
            Self::Feishu(channel) => BuiltinChannelHandle::Feishu(channel.tool_handle()),
            Self::Qq(channel) => BuiltinChannelHandle::Qq(channel.tool_handle()),
            Self::Telegram(channel) => BuiltinChannelHandle::Telegram(channel.tool_handle()),
        }
    }
}

/// 从配置构造内置 channel 列表。
pub fn build_channels(configs: &[ChannelConfig]) -> AppResult<Vec<BuiltinChannel>> {
    let mut channels = Vec::new();
    for config in configs.iter().filter(|config| config.enabled) {
        match config.kind.as_str() {
            "feishu" | "lark" => {
                channels.push(BuiltinChannel::Feishu(FeishuChannel::from_config(config)?));
            }
            "qq" | "qqbot" => {
                channels.push(BuiltinChannel::Qq(QqChannel::from_config(config)?));
            }
            "telegram" | "tg" => {
                channels.push(BuiltinChannel::Telegram(TelegramChannel::from_config(
                    config,
                )?));
            }
            other => return Err(AppError::UnsupportedChannel(other.to_string())),
        }
    }
    Ok(channels)
}
