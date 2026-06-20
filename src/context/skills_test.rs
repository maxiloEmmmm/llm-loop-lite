use std::fs;
use std::path::{Path, PathBuf};

use crate::home::AppPaths;
use crate::message::MessageSource;

use super::skills::{load_skills_instructions, parse_frontmatter, skill_roots};

/// 构造测试来源，适用于需要 user scope 的 skills 测试。
fn test_source() -> MessageSource {
    MessageSource {
        channel_name: "main".to_string(),
        platform: "feishu".to_string(),
        chat_id: "oc_test".to_string(),
        chat_type: "group".to_string(),
        user_id: Some("ou_test".to_string()),
        thread_id: None,
    }
}

/// 创建唯一临时目录，适用于 skills 加载测试隔离。
fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("llm-loop-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).expect("测试临时目录应能创建");
    root
}

/// 写入测试文件，适用于构造 SKILL.md。
fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("测试父目录应能创建");
    }
    fs::write(path, text).expect("测试文件应能写入");
}

/// skill roots 包含 llm-loop、codex home 和 cwd 每级 .agents/skills。
#[test]
fn skill_roots_include_codex_and_parent_chain() {
    let root = temp_root("skill-roots");
    let home = root.join("home");
    let cwd = root.join("work").join("pkg");
    let paths = AppPaths::from_home(&home);

    let source = test_source();
    let roots = skill_roots(&paths, &cwd, &source);

    assert!(roots.contains(&paths.skills_dir));
    assert!(roots.contains(&paths.codex_home.join("skills")));
    assert!(roots.contains(&root.join("work").join(".agents").join("skills")));
    assert!(roots.contains(&cwd.join(".agents").join("skills")));
}

/// frontmatter 会解析 name 与 description。
#[test]
fn skill_frontmatter_parses_name_and_description() {
    let raw = "---\nname: demo\ndescription: \"做一件事\"\n---\n正文";

    assert_eq!(
        parse_frontmatter(raw),
        Some(("demo".to_string(), "做一件事".to_string()))
    );
}

/// skills 初始上下文只渲染元数据和读取规则。
#[tokio::test]
async fn skills_instructions_render_available_skill_metadata() {
    let root = temp_root("skill-render");
    let home = root.join("home");
    let cwd = root.join("work").join("pkg");
    let paths = AppPaths::from_home(&home);
    write_file(
        &paths
            .codex_home
            .join("skills")
            .join("global")
            .join("SKILL.md"),
        "---\nname: global_skill\ndescription: 全局技能\n---\n正文",
    );
    write_file(
        &cwd.join(".agents")
            .join("skills")
            .join("local")
            .join("SKILL.md"),
        "---\nname: local_skill\ndescription: 本地技能\n---\n正文",
    );

    let source = test_source();
    let text = load_skills_instructions(&paths, &cwd, &source)
        .await
        .expect("skills 加载不应失败")
        .expect("应生成 skills 指令");

    assert!(text.contains("## Skills"));
    assert!(text.contains("global_skill: 全局技能"));
    assert!(text.contains("local_skill: 本地技能"));
    assert!(text.contains("Skill bodies live on disk at the listed paths"));
    assert!(!text.contains("~/.llm-loop/skills/<skill>/SKILL.md"));
    assert!(text.contains("open and read its `SKILL.md` completely"));
}
