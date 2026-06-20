# 工具层维护说明

这个目录是 llm-loop 的独立 tools runtime，不属于 Codex provider。

`spec.rs` 的 Responses API tool wire shape 参考自：

- `/tmp/codex-rs/codex-rs/tools/src/tool_spec.rs`
- `/tmp/codex-rs/codex-rs/tools/src/responses_api.rs`
- `/tmp/codex-rs/codex-rs/tools/src/json_schema.rs`

内置工具的参数名和描述优先对齐 Codex：

- `/tmp/codex-rs/codex-rs/core/src/tools/handlers/*_spec.rs`
- `/tmp/codex-rs/codex-rs/core/src/tools/handlers/sleep.rs`

维护规则：

- provider 只合并 `ToolRegistry::specs()` 产物，不在 provider 目录里实现本地工具。
- provider-only hosted tools 可以在 provider 目录按能力追加，例如 Responses hosted `web_search`、`image_generation`。
- 不要复制 Codex app-server、MCP runtime、多 agent runtime 到这里；这些不是当前低内存目标。
- 工具执行必须真实闭环：有 spec 就必须能解析 tool call、执行、回灌 output。
- 测试放在同级 `*_test.rs` 文件，不要写进逻辑文件。
