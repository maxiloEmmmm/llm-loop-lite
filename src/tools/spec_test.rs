use crate::tools::spec::{
    JsonSchema, ResponsesApiTool, ToolSpec, create_tools_json_for_responses_api,
};

/// 验证 function tool wire shape 与 Responses API 兼容。
#[test]
fn function_tool_serializes_to_responses_shape() {
    let tool = ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "Demo tool.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(Default::default(), None, Some(false.into())),
        output_schema: None,
    });
    let tools = create_tools_json_for_responses_api(&[tool]).expect("tool spec should serialize");

    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["name"], "demo");
    assert_eq!(
        tools[0]["parameters"]["additionalProperties"],
        serde_json::Value::Bool(false)
    );
}
