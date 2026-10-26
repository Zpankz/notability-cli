---
name: notability-files
description: "Use when moving files in or out of Notability: importing PDF, Office, images, audio, .ntb, web PDFs, Google Drive files or YouTube transcripts, exporting PDF/markdown/text, reading a PDF outline or rendering page previews. Run `notability files --help` for every flag."
requires_bin: notability
command: notability files
---

# notability files

Import and export: PDF, images, audio, .ntb, markdown, text, JSON, transcripts; page previews.

Use when moving files in or out of Notability: importing PDF, Office, images, audio, .ntb, web PDFs, Google Drive files or YouTube transcripts, exporting PDF/markdown/text, reading a PDF outline or rendering page previews.

## Commands

- `notability files export <notes>`: Export notes: md, txt, json, transcript, pdf (local render), ntb, source-pdf, assets, thumbnail. Flags: `--to`, `--out`, `--thumbnail`, `--label`, `--stdout`.
- `notability files render <note>`: Render one page of a note to PNG (local render: source PDF or paper, ink, shapes, text, images). Flags: `--page`, `--scale`, `--label`, `--out`.
- `notability files toc <note>`: Table of contents: pages (with their source PDF page) and each imported PDF's outline.

## files import

Bring files into the library: PDF, Office, web links, Google Drive, images, audio, .ntb, YouTube.

- `notability files import pdf <file>`: Import a PDF as a new note, or insert its pages into a note. Flags: `--into`, `--after-page`, `--title`, `--folder`, `--wait`. Safety: writes (MCP needs --allow-writes).
- `notability files import image <file>`: Import an image as a new note, or place it on a note's page. Flags: `--into`, `--page`, `--x`, `--y`, `--width`, `--title`, `--folder`, `--wait`. Safety: writes (MCP needs --allow-writes).
- `notability files import media <file>`: Import any supported file: PDF pages, an image, or audio as a recording. Flags: `--into`, `--page`, `--x`, `--y`, `--width`, `--after-page`, `--name`, `--transcribe`, `--title`, `--folder`, `--wait`. Safety: writes (MCP needs --allow-writes).
- `notability files import office <file>`: Import a Word, Excel or PowerPoint file (converted to PDF with LibreOffice). Flags: `--into`, `--after-page`, `--title`, `--folder`, `--wait`. Safety: writes (MCP needs --allow-writes).
- `notability files import url <url>`: Import a PDF from a web link (fetched without your Notability credentials). Flags: `--into`, `--after-page`, `--title`, `--folder`, `--wait`. Safety: writes (MCP needs --allow-writes).
- `notability files import drive <file>`: Import a Google Drive file: PDF, Office, or Google Docs/Sheets/Slides (as PDF). Flags: `--token-env`, `--into`, `--after-page`, `--title`, `--folder`, `--wait`. Safety: writes (MCP needs --allow-writes).
- `notability files import ntb <file>`: Import a Notability .ntb bundle as a new note. Flags: `--title`, `--folder`. Safety: writes (MCP needs --allow-writes).
- `notability files import youtube <url>`: Import a YouTube video's link and transcript as a new note, or into a note. Flags: `--into`, `--title`, `--folder`, `--lang`. Safety: writes (MCP needs --allow-writes).

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
