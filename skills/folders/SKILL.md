---
name: notability-folders
description: "Use when organising the library into folders, subjects and dividers: listing the tree, creating, renaming, colouring, moving or deleting folders. Run `notability folders --help` for every flag."
requires_bin: notability
command: notability folders
---

# notability folders

Organise the library: folders, subjects and dividers.

Use when organising the library into folders, subjects and dividers: listing the tree, creating, renaming, colouring, moving or deleting folders.

## Commands

- `notability folders list`: List folders (--parent, --tree). Flags: `--parent`, `--tree`, `--limit`.
- `notability folders order [parent]`: Children of a folder in display order.
- `notability folders create <title>`: Create a folder. Flags: `--parent`, `--color`. Safety: writes (MCP needs --allow-writes).
- `notability folders rename <folder> <new-title>`: Rename a folder. Safety: writes (MCP needs --allow-writes).
- `notability folders edit <folder>`: Edit title, colour, emoji, lock. Flags: `--title`, `--color`, `--emoji`, `--no-emoji`, `--lock`, `--unlock`. Safety: writes (MCP needs --allow-writes).
- `notability folders move <folder> [parent]`: Re-parent a folder. Flags: `--root`, `--position`, `--anchor`. Safety: writes (MCP needs --allow-writes).
- `notability folders reorder <folder>`: Change position among siblings. Flags: `--position` (required), `--anchor`. Safety: writes (MCP needs --allow-writes).
- `notability folders delete <folder>`: Delete a folder (--recursive). Flags: `--recursive`. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).

## folders organizers

Legacy subjects and dividers.

- `notability folders organizers list`: List subjects/dividers. Flags: `--type`.
- `notability folders organizers create <kind> <title>`: Create a subject or divider. Flags: `--parent`, `--color`. Safety: writes (MCP needs --allow-writes).
- `notability folders organizers edit <organizer>`: Edit a subject or divider. Flags: `--title`, `--color`, `--emoji`, `--lock`, `--unlock`. Safety: writes (MCP needs --allow-writes).
- `notability folders organizers delete <organizer>`: Delete a subject or divider. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
