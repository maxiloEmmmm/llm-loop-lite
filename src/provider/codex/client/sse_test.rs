use super::sse::extract_response_from_sse;

/// SSE delta 会拼接为最终文本。
#[test]
fn extract_text_from_delta_events() {
    let raw = r#"data: {"delta":"he"}

data: {"delta":"llo"}

data: [DONE]

"#;

    let response = extract_response_from_sse(raw).expect("应能解析 SSE");

    assert_eq!(response.text, "hello");
}

/// output_item.done 里的完整文本不会和 delta 重复拼接。
#[test]
fn output_item_done_text_is_not_appended_after_delta() {
    let raw = r#"data: {"type":"response.output_text.delta","delta":"he"}

data: {"type":"response.output_text.delta","delta":"llo"}

data: {"type":"response.output_item.done","item":{"type":"message","content":[{"type":"output_text","text":"hello"}]}}

data: [DONE]

"#;

    let response = extract_response_from_sse(raw).expect("应能解析 SSE");

    assert_eq!(response.text, "hello");
}
