use serde::{Deserialize, Serialize};

/// channel 能力集合，适用于 daemon 和 CLI 统一理解平台差异。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelCapabilities {
    /// 是否能原地修改已发送消息。
    pub patch_message: bool,
    /// 是否能用追加新消息模拟更新。
    pub append_update: bool,
    /// 是否能请求结构化用户输入。
    pub request_user_input: bool,
    /// 是否能使用 reaction 或附属表情确认收到。
    pub reaction_ack: bool,
    /// 是否能用文本确认状态。
    pub text_ack: bool,
    /// 是否能发送 typing/chat action。
    pub chat_action: bool,
    /// 是否能保留回复线程或引用关系。
    pub reply_threading: bool,
    /// 是否能读取入站附件。
    pub inbound_attachments: bool,
    /// 是否能发送出站附件。
    pub outbound_attachments: bool,
}

impl ChannelCapabilities {
    /// 返回无额外能力的基线，适用于默认 trait 实现。
    pub fn none() -> Self {
        Self {
            patch_message: false,
            append_update: false,
            request_user_input: false,
            reaction_ack: false,
            text_ack: false,
            chat_action: false,
            reply_threading: false,
            inbound_attachments: false,
            outbound_attachments: false,
        }
    }
}
