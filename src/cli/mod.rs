use std::sync::Arc;

use crate::config::AppConfig;
use crate::daemon::Daemon;
use crate::error::{AppError, AppResult};
use crate::home::AppPaths;
use crate::provider::codex::run_oauth_login;

mod doctor;
mod resources;

/// CLI 子命令，当前区分 daemon 默认启动和 OAuth 登录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliCommand {
    /// 不带子命令时启动 daemon。
    Daemon,
    /// 执行 Codex OAuth device code 登录。
    Login,
    /// 执行本地配置体检。
    Doctor,
    /// 查询运行中 daemon 的资源快照。
    Resources,
}

impl CliCommand {
    /// 从进程参数解析子命令，适用于 main 的同步启动层。
    pub fn parse(args: impl IntoIterator<Item = String>) -> AppResult<Self> {
        let args = args.into_iter().collect::<Vec<_>>();
        match args.as_slice() {
            [] => Ok(Self::Daemon),
            [command] if command == "login" => Ok(Self::Login),
            [command] if command == "doctor" => Ok(Self::Doctor),
            [command] if command == "resources" => Ok(Self::Resources),
            [command] if command == "help" || command == "--help" || command == "-h" => {
                Err(AppError::Cli(usage()))
            }
            [command] => Err(AppError::Cli(format!(
                "unknown command `{command}`\n{}",
                usage()
            ))),
            [command, ..] => Err(AppError::Cli(format!(
                "command `{command}` does not accept extra args\n{}",
                usage()
            ))),
        }
    }
}

/// 执行已解析的 CLI 命令，适用于 Tokio runtime 内部分发。
pub async fn run_cli_command(
    command: CliCommand,
    context: Arc<(AppConfig, AppPaths)>,
) -> AppResult<()> {
    match command {
        CliCommand::Daemon => {
            let (config, paths) = &*context;
            Arc::new(Daemon::new(config.clone(), paths.clone())?)
                .run()
                .await
        }
        CliCommand::Login => {
            let (_, paths) = &*context;
            run_oauth_login(paths).await
        }
        CliCommand::Doctor => {
            let (config, paths) = &*context;
            doctor::run_doctor(config, paths).await
        }
        CliCommand::Resources => {
            let (_, paths) = &*context;
            resources::run_resources(paths).await
        }
    }
}

/// 返回 CLI 用法文本，适用于参数错误输出。
fn usage() -> String {
    "usage: llm-loop [login|doctor|resources]".to_string()
}

#[cfg(test)]
mod cli_test;
