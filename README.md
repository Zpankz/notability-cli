# notability-cli

An unofficial command-line client and MCP server for the [Notability](https://notability.com) web app, written in Rust.

> **Unofficial.** Not affiliated with, endorsed by, or sponsored by Notability or Ginger Labs, Inc.
> It talks to an undocumented private web API that can change or break at any time. See
> [Limitations](#limitations) and [Disclaimer](#disclaimer).

## What it is / What it isn't

**It is:**

* A single `notability` binary that reads and edits your own Notability library from a terminal:
  notes, folders, typed text, math, ink, shapes, comments, pages, imports, exports, recordings,
  sharing, Learn (summaries, quizzes, flashcards) and Notability AI chat.
* An MCP server (stdio or HTTP) that exposes the same commands as tools, read-only by default.
* Scriptable: stable exit codes, JSON/YAML/TOON/Markdown/JSONL output, cursors for pagination, and
  dry-run previews for destructive actions.

**It isn't:**

* An official Notability product or a supported API client.
* A replacement for the Notability apps. Several features only exist on Notability's device apps
  (see [Limitations](#limitations)).
* A sync or backup service. It makes requests only when you run a command.
* A way to access anyone else's notes. It uses your own signed-in session and nothing else.

## Problem

Notability has no public API. Notes live in its own apps and web app, so they are hard to search
from a terminal, script, export in bulk, or hand to other tools. `notability-cli` speaks the
same web protocol as `notability.com/app` (GraphQL, a Socket.IO library feed and a FlatBuffers
note edit log), so you can work with your library from a shell, from scripts, or from any MCP
client.

## Features

111 commands under 9 nouns. [`docs/STATUS.md`](docs/STATUS.md) lists every command and whether
it has been verified end to end against the live API (59 have).

| Noun | What you can do |
|---|---|
| `notes` | List, filter, sort and paginate notes; get one note (metadata, pages, recordings, Learn status, or Markdown); search titles and text; build a local full-text index; create, rename, duplicate, move, favourite, switch paged/pageless; trash, restore, purge |
| `folders` | List folders as a list or tree; show display order; create, rename, edit colour/emoji/lock, move, reorder, delete (with `--recursive`); manage subjects and dividers |
| `content` | Show a note's decoded edit log; list and edit typed text (styles, paragraphs, checklists, code blocks, text boxes), LaTeX math blocks, ink strokes (draw, erase), shapes, images (crop, flip), comments (add, reply, edit, move, delete) and pages (add, delete, reorder, paper template); move a selection |
| `files` | Import PDFs, images, audio, `.ntb` bundles and YouTube transcripts; export to Markdown, text, JSON, transcript, `.ntb`, source PDF, assets or a locally rendered PDF; render a page to PNG; read the table of contents |
| `audio` | List and delete recordings; read transcripts (with timestamps); upload an audio file as a recording; record from this machine's microphone with ffmpeg and attach it, optionally transcribed |
| `share` | Show link scope and collaborators; set link access; revoke the link; send email invitations; change or remove a collaborator; answer access requests |
| `learn` | Read AI summaries and study-material status; generate summaries, quizzes and flashcards; wait for generation; run quiz sessions; review and rate due flashcards; import flashcards from a delimited file; explain a question |
| `chat` | Ask Notability AI (optionally about a note); list, read and delete threads; see what context the assistant gets for a note; index recent notes for chat |
| `account` | Sign in, refresh, sign out and check the session; profile, quotas, feature flags, calendar events, settings; read-only raw GraphQL queries |

Notes can be referenced by id, an id prefix of at least 6 characters, an exact title, or
`folder/path/title`.

## Install

You need a Rust toolchain (1.92 or newer; `rust-toolchain.toml` pins it for `rustup` users).
`ffmpeg` is optional and only needed for `audio record` and for importing audio formats other than
m4a, mp4 and webm.

Install from Git:

```sh
cargo install --git https://github.com/Zpankz/notability-cli nb-cli
```

Or build from source:

```sh
git clone https://github.com/Zpankz/notability-cli
cd notability-cli
cargo build --release
./target/release/notability --version
```

The release binary is about 19 MB and starts in a few milliseconds. All dependencies come from
crates.io.

## Authentication

Notability's web app keeps a single session token in an HttpOnly cookie called `nb-auth-token`.
The CLI never asks for your password and never takes the token as a command-line argument. There
are three ways to give it a session:

1. **From a signed-in browser over CDP.** Sign in at `https://notability.com/app` in a
   Chromium-based browser started with remote debugging, then run:

   ```sh
   # e.g. chromium --remote-debugging-port=9222
   notability account login --cdp http://127.0.0.1:9222
   ```

   The CLI connects to the browser's DevTools endpoint and calls `Storage.getCookies` once to read
   the `nb-auth-token` cookie. It does not open, attach to or script any page. The endpoint
   defaults to `$NOTABILITY_CDP`, or `http://127.0.0.1:9222`.

2. **From stdin**, if you copied the cookie value yourself (for example from the browser's
   developer tools):

   ```sh
   notability account login --token-stdin < token.txt
   ```

3. **From the environment:** `NOTABILITY_TOKEN` overrides any stored session and is never written
   to disk.

`account login` checks the token against the API before saving it. It is stored in
`credentials.json` inside `$NOTABILITY_CONFIG_DIR`, or `$XDG_CONFIG_HOME/notability-cli`, or
`~/.config/notability-cli`. The directory is created with mode `0700` and the file is written
with mode `0600`. A random device id used by the note protocol is kept next to it in
`settings.json`.

```sh
notability account status            # validates the session against the API
notability account status --offline  # inspect the stored token without a request
notability account refresh           # re-import the cookie from the browser
notability account logout            # delete the stored credentials
```

The token is shown redacted in all output.

## Usage

```sh
notability --help                      # nouns
notability notes --help                # commands under a noun
notability notes list --limit 20
notability notes search "roadmap" --format json
notability notes get "Projects/Kickoff" --format md
notability files export "Projects/Kickoff" --to md --to pdf --out ./export
notability content text add "Projects/Kickoff" "A new paragraph"
notability notes delete "Old draft" --yes
```

**Output formats.** `--format toon|json|yaml|md|jsonl` (TOON is the default).
`--json` prints the full `{ok, data, meta}` envelope. `--filter-output a,b.c` keeps only some keys,
and `--token-limit` / `--token-offset` page through large outputs. List commands return
`meta.next_cursor`; pass it back with `--cursor`.

**Destructive commands** (`notes delete`, `folders delete`, `notes trash empty`, …) ask for
confirmation in an interactive terminal and need `--yes` in scripts; without it they exit 2 with
`CONFIRM_REQUIRED`.

**Shell completions:**

```sh
eval "$(notability completions bash)"   # or zsh, fish, nushell
```

**Agent skills and manifests.** `notability --llms` prints a compact manifest of every command
(`--llms-full` for the full schemas), and `notability <command> --schema` prints one command's JSON
Schema. `notability skills add` writes skill files describing the CLI for supported coding agents,
and `notability mcp add` registers the MCP server with them.

**Environment variables:**

| Variable | Purpose |
|---|---|
| `NOTABILITY_TOKEN` | Session token (overrides stored credentials) |
| `NOTABILITY_CDP` | Default CDP endpoint for `account login` / `refresh` |
| `NOTABILITY_CONFIG_DIR` | Where `credentials.json` and `settings.json` live |
| `NOTABILITY_CACHE_DIR` | Local full-text index cache |
| `NOTABILITY_ALLOW_WRITES`, `NOTABILITY_ALLOW_AI` | Same as `--allow-writes` / `--allow-ai` for the MCP server |
| `NOTABILITY_API_BASE`, `NOTABILITY_COLLAB_BASE` | Override the API endpoints (testing) |

## MCP server

Every command is also an MCP tool, named after its path with `_` (for example `notes_list`,
`content_math_add`). Three resources are exposed as well: `notability://folders`,
`notability://notes/recent` and `notability://note/{note_id}` (a note as Markdown).

**stdio** (the usual form for desktop MCP clients):

```json
{
  "mcpServers": {
    "notability": {
      "command": "notability",
      "args": ["--mcp"]
    }
  }
}
```

**HTTP** (streamable HTTP on `/mcp`):

```sh
notability --mcp --http 127.0.0.1:8765
```

```json
{
  "mcpServers": {
    "notability": { "url": "http://127.0.0.1:8765/mcp" }
  }
}
```

The HTTP server has no authentication of its own and acts with your Notability session, so bind
it to `127.0.0.1` only. Tools that use this machine's devices (audio recording) are hidden over
HTTP.

**Progressive disclosure.** By default the server lists only 8 tools: 4 meta tools
(`search_tools`, `get_tool_details`, `call_read_tool`, `call_write_tool`) and 4 starters
(`account_me`, `notes_get`, `notes_list`, `notes_search`). The client finds the rest with
`search_tools`. Pass `--mcp-flat` to list every reachable tool directly, or
`--mcp-tools 'folders_*,notes_list'` to restrict the set.

**Gates.** The server is read-only unless you opt in:

| Flags | Reachable tools |
|---|---|
| (none) | 41 read-only tools |
| `--allow-writes` | 103 (adds create, edit, move, delete, import and sharing changes) |
| `--allow-ai` | 43 (adds AI reads that spend quota) |
| `--allow-writes --allow-ai` | 106 |

Local-only commands (`account login`, `logout`, `refresh`), the raw `account graphql` query and
`share invite` (which emails people) are never exposed over MCP.

**Dry-run confirm.** A destructive tool called without `confirm: true` does nothing and returns a
preview (`{"dry_run": true, "command": …, "args": …}`) so the client can check with you first.
Calling it again with `confirm: true` applies it.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | generic error |
| 2 | usage: bad flag or value, unknown command, bad cursor, missing `--yes` |
| 3 | auth: not signed in or session expired |
| 4 | not found |
| 5 | rate limited |
| 6 | server error |
| 7 | quota exceeded |
| 8 | unsupported by the API |
| 9 | write rejected by the server |

More detail on envelopes and output is in [`docs/OUTPUT.md`](docs/OUTPUT.md).

## Limitations

* **Private, undocumented API.** Notability does not publish or support this API. Everything here
  is based on how the web app behaves today, and any web release can change or break it without
  notice. [`docs/PROTOCOL.md`](docs/PROTOCOL.md) describes what is known.
* **Rate limiting.** The client waits at least 250 ms between requests and retries transport
  errors, HTTP 429 and 5xx with exponential backoff, honouring `Retry-After`. Exit code 5 means
  the server is still rate limiting you. Please don't use it for bulk scraping.
* **Quotas.** Transcription, Learn generation and AI chat spend the same quotas as the apps.
* **Device-only features.** Tables, tags, handwriting-to-text and handwritten math recognition
  exist only in Notability's device apps; the web API has nothing to call for them. Handwriting is
  available as vector ink, not text.
* **Not implemented:** table-of-contents quiz generation (`learn toc` / `generate --toc`), live
  transcription, and importing Office files, Google Drive files or PDFs by URL.
* **Rendered PDF export is a local approximation.** The web app renders "Export as PDF" in the
  browser; `files export --to pdf` and `files render` draw source PDFs, paper, ink, shapes, images
  and typed text locally, with simplified text styling and pen textures.

## Disclaimer

This project is for educational and non-commercial personal use only. It is not affiliated with,
endorsed by, or sponsored by Notability or Ginger Labs, Inc. Notability is a trademark of its
owner. Use it at your own risk and in accordance with Notability's terms of service, and only to
access your own account.

## License

[MIT](LICENSE). Copyright 2026 Zpankz.
