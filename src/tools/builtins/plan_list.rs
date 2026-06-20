use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::ids::new_reply_hash;
use crate::message::{MessageUpdate, OutboundFormat, OutboundMessage, outbound_target_from_source};
use crate::plan_store;
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolResult,
};
use crate::tools::spec::{JsonSchema, ResponsesApiTool, ToolSpec};

/// 计划状态表，按 session key 保存计划消息。
#[derive(Debug, Default)]
pub struct PlanStates {
    /// 每个 session 当前绑定的计划消息。
    items: HashMap<String, PlanState>,
    /// 计划持久化目录。
    store_root: Option<PathBuf>,
}

/// 单个 session 的计划消息状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanState {
    /// channel 实例名。
    pub channel_name: String,
    /// chat id。
    pub chat_id: String,
    /// 平台消息 id。
    pub message_id: String,
    /// 卡片排查短 hash，展示在计划消息最前面。
    #[serde(default)]
    reply_hash: Option<String>,
    /// 最近一次列表更新时间，毫秒时间戳。
    #[serde(default)]
    last_update_at_ms: u64,
    /// 当前计划列表。
    items: Vec<PlanListItem>,
}

/// 计划列表条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanListItem {
    /// 展示标题。
    title: String,
    /// 稳定 key。
    key: String,
    /// 当前状态。
    status: PlanStatus,
    /// 是否已经写入终态。
    #[serde(skip)]
    finalized: bool,
    /// 完成或失败时的耗时。
    #[serde(default)]
    cost: Option<String>,
    /// 状态补充说明。
    #[serde(default)]
    desc: Option<String>,
    /// 子计划列表。
    #[serde(default)]
    children: Vec<PlanListItem>,
}

/// `__plan_list` 参数。
#[derive(Debug, Clone, Deserialize)]
struct PlanListArgs {
    /// 初始计划列表。
    list: Vec<PlanListItem>,
}

/// `__plan_list_update` 参数。
#[derive(Debug, Clone, Deserialize)]
struct PlanListUpdateArgs {
    /// 要更新的计划 key。
    key: String,
    /// 新状态。
    status: PlanStatus,
    /// 状态补充说明。
    #[serde(default)]
    desc: Option<String>,
}

/// `__plan_list_edit` 参数。
#[derive(Debug, Clone, Deserialize)]
struct PlanListEditArgs {
    /// 编辑列表。
    list: Vec<PlanListEditItem>,
}

/// 单个计划编辑项。
#[derive(Debug, Clone, Deserialize)]
struct PlanListEditItem {
    /// 是否新增计划项。
    new: bool,
    /// 新增时的插入位置，使用用户可见的 1-based index。
    #[serde(default)]
    index: Option<usize>,
    /// 修改已有项时的 key 路径。
    #[serde(default)]
    key: Option<String>,
    /// 新增项标题或已有项的新标题。
    title: String,
}

/// 计划状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum PlanStatus {
    /// 正在执行。
    Ing,
    /// 已完成。
    Done,
    /// 执行失败。
    Failed,
    /// 等待中。
    Wait,
}

/// 创建计划列表消息的工具。
pub struct PlanListHandler;

/// 更新计划列表消息的工具。
pub struct PlanListUpdateHandler;

/// 编辑计划列表结构的工具。
pub struct PlanListEditHandler;

/// 标记计划进入汇总阶段的工具。
pub struct PlanListDoneHandler;

