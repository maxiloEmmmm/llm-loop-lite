use std::collections::HashMap;

use crate::context::InitialContext;
use crate::ids::new_session_id;
use crate::message::MessageSource;
use crate::provider::limits::DEFAULT_CONTEXT_WINDOW;
use crate::resource::ResourceUsage;

/// 单个 session 的内存状态，只保存轻量元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    /// 稳定路由 key，同一个来源会复用。
    pub key: String,
    /// 当前实际 session id，`/reset` 会替换它。
    pub id: String,
    /// 新 session 首轮注入的顶层模型指令。
    pub instructions: String,
    /// 初始上下文是否已经完成加载。
    pub initial_context_loaded: bool,
    /// 最大上下文 token 估算。
    pub max_context_tokens: Option<u64>,
    /// 已使用 token 计数，优先来自 provider usage。
    pub used_tokens: u64,
}

/// session 注册表，维护 message key 到当前 session id 的绑定。
#[derive(Debug, Default)]
pub struct SessionRegistry {
    /// 按稳定 key 存储 session 状态。
    sessions: HashMap<String, SessionState>,
}

impl SessionRegistry {
    /// 创建空注册表，适用于 daemon 启动。
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// 按消息来源获取或创建 session。
    pub fn get_or_create(&mut self, source: &MessageSource) -> &mut SessionState {
        let key = build_message_key(source);
        self.sessions
            .entry(key.clone())
            .or_insert_with(|| SessionState::new(key))
    }

    /// 按消息来源查找 session。
    pub fn get(&self, source: &MessageSource) -> Option<&SessionState> {
        let key = build_message_key(source);
        self.sessions.get(&key)
    }

    /// 安装已恢复的 session，适用于本地历史懒加载。
    pub fn insert_restored(&mut self, session: SessionState) {
        self.sessions.insert(session.key.clone(), session);
    }

    /// 返回当前内存中 session 数量，适用于资源统计。
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// 返回 session 注册表资源估算，适用于 resources 子命令解释内存来源。
    pub fn resource_usage(&self) -> ResourceUsage {
        let string_bytes = self
            .sessions
            .iter()
            .map(|(key, session)| {
                key.capacity()
                    + session.key.capacity()
                    + session.id.capacity()
                    + session.instructions.capacity()
            })
            .sum::<usize>();
        let entry_bytes = self
            .sessions
            .capacity()
            .saturating_mul(std::mem::size_of::<(String, SessionState)>());
        ResourceUsage::new(
            "sessions.registry",
            "hashmap",
            self.sessions.len(),
            Some(self.sessions.capacity()),
            entry_bytes.saturating_add(string_bytes),
        )
    }

    /// 重置指定来源的 session，并让原 key 绑定新的 session id。
    pub fn reset(&mut self, source: &MessageSource) -> &mut SessionState {
        let key = build_message_key(source);
        self.sessions
            .insert(key.clone(), SessionState::new(key.clone()));
        self.sessions
            .get_mut(&key)
            .expect("刚插入的 session 必须存在")
    }

    /// 累加 provider 返回的 token 用量。
    pub fn record_token_usage(&mut self, source: &MessageSource, total_tokens: Option<u64>) {
        let Some(total_tokens) = total_tokens else {
            return;
        };
        let session = self.get_or_create(source);
        session.used_tokens = session.used_tokens.saturating_add(total_tokens);
    }

    /// 判断指定来源是否需要加载新 session 的初始上下文。
    pub fn needs_initial_context(&mut self, source: &MessageSource) -> bool {
        !self.get_or_create(source).initial_context_loaded
    }

    /// 写入新 session 的初始上下文，适用于 provider 首轮请求前。
    pub fn set_initial_context(&mut self, source: &MessageSource, context: InitialContext) {
        let session = self.get_or_create(source);
        session.instructions = context.instructions;
        session.initial_context_loaded = true;
    }

    /// 写入模型上下文窗口，适用于 provider/model registry 每轮刷新。
    pub fn set_max_context_tokens(&mut self, source: &MessageSource, max_context_tokens: u64) {
        let session = self.get_or_create(source);
        session.max_context_tokens = Some(max_context_tokens);
    }
}

impl SessionState {
    /// 创建新的 session 状态，适用于首次消息或 `/reset`。
    pub fn new(key: String) -> Self {
        Self {
            key,
            id: new_session_id(),
            instructions: String::new(),
            initial_context_loaded: false,
            max_context_tokens: Some(DEFAULT_CONTEXT_WINDOW),
            used_tokens: 0,
        }
    }
}

/// 从来源构造稳定 message key，借鉴 Hermes 的 platform/chat/thread/user 分层。
pub fn build_message_key(source: &MessageSource) -> String {
    let mut parts = vec![
        "agent".to_string(),
        "main".to_string(),
        normalize_key_part(&source.platform),
        normalize_key_part(&source.chat_type),
    ];

    if !source.chat_id.is_empty() {
        parts.push(normalize_key_part(&source.chat_id));
    }
    if let Some(thread_id) = source.thread_id.as_ref().filter(|value| !value.is_empty()) {
        parts.push(normalize_key_part(thread_id));
    }
    if source.chat_type != "thread"
        && let Some(user_id) = source.user_id.as_ref().filter(|value| !value.is_empty())
    {
        parts.push(normalize_key_part(user_id));
    }

    parts.join(":")
}

/// 清理 key 片段，避免分隔符把 session key 结构打乱。
fn normalize_key_part(value: &str) -> String {
    value.trim().replace(':', "_")
}

#[cfg(test)]
mod session_test;
