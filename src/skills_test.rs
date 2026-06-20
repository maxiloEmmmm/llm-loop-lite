use crate::home::AppPaths;

use super::install_builtin_skills;

/// 内置 skills 会安装到 llm-loop 用户 skills 目录。
#[tokio::test]
async fn builtin_skills_are_installed_to_user_dir() {
    let home = std::env::temp_dir().join(format!("llm-loop-skills-{}", uuid::Uuid::new_v4()));
    let paths = AppPaths::from_home(&home);

    install_builtin_skills(&paths)
        .await
        .expect("内置 skills 安装不应失败");

    assert!(
        paths
            .skills_dir
            .join("__skill_add")
            .join("SKILL.md")
            .is_file()
    );
    assert!(paths.skills_dir.join("__mem").join("SKILL.md").is_file());
}
