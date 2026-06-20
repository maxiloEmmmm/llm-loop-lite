# llm-loop

`llm-loop` is a lightweight daemon that connects chat channels to coding-oriented LLM providers. It turns Feishu, Telegram, and QQ messages into persistent agent sessions, adds local tools and skills, manages context, and sends replies back through the original channel.

It is meant for people who want a small always-on bot process instead of a full agent platform. The daemon keeps the channel layer, provider layer, tools, skills, memories, and session cache in one compact Rust service.

## Highlights

- **Low memory footprint**: designed as a small daemon with compact local state and async IO.
- **Channel-first architecture**: Feishu and Telegram are the primary channels; QQ is supported in a more basic form.
- **Global and user-scoped memories**: global memories live in `~/.llm-loop/mems/`; user memories live in `~/.llm-loop/mems/__user/<user_key>/`.
- **Built-in skill management**: skills are discoverable prompt modules stored under `~/.llm-loop/skills/`, including built-in memory and skill-authoring skills.
- **Session persistence**: optional session cache restores conversation history after restart and supports context compaction.
- **Codex-style tool loop**: includes shell execution, patching, image handling, context inspection, cron tasks, plan updates, and structured user input.
- **Provider flexibility**: supports the Codex Responses path and Claude Messages path, including custom/OpenAI-compatible and Anthropic-compatible provider profiles.
- **Operational feedback**: channels can acknowledge received messages, reset sessions, stop active turns, and update plan/status messages according to platform capability.

## Channels

| Channel | Status | Notes |
| --- | --- | --- |
| Feishu / Lark | Primary | WebSocket receive path, message/reaction acknowledgements, cards, file/image handling, and updatable plan messages. |
| Telegram | Primary | Bot API polling, reactions/typing, reply threading, attachment download, and inline user input. |
| QQ Bot | Basic | WebSocket receive path and passive replies; platform limitations make some interactions less rich. |

## Providers

| Provider kind | API path | Typical use |
| --- | --- | --- |
| `codex` | OpenAI Responses-compatible | Codex OAuth auth or custom OpenAI-compatible providers. |
| `claude` | Anthropic Messages-compatible | Claude or Anthropic-compatible providers. |

Provider profiles can be defined directly in `~/.llm-loop/config.toml` or inherited from Codex-style `~/.codex/config.toml` `model_providers`.

## Configuration

The daemon reads `~/.llm-loop/config.toml`. Missing fields use conservative defaults.

```toml
work-dir = "/path/to/workspace"
cache-session = true
model = "gpt-5.5"
model_reasoning_effort = "high"
service_tier = "fast"
model_provider = "openai-compatible"

[log]
path = "/tmp/llm-loop.log"
max-size = 4194304

[provider]
kind = "codex"
model = "gpt-5.5"
custom_provider = "openai-compatible"
model_reasoning_effort = "high"
service_tier = "fast"

[model_providers.openai-compatible]
kind = "codex"
model = "gpt-5.5"
base_url = "https://example.invalid/v1"
env_key = "OPENAI_COMPAT_API_KEY"
requires_openai_auth = false

[model_providers.claude-compatible]
kind = "claude"
model = "claude-sonnet-4"
base_url = "https://example.invalid"
api-key-env = "ANTHROPIC_API_KEY"
max-tokens = 4096

[[channels]]
name = "feishu-main"
kind = "feishu"
enabled = true

[channels.feishu]
app_id_env = "FEISHU_APP_ID"
app_secret_env = "FEISHU_APP_SECRET"
domain = "feishu"
require_mention = true

[[channels]]
name = "tg-main"
kind = "telegram"
enabled = true

[channels.telegram]
bot_token_env = "TELEGRAM_BOT_TOKEN"
require_mention = false
send_typing = true
download_attachments = true

[[channels]]
name = "qq-main"
kind = "qq"
enabled = false

[channels.qq]
app_id_env = "QQBOT_APP_ID"
app_secret_env = "QQBOT_APP_SECRET"
```

### Important Paths

| Path | Purpose |
| --- | --- |
| `~/.llm-loop/config.toml` | Main daemon configuration. |
| `~/.llm-loop/sessions/` | Session history cache when `cache-session = true`. |
| `~/.llm-loop/mems/` | Global memories. |
| `~/.llm-loop/mems/__user/<user_key>/` | User-scoped memories. |
| `~/.llm-loop/skills/` | Built-in and user-installed skills. |
| `~/.llm-loop/crons/` | Channel-scoped scheduled tasks. |
| `~/.llm-loop/plans/` | Plan/status message state. |
| `~/.llm-loop/channel/` | Channel persistence and attachment store. |

## Memories and Skills

Memory files are plain Markdown:

```md
---
user: <creator-id>
updated_at: <last-modified-time>
---

The memory body.
```

Only the body is injected into model instructions. The frontmatter stays on disk for management metadata.

Skills are `SKILL.md` files with frontmatter metadata. New sessions receive a compact list of available skills and their trigger descriptions; the model reads a skill file only when the user request matches that skill.

## How It Differs

| Project | Primary shape | Strengths | Tradeoffs |
| --- | --- | --- | --- |
| `llm-loop` | Small Rust daemon for chat-channel agent loops | Low memory use, simple deployment, Feishu/TG focus, user/global memory, skills, Codex/Claude providers | Not a full multi-agent platform; QQ support is intentionally basic. |
| Hermes | Larger bot/agent service | Mature bot workflows and richer platform conventions | Heavier operational surface; less focused on tiny always-on deployments. |
| OpenClaw | Broader agent/runtime framework | More framework-level extensibility and agent orchestration | More moving parts when the goal is only a persistent chat-to-provider loop. |

`llm-loop` optimizes for a narrow path: receive a chat message, build the right instructions and history, call a provider, run local tools when needed, and reply through the same channel with minimal resident overhead.
