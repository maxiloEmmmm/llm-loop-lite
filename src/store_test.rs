use std::fs;
use std::time::{Duration, SystemTime};

use super::{cleanup_store_once, remove_session_store, store_hash};

/// store 清理会删除超过 7 天未使用的文件并清理空目录。
#[tokio::test]
async fn cleanup_store_removes_old_files_and_empty_dirs() {
    let root = std::env::temp_dir().join(format!("llm-loop-store-{}", uuid::Uuid::new_v4()));
    let old_dir = root.join("old");
    let fresh_dir = root.join("fresh");
    fs::create_dir_all(&old_dir).expect("应能创建旧目录");
    fs::create_dir_all(&fresh_dir).expect("应能创建新目录");
    let old_file = old_dir.join("a.txt");
    let fresh_file = fresh_dir.join("b.txt");
    fs::write(&old_file, "old").expect("应能写旧文件");
    fs::write(&fresh_file, "fresh").expect("应能写新文件");

    let old_time = filetime::FileTime::from_system_time(
        SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60),
    );
    filetime::set_file_times(&old_file, old_time, old_time).expect("应能设置旧文件时间");

    cleanup_store_once(&root).await.expect("清理不应失败");

    assert!(!old_file.exists());
    assert!(!old_dir.exists());
    assert!(fresh_file.exists());
}

/// reset 时只删除对应 session key 的 store 目录。
#[tokio::test]
async fn remove_session_store_removes_only_matching_key() {
    let root = std::env::temp_dir().join(format!("llm-loop-store-{}", uuid::Uuid::new_v4()));
    let target = root.join(store_hash("key-a"));
    let other = root.join(store_hash("key-b"));
    fs::create_dir_all(&target).expect("应能创建目标目录");
    fs::create_dir_all(&other).expect("应能创建其它目录");
    fs::write(target.join("a.txt"), "a").expect("应能写目标文件");
    fs::write(other.join("b.txt"), "b").expect("应能写其它文件");

    remove_session_store(&root, "key-a")
        .await
        .expect("删除 session store 不应失败");

    assert!(!target.exists());
    assert!(other.exists());
}
