---
name: notability
description: "Notability from the terminal or MCP: notes, folders, note content, files, audio, sharing, Learn study tools, AI chat and account. Use when the user mentions Notability or their notes there; this routes to the per-area notability-* skills."
requires_bin: notability
command: notability
---

# notability

Pick the area, then load its skill (or run `notability <area> --help`).

## Areas

- `notability notes` (skill `notability-notes`): Find, open and manage notes in your library (incl. trash). Use when you need to find a note (list, search, recent, by folder), open its details, create, rename, move, duplicate, tag or delete notes, or restore something from the trash.
- `notability folders` (skill `notability-folders`): Organise the library: folders, subjects and dividers. Use when organising the library into folders, subjects and dividers: listing the tree, creating, renaming, colouring, moving or deleting folders.
- `notability content` (skill `notability-content`): Read and edit what is inside a note: text, math, code, ink, shapes, comments, pages. Use when reading or editing what is inside a note: typed text, headings, checklists, code, LaTeX math, handwriting, shapes, images, comments and pages.
- `notability files` (skill `notability-files`): Import and export: PDF, images, audio, .ntb, markdown, text, JSON, transcripts; page previews. Use when moving files in or out of Notability: importing PDF, Office, images, audio, .ntb, web PDFs, Google Drive files or YouTube transcripts, exporting PDF/markdown/text, reading a PDF outline or rendering page previews.
- `notability audio` (skill `notability-audio`): Recordings and transcripts. Use when working with recordings: listing and downloading audio, reading transcripts, recording from this machine's microphone, or live-transcribing a recording or file.
- `notability share` (skill `notability-share`): Who can see a note: links, collaborators, access requests. Use when checking or changing who can see a note: share links, collaborators, access requests and finding invitable people in a business workspace.
- `notability learn` (skill `notability-learn`): Study tools: summaries, quizzes, flashcards, TOC quizzes, finish notes, study sessions, Brain Break, syllabus. Use when studying a note: AI summaries, Finish my notes, guided study sessions, quizzes (incl. table-of-contents quizzes), flashcards, Brain Break discussions and syllabus import.
- `notability chat` (skill `notability-chat`): Notability AI chat threads about your notes. Use when asking Notability AI questions about notes or reading and managing chat threads.
- `notability account` (skill `notability-account`): Sign-in session, profile, quotas, settings, calendar, raw GraphQL. Use when checking the sign-in session, profile, quotas, survey and use type, settings, calendar connections, or running a raw read-only GraphQL query.

## First steps

- `notability account login` signs in (stores the session locally); `notability account status` checks it.
- `notability notes list`, `notability notes search <query>` and `notability notes get <note>` find and open notes.
- `notability --mcp` serves the same commands as MCP tools (read-only unless `--allow-writes` / `--allow-ai`).
- `notability skills add` installs these skills for your coding agents; `notability skills list` shows them.

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