#[async_trait]
impl ToolHandler for PlanListHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "__plan_list"
    }

    /// 返回计划列表工具 spec。
    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Create a visible task plan message in the current channel from the model's own plan. Each item needs title, key, and status: ing, done, failed, or wait. Items may include nested children. Item key must not contain `.`. Every status is rendered with an icon, including wait.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "list".to_string(),
                    JsonSchema::array(plan_item_schema(), Some("Plan items.".to_string())),
                )]),
                Some(vec!["list".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 发送计划列表消息并保存平台 message id。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "__plan_list requires function arguments".to_string(),
            ));
        };
        let mut args: PlanListArgs = serde_json::from_str(arguments)?;
        validate_initial_items(&args.list)?;
        mark_finalized_items(&mut args.list);
        let Some(channel) = context.channel.clone() else {
            return Err(AppError::Tool(
                "__plan_list channel callback is not connected".to_string(),
            ));
        };
        let reply_hash = new_reply_hash(&context.session.key);
        let text = render_plan_message_with_hash(&reply_hash, &args.list);
        let (recipient, chat_id) = outbound_target_from_source(&context.source);
        let result = channel
            .send(OutboundMessage {
                channel_name: context.source.channel_name.clone(),
                chat_id,
                recipient,
                text,
                reply_to: None,
                format: OutboundFormat::Plan,
            })
            .await?;
        let Some(message_id) = result.message_id else {
            return Err(AppError::Tool(
                "__plan_list channel did not return message_id".to_string(),
            ));
        };
        crate::log_info!(
            "plan list sent session_key={} reply_hash={} message_id={}",
            context.session.key,
            reply_hash,
            message_id
        );
        let state = PlanState {
            channel_name: context.source.channel_name,
            chat_id: context.source.chat_id,
            message_id: message_id.clone(),
            reply_hash: Some(reply_hash),
            last_update_at_ms: current_time_ms(),
            items: args.list,
        };
        let mut plans = context.shared.plans.lock().await;
        plans.save_state(&context.session.key, state).await?;
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: json!(""),
        })
    }
}

#[async_trait]
impl ToolHandler for PlanListUpdateHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "__plan_list_update"
    }

    /// 返回计划更新工具 spec。
    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "key".to_string(),
                JsonSchema::string(Some(
                    "Plan item path to update. Use dot-separated keys for nested items."
                        .to_string(),
                )),
            ),
            (
                "status".to_string(),
                status_schema(Some("New item status.".to_string())),
            ),
            (
                "desc".to_string(),
                JsonSchema::string(Some(
                    "Optional short status report. Before working on each wait item, update it to ing. For ing, it may be updated repeatedly. After the item finishes, update that same ing item to done or failed once. Never update a wait item directly to done or failed."
                        .to_string(),
                )),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Update one item in the visible task plan message. Use dot-separated key paths for nested items. Before starting any wait item, update it to ing. Only the current ing item may be completed or failed; never jump directly from wait to done or failed. Keep at most one ing item at a time. Desc should be a short status report. Ing may update desc repeatedly. Done and failed are terminal states: provide desc once, and do not update that item again after terminal update. Cost is calculated by the tool from the previous list update time.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["key".to_string(), "status".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 更新已发送的计划列表消息。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "__plan_list_update requires function arguments".to_string(),
            ));
        };
        let args: PlanListUpdateArgs = serde_json::from_str(arguments)?;
        validate_update(&args)?;
        let Some(channel) = context.channel.clone() else {
            return Err(AppError::Tool(
                "__plan_list_update channel callback is not connected".to_string(),
            ));
        };
        let mut plans = context.shared.plans.lock().await;
        plans.restore_state_if_needed(&context.session.key).await?;
        let Some(state) = plans.items.get_mut(&context.session.key) else {
            return Err(AppError::Tool(
                "__plan_list_update requires __plan_list first".to_string(),
            ));
        };
        let now = current_time_ms();
        let elapsed_ms = elapsed_since_last_update(state, now);
        let Some(item) = find_item_mut(&mut state.items, &args.key) else {
            return Err(AppError::Tool(format!(
                "__plan_list_update unknown key `{}`",
                args.key
            )));
        };
        if item.finalized {
            return Err(AppError::Tool(format!(
                "__plan_list_update key `{}` is already done or failed",
                args.key
            )));
        }
        item.status = args.status;
        item.desc = args.desc;
        item.cost = computed_update_cost(item.status, elapsed_ms);
        if matches!(item.status, PlanStatus::Done | PlanStatus::Failed) {
            item.finalized = true;
        }
        state.last_update_at_ms = now;
        let text = render_state_message(state, &context.session.key);
        let update = MessageUpdate {
            channel_name: state.channel_name.clone(),
            message_id: state.message_id.clone(),
            text,
            format: OutboundFormat::Plan,
        };
        let message_id = state.message_id.clone();
        plans.persist_state(&context.session.key).await?;
        channel.update_message(update).await?;
        crate::log_info!(
            "plan list updated session_key={} message_id={}",
            context.session.key,
            message_id
        );
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: json!(""),
        })
    }
}

