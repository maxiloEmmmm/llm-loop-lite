use std::path::{Path, PathBuf};

use crate::error::AppResult;
use crate::home::AppPaths;

const MAX_AGENTS_BYTES: usize = 32 * 1024;
const AGENTS_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";

/// 加载 AGENTS.md 指令并渲染为模型上下文。
pub async fn load_agents_instructions(paths: &AppPaths, cwd: &Path) -> AppResult<Option<String>> {
    let mut docs = Vec::new();
    let global = paths.codex_home.join("AGENTS.md");
    if let Some(text) = read_limited(&global, MAX_AGENTS_BYTES).await? {
        docs.push((global, text));
    }
    for path in project_agents_paths(cwd) {
        if let Some(text) = read_limited(&path, MAX_AGENTS_BYTES).await? {
            docs.push((path, text));
        }
    }
    if docs.is_empty() {
        return Ok(None);
    }
    let label = cwd.display();
    let body = docs
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join(AGENTS_SEPARATOR);
    Ok(Some(format!(
        "# AGENTS.md instructions for {label}\n\n<INSTRUCTIONS>\n{body}\n</INSTRUCTIONS>"
    )))
}

/// 从文件系统根到 cwd 查找 AGENTS.md。
pub(super) fn project_agents_paths(cwd: &Path) -> Vec<PathBuf> {
    cwd.ancestors()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|dir| dir.join("AGENTS.md"))
        .filter(|path| path.is_file())
        .collect()
}

/// 读取限定大小的 UTF-8 文本。
async fn read_limited(path: &Path, limit: usize) -> AppResult<Option<String>> {
    match tokio::fs::read(path).await {
        Ok(mut bytes) => {
            if bytes.len() > limit {
                bytes.truncate(limit);
            }
            let text = String::from_utf8_lossy(&bytes).to_string();
            Ok((!text.trim().is_empty()).then_some(text))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}
