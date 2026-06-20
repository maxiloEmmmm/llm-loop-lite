use super::AppPaths;

/// 未配置 work-dir 时保留 home 作为工作目录。
#[test]
fn work_dir_defaults_to_home() {
    let home = std::path::PathBuf::from("/tmp/llm-loop-home-default");
    let paths = AppPaths::from_home(&home);

    assert_eq!(paths.work_dir, home);
    assert_eq!(
        paths.channel_data_dir,
        std::path::PathBuf::from("/tmp/llm-loop-home-default/.llm-loop/channel")
    );
    assert_eq!(
        paths.channel_store_dir,
        std::path::PathBuf::from("/tmp/llm-loop-home-default/.llm-loop/channel/store")
    );
    assert_eq!(
        paths.skills_dir,
        std::path::PathBuf::from("/tmp/llm-loop-home-default/.llm-loop/skills")
    );
    assert_eq!(
        paths.mems_dir,
        std::path::PathBuf::from("/tmp/llm-loop-home-default/.llm-loop/mems")
    );
    assert_eq!(
        paths.crons_dir,
        std::path::PathBuf::from("/tmp/llm-loop-home-default/.llm-loop/crons")
    );
}

/// 配置 work-dir 时替换 daemon 工作目录。
#[test]
fn work_dir_can_be_configured() {
    let home = std::path::PathBuf::from("/tmp/llm-loop-home-configured");
    let paths = AppPaths::from_home(&home).with_work_dir(Some("/tmp/llm-loop-work"));

    assert_eq!(
        paths.work_dir,
        std::path::PathBuf::from("/tmp/llm-loop-work")
    );
}
