#!/usr/bin/env bash
set -euo pipefail

# 情况说明：
# 需要部署 llm-loop 到 fake-host，用户说明该机器免密且由 systemctl 控制。
# 这个脚本只做只读探测，用来验证 ssh 和 systemctl 行为，避免猜服务名和路径。

HOST="${1:-fake-host}"

ssh -o BatchMode=yes -o ConnectTimeout=5 "${HOST}" '
set -euo pipefail
echo "HOST=$(hostname)"
echo "USER=$(id -un)"
echo "SYSTEMCTL=$(command -v systemctl || true)"
echo "LLM_LOOP_BIN=$(command -v llm-loop || true)"
systemctl list-units --type=service --all --no-pager --plain | grep -i "llm-loop\|llm_loop" || true
systemctl list-unit-files --type=service --no-pager --plain | grep -i "llm-loop\|llm_loop" || true
'
