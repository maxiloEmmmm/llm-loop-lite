use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

use crate::channel::{
    BuiltinChannel, BuiltinChannelHandle, Channel, ChannelAckCapability, ChannelAckKind,
    build_channels,
};
use crate::config::AppConfig;
use crate::context::{load_cron_context, load_initial_context, render_initial_context_for_log};
use crate::context_window::{
    context_window_status, prepare_context_window, prepare_context_window_with_summary,
};
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::ids::{new_reply_hash, new_session_id};
use crate::message::{
    InboundMessage, MessageSource, MessageUpdate, OutboundFormat, OutboundMessage, SendResult,
    UserInputRequest, UserInputResponse, outbound_target_from_source,
};
use crate::provider::{BuiltinProvider, Provider, build_provider};
use crate::scheduler::{SchedulerChannel, run_cron_scheduler};
use crate::session::{SessionRegistry, build_message_key};
use crate::session_store;
use crate::skills::install_builtin_skills;
use crate::store::{remove_session_store, spawn_store_cleaner};
use crate::tools::ToolRegistry;
use crate::tools::registry::ToolChannel;

/// daemon 主体，负责连接 channel、维护 session、调用 provider。
pub struct Daemon {
    /// 应用配置。
    config: AppConfig,
    /// 应用路径。
    paths: AppPaths,
    /// session 注册表。
    sessions: Arc<Mutex<SessionRegistry>>,
    /// session 级别处理锁，保证同一 key 串行。
    session_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// 当前正在执行的 provider 请求，适用于 `/stop` 跨过 session 队列取消。
    active_turns: Arc<StdMutex<HashMap<String, ActiveTurn>>>,
    /// 模型 provider。
    provider: BuiltinProvider,
    /// 独立工具注册表。
    tools: ToolRegistry,
    /// cron provider 调用并发闸门。
    cron_semaphore: Arc<Semaphore>,
}

/// 单轮活跃请求状态，适用于 `/stop` 定位并取消当前 provider future。
#[derive(Clone)]
struct ActiveTurn {
    /// 取消令牌，daemon 在 stop 命令到达时触发。
    token: CancellationToken,
    /// 完成通知，stop 命令用它等待取消清理结束。
    done: Arc<Notify>,
    /// 完成标记，防止 stop 命令错过 notify 后永久等待。
    finished: Arc<AtomicBool>,
}

impl ActiveTurn {
    /// 创建活跃请求状态，适用于 provider 请求开始前注册。
    fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            done: Arc::new(Notify::new()),
            finished: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 标记请求已结束，适用于 provider 返回或被 stop 取消后的清理。
    fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.done.notify_waiters();
    }

    /// 等待请求结束，适用于 stop 命令确认取消已经处理完成。
    async fn wait_finished(&self) {
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        self.done.notified().await;
    }
}

/// 活跃请求清理守卫，适用于任意早退路径自动释放 `/stop` 等待。
struct ActiveTurnGuard {
    /// session key，用于从活跃表移除当前请求。
    session_key: String,
    /// 活跃请求表。
    active_turns: Arc<StdMutex<HashMap<String, ActiveTurn>>>,
    /// 当前请求状态，用于避免误删后续请求。
    active: ActiveTurn,
}

impl ActiveTurnGuard {
    /// 创建清理守卫，适用于 provider 流程开始后绑定生命周期。
    fn new(
        session_key: String,
        active_turns: Arc<StdMutex<HashMap<String, ActiveTurn>>>,
        active: ActiveTurn,
    ) -> Self {
        Self {
            session_key,
            active_turns,
            active,
        }
    }
}

impl Drop for ActiveTurnGuard {
    /// 移除当前活跃请求并通知 stop，适用于函数早退和正常完成。
    fn drop(&mut self) {
        let removed = {
            let mut active_turns = self
                .active_turns
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            let should_remove = active_turns
                .get(&self.session_key)
                .is_some_and(|current| Arc::ptr_eq(&current.finished, &self.active.finished));
            if should_remove {
                active_turns.remove(&self.session_key)
            } else {
                None
            }
        };
        if let Some(removed) = removed {
            removed.finish();
        }
    }
}

