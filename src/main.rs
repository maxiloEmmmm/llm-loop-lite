use std::sync::Arc;

use llm_loop::cli::{CliCommand, run_cli_command};
use llm_loop::config::load_merged_config;
use llm_loop::error::AppResult;
use llm_loop::home::AppPaths;
use llm_loop::logger;
use tokio::runtime::Builder;

/// 进程入口，手动初始化 Tokio runtime 并分发 daemon。
fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

/// 同步启动层，负责加载配置并创建 runtime。
fn run() -> AppResult<()> {
    let command = CliCommand::parse(std::env::args().skip(1))?;
    let paths = AppPaths::from_env()?;
    let config = load_merged_config(&paths)?;
    logger::init(&config.log)?;
    let paths = paths.with_work_dir(config.work_dir.as_deref());

    let runtime = Builder::new_multi_thread().enable_all().build()?;

    let context = Arc::new((config, paths));
    runtime.block_on(async move { run_cli_command(command, context).await })
}
