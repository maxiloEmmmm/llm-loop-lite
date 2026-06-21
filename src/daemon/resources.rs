use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::channel::BuiltinChannelHandle;
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::resource::ResourceUsage;

use super::Daemon;

/// daemon 资源快照，适用于 CLI 通过 Unix socket 查询运行态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    /// 当前 daemon 进程 id。
    pub pid: u32,
    /// RSS 内存字节数，平台不支持时为 None。
    pub rss_bytes: Option<u64>,
    /// 虚拟内存字节数，平台不支持时为 None。
    pub virtual_bytes: Option<u64>,
    /// 当前内存中的 session 数。
    pub session_count: usize,
    /// 当前 session 处理锁数量。
    pub session_lock_count: usize,
    /// 当前活跃 provider 请求数量。
    pub active_turn_count: usize,
    /// cron provider 并发闸门剩余许可数。
    pub cron_available_permits: usize,
    /// 已启动 channel 运行态。
    pub channels: Vec<ChannelResource>,
    /// 当前进程内 llm-loop 自有结构的即时资源估算。
    pub memory: Vec<ResourceUsage>,
    /// 关键本地目录占用。
    pub paths: Vec<PathResource>,
}

/// 单个 channel 的运行态资源信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelResource {
    /// channel 实例名。
    pub name: String,
    /// channel 平台名。
    pub platform: String,
    /// channel 能力快照。
    pub capabilities: crate::channel::ChannelCapabilities,
}

/// 本地目录资源信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathResource {
    /// 目录标签。
    pub name: String,
    /// 目录路径。
    pub path: PathBuf,
    /// 文件数量。
    pub files: u64,
    /// 目录占用字节数。
    pub bytes: u64,
}

/// 启动 daemon Unix socket，适用于本机 CLI 查询运行态资源。
pub fn spawn_resource_socket(daemon: Arc<Daemon>, channels: Arc<Vec<BuiltinChannelHandle>>) {
    let socket_path = daemon.paths.daemon_socket_path.clone();
    tokio::spawn(async move {
        if let Err(err) = run_resource_socket(socket_path, daemon, channels).await {
            crate::log_info!("resource socket stopped: {err}");
        }
    });
}

