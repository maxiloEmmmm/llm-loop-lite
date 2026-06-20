use crate::session::SessionState;
use crate::session_store::ConversationItem;

/// Codex 风格默认压缩阈值分子。
const AUTO_COMPACT_RATIO_NUMERATOR: u64 = 9;
/// Codex 风格默认压缩阈值分母。
const AUTO_COMPACT_RATIO_DENOMINATOR: u64 = 10;
/// 压缩后最多保留的近期历史 token。
const COMPACT_USER_MESSAGE_MAX_TOKENS: u64 = 20_000;
/// 压缩摘要最多保留的旧用户请求数量。
const SUMMARY_USER_SNIPPET_LIMIT: usize = 12;
/// 单条旧用户请求摘要的最大字符数。
const SUMMARY_USER_SNIPPET_MAX_CHARS: usize = 220;
/// 粗略 token 换算比例。
const CHARS_PER_TOKEN: u64 = 4;
/// 压缩摘要前缀，避免模型把旧任务当成当前指令。
const SUMMARY_PREFIX: &str = "[CONTEXT COMPACTION - REFERENCE ONLY]";

/// 上下文预处理结果，适用于 provider 请求前替换历史。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextWindowPlan {
    /// provider 本轮应该看到的历史。
    pub history: Vec<ConversationItem>,
    /// 本轮请求粗略 token 估算。
    pub estimated_tokens: u64,
    /// 是否发生了压缩。
    pub compacted: bool,
    /// 被压缩移除的历史项数量。
    pub dropped_items: usize,
}

/// Codex 风格上下文窗口状态，适用于 daemon 决定是否提示用户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextWindowStatus {
    /// 本轮请求粗略 token 估算。
    pub estimated_tokens: u64,
    /// 自动压缩 token 阈值。
    pub auto_compact_limit: Option<u64>,
    /// 是否达到自动压缩阈值。
    pub limit_reached: bool,
}

/// 估算 Codex 风格上下文窗口状态，适用于压缩前发用户状态。
pub fn context_window_status(
    session: &SessionState,
    history: &[ConversationItem],
    user_input: &str,
) -> ContextWindowStatus {
    let estimated_tokens = estimate_request_tokens(session, history, user_input);
    let auto_compact_limit = session
        .max_context_tokens
        .map(|max_context_tokens| auto_compact_token_limit(max_context_tokens));
    let limit_reached = auto_compact_limit
        .map(|limit| estimated_tokens >= limit)
        .unwrap_or(false);
    ContextWindowStatus {
        estimated_tokens,
        auto_compact_limit,
        limit_reached,
    }
}

/// 根据 Codex 风格阈值准备 provider 历史。
pub fn prepare_context_window(
    session: &SessionState,
    history: &[ConversationItem],
    user_input: &str,
) -> ContextWindowPlan {
    prepare_context_window_inner(session, history, user_input, None)
}

/// 根据模型摘要准备 provider 历史，适用于 provider.compact 成功后的替换。
pub fn prepare_context_window_with_summary(
    session: &SessionState,
    history: &[ConversationItem],
    user_input: &str,
    summary_text: String,
) -> ContextWindowPlan {
    prepare_context_window_inner(session, history, user_input, Some(summary_text))
}

/// 根据 Codex 风格阈值准备上下文，适用于统一处理模型摘要和本地摘要。
fn prepare_context_window_inner(
    session: &SessionState,
    history: &[ConversationItem],
    user_input: &str,
    summary_text: Option<String>,
) -> ContextWindowPlan {
    let status = context_window_status(session, history, user_input);
    let estimated_tokens = status.estimated_tokens;
    if !status.limit_reached {
        return ContextWindowPlan {
            history: history.to_vec(),
            estimated_tokens,
            compacted: false,
            dropped_items: 0,
        };
    };
    if history.len() <= 2 {
        return ContextWindowPlan {
            history: history.to_vec(),
            estimated_tokens,
            compacted: false,
            dropped_items: 0,
        };
    }

    compact_history(history, estimated_tokens, summary_text)
}

/// 计算 Codex 风格自动压缩阈值，适用于默认 90% context window。
fn auto_compact_token_limit(max_context_tokens: u64) -> u64 {
    max_context_tokens.saturating_mul(AUTO_COMPACT_RATIO_NUMERATOR) / AUTO_COMPACT_RATIO_DENOMINATOR
}

