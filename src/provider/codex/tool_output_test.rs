use serde_json::json;

use super::{tool_call_item, tool_output_text};
use crate::tools::registry::{ToolOutputKind, ToolResult};

/// 对象输出会序列化成字符串，适用于 Responses tool output 回灌。
#[test]
fn function_tool_output_object_serializes_as_string() {
    let result = ToolResult {
        output_kind: ToolOutputKind::Function,
        call_id: "call_1".to_string(),
        output: json!({
            "success": true,
            "message_id": "om_1",
        }),
    };

    let item = tool_call_item("__plan_list", &result);

    assert_eq!(item["type"], "function_call_output");
    assert_eq!(item["call_id"], "call_1");
    assert_eq!(
        item["output"].as_str(),
        Some(r#"{"message_id":"om_1","success":true}"#)
    );
}

/// 空字符串输出保持为空，适用于只需要闭合 call_id 的 UI 工具。
#[test]
fn empty_tool_output_stays_empty_string() {
    let result = ToolResult {
        output_kind: ToolOutputKind::Function,
        call_id: "call_2".to_string(),
        output: json!(""),
    };

    let item = tool_call_item("__plan_list_update", &result);

    assert_eq!(item["output"].as_str(), Some(""));
}

/// custom tool 同样使用字符串输出，适用于 freeform 工具回灌。
#[test]
fn custom_tool_output_serializes_as_string() {
    let result = ToolResult {
        output_kind: ToolOutputKind::Custom,
        call_id: "call_3".to_string(),
        output: json!({
            "changed": ["a.rs"],
        }),
    };

    let item = tool_call_item("apply_patch", &result);

    assert_eq!(item["type"], "custom_tool_call_output");
    assert_eq!(item["name"], "apply_patch");
    assert_eq!(item["output"].as_str(), Some(r#"{"changed":["a.rs"]}"#));
}

/// 字符串输出不做二次 JSON 编码，适用于 shell 等文本结果。
#[test]
fn string_tool_output_is_not_double_encoded() {
    assert_eq!(tool_output_text(&json!("ok")), "ok");
}
