use super::reply_hash_from_parts;

/// 回复 hash 固定为 8 位十六进制，适用于飞书排查前缀。
#[test]
fn reply_hash_is_eight_hex_chars() {
    let hash = reply_hash_from_parts("agent:main:feishu:dm:chat:user", 123);

    assert_eq!(hash.len(), 8);
    assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()));
}

/// key 或时间变化都会改变回复 hash。
#[test]
fn reply_hash_changes_with_key_or_time() {
    let first = reply_hash_from_parts("key-a", 123);
    let second = reply_hash_from_parts("key-b", 123);
    let third = reply_hash_from_parts("key-a", 124);

    assert_ne!(first, second);
    assert_ne!(first, third);
}
