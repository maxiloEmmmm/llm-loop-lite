---
name: __mem
description: Use when the user explicitly asks to remember, forget, update, organize, or list long-term memories.
---

# __mem

Storage:

- Store global memories under `~/.llm-loop/mems/`.
- Store user memories under `~/.llm-loop/mems/__user/<user_key>/`.
- Use user-scoped memories by default.
- Use global memories only when the user explicitly asks for global, shared, or all-user memory.
- Each memory is one standalone `.md` file.
- The filename stem is the memory key.
- Keys may contain only ASCII letters and digits.
- Keys must not contain spaces, hyphens, underscores, or other symbols.
- The current `user_key`, `user_id`, global directory, and user directory are provided in the `Memories` section of the initial prompt.

File format:

```md
---
user: <creator>
updated_at: <last-modified-time>
---

<memory body>
```

Operations:

- Add: choose a stable key first, then write `<key>.md` under the current user's memory directory unless the user explicitly requested global memory.
- Update: edit only the file for the requested key in the requested scope. Default to the current user's scope.
- Delete: remove only the key the user explicitly asks to delete. Default to the current user's scope.
- List: show current user memory keys and short summaries by default. Include global memories only when the user asks for global/shared/all memories.
- `user` records the creator. Use the current `user_id` from the `Memories` section. Never write placeholder values such as `user`, `unknown`, or `xxx` when a real `user_id` is available.
- `updated_at` records the last modified time in ISO-8601 or another clear local-time format.
- Store only stable facts, preferences, long-term constraints, and information the user explicitly wants preserved.
- Do not store passwords, tokens, private keys, one-time codes, temporary deployment addresses, or short-lived state.
- Ensure the target memory directory exists before writing memory files.

Injection:

- New sessions automatically load all valid memory files.
- New sessions load global memories and the current user's memories.
- Only the memory body is injected into the system prompt.
- The YAML frontmatter is never injected.
