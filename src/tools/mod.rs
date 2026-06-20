//! 独立工具层，负责暴露模型可调用工具并执行本地工具。

pub mod builtins;
pub mod registry;
pub mod spec;

pub use registry::{ToolCall, ToolContext, ToolRegistry, ToolResult};
pub use spec::ToolSpec;

#[cfg(test)]
mod spec_test;
