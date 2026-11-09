use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, oneshot};

use crate::error::{AppError, AppResult};
use crate::message::{UserInputRequest, UserInputResponse};
use crate::resource::{ResourceUsage, estimate_answers_bytes, estimate_user_input_request_bytes};

/// 平台附加状态的资源估算接口，适用于公共等待表统计，例如 `() -> 0`。
pub(crate) trait PendingInputMetadata {
    /// 返回附加状态占用的动态字节数，例如 `card_message_id -> capacity`。
    fn resource_bytes(&self) -> usize;
}

impl PendingInputMetadata for () {
    /// 空附加状态不占用动态内存，例如 `() -> 0`。
    fn resource_bytes(&self) -> usize {
        0
    }
}

/// 等待中的结构化用户输入集合，适用于 channel 工具调用，例如 `request -> response`。
#[derive(Default)]
pub(crate) struct PendingUserInputs<M> {
    /// 按请求 ID 保存等待状态。
    items: HashMap<String, PendingUserInput<M>>,
}

/// 单个结构化用户输入等待状态，适用于多题逐步作答，例如 `2 questions -> 2 answers`。
struct PendingUserInput<M> {
    /// 原始问题，用于校验答案与超时自动选择。
    request: UserInputRequest,
    /// 已收集答案。
    answers: HashMap<String, Vec<String>>,
    /// 完成时唤醒工具调用。
    sender: Option<oneshot::Sender<UserInputResponse>>,
    /// 平台专用附加状态。
    metadata: M,
}

/// 新建等待项的接收端，适用于平台发送交互消息后等待，例如 `start -> waiter`。
pub(crate) struct PendingUserInputWaiter {
    /// 稳定请求 ID，用于平台回调定位等待项。
    request_id: String,
    /// 用户完成选择时收到响应。
    receiver: oneshot::Receiver<UserInputResponse>,
}

impl PendingUserInputWaiter {
    /// 返回请求 ID，适用于构造按钮数据，例如 `waiter -> req-42`。
    pub(crate) fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// 单次答案写入结果，适用于平台决定继续等待或更新完成态。
pub(crate) enum PendingAnswer {
    /// 没有匹配的等待项。
    Missing,
    /// 已接收答案。
    Accepted {
        /// 对应请求 ID。
        request_id: String,
        /// 是否已经收齐所有问题。
        finished: bool,
    },
}

impl PendingAnswer {
    /// 返回已接收答案的信息，适用于平台分支处理，例如 `Accepted -> Some((id, true))`。
    pub(crate) fn accepted(&self) -> Option<(&str, bool)> {
        match self {
            Self::Missing => None,
            Self::Accepted {
                request_id,
                finished,
            } => Some((request_id, *finished)),
        }
    }
}

/// 等待项只读视图，适用于平台渲染下一步交互状态。
pub(crate) struct PendingUserInputView<'a, M> {
    /// 原始问题。
    pub(crate) request: &'a UserInputRequest,
    /// 当前答案。
    pub(crate) answers: &'a HashMap<String, Vec<String>>,
    /// 平台专用附加状态。
    pub(crate) metadata: &'a M,
}

impl<M> PendingUserInputs<M> {
    /// 创建空等待集合，适用于 channel 初始化，例如 `new -> 0 items`。
    pub(crate) fn new() -> Self {
        Self {
            items: HashMap::new(),
        }
    }

    /// 登记新请求并返回等待端，适用于发送交互消息前，例如 `request -> req-id + receiver`。
    pub(crate) fn start(
        &mut self,
        request: UserInputRequest,
        metadata: M,
    ) -> PendingUserInputWaiter {
        let request_id = uuid::Uuid::new_v4().simple().to_string();
        let (sender, receiver) = oneshot::channel();
        self.items.insert(
            request_id.clone(),
            PendingUserInput {
                request,
                answers: HashMap::new(),
                sender: Some(sender),
                metadata,
            },
        );
        PendingUserInputWaiter {
            request_id,
            receiver,
        }
    }

    /// 移除等待项，适用于平台发送失败，例如 `req-42 -> removed`。
    pub(crate) fn remove(&mut self, request_id: &str) {
        self.items.remove(request_id);
    }

