//! 内置 skill 安装。

use crate::error::AppResult;
use crate::home::AppPaths;

const INTERNAL_SKILL_FILES: &[(&str, &[u8])] = &[
    (
        "__skill_add/SKILL.md",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/internal/skills/__skill_add/SKILL.md"
        )),
    ),
    (
        "__mem/SKILL.md",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/internal/skills/__mem/SKILL.md"
        )),
    ),
];

/// 安装内置 skills，适用于 daemon 启动时同步用户目录。
pub async fn install_builtin_skills(paths: &AppPaths) -> AppResult<()> {
    for (relative_path, bytes) in INTERNAL_SKILL_FILES {
        let target_path = paths.skills_dir.join(relative_path);
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(target_path, bytes).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "skills_test.rs"]
mod skills_test;
