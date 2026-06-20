# Codex request client

这个目录维护 llm-loop 轻量级 Codex HTTP/SSE 请求层。

参考来源：

- `/tmp/codex-rs/codex-rs/codex-api/src/common.rs`
- `/tmp/codex-rs/codex-rs/codex-api/src/endpoint/responses.rs`
- `/tmp/codex-rs/codex-rs/codex-api/src/requests/headers.rs`
- `/tmp/codex-rs/codex-rs/codex-api/src/sse/responses.rs`
- `/tmp/codex-rs/codex-rs/core/src/client.rs`

维护规则：

- 这是裁剪迁移的请求层，不是 vendor，不允许魔改第三方库。
- 只保留 llm-loop 当前需要的低内存路径。
- 新增请求字段前先对照 Codex 源码。
- 测试必须写在同级 `*_test.rs`。
