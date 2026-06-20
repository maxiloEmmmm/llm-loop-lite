//! 飞书事件 JSON 解析。

use serde::Deserialize;
use serde_json::Value;

/// 飞书事件 envelope。
#[derive(Debug, Deserialize)]
pub struct FeishuEventEnvelope {
    /// 事件头。
    pub header: FeishuEventHeader,
    /// 原始事件体。
    pub event: Option<Value>,
}

/// 飞书事件头。
#[derive(Debug, Deserialize)]
pub struct FeishuEventHeader {
    /// 事件类型。
    pub event_type: String,
}

/// 飞书收消息事件。
#[derive(Debug, Deserialize)]
pub struct FeishuReceiveMessageEvent {
    /// 发送者。
    pub sender: FeishuSender,
    /// 消息体。
    pub message: FeishuMessage,
}

/// 飞书发送者。
#[derive(Debug, Deserialize)]
pub struct FeishuSender {
    /// 发送者 id 集合。
    pub sender_id: FeishuSenderId,
}

/// 飞书发送者 id。
#[derive(Debug, Deserialize)]
pub struct FeishuSenderId {
    /// app-scoped open_id。
    pub open_id: Option<String>,
    /// tenant-scoped user_id。
    pub user_id: Option<String>,
    /// developer-scoped union_id。
    pub union_id: Option<String>,
}

/// 飞书消息。
#[derive(Debug, Deserialize)]
pub struct FeishuMessage {
    /// 消息 id。
    pub message_id: String,
    /// root id。
    pub root_id: Option<String>,
    /// parent id。
    pub parent_id: Option<String>,
    /// thread id。
    pub thread_id: Option<String>,
    /// chat id。
    pub chat_id: String,
    /// chat 类型。
    pub chat_type: String,
    /// 消息类型。
    pub message_type: String,
    /// 原始内容 JSON 字符串。
    pub content: String,
    /// @ 信息。
    #[serde(default)]
    pub mentions: Vec<FeishuMention>,
}

/// 飞书 mention。
#[derive(Debug, Deserialize)]
pub struct FeishuMention {
    /// mention key，例如 @_user_1。
    pub key: Option<String>,
    /// mention id，飞书事件可能返回字符串或旧结构。
    pub id: Option<Value>,
    /// mention id 类型，例如 open_id 或 union_id。
    pub id_type: Option<String>,
    /// mention 展示名。
    pub name: Option<String>,
}

impl FeishuReceiveMessageEvent {
    /// 判断消息是否 @ 当前机器人，适用于飞书群聊门禁。
    pub fn mentions_bot(&self, bot_open_id: &str, bot_name: Option<&str>) -> bool {
        if self.message.content.contains("@_all") {
            return true;
        }
        self.message
            .mentions
            .iter()
            .any(|mention| mention.matches_bot(bot_open_id, bot_name))
    }

    /// 返回 mention 摘要，适用于排查群聊 @ 门禁误判。
    pub fn mentions_summary(&self) -> String {
        self.message
            .mentions
            .iter()
            .map(FeishuMention::summary)
            .collect::<Vec<_>>()
            .join(";")
    }
}

impl FeishuMention {
    /// 判断 mention 是否命中机器人，适用于过滤 @ 其他人的消息。
    fn matches_bot(&self, bot_open_id: &str, bot_name: Option<&str>) -> bool {
        let key_hit = self.key.as_deref() == Some("@_all");
        let open_id_hit = self.matches_open_id(bot_open_id);
        let name_hit = bot_name
            .zip(self.name.as_deref())
            .is_some_and(|(bot, mention)| bot == mention);
        key_hit || open_id_hit || name_hit
    }

    /// 判断 mention id 是否命中 open_id，适用于兼容飞书事件新旧结构。
    fn matches_open_id(&self, bot_open_id: &str) -> bool {
        let Some(id) = self.id.as_ref() else {
            return false;
        };
        if self.id_type.as_deref() == Some("open_id")
            && id.as_str().is_some_and(|value| value == bot_open_id)
        {
            return true;
        }
        // 触发条件：飞书 WS 事件的 mention.id 可能是 id 集合对象。
        // 不能只走 string 路径：REST 详情和 WS 事件结构可能不一致。
        // 防止回归：群聊 @ 机器人时不会因 id 形态差异被丢弃。
        id.get("open_id")
            .and_then(Value::as_str)
            .is_some_and(|value| value == bot_open_id)
    }