/// 查询 daemon 资源快照，适用于 `llm-loop resources` 子命令。
pub async fn query_resource_snapshot(paths: &AppPaths) -> AppResult<ResourceSnapshot> {
    let mut stream = UnixStream::connect(&paths.daemon_socket_path)
        .await
        .map_err(|err| {
            AppError::Cli(format!(
                "daemon resource socket unavailable at {}: {}",
                paths.daemon_socket_path.display(),
                err
            ))
        })?;
    stream.write_all(b"resources\n").await?;
    let mut body = Vec::new();
    stream.read_to_end(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

/// 监听 Unix socket 并处理资源查询。
async fn run_resource_socket(
    socket_path: PathBuf,
    daemon: Arc<Daemon>,
    channels: Arc<Vec<BuiltinChannelHandle>>,
) -> AppResult<()> {
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::fs::remove_file(&socket_path).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let listener = UnixListener::bind(&socket_path)?;
    crate::log_info!("resource socket listening path={}", socket_path.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let daemon = Arc::clone(&daemon);
        let channels = Arc::clone(&channels);
        tokio::spawn(async move {
            if let Err(err) = handle_resource_client(stream, daemon, channels).await {
                crate::log_info!("resource socket client failed: {err}");
            }
        });
    }
}

/// 处理单个资源查询客户端。
async fn handle_resource_client(
    mut stream: UnixStream,
    daemon: Arc<Daemon>,
    channels: Arc<Vec<BuiltinChannelHandle>>,
) -> AppResult<()> {
    let mut request = [0_u8; 128];
    let read = stream.read(&mut request).await?;
    let command = std::str::from_utf8(&request[..read]).unwrap_or("").trim();
    if command != "resources" {
        stream
            .write_all(b"{\"error\":\"unknown command\"}\n")
            .await?;
        return Ok(());
    }
    let snapshot = daemon.resource_snapshot(channels.as_ref()).await;
    stream.write_all(&serde_json::to_vec(&snapshot)?).await?;
    stream.write_all(b"\n").await?;
    Ok(())
}

impl Daemon {
    /// 生成资源快照，适用于 Unix socket 查询运行中 daemon。
    pub(crate) async fn resource_snapshot(
        &self,
        channels: &[BuiltinChannelHandle],
    ) -> ResourceSnapshot {
        let (rss_bytes, virtual_bytes) = current_process_memory();
        let sessions = self.sessions.lock().await;
        let session_count = sessions.len();
        let session_usage = sessions.resource_usage();
        drop(sessions);
        let session_locks = self.session_locks.lock().await;
        let session_lock_count = session_locks.len();
        let session_lock_usage = ResourceUsage::new(
            "daemon.session_locks",
            "hashmap",
            session_locks.len(),
            Some(session_locks.capacity()),
            session_locks
                .capacity()
                .saturating_mul(std::mem::size_of::<(String, Arc<tokio::sync::Mutex<()>>)>())
                .saturating_add(session_locks.keys().map(String::capacity).sum::<usize>()),
        );
        drop(session_locks);
        let (active_turn_count, active_turn_usage) = {
            let active_turns = self
                .active_turns
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            (
                active_turns.len(),
                ResourceUsage::new(
                    "daemon.active_turns",
                    "hashmap",
                    active_turns.len(),
                    Some(active_turns.capacity()),
                    active_turns
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, super::ActiveTurn)>())
                        .saturating_add(active_turns.keys().map(String::capacity).sum::<usize>()),
                ),
            )
        };
        let mut memory = vec![session_usage, session_lock_usage, active_turn_usage];
        memory.extend(self.tools.resource_usage().await);
        for channel in channels {
            memory.extend(channel.resource_usage().await);
        }
        ResourceSnapshot {
            pid: std::process::id(),
            rss_bytes,
            virtual_bytes,
            session_count,
            session_lock_count,
            active_turn_count,
            cron_available_permits: self.cron_semaphore.available_permits(),
            channels: channels
                .iter()
                .map(|channel| ChannelResource {
                    name: channel.name().to_string(),
                    platform: channel.platform_name().to_string(),
                    capabilities: channel.capabilities(),
                })
                .collect(),
            memory,
            paths: resource_paths(&self.paths),
        }
    }
}

/// 返回当前进程内存信息，适用于 Linux 部署环境。
fn current_process_memory() -> (Option<u64>, Option<u64>) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let mut rss = None;
    let mut virt = None;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            rss = parse_status_kb(value);
        } else if let Some(value) = line.strip_prefix("VmSize:") {
            virt = parse_status_kb(value);
        }
    }
    (rss, virt)
}

/// 解析 `/proc/self/status` 的 kB 字段。
fn parse_status_kb(value: &str) -> Option<u64> {
    value
        .split_whitespace()
        .next()
        .and_then(|part| part.parse::<u64>().ok())
        .map(|kb| kb.saturating_mul(1024))
}

/// 汇总关键目录占用，适用于发现 session/cache 膨胀。
fn resource_paths(paths: &AppPaths) -> Vec<PathResource> {
    [
        ("sessions", &paths.sessions_dir),
        ("channel_store", &paths.channel_store_dir),
        ("mems", &paths.mems_dir),
        ("skills", &paths.skills_dir),
        ("crons", &paths.crons_dir),
        ("plans", &paths.plans_dir),
    ]
    .into_iter()
    .map(|(name, path)| {
        let (files, bytes) = dir_usage(path);
        PathResource {
            name: name.to_string(),
            path: path.clone(),
            files,
            bytes,
        }
    })
    .collect()
}

/// 递归统计目录文件数量和字节数。
fn dir_usage(path: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            let (child_files, child_bytes) = dir_usage(&entry.path());
            files = files.saturating_add(child_files);
            bytes = bytes.saturating_add(child_bytes);
        } else {
            files = files.saturating_add(1);
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    (files, bytes)
}
