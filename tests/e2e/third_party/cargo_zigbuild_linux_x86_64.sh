#!/usr/bin/env bash
set -euo pipefail

# 情况说明：
# 需要在 macOS 本机用 cargo-zigbuild 调用 Zig linker，
# 构建可部署到 fake-deploy-host Debian x86_64 的 llm-loop 二进制。
# 这个脚本验证 cargo-zigbuild、zig、目标产物路径和文件类型，
# 避免猜测 zigbuild 的参数行为。

TARGET="${1:-x86_64-unknown-linux-gnu}"
BIN="target/${TARGET}/release/llm-loop"

command -v zig
zig version
cargo zigbuild --help >/dev/null
cargo zigbuild --release --target "${TARGET}"
test -x "${BIN}"
file "${BIN}"
