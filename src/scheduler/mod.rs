use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, Timelike};
use tokio::sync::mpsc;

use crate::error::{AppError, AppResult};
use crate::message::{InboundMessage, MessageSource};

const CRON_FILE_NAME: &str = "cron.md";

/// scheduler 可投递的 channel 元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerChannel {
    /// channel 实例名，用于匹配 cron 目录前缀和回包路由。
    pub name: String,
    /// 平台名，用于构造 session key。
    pub platform: String,
}

/// 启动分钟级 cron 调度器，适用于 daemon 常驻时注入计划任务。
pub async fn run_cron_scheduler(
    crons_dir: PathBuf,
    channels: Vec<SchedulerChannel>,
    tx: mpsc::Sender<InboundMessage>,
) {
    let mut last_minute = Some(MinuteKey::from_datetime(&Local::now()));
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = Local::now();
        let minute = MinuteKey::from_datetime(&now);
        if last_minute == Some(minute) {
            continue;
        }
        last_minute = Some(minute);
        match due_messages(&crons_dir, &channels, now).await {
            Ok(messages) => {
                for message in messages {
                    if tx.send(message).await.is_err() {
                        crate::log_info!("cron enqueue stopped reason=daemon_queue_closed");
                        return;
                    }
                }
            }
            Err(err) => crate::log_info!("cron scan failed: {err}"),
        }
    }
}

/// 扫描当前分钟应触发的任务，适用于调度循环和测试。
pub(crate) async fn due_messages(
    crons_dir: &Path,
    channels: &[SchedulerChannel],
    now: DateTime<Local>,
) -> AppResult<Vec<InboundMessage>> {
    let mut messages = Vec::new();
    if !crons_dir.exists() {
        return Ok(messages);
    }
    let mut entries = tokio::fs::read_dir(crons_dir).await?;
    let moment = CronMoment::from_datetime(&now);
    let mut channel_refs = channels.iter().collect::<Vec<_>>();
    channel_refs.sort_by(|left, right| right.name.len().cmp(&left.name.len()));

    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(target) = parse_target_dir(&name, &channel_refs) else {
            crate::log_info!("cron dir ignored reason=unknown_channel dir={name}");
            continue;
        };
        collect_due_messages(entry.path(), target, &moment, &mut messages).await?;
    }
    Ok(messages)
}

/// 收集单个目标目录下的到期任务。
async fn collect_due_messages(
    dir: PathBuf,
    target: CronTarget<'_>,
    moment: &CronMoment,
    messages: &mut Vec<InboundMessage>,
) -> AppResult<()> {
    let cron_path = dir.join(CRON_FILE_NAME);
    if !cron_path.exists() {
        return Ok(());
    }
    let raw = tokio::fs::read_to_string(&cron_path).await?;
    let entries = parse_cron_file(&raw, &cron_path)?;
    for entry in entries {
        if !entry.schedule.matches(moment) {
            continue;
        }
        let task_path = resolve_task_path(&dir, &entry.task_path)?;
        let text = tokio::fs::read_to_string(&task_path).await?;
        if text.trim().is_empty() {
            crate::log_info!(
                "cron task ignored reason=empty_prompt path={}",
                task_path.display()
            );
            continue;
        }
        messages.push(InboundMessage::text(text, target.source(), None).scheduled());
    }
    Ok(())
}

/// 解析目标目录名，适用于 `channel_key` 目录约定。
fn parse_target_dir<'a>(
    dir_name: &str,
    channels: &[&'a SchedulerChannel],
) -> Option<CronTarget<'a>> {
    for channel in channels {
        let prefix = format!("{}_", channel.name);
        let Some(key) = dir_name.strip_prefix(&prefix) else {
            continue;
        };
        if key.trim().is_empty() {
            return None;
        }
        return Some(CronTarget {
            channel,
            key: key.to_string(),
            scope: CronScope::from_key(key),
        });
    }
    None
}

/// 校验并解析 cron.md 内容。
fn parse_cron_file(raw: &str, path: &Path) -> AppResult<Vec<CronEntry>> {
    let mut entries = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 6 {
            return Err(AppError::Cron(format!(
                "{}:{} cron 行必须是 5 个时间字段加 1 个任务文件",
                path.display(),
                index + 1
            )));
        }
        entries.push(CronEntry {
            schedule: CronSchedule::parse(&parts[0..5])?,
            task_path: PathBuf::from(parts[5]),
        });
    }
    Ok(entries)
}

