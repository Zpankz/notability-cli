---
name: notability-share
description: "Use when checking or changing who can see a note: share links, collaborators, access requests and finding invitable people in a business workspace. Run `notability share --help` for every flag."
requires_bin: notability
command: notability share
---

# notability share

Who can see a note: links, collaborators, access requests.

Use when checking or changing who can see a note: share links, collaborators, access requests and finding invitable people in a business workspace.

## Commands

- `notability share status <note>`: Link scope, access and collaborators.
- `notability share collaborators <note>`: Collaborators on a note.
- `notability share link <note>`: Set link scope/access (widens exposure). Flags: `--scope` (required), `--access`. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).
- `notability share revoke <note>`: Turn the share link off. Safety: writes (MCP needs --allow-writes).
- `notability share user <note> <user-id>`: Change or remove a collaborator's access. Flags: `--access`, `--remove`. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).
- `notability share answer <request-id> <decision>`: Grant or deny an access request. Flags: `--access`. Safety: writes (MCP needs --allow-writes).
- `notability share search-users <query>`: Search people in your business who can be invited (business accounts). Flags: `--note`, `--workspace`, `--limit`.
- `notability share invite <note> <emails>`: Email invitations (Notability emails each invitee). Flags: `--access`, `--yes`. Safety: writes (MCP needs --allow-writes); CLI only.

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