#[async_trait]
impl ToolHandler for PlanListEditHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "__plan_list_edit"
    }

    /// 返回计划编辑工具 spec。
    fn spec(&self) -> ToolSpec {
        let item_properties = BTreeMap::from([
            (
                "new".to_string(),
                JsonSchema::boolean(Some("True to insert a new plan item.".to_string())),
            ),
            (
                "index".to_string(),
                JsonSchema::integer(Some(
                    "Required when new is true. 1-based root-level insert index.".to_string(),
                )),
            ),
            (
                "key".to_string(),
                JsonSchema::string(Some(
                    "Required when new is false. Dot-separated key path of the item to rename."
                        .to_string(),
                )),
            ),
            (
                "title".to_string(),
                JsonSchema::string(Some(
                    "When new is true, title for the inserted plan item. When new is false, new title for the existing item."
                        .to_string(),
                )),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Edit the visible task plan structure. The model may use this when the plan changes. Each edit item has new, title, and either index for insertion or key for renaming. New insertions are root-level and use 1-based index.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "list".to_string(),
                    JsonSchema::array(
                        JsonSchema::object(
                            item_properties,
                            Some(vec!["new".to_string(), "title".to_string()]),
                            Some(false.into()),
                        ),
                        Some("Plan edit items.".to_string()),
                    ),
                )]),
                Some(vec!["list".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    /// 编辑已发送的计划列表消息。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let ToolInput::Function { arguments } = &call.input else {
            return Err(AppError::Tool(
                "__plan_list_edit requires function arguments".to_string(),
            ));
        };
        let args: PlanListEditArgs = serde_json::from_str(arguments)?;
        validate_edit(&args)?;
        let Some(channel) = context.channel.clone() else {
            return Err(AppError::Tool(
                "__plan_list_edit channel callback is not connected".to_string(),
            ));
        };
        let mut plans = context.shared.plans.lock().await;
        plans.restore_state_if_needed(&context.session.key).await?;
        let Some(state) = plans.items.get_mut(&context.session.key) else {
            return Err(AppError::Tool(
                "__plan_list_edit requires __plan_list first".to_string(),
            ));
        };
        for edit in args.list {
            apply_edit(&mut state.items, edit)?;
        }
        state.last_update_at_ms = current_time_ms();
        let text = render_state_message(state, &context.session.key);
        let update = MessageUpdate {
            channel_name: state.channel_name.clone(),
            message_id: state.message_id.clone(),
            text,
            format: OutboundFormat::Plan,
        };
        let message_id = state.message_id.clone();
        plans.persist_state(&context.session.key).await?;
        channel.update_message(update).await?;
        crate::log_info!(
            "plan list edited session_key={} message_id={}",
            context.session.key,
            message_id
        );
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: json!(""),
        })
    }
}

#[async_trait]
impl ToolHandler for PlanListDoneHandler {
    /// 返回工具名称。
    fn name(&self) -> &'static str {
        "__plan_list_done"
    }

    /// 返回计划汇总阶段工具 spec。
    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: self.name().to_string(),
            description: "Mark the visible task plan as finished and entering final summarization. This tool takes no arguments and returns no meaningful content. Call it after plan work is complete and before writing the final answer.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(BTreeMap::new(), Some(Vec::new()), Some(false.into())),
            output_schema: None,
        })
    }

    /// 将已发送的计划消息更新为汇总中提示。
    async fn execute(&self, call: ToolCall, context: ToolContext) -> AppResult<ToolResult> {
        let Some(channel) = context.channel.clone() else {
            return Err(AppError::Tool(
                "__plan_list_done channel callback is not connected".to_string(),
            ));
        };
        let mut plans = context.shared.plans.lock().await;
        plans.restore_state_if_needed(&context.session.key).await?;
        let Some(state) = plans.items.get_mut(&context.session.key) else {
            return Err(AppError::Tool(
                "__plan_list_done requires __plan_list first".to_string(),
            ));
        };
        state.last_update_at_ms = current_time_ms();
        let mut text = render_state_message(state, &context.session.key);
        text.push_str("\n\n汇总中...");
        let update = MessageUpdate {
            channel_name: state.channel_name.clone(),
            message_id: state.message_id.clone(),
            text,
            format: OutboundFormat::Plan,
        };
        let message_id = state.message_id.clone();
        plans.persist_state(&context.session.key).await?;
        channel.update_message(update).await?;
        crate::log_info!(
            "plan list done session_key={} message_id={}",
            context.session.key,
            message_id
        );
        Ok(ToolResult {
            output_kind: ToolOutputKind::Function,
            call_id: call.call_id,
            output: json!(""),
        })
    }
}

