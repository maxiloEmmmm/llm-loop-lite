#!/usr/bin/env bash
# 验证背景：
# 飞书开放平台 HTML 文档是前端壳，不便于命令行稳定检索。
# 官方页面提供同路径 .md 文档，当前脚本验证合并转发读取接口
# 仍指向 GET /open-apis/im/v1/messages/:message_id，
# 且响应 items 包含合并转发本体和子消息说明。
set -euo pipefail

doc="$(curl -fsSL 'https://open.feishu.cn/document/server-docs/im-v1/message/get.md')"

grep -F 'HTTP URL | https://open.feishu.cn/open-apis/im/v1/messages/:message_id' <<<"${doc}" >/dev/null
grep -F '如果查询的消息类型为合并转发（merge_forward），则返回的 `items` 中会包含 1 条合并转发消息和 N 条子消息' <<<"${doc}" >/dev/null
grep -F 'upper_message_id | string | 合并转发消息中，上一层级的消息 ID' <<<"${doc}" >/dev/null
