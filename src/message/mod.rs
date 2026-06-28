use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// 标准化后的入站消息，所有 channel 都要转换成这个结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMessage {
    /// 消息正文。
    pub text: String,
    /// 消息来源，用于生成 session key 和回包路由。
    pub source: MessageSource,
    /// 原平台消息 id，用于 reply 或去重。
    pub message_id: Option<String>,
    /// 入站附件，例如图片。
    pub attachments: Vec<InboundAttachment>,
    /// 入站时间戳。
    pub timestamp: SystemTime,
    /// 是否由本地调度器生成。
    pub scheduled: bool,
}

/// 标准化后的入站附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundAttachment {
    /// 图片附件。
    Image {
        /// MIME 类型。
        mime_type: String,
        /// 原始 bytes。
        bytes: Vec<u8>,
    },
    /// 已落盘的文件附件。
    StoredFile {
        /// 本地路径。
        path: PathBuf,
        /// 展示文件名。
        filename: String,
        /// MIME 类型。
        mime_type: String,
        /// 文件大小。
        size: u64,
    },
}

/// 标准化后的出站消息，由 daemon 路由给对应 channel。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMessage {
    /// 目标 channel 实例名。
    pub channel_name: String,
    /// 目标收件人 id；群为 chat_id，用户为 channel 内部约定的用户 key。
    pub chat_id: String,
    /// 目标收件人粒度，供 channel 决定具体发送 API 参数。
    pub recipient: OutboundRecipient,
    /// 出站正文。
    pub text: String,
    /// 需要回复的原消息 id。
    pub reply_to: Option<String>,
    /// 目标话题或线程 id，适用于 Telegram topic 这类会话内分流。
    pub thread_id: Option<String>,
    /// 出站消息格式。
    pub format: OutboundFormat,
}

/// 标准化后的消息更新请求，由工具层路由给对应 channel。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageUpdate {
    /// 目标 channel 实例名。
    pub channel_name: String,
    /// 目标 chat id，适用于不通过历史缓存直接更新平台消息。
    pub chat_id: Option<String>,
    /// 平台返回的原消息 id。
    pub message_id: String,
    /// 更新后的正文。
    pub text: String,
    /// 更新消息格式。
    pub format: OutboundFormat,
}

/// 出站消息格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundFormat {
    /// 普通文本消息。
    Text,
    /// 可更新计划列表消息。
    Plan,
}

/// 出站收件人粒度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundRecipient {
    /// 群或会话级收件人。
    Chat,
    /// 用户级收件人。
    User,
}

/// 从入站来源推导出站目标，适用于普通回复和 cron 主动通知。
pub fn outbound_target_from_source(source: &MessageSource) -> (OutboundRecipient, String) {
    if source.chat_type == "dm"
        && source.chat_id.trim().is_empty()
        && let Some(user_id) = source
            .user_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
    {
        return (OutboundRecipient::User, user_id.clone());
    }
    (OutboundRecipient::Chat, source.chat_id.clone())
}

/// 消息来源信息，保留 Hermes SessionSource 的最小可用子集。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MessageSource {
    /// channel 实例名。
    pub channel_name: String,
    /// 平台或 channel 类型。
    pub platform: String,
    /// 会话容器 id，例如群、频道、私聊。
    pub chat_id: String,
    /// 会话类型，例如 `dm`、`group`、`channel`、`thread`。
    pub chat_type: String,
    /// 发送者 id。
    pub user_id: Option<String>,
    /// 线程或 topic id。
    pub thread_id: Option<String>,
}

/// 发送结果，供 channel 返回平台消息 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResult {
    /// 是否发送成功。
    pub success: bool,
    /// 平台返回的消息 id。
    pub message_id: Option<String>,
}

/// request_user_input 的单个选项。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UserInputOption {
    /// 展示标签。
    pub label: String,
    /// 选项说明。
    pub description: String,
}

/// request_user_input 的单个问题。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UserInputQuestion {
    /// 稳定问题 id。
    pub id: String,
    /// 短标题。
    pub header: String,
    /// 问题正文。
    pub question: String,
    /// 候选选项。
    pub options: Vec<UserInputOption>,
}

