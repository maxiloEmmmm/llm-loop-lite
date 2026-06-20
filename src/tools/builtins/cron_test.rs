use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{CronHandler, CronStore};
use crate::message::MessageSource;
use crate::session::SessionState;
use crate::tools::builtins::PlanStates;
use crate::tools::registry::{
    ToolCall, ToolContext, ToolHandler, ToolInput, ToolOutputKind, ToolSharedState,
};

/// add 后应写入 cron.md 和 task-{key}.md。
#[tokio::test]
async fn cron_add_writes_task_and_definition() {
    let root = temp_dir("cron_add_writes_task_and_definition");
    let context = test_context(root.clone(), group_source());
    let output = execute(
        json!({
            "type": "add",
            "key": "daily",
            "time_step": ["0", "9", "*", "*", "1-5"],
            "prompt": "daily\n生成日报",
        }),
        context,
    )
    .await;

    assert_eq!(output, Value::String(String::new()));
    let dir = root.join("main_oc_group");
    assert_eq!(
        std::fs::read_to_string(dir.join("cron.md")).expect("应能读取 cron.md"),
        "0 9 * * 1-5 task-daily.md\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("task-daily.md")).expect("应能读取任务文件"),
        "daily\n生成日报"
    );
}

/// list 应只返回当前群粒度目录里的任务。
#[tokio::test]
async fn cron_list_reads_current_group_scope() {
    let root = temp_dir("cron_list_reads_current_group_scope");
    let dir = root.join("main_oc_group");
    std::fs::create_dir_all(&dir).expect("应能创建 cron 目录");
    std::fs::write(dir.join("cron.md"), "0 9 * * * task-daily.md\n").expect("应能写 cron.md");
    std::fs::write(dir.join("task-daily.md"), "daily\n生成日报").expect("应能写任务文件");

    let output = execute(
        json!({ "type": "list" }),
        test_context(root, group_source()),
    )
    .await;

    assert_eq!(
        output,
        json!([{
            "key": "daily",
            "time_step": ["0", "9", "*", "*", "*"],
            "title": "daily",
            "task": "生成日报"
        }])
    );
    let rendered = output.to_string();
    assert!(!rendered.contains("task-daily.md"));
    assert!(!rendered.contains("cron.md"));
}

/// cron 工具 spec 应声明辅助文件同级规则，适用于约束模型不要写全局脚本。
#[test]
fn cron_spec_declares_auxiliary_file_rule() {
    let spec = serde_json::to_string(&CronHandler.spec()).expect("spec 应能序列化");

    assert!(spec.contains("same directory"));
    assert!(spec.contains("task-<key>"));
    assert!(spec.contains("~/.llm-loop/cron"));
    assert!(spec.contains("Remove deletes"));
}

/// add 应拒绝旧单数目录，适用于阻止模型把辅助脚本写到全局 cron 下。
#[tokio::test]
async fn cron_add_rejects_global_auxiliary_path() {
    let root = temp_dir("cron_add_rejects_global_auxiliary_path");
    let context = test_context(root, group_source());
    let call = ToolCall {
        call_id: "call-1".to_string(),
        name: "__cron".to_string(),
        input: ToolInput::Function {
            arguments: json!({
                "type": "add",
                "key": "daily",
                "time_step": ["0", "9", "*", "*", "*"],
                "prompt": "daily\n请执行 ~/.llm-loop/cron/task-daily.sh",
            })
            .to_string(),
        },
    };

    let err = CronHandler
        .execute(call, context)
        .await
        .expect_err("全局旧 cron 目录应拒绝");

    assert!(err.to_string().contains("forbidden path"));
}

/// add 不应误伤 crons 目录名，适用于区分旧单数目录和当前目录结构。
#[tokio::test]
async fn cron_add_allows_crons_directory_word_boundary() {
    let root = temp_dir("cron_add_allows_crons_directory_word_boundary");
    let context = test_context(root, group_source());
    let output = execute(
        json!({
            "type": "add",
            "key": "daily",
            "time_step": ["0", "9", "*", "*", "*"],
            "prompt": "daily\n请执行 ~/.llm-loop/crons/main_oc_group/task-daily.sh",
        }),
        context,
    )
    .await;

    assert_eq!(output, Value::String(String::new()));
}

/// edit 应更新已有 cron 行和任务文件。
#[tokio::test]
async fn cron_edit_updates_existing_task() {
    let root = temp_dir("cron_edit_updates_existing_task");
    let dir = root.join("main_ou_user");
    std::fs::create_dir_all(&dir).expect("应能创建 cron 目录");
    std::fs::write(dir.join("cron.md"), "0 9 * * * task-daily.md\n").expect("应能写 cron.md");
    std::fs::write(dir.join("task-daily.md"), "daily\n旧任务").expect("应能写任务文件");

    let output = execute(
        json!({
            "type": "edit",
            "key": "daily",
            "time_step": ["30", "18", "*", "*", "mon-fri"],
            "prompt": "daily\n新任务",
        }),
        test_context(root.clone(), user_source()),
    )
    .await;

    assert_eq!(output, Value::String(String::new()));
    assert_eq!(
        std::fs::read_to_string(dir.join("cron.md")).expect("应能读取 cron.md"),
        "30 18 * * mon-fri task-daily.md\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("task-daily.md")).expect("应能读取任务文件"),
        "daily\n新任务"
    );
}

