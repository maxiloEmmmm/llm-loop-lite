use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::message::{
    MessageSource, MessageUpdate, OutboundMessage, SendResult, UserInputRequest, UserInputResponse,
};
use crate::resource::ResourceUsage;
use crate::session::SessionState;
use crate::tools::builtins;
use crate::tools::spec::ToolSpec;

/// 模型返回的工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// Responses call id，用于回灌 tool output。
    pub call_id: String,
    /// 工具名称。
    pub name: String,
    /// JSON 参数或 freeform 输入。
    pub input: ToolInput,
}

/// 工具输入类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolInput {
    /// function tool 参数。
    Function { arguments: String },
    /// custom freeform tool 输入。
    Custom { input: String },
}

/// 工具执行结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    /// 回灌 Responses 的 item type。
    pub output_kind: ToolOutputKind,
    /// call id。
    pub call_id: String,
    /// 输出正文。
    pub output: Value,
}

/// 工具输出 wire 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutputKind {
    /// function_call_output。
    Function,
    /// custom_tool_call_output。
    Custom,
}

/// 工具执行上下文。
#[derive(Clone)]
pub struct ToolContext {
    /// 当前 session 快照。
    pub session: SessionState,
    /// 当前消息来源。
    pub source: MessageSource,
    /// 当前工作目录。
    pub cwd: PathBuf,
    /// 共享工具状态。
    pub shared: Arc<ToolSharedState>,
    /// 用户输入请求回调。
    pub user_input: Option<Arc<dyn UserInputRequester>>,
    /// channel 消息发送和更新回调。
    pub channel: Option<Arc<dyn ToolChannel>>,
}

/// 工具层请求用户输入的抽象。
#[async_trait]
pub trait UserInputRequester: Send + Sync {
    /// 向具体 channel 请求结构化用户输入。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse>;
}

/// 工具层访问 channel 的抽象。
#[async_trait]
pub trait ToolChannel: Send + Sync {
    /// 发送消息到具体 channel。
    async fn send(&self, message: OutboundMessage) -> AppResult<SendResult>;

    /// 更新之前发送的消息。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult>;

    /// 请求用户结构化输入。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse>;
}

/// 工具共享状态。
pub struct ToolSharedState {
    /// 长命令 session 表。
    pub exec_sessions: Mutex<builtins::ExecSessions>,
    /// 计划消息状态表。
    pub plans: Mutex<builtins::PlanStates>,
    /// cron 工具状态表。
    pub crons: builtins::CronStore,
}

/// 单个工具处理器。
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// 返回工具名称。
    fn name(&self) -> &'static str;

    /// 返回工具 spec。
    fn spec(&self) -> ToolSpec;

    /// 执行工具调用。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult>;
}

/// 工具注册表。
#[derive(Clone)]
pub struct ToolRegistry {
    /// 处理器表。
    handlers: Arc<HashMap<String, Arc<dyn ToolHandler>>>,
    /// 共享状态。
    shared: Arc<ToolSharedState>,
}

impl ToolRegistry {
    /// 创建内置工具注册表。
    pub fn builtins(paths: AppPaths) -> Self {
        let mut handlers: HashMap<String, Arc<dyn ToolHandler>> = HashMap::new();
        for handler in builtins::handlers() {
            handlers.insert(handler.name().to_string(), handler);
        }
        Self {
            handlers: Arc::new(handlers),
            shared: Arc::new(ToolSharedState {
                exec_sessions: Mutex::default(),
                plans: Mutex::new(builtins::PlanStates::default().with_store_root(paths.plans_dir)),
                crons: builtins::CronStore::new(paths.crons_dir),
            }),
        }
    }

    /// 返回本地可执行工具 specs。
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.handlers
            .values()
            .map(|handler| handler.spec())
            .collect()
    }

    /// 返回移除指定工具后的注册表，适用于特定来源禁用危险工具。
    pub fn without_handler(&self, name: &str) -> Self {
        let handlers = self
            .handlers
            .iter()
            .filter(|(handler_name, _)| handler_name.as_str() != name)
            .map(|(handler_name, handler)| (handler_name.clone(), Arc::clone(handler)))
            .collect::<HashMap<_, _>>();
        Self {
            handlers: Arc::new(handlers),
            shared: Arc::clone(&self.shared),
        }
    }

    /// 构造工具上下文。
    pub fn context(
        &self,
        session: SessionState,
        source: MessageSource,
        cwd: &Path,
    ) -> AppResult<ToolContext> {
        Ok(ToolContext {
            session,
            source,
            cwd: cwd.to_path_buf(),
            shared: Arc::clone(&self.shared),
            user_input: None,
            channel: None,
        })
    }

    /// 构造带 channel 回调的工具上下文。
    pub fn context_with_channel(
        &self,
        session: SessionState,
        source: MessageSource,
        cwd: &Path,
        channel: Arc<dyn ToolChannel>,
    ) -> AppResult<ToolContext> {
        Ok(ToolContext {
            session,
            source,
            cwd: cwd.to_path_buf(),
            shared: Arc::clone(&self.shared),
            user_input: Some(Arc::new(ChannelUserInputRequester {
                channel: Arc::clone(&channel),
            })),
            channel: Some(channel),
        })
    }

    /// 执行单个工具调用。
    pub async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let Some(handler) = self.handlers.get(&call.name) else {
            return Err(AppError::Tool(format!("unsupported tool: {}", call.name)));
        };
        handler.execute(call, context).await
    }

    /// 将当前计划补偿为失败态，适用于 provider 请求失败后更新用户侧卡片。
    pub async fn fail_active_plan(
        &self,
        session_key: &str,
        desc: &str,
        channel: Arc<dyn ToolChannel>,
    ) -> AppResult<bool> {
        let update = {
            let mut plans = self.shared.plans.lock().await;
            plans.fail_active_item(session_key, desc).await?
        };
        let Some(update) = update else {
            return Ok(false);
        };
        channel.update_message(update).await?;
        Ok(true)
    }

    /// 删除当前计划状态，适用于 `/reset` 后释放内存和持久化文件。
    pub async fn remove_plan(&self, session_key: &str) -> AppResult<()> {
        self.shared
            .plans
            .lock()
            .await
            .remove_state(session_key)
            .await
    }

    /// 返回工具层资源估算，适用于 resources 子命令即时查询。
    pub async fn resource_usage(&self) -> Vec<ResourceUsage> {
        let mut usages = Vec::new();
        usages.push(ResourceUsage::new(
            "tools.handlers",
            "hashmap",
            self.handlers.len(),
            Some(self.handlers.capacity()),
            self.handlers
                .capacity()
                .saturating_mul(std::mem::size_of::<(String, Arc<dyn ToolHandler>)>())
                .saturating_add(self.handlers.keys().map(String::capacity).sum::<usize>()),
        ));
        usages.push(self.shared.exec_sessions.lock().await.resource_usage());
        usages.push(self.shared.plans.lock().await.resource_usage());
        usages
    }
}

/// 将 ToolChannel 适配为 UserInputRequester。
struct ChannelUserInputRequester {
    /// 当前 channel 的工具句柄。
    channel: Arc<dyn ToolChannel>,
}

#[async_trait]
impl UserInputRequester for ChannelUserInputRequester {
    /// 将用户输入请求转发给当前 channel。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        self.channel.request_user_input(source, request).await
    }
}
