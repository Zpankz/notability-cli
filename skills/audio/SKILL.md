---
name: notability-audio
description: "Use when working with recordings: listing and downloading audio, reading transcripts, recording from this machine's microphone, or live-transcribing a recording or file. Run `notability audio --help` for every flag."
requires_bin: notability
command: notability audio
---

# notability audio

Recordings and transcripts.

Use when working with recordings: listing and downloading audio, reading transcripts, recording from this machine's microphone, or live-transcribing a recording or file.

## Commands

- `notability audio list <note>`: Recordings on a note.
- `notability audio transcript <note>`: Transcript text (--timestamps). Flags: `--timestamps`.
- `notability audio upload <note> <file>`: Attach an audio file to a note as a recording. Flags: `--name`, `--start-ms`, `--transcribe`, `--wait`. Safety: writes (MCP needs --allow-writes).
- `notability audio delete <note> <recording-id>`: Delete a recording. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).

## audio live

Live transcription: stream audio and get the transcript and smart notes.

- `notability audio live start <note>`: Live transcription: stream the mic (or a file) and get the transcript and smart notes. Flags: `--input-file`, `--device`, `--seconds`, `--language`, `--realtime`, `--attach`, `--finalize`. Safety: spends AI quota and writes (MCP needs --allow-ai and --allow-writes); uses this machine's microphone; hidden over --http.
- `notability audio live finalize <session-id>`: Close a live-transcription session left open with --no-finalize. Flags: `--language`. Safety: spends AI quota (MCP needs --allow-ai).

## audio record

Record audio on this machine with ffmpeg and attach it to a note.

- `notability audio record start <note>`: Start recording audio for a note on this machine (ffmpeg). Flags: `--input-file`, `--device`. Safety: writes (MCP needs --allow-writes); uses this machine's microphone; hidden over --http.
- `notability audio record stop`: Stop the local recording and attach it to its note. Flags: `--transcribe`, `--keep-file`, `--discard`. Safety: writes (MCP needs --allow-writes); uses this machine's microphone; hidden over --http.
- `notability audio record status`: Whether a local recording is running. Safety: uses this machine's microphone; hidden over --http.

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