/// remove 应同时删除 cron 定义和任务文件。
#[tokio::test]
async fn cron_remove_deletes_definition_and_task_file() {
    let root = temp_dir("cron_remove_deletes_definition_and_task_file");
    let dir = root.join("main_oc_group");
    std::fs::create_dir_all(&dir).expect("应能创建 cron 目录");
    std::fs::write(
        dir.join("cron.md"),
        "0 9 * * * task-daily.md\n0 10 * * * task-weekly.md\n",
    )
    .expect("应能写 cron.md");
    std::fs::write(dir.join("task-daily.md"), "daily\n生成日报").expect("应能写日报任务");
    std::fs::write(dir.join("task-weekly.md"), "weekly\n生成周报").expect("应能写周报任务");

    let output = execute(
        json!({
            "type": "remove",
            "key": "daily",
        }),
        test_context(root.clone(), group_source()),
    )
    .await;

    assert_eq!(output, Value::String(String::new()));
    assert_eq!(
        std::fs::read_to_string(dir.join("cron.md")).expect("应能读取 cron.md"),
        "0 10 * * * task-weekly.md\n"
    );
    assert!(!dir.join("task-daily.md").exists());
    assert!(dir.join("task-weekly.md").exists());
}

/// remove 应同时删除同级辅助文件，适用于清理定时任务脚本资源。
#[tokio::test]
async fn cron_remove_deletes_same_key_auxiliary_files() {
    let root = temp_dir("cron_remove_deletes_same_key_auxiliary_files");
    let dir = root.join("main_oc_group");
    std::fs::create_dir_all(&dir).expect("应能创建 cron 目录");
    std::fs::write(
        dir.join("cron.md"),
        "0 9 * * * task-daily.md\n0 10 * * * task-daily_weather.md\n",
    )
    .expect("应能写 cron.md");
    std::fs::write(dir.join("task-daily.md"), "daily\n生成日报").expect("应能写任务");
    std::fs::write(dir.join("task-daily.py"), "print('daily')").expect("应能写辅助脚本");
    std::fs::write(dir.join("task-daily-fetch.py"), "print('fetch')").expect("应能写辅助脚本");
    std::fs::write(dir.join("task-daily_data.json"), "{}").expect("应能写辅助数据");
    std::fs::write(dir.join("task-daily_weather.md"), "daily_weather\n天气")
        .expect("应能写其他任务");

    let output = execute(
        json!({
            "type": "remove",
            "key": "daily",
        }),
        test_context(root.clone(), group_source()),
    )
    .await;

    assert_eq!(output, Value::String(String::new()));
    assert!(!dir.join("task-daily.md").exists());
    assert!(!dir.join("task-daily.py").exists());
    assert!(!dir.join("task-daily-fetch.py").exists());
    assert!(!dir.join("task-daily_data.json").exists());
    assert!(dir.join("task-daily_weather.md").exists());
}

/// add 遇到同 key 任务应拒绝，提示调用方改名。
#[tokio::test]
async fn cron_add_rejects_existing_key() {
    let root = temp_dir("cron_add_rejects_existing_key");
    let dir = root.join("main_oc_group");
    std::fs::create_dir_all(&dir).expect("应能创建 cron 目录");
    std::fs::write(dir.join("task-daily.md"), "daily\n生成日报").expect("应能写任务文件");
    let context = test_context(root, group_source());
    let call = ToolCall {
        call_id: "call-1".to_string(),
        name: "__cron".to_string(),
        input: ToolInput::Function {
            arguments: json!({
                "type": "add",
                "key": "daily",
                "time_step": ["0", "9", "*", "*", "*"],
                "prompt": "daily\n生成日报",
            })
            .to_string(),
        },
    };

    let err = CronHandler
        .execute(call, context)
        .await
        .expect_err("重复 key 应拒绝");

    assert!(err.to_string().contains("already exists"));
}

/// 执行 cron 工具并返回输出。
async fn execute(args: Value, context: ToolContext) -> Value {
    let result = CronHandler
        .execute(
            ToolCall {
                call_id: "call-1".to_string(),
                name: "__cron".to_string(),
                input: ToolInput::Function {
                    arguments: args.to_string(),
                },
            },
            context,
        )
        .await
        .expect("cron 工具执行应成功");
    assert_eq!(result.output_kind, ToolOutputKind::Function);
    result.output
}

/// 构造测试工具上下文。
fn test_context(root: std::path::PathBuf, source: MessageSource) -> ToolContext {
    ToolContext {
        session: SessionState::new("test-session".to_string()),
        source,
        cwd: root.clone(),
        shared: Arc::new(ToolSharedState {
            exec_sessions: Mutex::default(),
            plans: Mutex::new(PlanStates::default()),
            crons: CronStore::new(root),
        }),
        user_input: None,
        channel: None,
    }
}

/// 构造群来源。
fn group_source() -> MessageSource {
    MessageSource {
        channel_name: "main".to_string(),
        platform: "feishu".to_string(),
        chat_id: "oc_group".to_string(),
        chat_type: "group".to_string(),
        user_id: Some("ou_user".to_string()),
        thread_id: None,
    }
}

/// 构造用户来源。
fn user_source() -> MessageSource {
    MessageSource {
        channel_name: "main".to_string(),
        platform: "feishu".to_string(),
        chat_id: String::new(),
        chat_type: "dm".to_string(),
        user_id: Some("ou_user".to_string()),
        thread_id: None,
    }
}

/// 创建唯一临时目录，适用于 cron 工具文件测试。
fn temp_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("llm-loop-{name}-{nanos}"))
}