impl Daemon {
    /// 创建 daemon 实例，适用于 main 初始化 runtime 后进入异步流程。
    pub fn new(config: AppConfig, paths: AppPaths) -> AppResult<Self> {
        let provider = build_provider(config.clone(), paths.clone())?;
        let tools = ToolRegistry::builtins(paths.clone());
        Ok(Self {
            config,
            paths,
            sessions: Arc::new(Mutex::new(SessionRegistry::new())),
            session_locks: Arc::new(Mutex::new(HashMap::new())),
            active_turns: Arc::new(StdMutex::new(HashMap::new())),
            provider,
            tools,
            cron_semaphore: Arc::new(Semaphore::new(3)),
        })
    }

    /// 启动 daemon，直到收到 Ctrl-C 或 channel 队列关闭。
    pub async fn run(self: Arc<Self>) -> AppResult<()> {
        install_builtin_skills(&self.paths).await?;
        spawn_store_cleaner(self.paths.channel_store_dir.clone());
        let (tx, mut rx) = mpsc::channel::<InboundMessage>(128);
        let mut channels = build_channels(&self.config.channels)?;

        for channel in &mut channels {
            channel.start(tx.clone(), &self.paths).await?;
        }
        let handles: Arc<Vec<BuiltinChannelHandle>> = Arc::new(
            channels
                .iter()
                .map(BuiltinChannel::tool_handle)
                .collect::<Vec<_>>(),
        );
        spawn_cron_scheduler(
            self.paths.crons_dir.clone(),
            handles.as_ref().as_slice(),
            tx.clone(),
        );
        drop(tx);

        loop {
            tokio::select! {
                maybe_message = rx.recv() => {
                    match maybe_message {
                        Some(message) => {
                            let daemon = Arc::clone(&self);
                            let handles = Arc::clone(&handles);
                            tokio::spawn(async move {
                                if let Err(err) = daemon.handle_message(message, handles).await {
                                    crate::log_info!("daemon message task failed: {err}");
                                }
                            });
                        }
                        None => {
                            if channels.is_empty() {
                                tokio::signal::ctrl_c().await?;
                            }
                            break;
                        }
                    }
                }
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    break;
                }
            }
        }

