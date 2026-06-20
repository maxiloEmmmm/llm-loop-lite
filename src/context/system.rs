use crate::error::AppResult;

const SYSTEM_PROMPT_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/internal/prompt/system.md"
));
const CRON_PROMPT_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/internal/prompt/cron.md"
));

/// 加载项目内 system prompt，适用于新 session 首轮最前置提示词。
pub async fn load_system_prompt() -> AppResult<Option<String>> {
    let text = String::from_utf8_lossy(SYSTEM_PROMPT_BYTES).to_string();
    Ok((!text.trim().is_empty()).then_some(text))
}

/// 加载 cron prompt，适用于调度器触发的一次性任务。
pub async fn load_cron_prompt() -> AppResult<Option<String>> {
    let text = String::from_utf8_lossy(CRON_PROMPT_BYTES).to_string();
    Ok((!text.trim().is_empty()).then_some(text))
}
