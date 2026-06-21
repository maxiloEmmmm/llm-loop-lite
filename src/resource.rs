use serde::{Deserialize, Serialize};

use crate::message::{MessageSource, UserInputRequest};

/// 内存资源统计项，适用于即时解释 daemon 当前内存来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceUsage {
    /// 资源名称。
    pub name: String,
    /// 资源类型，例如 hashmap、cache、string。
    pub kind: String,
    /// 当前条目数量。
    pub items: usize,
    /// 容量或上限；无法表达时为 None。
    pub capacity: Option<usize>,
    /// 估算字节数，只统计本进程可见的容器和缓冲容量。
    pub estimated_bytes: usize,
}

/// 估算消息来源字符串容量。
pub fn estimate_message_source_bytes(source: &MessageSource) -> usize {
    source.channel_name.capacity()
        + source.platform.capacity()
        + source.chat_id.capacity()
        + source.chat_type.capacity()
        + source.user_id.as_ref().map(String::capacity).unwrap_or(0)
        + source.thread_id.as_ref().map(String::capacity).unwrap_or(0)
}

/// 估算结构化用户输入请求容量。
pub fn estimate_user_input_request_bytes(request: &UserInputRequest) -> usize {
    request
        .questions
        .capacity()
        .saturating_mul(std::mem::size_of::<crate::message::UserInputQuestion>())
        .saturating_add(
            request
                .questions
                .iter()
                .map(|question| {
                    question.id.capacity()
                        + question.header.capacity()
                        + question.question.capacity()
                        + question
                            .options
                            .capacity()
                            .saturating_mul(std::mem::size_of::<crate::message::UserInputOption>())
                        + question
                            .options
                            .iter()
                            .map(|option| option.label.capacity() + option.description.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>(),
        )
}

/// 估算答案表容量。
pub fn estimate_answers_bytes(answers: &std::collections::HashMap<String, Vec<String>>) -> usize {
    answers
        .capacity()
        .saturating_mul(std::mem::size_of::<(String, Vec<String>)>())
        .saturating_add(
            answers
                .iter()
                .map(|(key, values)| {
                    key.capacity()
                        + values
                            .capacity()
                            .saturating_mul(std::mem::size_of::<String>())
                        + values.iter().map(String::capacity).sum::<usize>()
                })
                .sum::<usize>(),
        )
}

impl ResourceUsage {
    /// 构造资源统计项，适用于各模块即时上报自身容器占用。
    pub fn new(
        name: impl Into<String>,
        kind: impl Into<String>,
        items: usize,
        capacity: Option<usize>,
        estimated_bytes: usize,
    ) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            items,
            capacity,
            estimated_bytes,
        }
    }
}