/// 粗略估算本轮请求 token，适用于压缩触发判断。
pub fn estimate_request_tokens(
    session: &SessionState,
    history: &[ConversationItem],
    user_input: &str,
) -> u64 {
    let instruction_tokens = estimate_text_tokens(&session.instructions);
    let history_tokens = effective_history(history, user_input)
        .iter()
        .map(estimate_item_tokens)
        .fold(0_u64, u64::saturating_add);
    instruction_tokens
        .saturating_add(history_tokens)
        .saturating_add(estimate_text_tokens(user_input))
}

/// 对历史做确定性压缩，适用于尚未接入独立 summarizer 的 provider 前置层。
fn compact_history(
    history: &[ConversationItem],
    estimated_tokens: u64,
    summary_text: Option<String>,
) -> ContextWindowPlan {
    let mut retained_reversed = Vec::new();
    let mut retained_tokens = 0_u64;
    for item in history.iter().rev() {
        let item_tokens = estimate_item_tokens(item);
        if retained_tokens.saturating_add(item_tokens) > COMPACT_USER_MESSAGE_MAX_TOKENS
            && !retained_reversed.is_empty()
        {
            break;
        }
        retained_tokens = retained_tokens.saturating_add(item_tokens);
        retained_reversed.push(item.clone());
    }
    retained_reversed.reverse();

    let dropped_items = history.len().saturating_sub(retained_reversed.len());
    if dropped_items == 0 {
        return ContextWindowPlan {
            history: history.to_vec(),
            estimated_tokens,
            compacted: false,
            dropped_items: 0,
        };
    }

    let summary = summary_text
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| build_compaction_summary(&history[..dropped_items], dropped_items));
    let compacted = prepend_compaction_summary(summary, retained_reversed);

    ContextWindowPlan {
        history: compacted,
        estimated_tokens,
        compacted: true,
        dropped_items,
    }
}

/// 注入压缩摘要，适用于避免 provider 收到连续 user 消息。
fn prepend_compaction_summary(
    summary: String,
    mut retained: Vec<ConversationItem>,
) -> Vec<ConversationItem> {
    match retained.first_mut() {
        Some(ConversationItem::User { text }) => {
            *text = format!("{summary}\n\n{text}");
            retained
        }
        _ => {
            let mut compacted = Vec::with_capacity(retained.len() + 1);
            compacted.push(ConversationItem::User { text: summary });
            compacted.extend(retained);
            compacted
        }
    }
}

/// 构造压缩 handoff，适用于明确隔离旧任务和当前用户消息。
fn build_compaction_summary(dropped: &[ConversationItem], dropped_items: usize) -> String {
    let mut snippets = Vec::new();
    for item in dropped {
        if snippets.len() >= SUMMARY_USER_SNIPPET_LIMIT {
            break;
        }
        if let ConversationItem::User { text } = item {
            let snippet = truncate_chars(text.trim(), SUMMARY_USER_SNIPPET_MAX_CHARS);
            if !snippet.is_empty() {
                snippets.push(format!("- {snippet}"));
            }
        }
    }

    let mut summary = format!(
        "{SUMMARY_PREFIX}\n\
        Earlier turns were compacted because this session reached the \
        Codex-style auto compact limit. Treat this block as background only; \
        the latest user message after this block is authoritative.\n\
        Compacted history items: {dropped_items}."
    );
    if !snippets.is_empty() {
        summary.push_str("\nOld user requests kept as weak hints:\n");
        summary.push_str(&snippets.join("\n"));
    }
    summary
}

/// 返回 provider 请求里实际会携带的历史，避免末尾同文本用户消息重复计算。
fn effective_history<'a>(
    history: &'a [ConversationItem],
    user_input: &str,
) -> &'a [ConversationItem] {
    let skip_last_user = matches!(
        history.last(),
        Some(ConversationItem::User { text }) if text == user_input
    );
    if skip_last_user {
        &history[..history.len().saturating_sub(1)]
    } else {
        history
    }
}

/// 估算单条对话项 token，适用于无 tokenizer 的轻量 daemon。
fn estimate_item_tokens(item: &ConversationItem) -> u64 {
    match item {
        ConversationItem::User { text } | ConversationItem::Assistant { text } => {
            estimate_text_tokens(text)
        }
    }
}

/// 按字符粗估 token，适用于压缩触发而非计费。
fn estimate_text_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    chars.div_ceil(CHARS_PER_TOKEN).saturating_add(4)
}

/// 截断字符串到指定字符数，适用于压缩摘要中的旧请求摘录。
fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

#[cfg(test)]
#[path = "context_window_test.rs"]
mod context_window_test;
