---
name: notability-chat
description: "Use when asking Notability AI questions about notes or reading and managing chat threads. Run `notability chat --help` for every flag."
requires_bin: notability
command: notability chat
---

# notability chat

Notability AI chat threads about your notes.

Use when asking Notability AI questions about notes or reading and managing chat threads.

## Commands

- `notability chat threads`: List chat threads. Flags: `--limit`.
- `notability chat read <thread-id>`: Read a thread.
- `notability chat ask <question>`: Ask Notability AI (optionally about a note). Flags: `--thread`, `--note`, `--delete-after`. Safety: spends AI quota (MCP needs --allow-ai).
- `notability chat context <note>`: What the assistant's note_read tool receives for a note (typed, PDF and transcript text).
- `notability chat delete <thread-id>`: Delete a thread. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).
- `notability chat backfill`: Index recently edited notes for AI chat. Safety: writes (MCP needs --allow-writes).

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