/// 解析并限制任务路径，避免相对路径逃出目标目录。
fn resolve_task_path(dir: &Path, task_path: &Path) -> AppResult<PathBuf> {
    if task_path.is_absolute() {
        return Err(AppError::Cron(format!(
            "cron task path must be relative: {}",
            task_path.display()
        )));
    }
    let root = dir.canonicalize()?;
    let full = dir.join(task_path).canonicalize()?;
    if !full.starts_with(&root) {
        return Err(AppError::Cron(format!(
            "cron task path escapes target dir: {}",
            task_path.display()
        )));
    }
    Ok(full)
}

/// 单个 cron 配置行。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CronEntry {
    /// 时间匹配规则。
    schedule: CronSchedule,
    /// 相对任务文件路径。
    task_path: PathBuf,
}

/// 标准 5 段 cron 时间规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CronSchedule {
    /// 分钟字段，范围 0-59。
    minute: CronField,
    /// 小时字段，范围 0-23。
    hour: CronField,
    /// 日期字段，范围 1-31。
    day_of_month: CronField,
    /// 月份字段，范围 1-12。
    month: CronField,
    /// 星期字段，范围 0-7，其中 0 和 7 都代表周日。
    day_of_week: CronField,
}

impl CronSchedule {
    /// 解析 5 个 cron 字段，适用于 cron.md 单行配置。
    pub(crate) fn parse(parts: &[&str]) -> AppResult<Self> {
        if parts.len() != 5 {
            return Err(AppError::Cron(
                "cron schedule must contain 5 fields".to_string(),
            ));
        }
        Ok(Self {
            minute: CronField::parse(parts[0], 0, 59, FieldNameSet::None)?,
            hour: CronField::parse(parts[1], 0, 23, FieldNameSet::None)?,
            day_of_month: CronField::parse(parts[2], 1, 31, FieldNameSet::None)?,
            month: CronField::parse(parts[3], 1, 12, FieldNameSet::Month)?,
            day_of_week: CronField::parse(parts[4], 0, 7, FieldNameSet::Weekday)?,
        })
    }

    /// 判断当前分钟是否命中，日期与星期按 crontab 传统 OR 语义处理。
    fn matches(&self, moment: &CronMoment) -> bool {
        if !self.minute.matches(moment.minute)
            || !self.hour.matches(moment.hour)
            || !self.month.matches(moment.month)
        {
            return false;
        }
        let day_match = self.day_of_month.matches(moment.day_of_month);
        let weekday_match = self.day_of_week.matches(moment.day_of_week);
        match (self.day_of_month.is_wildcard, self.day_of_week.is_wildcard) {
            (true, true) => true,
            (true, false) => weekday_match,
            (false, true) => day_match,
            (false, false) => day_match || weekday_match,
        }
    }
}

/// 单个 cron 字段的可命中集合。
#[derive(Debug, Clone, PartialEq, Eq)]
struct CronField {
    /// 是否为无约束字段。
    is_wildcard: bool,
    /// 字段可命中的数值。
    values: BTreeSet<u32>,
}

impl CronField {
    /// 解析单个 cron 字段，支持 `*`、列表、范围和步进。
    fn parse(raw: &str, min: u32, max: u32, names: FieldNameSet) -> AppResult<Self> {
        let mut values = BTreeSet::new();
        let is_wildcard = raw == "*" || raw.starts_with("*/");
        for item in raw.split(',') {
            parse_field_item(item, min, max, names, &mut values)?;
        }
        Ok(Self {
            is_wildcard,
            values,
        })
    }

    /// 判断字段是否包含指定值。
    fn matches(&self, value: u32) -> bool {
        self.values.contains(&value)
    }
}

/// 字段可用的英文名称集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldNameSet {
    /// 不允许名称。
    None,
    /// 月份名称。
    Month,
    /// 星期名称。
    Weekday,
}

/// 解析逗号列表中的一个字段片段。
fn parse_field_item(
    raw: &str,
    min: u32,
    max: u32,
    names: FieldNameSet,
    values: &mut BTreeSet<u32>,
) -> AppResult<()> {
    let (base, step) = split_step(raw)?;
    let step = step.unwrap_or(1);
    if step == 0 {
        return Err(AppError::Cron(format!("cron step must be > 0: {raw}")));
    }
    let (start, end) = if base == "*" {
        (min, max)
    } else if let Some((left, right)) = base.split_once('-') {
        (
            parse_field_value(left, min, max, names)?,
            parse_field_value(right, min, max, names)?,
        )
    } else {
        let value = parse_field_value(base, min, max, names)?;
        (value, value)
    };
    if start > end {
        return Err(AppError::Cron(format!("cron range start > end: {raw}")));
    }
    let mut value = start;
    while value <= end {
        values.insert(normalize_weekday(value, names));
        value = value.saturating_add(step);
        if step == 0 {
            break;
        }
    }
    Ok(())
}

