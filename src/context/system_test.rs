use super::{InitialContext, render_initial_context_for_log};

/// 测试初始上下文日志渲染，适用于确认完整提示词可直接排查。
#[test]
fn render_initial_context_for_log_includes_instruction_content() {
    let context = InitialContext {
        instructions: "alpha\n\nbeta".to_string(),
    };

    let rendered = render_initial_context_for_log(&context);

    assert!(rendered.contains("----- instructions -----"));
    assert!(rendered.contains("alpha"));
    assert!(rendered.contains("beta"));
}

/// 测试空初始上下文日志渲染，适用于无提示词文件和无 skills 的场景。
#[test]
fn render_initial_context_for_log_marks_empty_context() {
    let context = InitialContext {
        instructions: String::new(),
    };

    assert_eq!(render_initial_context_for_log(&context), "<empty>");
}