        stop_channels(&mut channels).await
    }

    /// 处理单条入站消息，当前完成 `/reset` 和 provider 调用骨架。
    async fn handle_message(
        self: Arc<Self>,
        message: InboundMessage,
        channels: Arc<Vec<BuiltinChannelHandle>>,
    ) -> AppResult<()> {
        let _cron_permit = self.acquire_cron_permit_if_needed(&message).await?;
        let is_cron_task = message.is_cron_task();
        let session_key = build_message_key(&message.source);
        Self::log_inbound_message(&message, &session_key);
        if message.is_stop_command() {
            self.handle_stop_command(&message, &channels, &session_key)
                .await?;
            return Ok(());
        }
        let session_lock = self.session_lock(&session_key).await;
        let _guard = session_lock.lock().await;
        crate::log_info!(
            "daemon session lock acquired session_key={} message_id={}",
            session_key,
            message.message_id.as_deref().unwrap_or("")
        );
        if message.is_reset_command() {
            self.cancel_active_turn(&session_key);
            let mut sessions = self.sessions.lock().await;
            let session = sessions.reset(&message.source);
            let new_session_id = session.id.clone();
            crate::log_info!(
                "daemon session reset session_key={} new_session_id={}",
                session.key,
                new_session_id
            );
            drop(sessions);
            if let Err(err) =
                remove_session_store(&self.paths.channel_store_dir, &session_key).await
            {
                crate::log_info!(
                    "store session remove failed session_key={session_key} error={err}"
                );
            }
            if let Err(err) =
                session_store::remove_session(&self.paths.sessions_dir, &session_key).await
            {
                crate::log_info!(
                    "session history remove failed session_key={session_key} error={err}"
                );
            }
            if let Err(err) = self.tools.remove_plan(&session_key).await {
                crate::log_info!("plan state remove failed session_key={session_key} error={err}");
            }
            Self::acknowledge_channel(&channels, &message, ChannelAckKind::ResetDone).await;
            return Ok(());
        }

        let active_turn = self.register_active_turn(&session_key, "message");
        let _active_guard = ActiveTurnGuard::new(
            session_key.clone(),
            Arc::clone(&self.active_turns),
            active_turn.clone(),
        );
        Self::acknowledge_channel(&channels, &message, ChannelAckKind::Received).await;
        if active_turn.token.is_cancelled() {
            crate::log_info!("daemon message cancelled before context session_key={session_key}");
            return Ok(());
        }

        let session = if is_cron_task {
            let context = load_cron_context().await?;
            crate::log_info!(
                "daemon cron_context loaded chars={}\n{}",
                context.instructions.chars().count(),
                render_initial_context_for_log(&context)
            );
            let mut session = crate::session::SessionState::new(format!(
                "{session_key}:cron:{}",
                new_session_id()
            ));
            session.instructions = context.instructions;
            session.initial_context_loaded = true;
            session
        } else {
            self.restore_session_if_needed(&message).await?;
            if self.session_needs_initial_context(&message).await {
                let context = load_initial_context(&self.paths, &message.source).await?;
                let rendered_prompt = render_initial_context_for_log(&context);
                crate::log_info!(
                    "daemon initial_context loaded chars={} work_dir={}\n{}",
                    context.instructions.chars().count(),
                    self.paths.work_dir.display(),
                    rendered_prompt
                );
                let mut sessions = self.sessions.lock().await;
                sessions.set_initial_context(&message.source, context);
            }
            let mut sessions = self.sessions.lock().await;
            sessions.get_or_create(&message.source).clone()
        };
        if active_turn.token.is_cancelled() {
            crate::log_info!("daemon message cancelled after context session_key={session_key}");
            return Ok(());
        }
        let base_tool_channel = channels
            .iter()
            .find(|channel| channel.name() == message.source.channel_name)
            .map(|channel| {
                Arc::new(channel.clone()) as Arc<dyn crate::tools::registry::ToolChannel>
            })
            .ok_or_else(|| {
                AppError::Channel(format!(
                    "channel `{}` not found for tool context",
                    message.source.channel_name
                ))
            })?;
        let mut history = if is_cron_task {
            Vec::new()
        } else if self.config.cache_session {
            session_store::load_history(&self.paths.sessions_dir, &session.key).await?
        } else {
            Vec::new()
        };
        if active_turn.token.is_cancelled() {
            crate::log_info!(
                "daemon message cancelled after history load session_key={session_key}"
            );
            return Ok(());
        }
        if !is_cron_task && self.config.cache_session {
            let status = context_window_status(&session, &history, &message.text);
            if status.limit_reached {
                Self::send_context_compaction_status(
                    Arc::clone(&base_tool_channel),
                    &message,
                    format!(
                        "上下文接近窗口上限，开始压缩... (~{} / {} tokens)",
                        status.estimated_tokens,
                        status.auto_compact_limit.unwrap_or_default()
                    ),
                )
                .await;
            }
            let model_summary = if status.limit_reached {
                match self
                    .run_cancellable_provider_request(
                        &session.key,
                        "compact",
                        self.provider.compact(&session, &history),
                    )
                    .await
                {
                    Ok(Some(reply)) => {
                        crate::log_info!(
                            "daemon context compact summary generated session_key={} summary_chars={} total_tokens={:?}",
                            session.key,
                            reply.summary.chars().count(),
                            reply.total_tokens
                        );
                        Some(reply.summary)
                    }
                    Ok(None) => return Ok(()),
                    Err(err) => {
                        crate::log_info!(
                            "daemon context compact summary failed session_key={} error={}",
                            session.key,
                            err
                        );
                        None
                    }
                }
            } else {
                None
            };
            let context_plan = if let Some(summary) = model_summary {
                prepare_context_window_with_summary(&session, &history, &message.text, summary)
            } else {
                prepare_context_window(&session, &history, &message.text)
            };
            if context_plan.compacted {
                let dropped_items = context_plan.dropped_items;
                crate::log_info!(
                    "daemon context compacted session_key={} estimated_tokens={} dropped_items={} history_items_before={} history_items_after={}",
                    session.key,
                    context_plan.estimated_tokens,
                    dropped_items,
                    history.len(),
                    context_plan.history.len()
                );
                session_store::append_compaction(
                    &self.paths.sessions_dir,
                    &session,
                    context_plan.history.clone(),
                )
                .await?;
                history = context_plan.history;
                Self::send_context_compaction_status(
                    Arc::clone(&base_tool_channel),
                    &message,
                    format!(
                        "上下文压缩完成，保留 {} 条历史，压缩 {} 条。",
                        history.len(),
                        dropped_items
                    ),
                )
                .await;
            } else if status.limit_reached {
                Self::send_context_compaction_status(
                    Arc::clone(&base_tool_channel),
                    &message,
                    "上下文压缩结束，没有可压缩的历史。".to_string(),
                )
                .await;
            }
        }
        crate::log_info!(
            "daemon provider start session_key={} session_id={} cache_session={} history_items={} used_tokens={}",
            session.key,
            session.id,
            self.config.cache_session,
            history.len(),
            session.used_tokens
        );
        if active_turn.token.is_cancelled() {
            crate::log_info!("daemon message cancelled before provider session_key={session_key}");
            return Ok(());
        }
        let tool_channel = Arc::new(SingleReplyChannel::new(
            Arc::clone(&base_tool_channel),
            message.message_id.clone(),
        )) as Arc<dyn crate::tools::registry::ToolChannel>;
        let tools = if message.is_cron_task() {
            // 触发条件：cron 调度器把任务提示词注入 provider。
            // 常规工具列表包含 __cron，会让模型把“每分钟”等任务文本误判成改调度。
            // 调度执行态禁用 __cron，防止定时任务自我修改或递归管理。
            self.tools.without_handler("__cron")
        } else {
            self.tools.clone()
        };

        let provider_future = self.provider.complete(
            &session,
            &history,
            &message.source,
            &message.text,
            &message.attachments,
            &tools,
            Arc::clone(&tool_channel),
        );
        let reply = match self
            .run_cancellable_provider_request(&session.key, "complete", provider_future)
            .await
        {
            Ok(Some(reply)) => reply,
            Ok(None) => return Ok(()),
            Err(err) => {
                self.handle_provider_error(&message, &session.key, &err, tool_channel)
                    .await?;
                return Ok(());
            }
        };
        crate::log_info!(
            "daemon provider finished session_key={} reply_chars={} raw_items={} total_tokens={:?} reset_session={}",
            session.key,
            reply.text.chars().count(),
            reply.raw_items.len(),
            reply.total_tokens,
            reply.reset_session
        );
        {
            if !is_cron_task {
                let mut sessions = self.sessions.lock().await;
                if reply.reset_session {
                    sessions.reset(&message.source);
                }
                sessions.record_token_usage(&message.source, reply.total_tokens);
            }
        }
        if !is_cron_task && !reply.reset_session && self.config.cache_session {
            session_store::append_turn(
                &self.paths.sessions_dir,
                &session,
                &message.text,
                &reply.text,
                reply.total_tokens,
            )
            .await?;
        } else if !is_cron_task
            && let Err(err) =
                session_store::remove_session(&self.paths.sessions_dir, &session.key).await
        {
            crate::log_info!(
                "session history remove after provider reset failed session_key={} error={}",
                session.key,
                err
            );
        }
        let reply_hash = new_reply_hash(&session.key);
        let outbound_text = format!("[{reply_hash}] {}", reply.text);
        crate::log_info!(
            "daemon outbound prepared session_key={} reply_hash={} reply_chars={}",
            session.key,
            reply_hash,
            outbound_text.chars().count()
        );
        let (recipient, chat_id) = outbound_target_from_source(&message.source);
        let outbound = OutboundMessage {
            channel_name: message.source.channel_name,
            chat_id,
            recipient,
            text: outbound_text,
            reply_to: message.message_id,
            format: OutboundFormat::Text,
        };
        if let Err(send_err) = tool_channel.send(outbound).await {
            crate::log_info!("daemon final reply update failed: {send_err}");
        }
        Ok(())
    }

    /// 记录入站消息，适用于所有命令和普通请求统一排查。
    fn log_inbound_message(message: &InboundMessage, session_key: &str) {
        crate::log_info!(
            "daemon inbound channel={} platform={} chat_type={} chat_id={} thread_id={} user_id={} message_id={} session_key={}",
            message.source.channel_name,
            message.source.platform,
            message.source.chat_type,
            message.source.chat_id,
            message.source.thread_id.as_deref().unwrap_or(""),
            message.source.user_id.as_deref().unwrap_or(""),
            message.message_id.as_deref().unwrap_or(""),
            session_key
        );
    }

    /// 处理 stop 命令，适用于不等待 session 队列直接取消当前请求。
    async fn handle_stop_command(
        &self,
        message: &InboundMessage,
        channels: &[BuiltinChannelHandle],
        session_key: &str,
    ) -> AppResult<()> {
        crate::log_info!(
            "daemon stop requested session_key={} message_id={}",
            session_key,
            message.message_id.as_deref().unwrap_or("")
        );
        if let Some(active) = self.cancel_active_turn(session_key) {
            active.wait_finished().await;
            crate::log_info!("daemon stop completed session_key={session_key}");
        } else {
            crate::log_info!(
                "daemon stop completed reason=no_active_turn session_key={session_key}"
            );
        }
        Self::acknowledge_channel(channels, message, ChannelAckKind::StopDone).await;
        Ok(())
    }

    /// 取消当前活跃请求，适用于 stop/reset 需要打断 provider future。
    fn cancel_active_turn(&self, session_key: &str) -> Option<ActiveTurn> {
        let active = self
            .active_turns
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(session_key)
            .cloned();
        if let Some(active) = active.as_ref() {
            active.token.cancel();
        }
        active
    }

    /// 注册活跃请求，适用于普通消息进入可取消生命周期。
    fn register_active_turn(&self, session_key: &str, action: &str) -> ActiveTurn {
        let active = ActiveTurn::new();
        let replaced = self
            .active_turns
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(session_key.to_string(), active.clone())
            .is_some();
        if replaced {
            crate::log_info!(
                "daemon active turn replaced session_key={} action={}",
                session_key,
                action
            );
        }
        crate::log_info!(
            "daemon active turn registered session_key={} action={}",
            session_key,
            action
        );
        active
    }

    /// 获取当前活跃请求，适用于 provider 请求复用消息级取消令牌。
    fn active_turn(&self, session_key: &str) -> Option<ActiveTurn> {
        self.active_turns
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(session_key)
            .cloned()
    }

    /// 包装 provider 请求，适用于 `/stop` 通过取消令牌中断等待中的 future。
    async fn run_cancellable_provider_request<F, T>(
        &self,
        session_key: &str,
        action: &str,
        future: F,
    ) -> AppResult<Option<T>>
    where
        F: Future<Output = AppResult<T>>,
    {
        let Some(active) = self.active_turn(session_key) else {
            return future.await.map(Some);
        };
        crate::log_info!(
            "daemon cancellable provider action started session_key={} action={}",
            session_key,
            action
        );
        tokio::select! {
            result = future => result.map(Some),
            () = active.token.cancelled() => {
                crate::log_info!(
                    "daemon cancellable provider action cancelled session_key={} action={}",
                    session_key,
                    action
                );
                Ok(None)
            }
        }
    }

    /// 发送上下文压缩状态，适用于压缩耗时前后给用户可见反馈。
    async fn send_context_compaction_status(
        channel: Arc<dyn crate::tools::registry::ToolChannel>,
        message: &InboundMessage,
        text: String,
    ) {
        let (recipient, chat_id) = outbound_target_from_source(&message.source);
        let outbound = OutboundMessage {
            channel_name: message.source.channel_name.clone(),
            chat_id,
            recipient,
            text,
            reply_to: message.message_id.clone(),
            format: OutboundFormat::Text,
        };
        if let Err(err) = channel.send(outbound).await {
            crate::log_info!("daemon context compaction status send failed: {err}");
        }
    }

    /// 处理 provider 失败，适用于请求中断后补偿计划卡片和用户回复。
    async fn handle_provider_error(
        &self,
        message: &InboundMessage,
        session_key: &str,
        err: &AppError,
        tool_channel: Arc<dyn crate::tools::registry::ToolChannel>,
    ) -> AppResult<()> {
        crate::log_info!("provider error: {err}");
        let desc = provider_error_desc(err);
        crate::log_info!(
            "daemon provider failure handled session_key={} message_id={} desc={} error={}",
            session_key,
            message.message_id.as_deref().unwrap_or(""),
            desc,
            err
        );
        match self
            .tools
            .fail_active_plan(session_key, &desc, Arc::clone(&tool_channel))
            .await
        {
            Ok(true) => {
                crate::log_info!("daemon active plan marked failed session_key={session_key}")
            }
            Ok(false) => crate::log_info!("daemon active plan not found session_key={session_key}"),
            Err(update_err) => crate::log_info!(
                "daemon active plan failed update error session_key={session_key} error={update_err}"
            ),
        }
        let reply_hash = new_reply_hash(session_key);
        crate::log_info!(
            "daemon provider failure reply prepared session_key={} inbound_message_id={} reply_hash={} desc={}",
            session_key,
            message.message_id.as_deref().unwrap_or(""),
            reply_hash,
            desc
        );
        let (recipient, chat_id) = outbound_target_from_source(&message.source);
        let outbound = OutboundMessage {
            channel_name: message.source.channel_name.clone(),
            chat_id,
            recipient,
            text: format!("[{reply_hash}] {desc}"),
            reply_to: message.message_id.clone(),
            format: OutboundFormat::Text,
        };
        let _ = tool_channel.send(outbound).await?;
        Ok(())
    }

    /// 按统一确认语义调用 channel，具体反馈方式由 channel 能力决定。
    async fn acknowledge_channel(
        channels: &[BuiltinChannelHandle],
        message: &InboundMessage,
        kind: ChannelAckKind,
    ) {
        let Some(channel) = channels
            .iter()
            .find(|channel| channel.name() == message.source.channel_name)
        else {
            crate::log_info!(
                "channel acknowledge skipped reason=missing_channel kind={:?} channel={}",
                kind,
                message.source.channel_name
            );
            return;
        };
        let capability = channel.ack_capability(kind);
        if matches!(capability, ChannelAckCapability::None) {
            crate::log_info!(
                "channel acknowledge skipped reason=capability_none kind={:?} channel={}",
                kind,
                channel.name()
            );
            return;
        }
        match channel.acknowledge(message, kind).await {
            Ok(()) => crate::log_info!(
                "channel acknowledge finished kind={:?} capability={:?} channel={}",
                kind,
                capability,
                channel.name()
            ),
            Err(err) => crate::log_info!(
                "channel acknowledge failed kind={:?} capability={:?} channel={} error={}",
                kind,
                capability,
                channel.name(),
                err
            ),
        }
    }

    /// 判断当前消息绑定的 session 是否需要首轮上下文。
    async fn session_needs_initial_context(&self, message: &InboundMessage) -> bool {
        let mut sessions = self.sessions.lock().await;
        sessions.needs_initial_context(&message.source)
    }

    /// 按需从本地历史恢复 session，适用于 daemon 重启后的同 key 续聊。
    async fn restore_session_if_needed(&self, message: &InboundMessage) -> AppResult<()> {
        if !self.config.cache_session {
            return Ok(());
        }
        {
            let sessions = self.sessions.lock().await;
            if sessions.get(&message.source).is_some() {
                return Ok(());
            }
        }
        let session_key = build_message_key(&message.source);
        let Some(restored) =
            session_store::load_session_meta(&self.paths.sessions_dir, &session_key).await?
        else {
            return Ok(());
        };
        crate::log_info!(
            "daemon session restored session_key={} session_id={} used_tokens={}",
            restored.key,
            restored.id,
            restored.used_tokens
        );
        let mut sessions = self.sessions.lock().await;
        sessions.insert_restored(restored);
        Ok(())
    }

    /// 返回配置路径，适用于状态接口和调试输出。
    pub fn config_path(&self) -> &std::path::Path {
        &self.paths.config_path
    }

    /// 获取指定 session key 的处理锁。
    async fn session_lock(&self, session_key: &str) -> Arc<Mutex<()>> {
        let mut locks = self.session_locks.lock().await;
        locks
            .entry(session_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// cron 消息进入 provider 前获取并发许可，普通用户消息不受影响。
    async fn acquire_cron_permit_if_needed(
        &self,
        message: &InboundMessage,
    ) -> AppResult<Option<OwnedSemaphorePermit>> {
        if !message.is_cron_task() {
            return Ok(None);
        }
        self.cron_semaphore
            .clone()
            .acquire_owned()
            .await
            .map(Some)
            .map_err(|_| AppError::Cron("cron semaphore closed".to_string()))
    }
}

/// 单入站消息回复通道，保证同一个平台 message_id 只产生一条机器人回复。
struct SingleReplyChannel {
    /// 真实 channel 句柄。
    inner: Arc<dyn crate::tools::registry::ToolChannel>,
    /// 当前入站消息 id，第一次发送时用它创建回复关系。
    inbound_message_id: Option<String>,
    /// 本轮已经创建的机器人回复消息 id。
    reply_message_id: Mutex<Option<String>>,
}

impl SingleReplyChannel {
    /// 创建单回复包装器，适用于 daemon 处理一条入站消息期间共享给 tools 和最终回复。
    fn new(
        inner: Arc<dyn crate::tools::registry::ToolChannel>,
        inbound_message_id: Option<String>,
    ) -> Self {
        Self {
            inner,
            inbound_message_id,
            reply_message_id: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ToolChannel for SingleReplyChannel {
    /// 第一次真实发送，后续发送都改同一条机器人回复。
    async fn send(&self, mut message: OutboundMessage) -> AppResult<SendResult> {
        let mut reply_message_id = self.reply_message_id.lock().await;
        if let Some(message_id) = reply_message_id.clone() {
            let result = self
                .inner
                .update_message(MessageUpdate {
                    channel_name: message.channel_name,
                    message_id: message_id.clone(),
                    text: message.text,
                    format: message.format,
                })
                .await?;
            return Ok(SendResult {
                success: result.success,
                message_id: Some(message_id),
            });
        }

        message.reply_to = self.inbound_message_id.clone();
        let result = self.inner.send(message).await?;
        if let Some(message_id) = result.message_id.clone() {
            *reply_message_id = Some(message_id);
        }
        Ok(result)
    }

    /// 更新当前机器人回复；如果还没记录到本轮回复，则按传入 id 更新。
    async fn update_message(&self, message: MessageUpdate) -> AppResult<SendResult> {
        let mut reply_message_id = self.reply_message_id.lock().await;
        let target_message_id = reply_message_id
            .clone()
            .unwrap_or_else(|| message.message_id.clone());
        let result = self
            .inner
            .update_message(MessageUpdate {
                channel_name: message.channel_name,
                message_id: target_message_id.clone(),
                text: message.text,
                format: message.format,
            })
            .await?;
        if reply_message_id.is_none() {
            *reply_message_id = Some(target_message_id.clone());
        }
        Ok(SendResult {
            success: result.success,
            message_id: Some(target_message_id),
        })
    }

    /// 透传结构化用户输入请求，适用于飞书卡片等 channel 能力。
    async fn request_user_input(
        &self,
        source: &MessageSource,
        request: UserInputRequest,
    ) -> AppResult<UserInputResponse> {
        self.inner.request_user_input(source, request).await
    }
}

/// 关闭所有已启动 channel，适用于 daemon 正常退出。
async fn stop_channels(channels: &mut [BuiltinChannel]) -> AppResult<()> {
    for channel in channels {
        channel.stop().await?;
    }
    Ok(())
}

/// 启动 cron 调度器，适用于把本地计划任务注入 daemon 入站队列。
fn spawn_cron_scheduler(
    crons_dir: std::path::PathBuf,
    channels: &[BuiltinChannelHandle],
    tx: mpsc::Sender<InboundMessage>,
) {
    let scheduler_channels = channels
        .iter()
        .map(|channel| SchedulerChannel {
            name: channel.name().to_string(),
            platform: channel.platform_name().to_string(),
        })
        .collect::<Vec<_>>();
    tokio::spawn(async move {
        run_cron_scheduler(crons_dir, scheduler_channels, tx).await;
    });
}

/// 返回用户侧 provider 失败简述，适用于避免暴露过长上游错误。
fn provider_error_desc(err: &AppError) -> String {
    let message = err.to_string();
    if let Some(status) = provider_status_code(&message) {
        if let Some(summary) = provider_error_message(&message) {
            return format!("provider {status}，{summary}");
        }
        return format!("provider {status}，上游请求失败");
    }
    "provider 请求失败".to_string()
}

/// 提取 provider 5xx 状态码，适用于用户侧短错误文案。
fn provider_status_code(message: &str) -> Option<String> {
    message
        .as_bytes()
        .windows(3)
        .find(|window| {
            window[0] == b'5' && window[1].is_ascii_digit() && window[2].is_ascii_digit()
        })
        .and_then(|window| std::str::from_utf8(window).ok())
        .map(str::to_string)
}

/// 提取 provider 错误摘要，适用于保留上游关键信息。
fn provider_error_message(message: &str) -> Option<String> {
    let value = message
        .split("\"message\":\"")
        .nth(1)?
        .split('"')
        .next()?
        .trim();
    (!value.is_empty()).then(|| value.to_string())
}