/// 拆分步进表达式，适用于 `*/5` 和 `1-10/2`。
fn split_step(raw: &str) -> AppResult<(&str, Option<u32>)> {
    let mut parts = raw.split('/');
    let base = parts.next().unwrap_or_default();
    let step = parts
        .next()
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| AppError::Cron(format!("invalid cron step: {raw}")))
        })
        .transpose()?;
    if parts.next().is_some() || base.is_empty() {
        return Err(AppError::Cron(format!("invalid cron field item: {raw}")));
    }
    Ok((base, step))
}

/// 解析字段数值或名称。
fn parse_field_value(raw: &str, min: u32, max: u32, names: FieldNameSet) -> AppResult<u32> {
    let value = parse_named_value(raw, names)
        .or_else(|| raw.parse::<u32>().ok())
        .ok_or_else(|| AppError::Cron(format!("invalid cron value: {raw}")))?;
    if value < min || value > max {
        return Err(AppError::Cron(format!("cron value out of range: {raw}")));
    }
    Ok(value)
}

/// 解析月份和星期英文缩写。
fn parse_named_value(raw: &str, names: FieldNameSet) -> Option<u32> {
    let raw = raw.to_ascii_lowercase();
    match names {
        FieldNameSet::None => None,
        FieldNameSet::Month => [
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        ]
        .iter()
        .position(|item| *item == raw)
        .map(|index| index as u32 + 1),
        FieldNameSet::Weekday => ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
            .iter()
            .position(|item| *item == raw)
            .map(|index| index as u32),
    }
}

/// 归一化星期字段，适用于兼容 0 和 7 都表示周日。
fn normalize_weekday(value: u32, names: FieldNameSet) -> u32 {
    if names == FieldNameSet::Weekday && value == 7 {
        0
    } else {
        value
    }
}

/// 当前分钟的 cron 匹配快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CronMoment {
    /// 分钟。
    minute: u32,
    /// 小时。
    hour: u32,
    /// 日期。
    day_of_month: u32,
    /// 月份。
    month: u32,
    /// 星期，周日为 0。
    day_of_week: u32,
}

impl CronMoment {
    /// 从本地时间生成匹配快照。
    fn from_datetime(now: &DateTime<Local>) -> Self {
        Self {
            minute: now.minute(),
            hour: now.hour(),
            day_of_month: now.day(),
            month: now.month(),
            day_of_week: now.weekday().num_days_from_sunday(),
        }
    }
}

/// 防止同一分钟重复触发的时间键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MinuteKey {
    /// 年份。
    year: i32,
    /// 月份。
    month: u32,
    /// 日期。
    day: u32,
    /// 小时。
    hour: u32,
    /// 分钟。
    minute: u32,
}

impl MinuteKey {
    /// 从本地时间生成分钟键。
    fn from_datetime(now: &DateTime<Local>) -> Self {
        Self {
            year: now.year(),
            month: now.month(),
            day: now.day(),
            hour: now.hour(),
            minute: now.minute(),
        }
    }
}

/// cron 目录对应的目标。
#[derive(Debug, Clone)]
struct CronTarget<'a> {
    /// 目标 channel。
    channel: &'a SchedulerChannel,
    /// 群 key 或用户 key。
    key: String,
    /// 目标粒度。
    scope: CronScope,
}

impl CronTarget<'_> {
    /// 构造消息来源，适用于复用 daemon 的 provider 处理链路。
    fn source(&self) -> MessageSource {
        MessageSource {
            channel_name: self.channel.name.clone(),
            platform: self.channel.platform.clone(),
            chat_id: match self.scope {
                CronScope::Group => self.key.clone(),
                CronScope::User => String::new(),
            },
            chat_type: match self.scope {
                CronScope::Group => "group".to_string(),
                CronScope::User => "dm".to_string(),
            },
            user_id: match self.scope {
                CronScope::Group => None,
                CronScope::User => Some(self.key.clone()),
            },
            thread_id: None,
        }
    }
}

/// cron 目标粒度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CronScope {
    /// 群粒度。
    Group,
    /// 用户粒度。
    User,
}

impl CronScope {
    /// 从 key 判断目标粒度；飞书 `oc_` 为群，其余按用户 open_id 处理。
    fn from_key(key: &str) -> Self {
        if key.starts_with("oc_") {
            Self::Group
        } else {
            Self::User
        }
    }
}

#[cfg(test)]
mod scheduler_test;
