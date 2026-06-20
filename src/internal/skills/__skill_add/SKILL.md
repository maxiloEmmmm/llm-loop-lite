---
name: __skill_add
description: Use when the user wants to save reusable behavior, workflow knowledge, tool experience, or domain guidance as a skill.
---

# __skill_add

Naming:

- Built-in llm-loop skills use the `__` prefix. User-created skills must not use this prefix.
- Global skills are stored at `~/.llm-loop/skills/<skill-name>/SKILL.md`.
- User-scoped skills are stored at `~/.llm-loop/skills/__user/<channel>__<user_id>/<skill-name>/SKILL.md`.

Scope:

- Use a global skill when the user wants the behavior available to all sessions and users.
- Use a user-scoped skill when the behavior is private to the current channel/user identity.
- Ask the user which scope to use before saving a new skill.

Writing requirements:

- Confirm the stable skill name, intended scope, and trigger description before writing.
- Save each skill as an isolated directory containing `SKILL.md`.
- The frontmatter `description` must be short and useful for deciding when to load the skill.
- The body should contain executable constraints, steps, and important context only.
- Do not store one-off information, secrets, tokens, private keys, temporary paths, or short-lived state in a skill.
