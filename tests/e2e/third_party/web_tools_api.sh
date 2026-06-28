#!/usr/bin/env bash
# 说明:
# 验证 webfetch/websearch 依赖的第三方 API 形态仍可用。
# 覆盖范围:
# - Jina Reader: https://r.jina.ai/{url}
# - Direct fallback: 目标网页本身可直接读取
# - Web search backend: MCP tools/call 协议
# 这个脚本不启动 llm-loop，不触发部署，只验证外部 API 合约。

set -euo pipefail

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

require_nonempty_file() {
  local path="$1"
  local label="$2"
  [[ -s "$path" ]] || fail "$label returned empty body"
}

reader_out="$tmp_dir/jina-reader.txt"
curl -fsS --max-time 25 \
  -H 'Accept: text/plain' \
  -H 'X-Return-Format: markdown' \
  -H 'X-No-Cache: false' \
  'https://r.jina.ai/https://example.com' \
  >"$reader_out"
require_nonempty_file "$reader_out" "jina reader"
grep -qi 'Example Domain' "$reader_out" || fail "jina reader did not return expected page content"

direct_out="$tmp_dir/direct.html"
curl -fsS --max-time 20 \
  -H 'Accept: text/html,application/xhtml+xml,text/plain;q=0.8,*/*;q=0.1' \
  'https://example.com' \
  >"$direct_out"
require_nonempty_file "$direct_out" "direct fetch"
grep -qi 'Example Domain' "$direct_out" || fail "direct fetch did not return expected page content"

search_url='https://mcp.exa.ai/mcp'
if [[ -n "${EXA_API_KEY:-}" ]]; then
  search_url="${search_url}?exaApiKey=${EXA_API_KEY}"
fi

search_payload="$tmp_dir/search-payload.json"
cat >"$search_payload" <<'JSON'
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "web_search_exa",
    "arguments": {
      "query": "OpenAI official website",
      "type": "fast",
      "numResults": 1,
      "livecrawl": "fallback",
      "contextMaxCharacters": 2000
    }
  }
}
JSON

search_out="$tmp_dir/search-response.txt"
curl -fsS --max-time 30 \
  -X POST \
  -H 'Accept: application/json, text/event-stream' \
  -H 'Content-Type: application/json' \
  --data-binary "@$search_payload" \
  "$search_url" \
  >"$search_out"
require_nonempty_file "$search_out" "web search backend"

if grep -q '"error"' "$search_out"; then
  cat "$search_out" >&2
  fail "web search backend returned JSON-RPC error"
fi

grep -Eq '"result"|"content"|^data:' "$search_out" || fail "web search backend response shape is unexpected"

printf 'OK: web tools third-party API contracts are reachable\n'
