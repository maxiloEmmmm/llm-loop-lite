use std::path::{Path, PathBuf};

use crate::error::AppResult;
use crate::home::AppPaths;
use crate::message::MessageSource;

const SKILL_METADATA_CHAR_BUDGET: usize = 8_000;

/// skill 元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillMetadata {
    /// skill 名称。
    name: String,
    /// skill 描述。
    description: String,
    /// SKILL.md 路径。
    path: PathBuf,
}

/// 加载 skills 列表并渲染为模型上下文。
pub async fn load_skills_instructions(
    paths: &AppPaths,
    cwd: &Path,
    source: &MessageSource,
) -> AppResult<Option<String>> {
    let mut skills = Vec::new();
    for root in skill_roots(paths, cwd, source) {
        skills.extend(read_skills_from_root(&root).await?);
    }
    skills.sort_by(|left, right| left.name.cmp(&right.name).then(left.path.cmp(&right.path)));
    skills.dedup_by(|left, right| left.path == right.path);
    if skills.is_empty() {
        return Ok(None);
    }
    let mut lines = Vec::new();
    lines.push("## Skills".to_string());
    lines.push("A skill is a set of local instructions to follow that is stored in a `SKILL.md` file. Below is the list of skills that can be used. Each entry includes a name, description, and file path.".to_string());
    lines.push("### Available skills".to_string());
    let mut used = 0_usize;
    for skill in skills {
        let line = format!(
            "- {}: {} (file: {})",
            skill.name,
            skill.description,
            skill.path.display()
        );
        used = used.saturating_add(line.chars().count());
        if used > SKILL_METADATA_CHAR_BUDGET {
            break;
        }
        lines.push(line);
    }
    lines.push("### How to use skills".to_string());
    lines.push("- Discovery: The list above is the skills available in this session (name + description + file path). Skill bodies live on disk at the listed paths.".to_string());
    lines.push("- Missing/blocked: If a named skill is not in the list or the path cannot be read, say so briefly and continue with the best fallback.".to_string());
    lines.push("- Trigger rules: If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a skill's description shown above, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.".to_string());
    lines.push("- How to use a skill: after deciding to use a skill, open and read its `SKILL.md` completely before taking task actions. Resolve relative paths against the directory containing that `SKILL.md`.".to_string());
    lines.push("- If `scripts/`, `references/`, `assets/`, or templates exist, prefer using those files instead of recreating their contents.".to_string());
    Ok(Some(format!("\n{}\n", lines.join("\n"))))
}

/// 收集 skill roots。
pub(super) fn skill_roots(paths: &AppPaths, cwd: &Path, source: &MessageSource) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.push(paths.skills_dir.clone());
    if let Some(scope) = user_skill_scope(source) {
        roots.push(paths.skills_dir.join("__user").join(scope));
    }
    roots.push(paths.codex_home.join("skills"));
    for dir in cwd.ancestors().collect::<Vec<_>>().into_iter().rev() {
        roots.push(dir.join(".agents").join("skills"));
    }
    roots
}

/// 生成用户级 skill scope，适用于隔离不同 channel/user 的私有技能。
fn user_skill_scope(source: &MessageSource) -> Option<String> {
    let channel = if source.channel_name.trim().is_empty() {
        source.platform.trim()
    } else {
        source.channel_name.trim()
    };
    let user_id = source.user_id.as_deref()?.trim();
    if channel.is_empty() || user_id.is_empty() {
        return None;
    }
    Some(format!(
        "{}__{}",
        normalize_scope_part(channel),
        normalize_scope_part(user_id)
    ))
}

/// 清理用户级 skill scope 片段，避免平台 id 影响目录结构。
fn normalize_scope_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// 读取一个 skills root。
async fn read_skills_from_root(root: &Path) -> AppResult<Vec<SkillMetadata>> {
    let mut output = Vec::new();
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(output),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path().join("SKILL.md");
        if path.is_file()
            && let Some(skill) = read_skill_metadata(&path).await?
        {
            output.push(skill);
        }
    }
    Ok(output)
}

/// 读取 SKILL.md frontmatter。
async fn read_skill_metadata(path: &Path) -> AppResult<Option<SkillMetadata>> {
    let raw = tokio::fs::read_to_string(path).await?;
    let Some((name, description)) = parse_frontmatter(&raw) else {
        return Ok(None);
    };
    Ok(Some(SkillMetadata {
        name,
        description,
        path: path.to_path_buf(),
    }))
}

/// 解析 name/description frontmatter。
pub(super) fn parse_frontmatter(raw: &str) -> Option<(String, String)> {
    let mut lines = raw.lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut name = None;
    let mut description = None;
    for line in lines {
        if line == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(clean_yaml_scalar(value));
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(clean_yaml_scalar(value));
        }
    }
    Some((name?, description.unwrap_or_default()))
}

/// 清理简单 YAML 标量。
fn clean_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}
