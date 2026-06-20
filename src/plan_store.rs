use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use crate::error::AppResult;
use crate::store::store_hash;
use crate::tools::builtins::PlanState;

/// 加载计划状态，适用于 daemon 重启后继续更新原飞书卡片。
pub async fn load_plan(root: &Path, session_key: &str) -> AppResult<Option<PlanState>> {
    let path = plan_path(root, session_key);
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let state = serde_json::from_str(&content)?;
    Ok(Some(state))
}

/// 保存计划状态，适用于创建、更新、编辑计划后的轻量持久化。
pub async fn save_plan(root: &Path, session_key: &str, state: &PlanState) -> AppResult<()> {
    let path = plan_path(root, session_key);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp_path = path.with_extension("json.tmp");
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp_path)
        .await?;
    let mut content = serde_json::to_vec(state)?;
    content.push(b'\n');
    file.write_all(&content).await?;
    file.flush().await?;
    drop(file);
    tokio::fs::rename(tmp_path, path).await?;
    Ok(())
}

/// 删除计划状态，适用于 `/reset` 后释放旧卡片引用。
pub async fn remove_plan(root: &Path, session_key: &str) -> AppResult<()> {
    match tokio::fs::remove_file(plan_path(root, session_key)).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// 计算计划状态文件路径。
fn plan_path(root: &Path, session_key: &str) -> PathBuf {
    root.join(format!("{}.json", store_hash(session_key)))
}