impl PlanStates {
    /// 绑定持久化目录，适用于 daemon 启动时注入应用路径。
    pub fn with_store_root(mut self, store_root: PathBuf) -> Self {
        self.store_root = Some(store_root);
        self
    }

    /// 保存指定 session 的计划状态，适用于工具创建计划。
    pub async fn save_state(&mut self, session_key: &str, state: PlanState) -> AppResult<()> {
        self.items.insert(session_key.to_string(), state);
        self.persist_state(session_key).await
    }

    /// 恢复指定 session 的计划状态，适用于 daemon 重启后懒加载。
    pub async fn restore_state_if_needed(&mut self, session_key: &str) -> AppResult<bool> {
        if self.items.contains_key(session_key) {
            return Ok(true);
        }
        let Some(root) = self.store_root.as_deref() else {
            return Ok(false);
        };
        let Some(mut state) = plan_store::load_plan(root, session_key).await? else {
            return Ok(false);
        };
        if state.last_update_at_ms == 0 {
            state.last_update_at_ms = current_time_ms();
        }
        mark_finalized_items(&mut state.items);
        self.items.insert(session_key.to_string(), state);
        Ok(true)
    }

    /// 删除指定 session 的计划状态，适用于 `/reset` 后释放卡片引用。
    pub async fn remove_state(&mut self, session_key: &str) -> AppResult<()> {
        self.items.remove(session_key);
        if let Some(root) = self.store_root.as_deref() {
            plan_store::remove_plan(root, session_key).await?;
        }
        Ok(())
    }

    /// 持久化指定 session 的当前计划状态。
    pub async fn persist_state(&self, session_key: &str) -> AppResult<()> {
        let Some(root) = self.store_root.as_deref() else {
            return Ok(());
        };
        let Some(state) = self.items.get(session_key) else {
            return Ok(());
        };
        plan_store::save_plan(root, session_key, state).await
    }

    /// 将当前执行中计划标记失败，适用于 provider 中断后的 UI 补偿。
    pub async fn fail_active_item(
        &mut self,
        session_key: &str,
        desc: &str,
    ) -> AppResult<Option<MessageUpdate>> {
        if !self.restore_state_if_needed(session_key).await? {
            return Ok(None);
        }
        let Some(state) = self.items.get_mut(session_key) else {
            return Ok(None);
        };
        let now = current_time_ms();
        let elapsed_ms = elapsed_since_last_update(state, now);
        let Some(item) = find_first_ing_item_mut(&mut state.items) else {
            return Ok(None);
        };
        item.status = PlanStatus::Failed;
        item.finalized = true;
        item.cost = computed_update_cost(item.status, elapsed_ms);
        item.desc = Some(desc.to_string());
        state.last_update_at_ms = now;
        let text = render_state_message(state, session_key);
        let update = MessageUpdate {
            channel_name: state.channel_name.clone(),
            message_id: state.message_id.clone(),
            text,
            format: OutboundFormat::Plan,
        };
        self.persist_state(session_key).await?;
        crate::log_info!(
            "plan list failed active item session_key={} message_id={} desc={}",
            session_key,
            update.message_id,
            desc
        );
        Ok(Some(update))
    }
}

/// 构造计划条目 schema。
fn plan_item_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "title".to_string(),
                JsonSchema::string(Some("Plan item title.".to_string())),
            ),
            (
                "key".to_string(),
                JsonSchema::string(Some("Stable plan item key.".to_string())),
            ),
            (
                "status".to_string(),
                status_schema(Some("Item status.".to_string())),
            ),
            (
                "desc".to_string(),
                JsonSchema::string(Some(
                    "Optional short status report. For initial done or failed items, provide final desc once."
                        .to_string(),
                )),
            ),
            (
                "children".to_string(),
                JsonSchema::array(
                    JsonSchema::object(BTreeMap::new(), None, Some(true.into())),
                    Some("Nested plan items. Child keys must not contain `.`.".to_string()),
                ),
            ),
        ]),
        Some(vec![
            "title".to_string(),
            "key".to_string(),
            "status".to_string(),
        ]),
        Some(false.into()),
    )
}