    /// 返回 mention 调试摘要，适用于日志中定位 id 形态差异。
    fn summary(&self) -> String {
        format!(
            "key={} id_type={} id={} name={}",
            self.key.as_deref().unwrap_or(""),
            self.id_type.as_deref().unwrap_or(""),
            self.id
                .as_ref()
                .map(|value| value.to_string())
                .unwrap_or_default(),
            self.name.as_deref().unwrap_or("")
        )
    }
}

impl FeishuMessage {
    /// 提取当前支持的文本内容。
    pub fn text(&self) -> Option<String> {
        message_text(&self.message_type, &self.content)
    }
}

/// 提取消息正文，适用于事件消息和合并转发子消息复用。
pub fn message_text(message_type: &str, content: &str) -> Option<String> {
    match message_type {
        "text" => text_from_content(content),
        "post" => post_text_from_content(content),
        "image" => Some("[图片]".to_string()),
        "file" => Some(file_text_from_content(content)),
        "audio" => Some("[语音]".to_string()),
        "video" => Some("[视频]".to_string()),
        "sticker" => Some("[表情]".to_string()),
        _ => None,
    }
}

/// 从消息 content 中提取图片 key，适用于事件消息和合并转发子消息复用。
pub fn image_keys_from_content(content: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    collect_image_keys(&value, &mut keys);
    keys.sort();
    keys.dedup();
    keys
}

/// 从消息 content 中提取文件资源，适用于事件消息和合并转发子消息复用。
pub fn file_resources_from_content(content: &str) -> Vec<FeishuFileResource> {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return Vec::new();
    };
    let mut resources = Vec::new();
    collect_file_resources(&value, &mut resources);
    resources.sort_by(|left, right| left.file_key.cmp(&right.file_key));
    resources.dedup_by(|left, right| left.file_key == right.file_key);
    resources
}

/// 飞书文件资源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuFileResource {
    /// 文件 key。
    pub file_key: String,
    /// 文件名。
    pub filename: Option<String>,
}

/// 从 text 消息 content 中解析文本。
fn text_from_content(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    value
        .get("text")
        .and_then(Value::as_str)
        .map(clean_text)
        .filter(|text| !text.trim().is_empty())
}

/// 从 post 消息 content 中提取粗略文本。
fn post_text_from_content(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let mut parts = Vec::new();
    collect_text(&value, &mut parts);
    let text = clean_text(&parts.join("\n"));
    (!text.trim().is_empty()).then_some(text)
}

/// 从 file 消息 content 中提取文件名。
fn file_text_from_content(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return "[文件]".to_string();
    };
    value
        .get("file_name")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .map(|name| format!("[文件] {name}"))
        .unwrap_or_else(|| "[文件]".to_string())
}

/// 递归收集 post 文本节点。
fn collect_text(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
            for value in map.values() {
                collect_text(value, parts);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_text(item, parts);
            }
        }
        _ => {}
    }
}

/// 递归收集图片 key。
fn collect_image_keys(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(image_key) = map.get("image_key").and_then(Value::as_str) {
                keys.push(image_key.to_string());
            }
            for value in map.values() {
                collect_image_keys(value, keys);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_image_keys(item, keys);
            }
        }
        _ => {}
    }
}

/// 递归收集文件资源。
fn collect_file_resources(value: &Value, resources: &mut Vec<FeishuFileResource>) {
    match value {
        Value::Object(map) => {
            if let Some(file_key) = map.get("file_key").and_then(Value::as_str) {
                let filename = map
                    .get("file_name")
                    .or_else(|| map.get("name"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                resources.push(FeishuFileResource {
                    file_key: file_key.to_string(),
                    filename,
                });
            }
            for value in map.values() {
                collect_file_resources(value, resources);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_file_resources(item, resources);
            }
        }
        _ => {}
    }
}

/// 清理飞书文本里的 mention 占位。
fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[path = "event_test.rs"]
mod event_test;
