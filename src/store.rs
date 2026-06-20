//! 通用附件存储清理。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::AppResult;

const STORE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const CLEAN_HOUR_LOCAL: u64 = 4;

/// 生成 store 短 hash，适用于 session 目录和文件名前缀。
pub fn store_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", hash as u32)
}

/// 删除指定 session key 的附件目录。
pub async fn remove_session_store(root: &Path, session_key: &str) -> AppResult<()> {
    let dir = root.join(store_hash(session_key));
    match tokio::fs::remove_dir_all(&dir).await {
        Ok(()) => {
            crate::log_info!("store session removed dir={}", dir.display());
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// 启动通用 store 清理任务，适用于 daemon 常驻运行。
pub fn spawn_store_cleaner(root: PathBuf) {
    tokio::spawn(async move {
        if let Err(err) = cleanup_store_once(&root).await {
            crate::log_info!("store cleanup failed: {err}");
        }
        loop {
            tokio::time::sleep(duration_until_next_clean()).await;
            if let Err(err) = cleanup_store_once(&root).await {
                crate::log_info!("store cleanup failed: {err}");
            }
        }
    });
}

/// 清理一次超过 7 天未使用的附件文件。
pub async fn cleanup_store_once(root: &Path) -> AppResult<()> {
    let cutoff = SystemTime::now()
        .checked_sub(STORE_TTL)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    cleanup_tree(root, cutoff).await?;
    Ok(())
}

/// 迭代清理目录树。
async fn cleanup_tree(root: &Path, cutoff: SystemTime) -> AppResult<()> {
    let mut stack = vec![root.to_path_buf()];
    let mut dirs = Vec::new();
    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        dirs.push(dir);
        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();
            let metadata = entry.metadata().await?;
            if metadata.is_dir() {
                stack.push(entry_path);
            } else if file_is_expired(&metadata, cutoff) {
                tokio::fs::remove_file(&entry_path).await?;
            }
        }
    }
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in dirs {
        if dir != root {
            let _ = tokio::fs::remove_dir(&dir).await;
        }
    }
    Ok(())
}

/// 判断文件是否超过保留时间，优先用访问时间，缺失时用修改时间。
fn file_is_expired(metadata: &std::fs::Metadata, cutoff: SystemTime) -> bool {
    metadata
        .accessed()
        .or_else(|_| metadata.modified())
        .is_ok_and(|time| time < cutoff)
}

/// 计算距离下一个本地凌晨 4 点的时长。
fn duration_until_next_clean() -> Duration {
    let now = chrono::Local::now();
    let today = now.date_naive();
    let target_today = today
        .and_hms_opt(CLEAN_HOUR_LOCAL as u32, 0, 0)
        .expect("固定清理时间必须有效");
    let target = if now.naive_local() < target_today {
        target_today
    } else {
        (today + chrono::Duration::days(1))
            .and_hms_opt(CLEAN_HOUR_LOCAL as u32, 0, 0)
            .expect("固定清理时间必须有效")
    };
    let seconds = (target - now.naive_local()).num_seconds().max(1) as u64;
    Duration::from_secs(seconds)
}

#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
