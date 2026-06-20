use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;

use super::{CronMoment, CronSchedule, SchedulerChannel, due_messages};

/// 步进、范围和星期名称应按标准 cron 字段匹配。
#[test]
fn cron_schedule_matches_step_range_and_weekday_names() {
    let schedule =
        CronSchedule::parse(&["*/15", "9-17", "*", "*", "mon-fri"]).expect("cron 表达式应能解析");

    assert!(schedule.matches(&CronMoment {
        minute: 30,
        hour: 9,
        day_of_month: 8,
        month: 6,
        day_of_week: 1,
    }));
    assert!(!schedule.matches(&CronMoment {
        minute: 31,
        hour: 9,
        day_of_month: 8,
        month: 6,
        day_of_week: 1,
    }));
    assert!(!schedule.matches(&CronMoment {
        minute: 30,
        hour: 9,
        day_of_month: 8,
        month: 6,
        day_of_week: 0,
    }));
}

/// 日期和星期同时受限时应采用 crontab 的 OR 语义。
#[test]
fn cron_schedule_uses_or_for_day_of_month_and_weekday() {
    let schedule = CronSchedule::parse(&["0", "0", "1", "*", "sun"]).expect("cron 表达式应能解析");

    assert!(schedule.matches(&CronMoment {
        minute: 0,
        hour: 0,
        day_of_month: 1,
        month: 6,
        day_of_week: 1,
    }));
    assert!(schedule.matches(&CronMoment {
        minute: 0,
        hour: 0,
        day_of_month: 2,
        month: 6,
        day_of_week: 0,
    }));
    assert!(!schedule.matches(&CronMoment {
        minute: 0,
        hour: 0,
        day_of_month: 2,
        month: 6,
        day_of_week: 1,
    }));
}

/// 群目录应生成群粒度入站消息。
#[tokio::test]
async fn due_messages_reads_group_target_dir() {
    let root = temp_dir("due_messages_reads_group_target_dir");
    let dir = root.join("main_oc_group");
    std::fs::create_dir_all(&dir).expect("应能创建 cron 目录");
    std::fs::write(dir.join("cron.md"), "* * * * * task-a.md\n").expect("应能写 cron.md");
    std::fs::write(dir.join("task-a.md"), "群任务").expect("应能写任务文件");
    let channels = vec![SchedulerChannel {
        name: "main".to_string(),
        platform: "feishu".to_string(),
    }];

    let messages = due_messages(&root, &channels, Local::now())
        .await
        .expect("应能扫描 cron 消息");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "群任务");
    assert_eq!(messages[0].source.channel_name, "main");
    assert_eq!(messages[0].source.chat_type, "group");
    assert_eq!(messages[0].source.chat_id, "oc_group");
    assert_eq!(messages[0].source.user_id, None);
    assert!(messages[0].is_cron_task());
    assert_eq!(messages[0].message_id, None);
}

/// 非群前缀 key 应生成用户粒度入站消息。
#[tokio::test]
async fn due_messages_reads_user_target_dir() {
    let root = temp_dir("due_messages_reads_user_target_dir");
    let dir = root.join("main_ou_user");
    std::fs::create_dir_all(&dir).expect("应能创建 cron 目录");
    std::fs::write(dir.join("cron.md"), "* * * * * task-a.md\n").expect("应能写 cron.md");
    std::fs::write(dir.join("task-a.md"), "用户任务").expect("应能写任务文件");
    let channels = vec![SchedulerChannel {
        name: "main".to_string(),
        platform: "feishu".to_string(),
    }];

    let messages = due_messages(&root, &channels, Local::now())
        .await
        .expect("应能扫描 cron 消息");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "用户任务");
    assert_eq!(messages[0].source.chat_type, "dm");
    assert_eq!(messages[0].source.chat_id, "");
    assert_eq!(messages[0].source.user_id.as_deref(), Some("ou_user"));
    assert!(messages[0].is_cron_task());
    assert_eq!(messages[0].message_id, None);
}

/// 创建唯一临时目录，适用于 scheduler 文件扫描测试。
fn temp_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!("llm-loop-{name}-{nanos}"))
}
