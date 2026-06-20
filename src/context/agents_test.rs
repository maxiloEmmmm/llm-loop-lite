use std::fs;
use std::path::{Path, PathBuf};

use crate::home::AppPaths;

use super::agents::{load_agents_instructions, project_agents_paths};

/// 创建唯一临时目录，适用于上下文加载测试隔离。
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("llm-loop-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).expect("测试临时目录应能创建");
    root
}

/// 写入测试文件，适用于构造 AGENTS.md 层级。
fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("测试父目录应能创建");
    }
    fs::write(path, text).expect("测试文件应能写入");
}

/// AGENTS.md 会按从外层到 cwd 的顺序发现。
#[test]
fn agents_paths_are_ordered_from_root_to_cwd() {
    let root = temp_root("agents-order");
    let project = root.join("repo");
    let child = project.join("a").join("b");
    write_file(&project.join("AGENTS.md"), "project");
    write_file(&child.join("AGENTS.md"), "child");

    let paths = project_agents_paths(&child);
    let project_index = paths
        .iter()
        .position(|path| path == &project.join("AGENTS.md"))
        .expect("应发现项目 AGENTS.md");
    let child_index = paths
        .iter()
        .position(|path| path == &child.join("AGENTS.md"))
        .expect("应发现 cwd AGENTS.md");

    assert!(project_index < child_index);
}

/// 全局和工作目录 AGENTS.md 会被包装成 developer 指令。
#[tokio::test]
async fn agents_instructions_include_global_and_workdir_docs() {
    let root = temp_root("agents-render");
    let home = root.join("home");
    let cwd = root.join("work").join("pkg");
    let paths = AppPaths::from_home(&home);
    write_file(&paths.codex_home.join("AGENTS.md"), "global doc");
    write_file(&cwd.join("AGENTS.md"), "cwd doc");

    let text = load_agents_instructions(&paths, &cwd)
        .await
        .expect("AGENTS 加载不应失败")
        .expect("应生成 AGENTS 指令");

    assert!(text.contains("# AGENTS.md instructions for"));
    assert!(text.contains("<INSTRUCTIONS>"));
    assert!(text.contains("global doc"));
    assert!(text.contains("cwd doc"));
    assert!(text.find("global doc") < text.find("cwd doc"));
}