/// 构造状态枚举 schema。
fn status_schema(description: Option<String>) -> JsonSchema {
    JsonSchema::string_enum(
        vec![json!("ing"), json!("done"), json!("failed"), json!("wait")],
        description,
    )
}

/// 校验初始计划列表。
fn validate_initial_items(items: &[PlanListItem]) -> AppResult<()> {
    if items.is_empty() {
        return Err(AppError::Tool(
            "__plan_list list cannot be empty".to_string(),
        ));
    }
    validate_plan_items(items)
}

/// 递归校验计划条目。
fn validate_plan_items(items: &[PlanListItem]) -> AppResult<()> {
    for item in items {
        if item.title.trim().is_empty() {
            return Err(AppError::Tool(
                "__plan_list title cannot be empty".to_string(),
            ));
        }
        if item.key.trim().is_empty() {
            return Err(AppError::Tool(
                "__plan_list key cannot be empty".to_string(),
            ));
        }
        if item.key.contains('.') {
            return Err(AppError::Tool(format!(
                "__plan_list key `{}` cannot contain `.`",
                item.key
            )));
        }
        validate_plan_items(&item.children)?;
    }
    Ok(())
}

/// 校验计划更新参数。
fn validate_update(args: &PlanListUpdateArgs) -> AppResult<()> {
    if args.key.trim().is_empty() {
        return Err(AppError::Tool(
            "__plan_list_update key cannot be empty".to_string(),
        ));
    }
    Ok(())
}

/// 计算终态耗时，适用于避免让模型猜测执行耗时。
fn computed_update_cost(status: PlanStatus, elapsed_ms: u64) -> Option<String> {
    match status {
        PlanStatus::Done | PlanStatus::Failed => Some(format_elapsed_ms(elapsed_ms)),
        PlanStatus::Ing | PlanStatus::Wait => None,
    }
}

/// 计算距离上次列表更新的耗时。
fn elapsed_since_last_update(state: &PlanState, now: u64) -> u64 {
    let last_update_at_ms = if state.last_update_at_ms == 0 {
        now
    } else {
        state.last_update_at_ms
    };
    now.saturating_sub(last_update_at_ms)
}

/// 返回当前毫秒时间戳，适用于计划耗时计算和持久化。
fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

