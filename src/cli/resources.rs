use crate::daemon::resources::{ResourceSnapshot, query_resource_snapshot};
use crate::error::AppResult;
use crate::home::AppPaths;

/// 执行 resources 子命令，适用于查询运行中 daemon 的资源快照。
pub async fn run_resources(paths: &AppPaths) -> AppResult<()> {
    let snapshot = query_resource_snapshot(paths).await?;
    print_snapshot(&snapshot);
    Ok(())
}

/// 打印资源快照。
fn print_snapshot(snapshot: &ResourceSnapshot) {
    println!("process");
    println!("  pid: {}", snapshot.pid);
    println!("  rss: {}", format_bytes(snapshot.rss_bytes));
    println!("  virtual: {}", format_bytes(snapshot.virtual_bytes));
    println!();
    println!("runtime");
    println!("  sessions: {}", snapshot.session_count);
    println!("  session_locks: {}", snapshot.session_lock_count);
    println!("  active_turns: {}", snapshot.active_turn_count);
    println!(
        "  cron_available_permits: {}",
        snapshot.cron_available_permits
    );
    println!();
    println!("channels");
    for channel in &snapshot.channels {
        let caps = &channel.capabilities;
        println!("  {} ({})", channel.name, channel.platform);
        println!(
            "    patch={} append_update={} input={} reaction={} text_ack={} typing={} reply={} inbound_attach={} outbound_attach={}",
            caps.patch_message,
            caps.append_update,
            caps.request_user_input,
            caps.reaction_ack,
            caps.text_ack,
            caps.chat_action,
            caps.reply_threading,
            caps.inbound_attachments,
            caps.outbound_attachments,
        );
    }
    println!();
    println!("memory");
    let mut memory = snapshot.memory.clone();
    memory.sort_by(|left, right| right.estimated_bytes.cmp(&left.estimated_bytes));
    for item in memory {
        let capacity = item
            .capacity
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  {:<34} kind={:<8} items={:<6} capacity={:<6} estimated_bytes={}",
            item.name, item.kind, item.items, capacity, item.estimated_bytes
        );
    }
    println!();
    println!("paths");
    for path in &snapshot.paths {
        println!(
            "  {:<14} files={:<6} bytes={:<10} {}",
            path.name,
            path.files,
            path.bytes,
            path.path.display()
        );
    }
}

/// 格式化可选字节数。
fn format_bytes(bytes: Option<u64>) -> String {
    bytes
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}
