use crate::error::{AppError, AppResult};

/// 单个上游 SSE 事件，适用于跨 HTTP chunk 还原 `event + data`。
pub(crate) struct SseEvent {
    /// 可选事件名。
    pub(crate) event: Option<String>,
    /// 合并后的 data 内容。
    pub(crate) data: String,
}

/// 增量 SSE 解码器，适用于上游任意位置切分 HTTP chunk，例如 `2 chunks -> 1 event`。
#[derive(Default)]
pub(crate) struct SseDecoder {
    /// 尚未形成完整事件的原始字节。
    pending: Vec<u8>,
}

impl SseDecoder {
    /// 创建空解码器，例如 `new -> 0 pending bytes`。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 追加 HTTP chunk 并返回完整事件，例如 `"data: x\n" + "\n" -> event(x)`。
    pub(crate) fn push(&mut self, chunk: &[u8]) -> AppResult<Vec<SseEvent>> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((end, delimiter_len)) = find_event_boundary(&self.pending) {
            let raw = self.pending.drain(..end).collect::<Vec<_>>();
            self.pending.drain(..delimiter_len);
            if let Some(event) = parse_event(&raw)? {
                events.push(event);
            }
        }
        Ok(events)
    }
}

/// 查找 SSE 空行边界，同时支持 LF 与 CRLF，例如 `bytes -> (end, delimiter)`。
fn find_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    for index in 0..bytes.len().saturating_sub(1) {
        if bytes[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
        if bytes[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
    }
    None
}

/// 解析单个 SSE 事件块，例如 `event: delta + data: {...} -> SseEvent`。
fn parse_event(raw: &[u8]) -> AppResult<Option<SseEvent>> {
    let raw = std::str::from_utf8(raw)
        .map_err(|err| AppError::Provider(format!("upstream SSE is not UTF-8: {err}")))?;
    let mut event = None;
    let mut data = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim_start().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    Ok(Some(SseEvent {
        event,
        data: data.join("\n"),
    }))
}
