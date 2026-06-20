use std::fs;
use std::path::Path;

use crate::home::AppPaths;
use crate::message::MessageSource;

use super::mems::{
    load_memory_instructions_for_test, memory_key_from_path_for_test, memory_user_key_for_test,
    strip_frontmatter_for_test,
};

/// 构造测试消息来源，适用于用户级记忆加载。
fn test_source() -> MessageSource {
    MessageSource {
        channel_name: "main".to_string(),
        platform: "telegram".to_string(),
        chat_id: "chat1".to_string(),
        chat_type: "group".to_string(),
        user_id: Some("ou_test.1".to_string()),
        thread_id: None,
    }
}

/// 创建唯一临时 home，适用于记忆加载测试隔离。
fn temp_home(name: &str) -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("llm-loop-mems-{name}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("测试 home 应能创建");
    home
}

/// 写入测试文件，适用于构造记忆目录。
fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("测试父目录应能创建");
    }
    fs::write(path, text).expect("测试文件应能写入");
}

/// 记忆 key 只允许 ASCII 字母和数字，适用于过滤非法文件名。
#[test]
fn memory_key_allows_only_ascii_alphanumeric_md_files() {
    assert_eq!(
        memory_key_from_path_for_test(Path::new("Alpha123.md")),
        Some("Alpha123".to_string())
    );
    assert_eq!(
        memory_key_from_path_for_test(Path::new("alpha_beta.md")),
        None
    );
    assert_eq!(
        memory_key_from_path_for_test(Path::new("alpha-beta.md")),
        None
    );
    assert_eq!(memory_key_from_path_for_test(Path::new("alpha.txt")), None);
}

/// 用户记忆目录 key 会规整发送者 id，适用于避免路径穿越。
#[test]
fn memory_user_key_normalizes_sender_id() {
    assert_eq!(
        memory_user_key_for_test(&test_source()),
        Some("ou_test_1".to_string())
    );
}

/// 注入记忆前会剥离 YAML frontmatter。
#[test]
fn strip_frontmatter_removes_metadata_block() {
    let raw = "---\nuser: u1\nupdated_at: 2026-06-20T20:00:00+08:00\n---\n\nbody";

    assert_eq!(strip_frontmatter_for_test(raw).trim(), "body");
}

/// 初始记忆上下文只包含合法记忆正文，不包含 frontmatter。
#[tokio::test]
async fn load_memory_instructions_reads_valid_memory_bodies() {
    let paths = AppPaths::from_home(temp_home("load"));
    let source = test_source();
    let user_key = memory_user_key_for_test(&source).expect("测试来源应有用户 key");
    write_file(
        &paths.mems_dir.join("ProjectA.md"),
        "---\nuser: user1\nupdated_at: now\n---\n\nremember this",
    );
    write_file(
        &paths
            .mems_dir
            .join("__user")
            .join(&user_key)
            .join("PersonalA.md"),
        "---\nuser: ou_test.1\nupdated_at: now\n---\n\npersonal memory",
    );
    write_file(
        &paths
            .mems_dir
            .join("__user")
            .join("other")
            .join("PersonalB.md"),
        "other user memory",
    );
    write_file(&paths.mems_dir.join("bad-key.md"), "should not load");

    let text = load_memory_instructions_for_test(&paths, &source)
        .await
        .expect("记忆加载不应失败")
        .expect("应生成记忆上下文");

    assert!(text.contains("- default_scope: user"));
    assert!(text.contains("- user_id: ou_test.1"));
    assert!(text.contains("### Global:ProjectA"));
    assert!(text.contains("### User:PersonalA"));
    assert!(text.contains("remember this"));
    assert!(text.contains("personal memory"));
    assert!(!text.contains("user: user1"));
    assert!(!text.contains("should not load"));
    assert!(!text.contains("other user memory"));
}