/// 格式化毫秒耗时，适用于用户侧紧凑展示。
fn format_elapsed_ms(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1000;
    if seconds == 0 {
        return "<1s".to_string();
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m{seconds:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h{minutes:02}m{seconds:02}s")
}

/// 校验计划编辑参数。
fn validate_edit(args: &PlanListEditArgs) -> AppResult<()> {
    if args.list.is_empty() {
        return Err(AppError::Tool(
            "__plan_list_edit list cannot be empty".to_string(),
        ));
    }
    for item in &args.list {
        if item.title.trim().is_empty() {
            return Err(AppError::Tool(
                "__plan_list_edit title cannot be empty".to_string(),
            ));
        }
        if item.new {
            if item.index.is_none() {
                return Err(AppError::Tool(
                    "__plan_list_edit index is required when new is true".to_string(),
                ));
            }
        } else if item
            .key
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(AppError::Tool(
                "__plan_list_edit key is required when new is false".to_string(),
            ));
        }
    }
    Ok(())
}

/// 应用单个计划编辑。
fn apply_edit(items: &mut Vec<PlanListItem>, edit: PlanListEditItem) -> AppResult<()> {
    if edit.new {
        let index = edit.index.expect("validate_edit 已保证 index 存在");
        let insert_index = index.saturating_sub(1).min(items.len());
        items.insert(insert_index, new_plan_item(edit.title));
        return Ok(());
    }
    let key = edit.key.expect("validate_edit 已保证 key 存在");
    let Some(item) = find_item_mut(items, &key) else {
        return Err(AppError::Tool(format!(
            "__plan_list_edit unknown key `{key}`"
        )));
    };
    item.title = edit.title;
    Ok(())
}

/// 创建新增计划项。
fn new_plan_item(title: String) -> PlanListItem {
    PlanListItem {
        key: new_edit_key(),
        title,
        status: PlanStatus::Wait,
        finalized: false,
        cost: None,
        desc: None,
        children: Vec::new(),
    }
}

/// 生成新增计划项 key。
fn new_edit_key() -> String {
    format!("item_{}", uuid::Uuid::new_v4().simple())
}

/// 标记初始列表里的终态项。
fn mark_finalized_items(items: &mut [PlanListItem]) {
    for item in items {
        item.finalized = matches!(item.status, PlanStatus::Done | PlanStatus::Failed);
        mark_finalized_items(&mut item.children);
    }
}

/// 渲染计划消息正文。
fn render_plan_message(items: &[PlanListItem]) -> String {
    let mut lines = Vec::new();
    render_plan_lines(items, 0, Vec::new(), &mut lines);
    lines.join("\n")
}

/// 渲染带短 hash 的计划消息，适用于用户侧排查卡片更新链路。
fn render_plan_message_with_hash(reply_hash: &str, items: &[PlanListItem]) -> String {
    format!("[{reply_hash}]\n{}", render_plan_message(items))
}

/// 渲染计划状态消息，适用于更新和失败补偿时保留同一个短 hash。
fn render_state_message(state: &mut PlanState, session_key: &str) -> String {
    let reply_hash = state
        .reply_hash
        .get_or_insert_with(|| new_reply_hash(session_key));
    render_plan_message_with_hash(reply_hash, &state.items)
}

/// 递归渲染计划行。
fn render_plan_lines(
    items: &[PlanListItem],
    depth: usize,
    prefix: Vec<usize>,
    lines: &mut Vec<String>,
) {
    for (index, item) in items.iter().enumerate() {
        let mut current = prefix.clone();
        current.push(index + 1);
        let number = current
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(".");
        let indent = "  ".repeat(depth);
        let mut line = format!("{}{}. {}", indent, number, item.title.trim());
        if let Some(icon) = status_icon(item.status) {
            line.push_str(&format!(" {icon}"));
        }
        if let Some(cost) = item
            .cost
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            line.push_str(&format!(" {}", cost.trim()));
        }
        lines.push(line);
        render_plan_lines(&item.children, depth + 1, current, lines);
        if !matches!(item.status, PlanStatus::Wait)
            && let Some(desc) = item
                .desc
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        {
            lines.push(format!("{}  {}", indent, desc.trim()));
        }
    }
}

/// 返回状态图标，wait 不展示。
fn status_icon(status: PlanStatus) -> Option<&'static str> {
    match status {
        PlanStatus::Ing => Some("☕"),
        PlanStatus::Done => Some("✅"),
        PlanStatus::Failed => Some("💥"),
        PlanStatus::Wait => Some("🕒"),
    }
}

/// 按点号路径查找计划项。
fn find_item_mut<'a>(items: &'a mut [PlanListItem], path: &str) -> Option<&'a mut PlanListItem> {
    let mut parts = path.split('.');
    let first = parts.next()?;
    let item = items.iter_mut().find(|item| item.key == first)?;
    find_child_item_mut(item, parts)
}

/// 查找第一个执行中计划项，适用于 provider 失败时补偿终态。
fn find_first_ing_item_mut(items: &mut [PlanListItem]) -> Option<&mut PlanListItem> {
    for item in items {
        if item.status == PlanStatus::Ing && !item.finalized {
            return Some(item);
        }
        if let Some(child) = find_first_ing_item_mut(&mut item.children) {
            return Some(child);
        }
    }
    None
}

/// 递归查找子计划项。
fn find_child_item_mut<'items, 'path, I>(
    item: &'items mut PlanListItem,
    mut parts: I,
) -> Option<&'items mut PlanListItem>
where
    I: Iterator<Item = &'path str>,
{
    let Some(part) = parts.next() else {
        return Some(item);
    };
    let child = item.children.iter_mut().find(|child| child.key == part)?;
    find_child_item_mut(child, parts)
}

#[cfg(test)]
#[path = "plan_list_test.rs"]
mod plan_list_test;
