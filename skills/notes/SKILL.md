---
name: notability-notes
description: "Use when you need to find a note (list, search, recent, by folder), open its details, create, rename, move, duplicate, tag or delete notes, or restore something from the trash. Run `notability notes --help` for every flag."
requires_bin: notability
command: notability notes
---

# notability notes

Find, open and manage notes in your library (incl. trash).

Use when you need to find a note (list, search, recent, by folder), open its details, create, rename, move, duplicate, tag or delete notes, or restore something from the trash.

## Commands

- `notability notes list`: List notes (filter by folder, type, favourites; sort; paginate). Flags: `--folder`, `--sort`, `--asc`, `--favorites`, `--type`, `--limit`.
- `notability notes search <query>`: Search titles, optionally typed/handwritten text. Flags: `--content`, `--content-limit`, `--limit`.
- `notability notes get <note>`: One note: metadata, pages, recordings, Learn status; --format md for markdown. Flags: `--learn`, `--as`.
- `notability notes index`: Build/refresh the local full-text index (cache only). Flags: `--limit`, `--refresh`, `--clear`.
- `notability notes create <title>`: Create an empty, seeded note. Flags: `--folder`. Safety: writes (MCP needs --allow-writes).
- `notability notes rename <note> <new-title>`: Rename a note. Safety: writes (MCP needs --allow-writes).
- `notability notes duplicate <note>`: Copy a note. Flags: `--title`. Safety: writes (MCP needs --allow-writes).
- `notability notes move <note> [folder]`: Move a note to a folder (or unfiled). Safety: writes (MCP needs --allow-writes).
- `notability notes favorite <note>`: Star or unstar a note (--off). Flags: `--off`. Safety: writes (MCP needs --allow-writes).
- `notability notes layout <note> <mode>`: Switch paged / pageless. Safety: writes (MCP needs --allow-writes).
- `notability notes delete <note>`: Move to trash, or --permanent. Flags: `--permanent`. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).

## notes trash

Deleted notes: list, restore, purge, empty.

- `notability notes trash list`: List trashed notes. Flags: `--match`, `--limit`.
- `notability notes trash restore <note>`: Restore a trashed note. Safety: writes (MCP needs --allow-writes).
- `notability notes trash purge <note>`: Permanently delete one trashed note. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).
- `notability notes trash empty`: Purge trash (--match/--all, --dry-run). Flags: `--match`, `--all`, `--dry-run`. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
