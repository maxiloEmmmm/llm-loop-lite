//! 初始上下文加载，包含 AGENTS.md 与 skills 列表。

mod agents;
mod mems;
mod skills;
mod system;

use crate::error::AppResult;
use crate::home::AppPaths;
use crate::message::MessageSource;

/// 初始上下文片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialContext {
    /// 顶层模型指令，适用于 Codex `instructions` 或 Claude `system`。
    pub instructions: String,
}

/// 加载初始上下文，适用于新 session 的第一轮请求。
pub async fn load_initial_context(
    paths: &AppPaths,
    source: &MessageSource,
) -> AppResult<InitialContext> {
    let cwd = &paths.work_dir;
    let mut blocks = Vec::new();
    if let Some(text) = system::load_system_prompt().await? {
        blocks.push(text);
    }
    if let Some(text) = agents::load_agents_instructions(paths, cwd).await? {
        blocks.push(instruction_block("AGENTS.md Instructions", text));
    }
    if let Some(text) = mems::load_memory_instructions(paths, source).await? {
        blocks.push(instruction_block("Memories", text));
    }
    if let Some(text) = skills::load_skills_instructions(paths, cwd, source).await? {
        blocks.push(instruction_block("Available Skills", text));
    }
    Ok(InitialContext {
        instructions: join_instruction_blocks(blocks),
    })
}

/// 加载 cron 专用上下文，适用于每次调度触发的独立任务。
pub async fn load_cron_context() -> AppResult<InitialContext> {
    Ok(InitialContext {
        instructions: system::load_cron_prompt().await?.unwrap_or_default(),
    })
}

/// 渲染初始上下文日志，适用于排查新 session 首轮提示词。
pub fn render_initial_context_for_log(context: &InitialContext) -> String {
    if context.instructions.trim().is_empty() {
        return "<empty>".to_string();
    }
    format!("----- instructions -----\n{}", context.instructions)
}

/// 添加指令块标题，适用于在顶层 instructions 里保留来源边界。
fn instruction_block(title: &str, text: String) -> String {
    format!("## {title}\n{text}")
}

/// 合并指令块，适用于避免空块污染顶层 instructions。
fn join_instruction_blocks(blocks: Vec<String>) -> String {
    blocks
        .into_iter()
        .map(|block| block.trim().to_string())
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod agents_test;

#[cfg(test)]
mod mems_test;

#[cfg(test)]
mod skills_test;

#[cfg(test)]
mod system_test;
