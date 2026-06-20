use std::path::Path;

use crate::error::AppResult;
use crate::home::AppPaths;
use crate::message::MessageSource;

const MEMORY_CHAR_BUDGET: usize = 24_000;

/// 单条记忆元数据，适用于稳定排序后注入初始上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryDocument {
    /// 记忆归属范围。
    scope: MemoryScope,
    /// 记忆 key，对应文件名主体。
    key: String,
    /// 记忆正文，不包含 YAML frontmatter。
    body: String,
}

/// 记忆归属范围，适用于区分全局和当前用户记忆。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryScope {
    /// 全局记忆。
    Global,
    /// 当前用户记忆。
    User,
}

impl MemoryScope {
    /// 返回提示词里的范围标签。
    fn label(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::User => "User",
        }
    }
}

/// 加载全局和当前用户记忆并渲染为模型上下文。
pub async fn load_memory_instructions(
    paths: &AppPaths,
    source: &MessageSource,
) -> AppResult<Option<String>> {
    let user_key = memory_user_key(source);
    let mut memories = read_memories_from_dir(&paths.mems_dir, MemoryScope::Global).await?;
    if let Some(user_key) = user_key.as_ref() {
        memories.extend(
            read_memories_from_dir(
                &paths.mems_dir.join("__user").join(user_key),
                MemoryScope::User,
            )
            .await?,
        );
    }
    memories.sort_by(|left, right| {
        left.scope
            .label()
            .cmp(right.scope.label())
            .then(left.key.cmp(&right.key))
    });
    let mut lines = Vec::new();
    lines.extend(memory_scope_lines(paths, source, user_key.as_deref()));
    let mut used = 0_usize;
    for memory in memories {
        let body = memory.body.trim();
        if body.is_empty() {
            continue;
        }
        let block = format!("### {}:{}\n{}", memory.scope.label(), memory.key, body);
        used = used.saturating_add(block.chars().count());
        if used > MEMORY_CHAR_BUDGET {
            break;
        }
        lines.push(block);
    }
    if lines.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("\n{}\n", lines.join("\n\n"))))
}

/// 读取记忆目录，适用于只接受合法 key 的 markdown 文件。
async fn read_memories_from_dir(root: &Path, scope: MemoryScope) -> AppResult<Vec<MemoryDocument>> {
    let mut output = Vec::new();
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(output),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let Some(key) = memory_key_from_path(&path) else {
            continue;
        };
        let raw = tokio::fs::read_to_string(&path).await?;
        output.push(MemoryDocument {
            scope,
            key,
            body: strip_frontmatter(&raw).trim().to_string(),
        });
    }
    Ok(output)
}

/// 渲染当前记忆写入范围，适用于 __mem skill 写入正确用户目录。
fn memory_scope_lines(
    paths: &AppPaths,
    source: &MessageSource,
    user_key: Option<&str>,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Memory write scope:".to_string());
    lines.push(format!("- global_dir: {}", paths.mems_dir.display()));
    if let Some(user_key) = user_key {
        lines.push(format!(
            "- default_scope: user\n- user_id: {}\n- user_key: {}\n- user_dir: {}",
            source.user_id.as_deref().unwrap_or(""),
            user_key,
            paths.mems_dir.join("__user").join(user_key).display()
        ));
    } else {
        lines.push("- default_scope: global\n- user_id: \n- user_key: \n- user_dir: ".to_string());
    }
    lines
}

/// 生成当前用户记忆目录 key，适用于隔离不同发送者的默认记忆。
fn memory_user_key(source: &MessageSource) -> Option<String> {
    let user_id = source.user_id.as_deref()?.trim();
    if user_id.is_empty() {
        return None;
    }
    let key = user_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    (!key.is_empty()).then_some(key)
}

/// 从路径提取合法记忆 key，适用于过滤非 `[A-Za-z0-9]+.md` 文件。
fn memory_key_from_path(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return None;
    }
    let key = path.file_stem()?.to_str()?;
    if key.is_empty() || !key.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    Some(key.to_string())
}

/// 去掉 YAML frontmatter，适用于把记忆正文注入系统提示词。
fn strip_frontmatter(raw: &str) -> &str {
    let Some(rest) = raw.strip_prefix("---\n") else {
        return raw;
    };
    let Some(end) = rest.find("\n---") else {
        return raw;
    };
    let after_marker = &rest[end + "\n---".len()..];
    after_marker.strip_prefix('\n').unwrap_or(after_marker)
}

#[cfg(test)]
/// 测试专用：暴露记忆 key 解析逻辑。
pub(super) fn memory_key_from_path_for_test(path: &Path) -> Option<String> {
    memory_key_from_path(path)
}

#[cfg(test)]
/// 测试专用：暴露用户记忆目录 key 解析逻辑。
pub(super) fn memory_user_key_for_test(source: &MessageSource) -> Option<String> {
    memory_user_key(source)
}

#[cfg(test)]
/// 测试专用：暴露 frontmatter 剥离逻辑。
pub(super) fn strip_frontmatter_for_test(raw: &str) -> &str {
    strip_frontmatter(raw)
}

#[cfg(test)]
/// 测试专用：加载记忆上下文。
pub(super) async fn load_memory_instructions_for_test(
    paths: &AppPaths,
    source: &MessageSource,
) -> AppResult<Option<String>> {
    load_memory_instructions(paths, source).await
}
