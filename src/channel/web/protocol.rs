use serde::{Deserialize, Serialize};

use crate::message::UserInputQuestion;

/// 浏览器发往 Web channel 的事件，适用于 WebSocket 文本帧，例如 `hello -> ready`。
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ClientEvent {
    /// 建立已认证会话，必须作为连接后的第一条事件。
    Hello {
        /// 可选共享令牌；服务端未配置令牌时允许缺省。
        token: Option<String>,
        /// 浏览器持久化客户端 ID，用于隔离用户 session。
        #[serde(rename = "clientId")]
        client_id: String,
        /// 页面持久化会话 ID，用于路由回包。
        #[serde(rename = "conversationId")]
        conversation_id: String,
    },
    /// 用户发送普通文本消息。
    Message {
        /// 浏览器生成的消息 ID。
        id: String,
        /// 用户输入正文。
        text: String,
    },
    /// 用户回答结构化输入问题。
    Answer {
        /// 服务端生成的输入请求 ID。
        #[serde(rename = "requestId")]
        request_id: String,
        /// 工具问题 ID。
        #[serde(rename = "questionId")]
        question_id: String,
        /// 用户选择或输入的值。
        value: String,
    },
    /// 保活请求。
    Ping,
}

/// Web channel 发往浏览器的事件，适用于 UI 增量更新，例如 `message -> bubble`。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ServerEvent {
    /// 连接认证完成。
    Ready {
        /// 当前 channel 实例名。
        #[serde(rename = "channelName")]
        channel_name: String,
        /// 当前会话 ID。
        #[serde(rename = "conversationId")]
        conversation_id: String,
    },
    /// 新增一条服务端消息。
    Message {
        /// 服务端消息 ID。
        id: String,
        /// 消息正文。
        text: String,
        /// 对应浏览器入站消息 ID。
        #[serde(rename = "replyTo", skip_serializing_if = "Option::is_none")]
        reply_to: Option<String>,
        /// 消息展示格式。
        format: String,
    },
    /// 原地更新已有服务端消息。
    MessageUpdated {
        /// 被更新的服务端消息 ID。
        id: String,
        /// 更新后的正文。
        text: String,
        /// 消息展示格式。
        format: String,
    },
    /// 请求浏览器展示结构化选项。
    InputRequest {
        /// 服务端输入请求 ID。
        #[serde(rename = "requestId")]
        request_id: String,
        /// 需要用户回答的问题。
        questions: Vec<UserInputQuestion>,
    },
    /// daemon 已接收或完成命令。
    Ack {
        /// 对应浏览器入站消息 ID。
        #[serde(rename = "messageId", skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        /// 确认类型。
        kind: String,
    },
    /// 保活响应。
    Pong,
    /// 协议或业务错误。
    Error {
        /// 可直接展示的错误摘要。
        message: String,
    },
}