    /// 清空等待项，适用于 channel reset 或停止，例如 `3 items -> 0 items`。
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    /// 返回等待项只读视图，适用于平台校验按钮索引，例如 `req-42 -> request`。
    pub(crate) fn view(&self, request_id: &str) -> Option<PendingUserInputView<'_, M>> {
        self.items
            .get(request_id)
            .map(|pending| PendingUserInputView {
                request: &pending.request,
                answers: &pending.answers,
                metadata: &pending.metadata,
            })
    }

    /// 返回平台附加状态的可变引用，适用于绑定卡片消息，例如 `req-42 -> metadata`。
    pub(crate) fn metadata_mut(&mut self, request_id: &str) -> Option<&mut M> {
        self.items
            .get_mut(request_id)
            .map(|pending| &mut pending.metadata)
    }

    /// 记录指定问题的答案，适用于按钮回调，例如 `req-42 + confirm + yes -> finished`。
    pub(crate) fn answer(
        &mut self,
        request_id: &str,
        question_id: &str,
        value: String,
    ) -> PendingAnswer {
        let Some(pending) = self.items.get_mut(request_id) else {
            return PendingAnswer::Missing;
        };
        pending.answers.insert(question_id.to_string(), vec![value]);
        let finished = pending
            .request
            .questions
            .iter()
            .all(|question| pending.answers.contains_key(&question.id));
        PendingAnswer::Accepted {
            request_id: request_id.to_string(),
            finished,
        }
    }

    /// 按附加状态定位请求并回答下一题，适用于文本兜底，例如 `same source + text -> accepted`。
    pub(crate) fn answer_next_where(
        &mut self,
        predicate: impl Fn(&M) -> bool,
        value: String,
    ) -> PendingAnswer {
        let Some((request_id, question_id)) =
            self.items.iter().find_map(|(request_id, pending)| {
                if !predicate(&pending.metadata) {
                    return None;
                }
                pending
                    .request
                    .questions
                    .iter()
                    .find(|question| !pending.answers.contains_key(&question.id))
                    .map(|question| (request_id.clone(), question.id.clone()))
            })
        else {
            return PendingAnswer::Missing;
        };
        self.answer(&request_id, &question_id, value)
    }

    /// 完成请求并唤醒等待者，适用于收齐答案后，例如 `req-42 -> response sent`。
    pub(crate) fn finish(&mut self, request_id: &str) -> bool {
        let Some(mut pending) = self.items.remove(request_id) else {
            return false;
        };
        if let Some(sender) = pending.sender.take() {
            let _ = sender.send(UserInputResponse {
                answers: pending.answers,
            });
        }
        true
    }

    /// 超时后为未答问题选择首项，适用于自动决策，例如 `req-42 -> completed response`。
    pub(crate) fn auto_resolve(&mut self, request_id: &str) -> AppResult<UserInputResponse> {
        let Some(pending) = self.items.remove(request_id) else {
            return Err(AppError::Channel(
                "user input request is missing".to_string(),
            ));
        };
        let mut answers = pending.answers;
        for question in pending.request.questions {
            answers.entry(question.id).or_insert_with(|| {
                question
                    .options
                    .first()
                    .map(|option| vec![option.label.clone()])
                    .unwrap_or_default()
            });
        }
        Ok(UserInputResponse { answers })
    }
}

impl<M: PendingInputMetadata> PendingUserInputs<M> {
    /// 返回等待集合资源估算，适用于 resources 命令，例如 `pending_inputs -> usage`。
    pub(crate) fn resource_usage(&self, name: &str) -> ResourceUsage {
        let body_bytes = self
            .items
            .iter()
            .map(|(key, pending)| {
                key.capacity()
                    + estimate_user_input_request_bytes(&pending.request)
                    + estimate_answers_bytes(&pending.answers)
                    + pending.metadata.resource_bytes()
            })
            .sum::<usize>();
        let entry_bytes = self
            .items
            .capacity()
            .saturating_mul(std::mem::size_of::<(String, PendingUserInput<M>)>());
        ResourceUsage::new(
            name,
            "hashmap",
            self.items.len(),
            Some(self.items.capacity()),
            entry_bytes.saturating_add(body_bytes),
        )
    }
}

/// 等待平台用户输入或执行超时自动选择，适用于工具调用，例如 `30s -> response`。
pub(crate) async fn wait_for_user_input<M>(
    pending: &Arc<Mutex<PendingUserInputs<M>>>,
    waiter: PendingUserInputWaiter,
    timeout_ms: Option<u64>,
    channel_name: &str,
) -> AppResult<UserInputResponse> {
    let PendingUserInputWaiter {
        request_id,
        receiver,
    } = waiter;
    if let Some(timeout_ms) = timeout_ms {
        return match tokio::time::timeout(Duration::from_millis(timeout_ms), receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(AppError::Channel(format!(
                "{channel_name} user input waiter was closed"
            ))),
            Err(_) => pending.lock().await.auto_resolve(&request_id),
        };
    }
    receiver
        .await
        .map_err(|_| AppError::Channel(format!("{channel_name} user input waiter was closed")))
}
