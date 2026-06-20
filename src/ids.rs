use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 进程内单调计数器，用于降低同毫秒内 session id 碰撞概率。
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 生成 session id，适用于本地 daemon 的进程内会话标识。
pub fn new_session_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("sess_{millis:x}_{seq:x}")
}

/// 生成回复排查短 hash，适用于把同一 session key 下的单次回复标记到消息前缀。
pub fn new_reply_hash(session_key: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    reply_hash_from_parts(session_key, nanos)
}

/// 基于 key 和时间生成 8 位十六进制 hash。
pub(crate) fn reply_hash_from_parts(session_key: &str, nanos: u128) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in session_key
        .as_bytes()
        .iter()
        .copied()
        .chain(nanos.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", hash as u32)
}

#[cfg(test)]
#[path = "ids_test.rs"]
mod ids_test;