/// request_user_input 请求。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UserInputRequest {
    /// 问题列表。
    pub questions: Vec<UserInputQuestion>,
    /// 自动解析超时毫秒数。
    #[serde(rename = "autoResolutionMs", skip_serializing_if = "Option::is_none")]
    pub auto_resolution_ms: Option<u64>,
}

/// request_user_input 响应。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UserInputResponse {
    /// 按问题 id 返回的答案。
    pub answers: std::collections::HashMap<String, Vec<String>>,
}

impl Default for MessageSource {
    /// 返回空来源，主要用于反序列化缺省字段。
    fn default() -> Self {
        Self {
            channel_name: String::new(),
            platform: String::new(),
            chat_id: String::new(),
            chat_type: "dm".to_string(),
            user_id: None,
            thread_id: None,
        }
    }
}

impl InboundMessage {
    /// 构造文本入站消息，适用于 channel 适配器完成标准化后提交给 daemon。
    pub fn text(
        text: impl Into<String>,
        source: MessageSource,
        message_id: Option<String>,
    ) -> Self {
        Self {
            text: text.into(),
            source,
            message_id,
            attachments: Vec::new(),
            timestamp: SystemTime::now(),
            scheduled: false,
        }
    }

    /// 标记为本地调度任务，适用于 cron 注入 daemon 入站队列。
    pub fn scheduled(mut self) -> Self {
        self.scheduled = true;
        self
    }

    /// 附加图片，适用于 channel 下载平台图片资源后提交给 provider。
    pub fn with_image(mut self, mime_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        self.attachments.push(InboundAttachment::Image {
            mime_type: mime_type.into(),
            bytes,
        });
        self
    }

    /// 判断消息是否为 `/reset` 命令。
    pub fn is_reset_command(&self) -> bool {
        is_reset_command_text(&self.text)
    }

    /// 判断消息是否为 `/stop` 命令。
    pub fn is_stop_command(&self) -> bool {
        is_stop_command_text(&self.text)
    }

    /// 判断消息是否为 `/status` 命令。
    pub fn is_status_command(&self) -> bool {
        is_status_command_text(&self.text)
    }

    /// 判断消息是否来自本地 cron，适用于 daemon 做计划任务专用限流。
    pub fn is_cron_task(&self) -> bool {
        self.scheduled
    }
}

/// 判断 reset 命令文本，适用于群聊里 @ 机器人后执行命令。
fn is_reset_command_text(text: &str) -> bool {
    // 触发条件：飞书群聊文本会把 @ 机器人保留成 @_user_1。
    // 不能直接精确匹配原文：群里必须 @ 才能触发机器人。
    // 防止回归：@机器人 /reset 不再落入 provider 生成文字回复。
    is_single_command_text(text, "/reset")
}

/// 判断 stop 命令文本，适用于群聊里 @ 机器人后取消当前请求。
fn is_stop_command_text(text: &str) -> bool {
    // 触发条件：同一会话已有 provider 请求正在等待上游返回。
    // 不能走常规 provider 路径：它会被 session 串行锁挡住。
    // 防止回归：@机器人 /stop 不再排队到当前请求之后。
    is_single_command_text(text, "/stop")
}

/// 判断 status 命令文本，适用于不经过 provider 直接返回服务状态。
fn is_status_command_text(text: &str) -> bool {
    // 触发条件：用户只想查看运行态而不是向模型提问。
    // 不能走常规 provider 路径：状态查询会额外消耗 token 且可能排队。
    // 防止回归：@机器人 /status 不再进入模型上下文。
    is_single_command_text(text, "/status")
}

/// 判断单个斜杠命令，适用于复用 mention 前缀清理规则。
fn is_single_command_text(text: &str, command: &str) -> bool {
    let mut parts = text
        .split_whitespace()
        .skip_while(|part| is_mention_placeholder(part));
    matches!(parts.next(), Some(part) if part == command) && parts.next().is_none()
}

/// 判断是否是平台 mention 占位，适用于命令识别前清理前缀。
fn is_mention_placeholder(part: &str) -> bool {
    part.starts_with("@_")
}

#[cfg(test)]
mod message_test;
