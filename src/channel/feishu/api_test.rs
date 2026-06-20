use super::{
    MAX_MARKDOWN_TABLES_PER_CARD, compact_markdown_card_content, count_markdown_tables,
    feishu_render_outbound_text, split_markdown_table_batches,
};
use crate::message::OutboundFormat;

/// 构造 Markdown 表格，适用于分批算法测试。
fn table(index: usize) -> String {
    format!("| 名称 | 值 |\n| --- | --- |\n| t{index} | {index} |")
}

/// 验证超过飞书单卡表格上限时会拆成多条消息。
#[test]
fn split_markdown_tables_when_over_card_limit() {
    let text = (1..=6).map(table).collect::<Vec<_>>().join("\n\n");

    let batches = split_markdown_table_batches(&text, MAX_MARKDOWN_TABLES_PER_CARD);

    assert_eq!(batches.len(), 2);
    assert_eq!(count_markdown_tables(&batches[0]), 5);
    assert_eq!(count_markdown_tables(&batches[1]), 1);
}

/// 验证代码块里的管道表格不会触发分批。
#[test]
fn split_markdown_tables_ignores_code_fence() {
    let text = "前置\n```markdown\n| a | b |\n| --- | --- |\n| 1 | 2 |\n```\n后置";

    let batches = split_markdown_table_batches(text, MAX_MARKDOWN_TABLES_PER_CARD);

    assert_eq!(batches, vec![text.to_string()]);
    assert_eq!(count_markdown_tables(text), 0);
}

/// 验证普通段落会跟随相邻表格，避免拆出空消息。
#[test]
fn split_markdown_tables_keeps_prose_with_batches() {
    let text = format!("开头\n{}\n中段\n{}\n结尾", table(1), table(2));

    let batches = split_markdown_table_batches(&text, 1);

    assert_eq!(batches.len(), 2);
    assert!(batches[0].starts_with("开头"));
    assert!(batches[0].contains("中段"));
    assert!(batches[1].ends_with("结尾"));
}

/// 飞书卡片正文会去掉普通空白行，适用于压紧段落间距。
#[test]
fn compact_markdown_card_content_removes_blank_lines() {
    let text = "[hash] 第一行\n\n第二行\n\n\n第三行";

    let compacted = compact_markdown_card_content(text);

    assert_eq!(compacted, "[hash] 第一行\n第二行\n第三行");
}

/// 飞书卡片正文保留代码块里的空白行，避免破坏代码展示。
#[test]
fn compact_markdown_card_content_keeps_code_block_blank_lines() {
    let text = "说明\n\n```text\n第一行\n\n第三行\n```\n\n结尾";

    let compacted = compact_markdown_card_content(text);

    assert_eq!(compacted, "说明\n```text\n第一行\n\n第三行\n```\n结尾");
}

/// 飞书计划文本会转义编号，避免卡片 markdown 重排计划层级。
#[test]
fn feishu_render_outbound_text_escapes_plan_markers() {
    let text = "[hash]\n1. 根计划\n  1.1. 子任务\n正文 2. 不处理";

    let rendered = feishu_render_outbound_text(text, OutboundFormat::Plan);

    assert_eq!(
        rendered,
        "[hash]\n1\\. 根计划\n  1.1\\. 子任务\n正文 2. 不处理"
    );
    assert_eq!(
        feishu_render_outbound_text(text, OutboundFormat::Text),
        text
    );
}
