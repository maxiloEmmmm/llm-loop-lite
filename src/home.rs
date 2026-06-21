use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// 应用所有用户级文件路径，当前固定在 `~/.llm-loop` 下。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// 用户主目录路径。
    pub home_dir: PathBuf,
    /// daemon 工作目录，默认使用用户主目录。
    pub work_dir: PathBuf,
    /// 应用数据目录路径。
    pub app_dir: PathBuf,
    /// channel 持久化数据目录，不参与附件 TTL 清理。
    pub channel_data_dir: PathBuf,
    /// channel 通用附件存储目录。
    pub channel_store_dir: PathBuf,
    /// llm-loop 用户 skills 目录。
    pub skills_dir: PathBuf,
    /// llm-loop 全局记忆目录。
    pub mems_dir: PathBuf,
    /// session 历史目录。
    pub sessions_dir: PathBuf,
    /// 定时任务目录。
    pub crons_dir: PathBuf,
    /// 计划消息状态目录。
    pub plans_dir: PathBuf,
    /// Codex 用户目录路径。
    pub codex_home: PathBuf,
    /// TOML 配置文件路径。
    pub config_path: PathBuf,
    /// Codex CLI 配置文件路径，用于复用 custom provider 定义。
    pub codex_config_path: PathBuf,
    /// Codex OAuth/custom auth 存储文件路径。
    pub auth_path: PathBuf,
    /// Codex 风格 installation id 存储文件路径。
    pub installation_id_path: PathBuf,
    /// daemon 本地 Unix socket 路径，供 CLI 子命令查询运行态。
    pub daemon_socket_path: PathBuf,
}

impl AppPaths {
    /// 从当前进程环境解析路径，适用于 daemon 正常启动。
    pub fn from_env() -> AppResult<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(AppError::MissingHome)?;
        Ok(Self::from_home(home))
    }

    /// 从指定 home 构造路径，适用于测试和未来多 profile 场景。
    pub fn from_home(home_dir: impl AsRef<Path>) -> Self {
        let home_dir = home_dir.as_ref().to_path_buf();
        let work_dir = home_dir.clone();
        let app_dir = home_dir.join(".llm-loop");
        let codex_home = home_dir.join(".codex");
        let config_path = app_dir.join("config.toml");
        let codex_config_path = codex_home.join("config.toml");
        let auth_path = app_dir.join("auth.json");
        let installation_id_path = app_dir.join("installation_id");
        let daemon_socket_path = app_dir.join("llm-loop.sock");
        let channel_data_dir = app_dir.join("channel");
        let channel_store_dir = channel_data_dir.join("store");
        let skills_dir = app_dir.join("skills");
        let mems_dir = app_dir.join("mems");
        let sessions_dir = app_dir.join("sessions");
        let crons_dir = app_dir.join("crons");
        let plans_dir = app_dir.join("plans");
        Self {
            home_dir,
            work_dir,
            app_dir,
            channel_data_dir,
            channel_store_dir,
            skills_dir,
            mems_dir,
            sessions_dir,
            crons_dir,
            plans_dir,
            codex_home,
            config_path,
            codex_config_path,
            auth_path,
            installation_id_path,
            daemon_socket_path,
        }
    }

    /// 应用配置里的工作目录，适用于 daemon 启动后固定上下文根。
    pub fn with_work_dir(mut self, work_dir: Option<&str>) -> Self {
        if let Some(work_dir) = work_dir.and_then(resolve_configured_work_dir) {
            self.work_dir = work_dir;
        }
        self
    }
}

/// 解析配置中的工作目录，空值表示继续使用默认 home。
fn resolve_configured_work_dir(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    Some(PathBuf::from(trimmed))
}

#[cfg(test)]
#[path = "home_test.rs"]
mod home_test;
