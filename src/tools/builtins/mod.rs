//! 内置工具实现。

mod context;
mod cron;
mod image;
mod patch;
mod plan;
mod plan_list;
mod shell;
mod sleep;
mod user_input;
mod web;

use std::sync::Arc;

pub use cron::CronStore;
pub use plan_list::{PlanState, PlanStates};
pub use shell::ExecSessions;

use crate::tools::registry::ToolHandler;

/// 返回所有默认启用的本地工具处理器。
pub fn handlers() -> Vec<Arc<dyn ToolHandler>> {
    vec![
        Arc::new(shell::ExecCommandHandler),
        Arc::new(shell::WriteStdinHandler),
        Arc::new(shell::ShellCommandHandler),
        Arc::new(patch::ApplyPatchHandler),
        Arc::new(image::ViewImageHandler),
        Arc::new(plan::UpdatePlanHandler),
        Arc::new(plan_list::PlanListHandler),
        Arc::new(plan_list::PlanListUpdateHandler),
        Arc::new(plan_list::PlanListEditHandler),
        Arc::new(plan_list::PlanListDoneHandler),
        Arc::new(user_input::RequestUserInputHandler),
        Arc::new(cron::CronHandler),
        Arc::new(context::NewContextHandler),
        Arc::new(context::GetContextRemainingHandler),
        Arc::new(sleep::SleepHandler),
        Arc::new(web::WebSearchHandler),
        Arc::new(web::WebFetchHandler),
    ]
}
