use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AppResult;
use crate::message::{InboundAttachment, MessageSource};
use crate::session::build_message_key;
use crate::store::store_hash;

/// 已下载附件元信息，适用于各 channel 把平台资源统一落盘。
pub struct DownloadedAttachment<'a> {
    /// 原始文件名或平台资源名。
    pub filename: &'a str,
    /// MIME 类型。
    pub mime_type: &'a str,
    /// 已下载的文件内容。
    pub bytes: &'a [u8],
}

/// 落盘入站附件，适用于 channel 下载资源后只向 provider 暴露路径。
pub async fn store_inbound_attachment(
    store_root: &Path,
    source: &MessageSource,
    attachment: DownloadedAttachment<'_>,
) -> AppResult<InboundAttachment> {
    let session_key = build_message_key(source);
    let session_dir = store_root.join(store_hash(&session_key));
    tokio::fs::create_dir_all(&session_dir).await?;
    let filename = sanitize_filename(attachment.filename);
    let path = session_dir.join(unique_store_filename(&filename));
    tokio::fs::write(&path, attachment.bytes).await?;
    Ok(InboundAttachment::StoredFile {
        path,
        filename,
        mime_type: attachment.mime_type.to_string(),
        size: attachment.bytes.len() as u64,
    })
}

/// 构造唯一落盘文件名，适用于同名附件不互相覆盖。
fn unique_store_filename(filename: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos}_{filename}")
}

/// 清理平台文件名，避免路径穿越和奇怪分隔符。
pub fn sanitize_filename(filename: &str) -> String {
    let cleaned = filename
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect::<String>();
    let trimmed = cleaned
        .trim()
        .trim_matches('.')
        .chars()
        .take(120)
        .collect::<String>();
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed
    }
}
