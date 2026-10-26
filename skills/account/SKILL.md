---
name: notability-account
description: "Use when checking the sign-in session, profile, quotas, survey and use type, settings, calendar connections, or running a raw read-only GraphQL query. Run `notability account --help` for every flag."
requires_bin: notability
command: notability account
---

# notability account

Sign-in session, profile, quotas, settings, calendar, raw GraphQL.

Use when checking the sign-in session, profile, quotas, survey and use type, settings, calendar connections, or running a raw read-only GraphQL query.

## Commands

- `notability account login`: Import a session (from a browser over CDP, or a token on stdin). Flags: `--cdp`, `--token-stdin`. Safety: local only, CLI only (not over MCP).
- `notability account refresh`: Re-import the session cookie. Flags: `--cdp`. Safety: local only, CLI only (not over MCP).
- `notability account logout`: Forget the stored session. Safety: local only, CLI only (not over MCP).
- `notability account status`: Is the session valid? (--offline). Flags: `--offline`.
- `notability account me`: Profile of the signed-in user.
- `notability account quotas`: AI / transcription quotas.
- `notability account flags`: Feature flags.
- `notability account stickers`: Sticker packs.
- `notability account survey`: Onboarding survey answers.
- `notability account survey-set <field>`: Set onboarding survey answers (changes your profile). Safety: writes (MCP needs --allow-writes); changes the note or profile: needs --yes (MCP: confirm=true).
- `notability account survey-remove <key>`: Remove onboarding survey answers by key (changes your profile). Safety: writes (MCP needs --allow-writes); changes the note or profile: needs --yes (MCP: confirm=true); mapped from the web app, not yet confirmed live.
- `notability account use-type <use-type>`: Set how you use Notability: student, educator, business or personal. Safety: writes (MCP needs --allow-writes); changes the note or profile: needs --yes (MCP: confirm=true).
- `notability account calendar`: Upcoming calendar events. Flags: `--days`, `--limit`.
- `notability account graphql <query>`: Raw GraphQL query (queries only). Flags: `--vars`. Safety: CLI only.

## account settings

Account settings and toggles.

- `notability account settings get`: Show settings.
- `notability account settings set <key> <value>`: Set a setting value (stored as a string). Safety: writes (MCP needs --allow-writes).
- `notability account settings toggle <name> <state>`: Turn a boolean setting on/off. Safety: writes (MCP needs --allow-writes).
- `notability account settings name <full-name>`: Change display name. Safety: writes (MCP needs --allow-writes).

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
