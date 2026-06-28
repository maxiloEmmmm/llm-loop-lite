use tokio::sync::mpsc;

mod attachments;
mod builtin;
mod capability;
mod feishu;
mod qq;
mod telegram;

#[cfg(test)]
mod qq_test;

pub use builtin::{BuiltinChannel, BuiltinChannelHandle, build_channels};
pub use capability::ChannelCapabilities;

use crate::error::AppResult;
use crate::home::AppPaths;
use crate::message::{
    InboundMessage, MessageSource, MessageUpdate, OutboundMessage, SendResult, UserInputRequest,
    UserInputResponse,
};

/// channel 确认语义，由 daemon 表达业务意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAckKind {
    /// 入站消息已被 daemon 接住。
    Received,
    /// reset 已完成。
    ResetDone,
    /// stop 已完成。
    StopDone,
}

/// channel 确认能力，由具体平台声明最合适的反馈方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelAckCapability {
    /// 使用平台 reaction 或附属表情。
    Reaction,
    /// 使用短文本回复。
    TextReply,
    /// 使用输入中等临时状态。
    ChatAction,
    /// 不提供确认反馈。
    None,
}

/// channel 抽象，负责连接外部消息源、标准化入站消息、发送出站消息。
#[allow(async_fn_in_trait)]
pub trait Channel: Send {
    /// 返回 channel 实例名，适用于路由和日志。
    fn name(&self) -> &str;

    /// 启动 channel，并将入站消息推入 daemon 队列。
    async fn start(&mut self, tx: mpsc::Sender<InboundMessage>, paths: &AppPaths) -> AppResult<()>;

    /// 停止 channel，适用于 daemon 关闭或重载。
    async fn stop(&mut self) -> AppResult<()>;

    /// 发送出站消息到平台。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult>;

    /// 更新之前由本 channel 发送的消息。
    async fn update_message(&self, _message: MessageUpdate) -> AppResult<SendResult> {
        Err(crate::error::AppError::Channel(
            "update_message is not implemented by this channel".to_string(),
        ))
    }

    /// 返回确认能力，适用于 daemon 选择统一确认链路。
    fn ack_capability(&self, _kind: ChannelAckKind) -> ChannelAckCapability {
        ChannelAckCapability::None
    }

    /// 返回 channel 能力集合，适用于上层按能力选择接口。
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::none()
    }

    /// 执行确认反馈，具体平台自行选择 reaction、文本或临时状态。
    async fn acknowledge(&self, _message: &InboundMessage, _kind: ChannelAckKind) -> AppResult<()> {
        Ok(())
    }

    /// 向 channel 请求结构化用户输入。
    async fn request_user_input(
        &self,
        _source: &MessageSource,
        _request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        Err(crate::error::AppError::Channel(
            "request_user_input is not implemented by this channel".to_string(),
        ))
    }
}
