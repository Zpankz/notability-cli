---
name: notability-learn
description: "Use when studying a note: AI summaries, Finish my notes, guided study sessions, quizzes (incl. table-of-contents quizzes), flashcards, Brain Break discussions and syllabus import. Run `notability learn --help` for every flag."
requires_bin: notability
command: notability learn
---

# notability learn

Study tools: summaries, quizzes, flashcards, TOC quizzes, finish notes, study sessions, Brain Break, syllabus.

Use when studying a note: AI summaries, Finish my notes, guided study sessions, quizzes (incl. table-of-contents quizzes), flashcards, Brain Break discussions and syllabus import.

## Commands

- `notability learn materials <note>`: Study-material status and contents.
- `notability learn summary <note>`: AI summary already generated for a note.
- `notability learn wait <note>`: Wait for generation to finish (--timeout). Flags: `--timeout`.
- `notability learn generate <note>`: Generate summary/quizzes/flashcards (spends quota). Flags: `--text`, `--text-file`, `--quizzes`, `--flashcards`, `--language`, `--wait`. Safety: spends AI quota and writes (MCP needs --allow-ai and --allow-writes).
- `notability learn explain <question-id>`: Explain a quiz question. Safety: spends AI quota (MCP needs --allow-ai).
- `notability learn finish <note>`: Finish my notes: AI write-up from the note's text and transcripts, added to the note. Flags: `--use-existing`, `--no-insert`, `--at-start`, `--dry-run`, `--timeout`. Safety: spends AI quota and writes (MCP needs --allow-ai and --allow-writes); changes the note or profile: needs --yes (MCP: confirm=true).
- `notability learn syllabus <file>`: Import a syllabus PDF: AI reads the course and dates; creates a course folder with the PDF. Flags: `--parent`, `--name`, `--no-import`. Safety: spends AI quota and writes (MCP needs --allow-ai and --allow-writes).

## learn cards

Flashcards: due, rate, import.

- `notability learn cards due`: Flashcards due for review. Flags: `--note`.
- `notability learn cards rate <note> <session-id> <question-index> <rating>`: Rate a flashcard. Safety: writes (MCP needs --allow-writes).
- `notability learn cards import <note> <file>`: Import flashcards from a delimited file. Flags: `--sep`. Safety: writes (MCP needs --allow-writes).

## learn discuss

Brain Break: an AI discussion about the note, turn by turn.

- `notability learn discuss start <note>`: Brain Break: start an AI discussion about the note (topics + opening line). Safety: spends AI quota (MCP needs --allow-ai); mapped from the web app, not yet confirmed live.
- `notability learn discuss reply <note>`: Brain Break: next turn (pick --topic, a --choice from the last reply, or --say). Flags: `--topic`, `--choice`, `--say`. Safety: spends AI quota (MCP needs --allow-ai); mapped from the web app, not yet confirmed live.
- `notability learn discuss end <note>`: Brain Break: forget the local discussion state for a note. Safety: local only, CLI only (not over MCP).

## learn quiz

Quiz sessions: list, start, answer, finish.

- `notability learn quiz list`: Quiz sessions (--note --status). Flags: `--note`, `--status`, `--limit`.
- `notability learn quiz start <note>`: Start a quiz session. Flags: `--mode`. Safety: writes (MCP needs --allow-writes).
- `notability learn quiz answer <note> <session-id> <question-index>`: Answer a question. Flags: `--mode`, `--status` (required), `--choice`, `--fill`, `--finish`. Safety: writes (MCP needs --allow-writes).
- `notability learn quiz finish <note> <session-id>`: Finish a session. Flags: `--mode`. Safety: writes (MCP needs --allow-writes).

## learn session

Guided study session: memorize, practice and test stages, graded answers.

- `notability learn session start <note>`: Start or resume a study-session stage; shows the first open question. Flags: `--stage`. Safety: writes (MCP needs --allow-writes).
- `notability learn session next <note>`: Show the stage's next open question and progress. Flags: `--stage`. Safety: writes (MCP needs --allow-writes).
- `notability learn session answer <note>`: Answer the current question; graded like the app, then shows the next one. Flags: `--stage`, `--answer`, `--knew`, `--missed`. Safety: writes (MCP needs --allow-writes).
- `notability learn session end <note>`: Complete the stage's session and show the score and the next stage. Flags: `--stage`. Safety: writes (MCP needs --allow-writes).

## learn toc

Quizzes from chosen sections of a PDF's table of contents.

- `notability learn toc list <note>`: Top-level sections of the note's PDF table of contents.
- `notability learn toc quiz <note>`: Generate study materials from chosen TOC sections (spends one AI job). Flags: `--section`, `--quizzes`, `--flashcards`, `--language`, `--wait`. Safety: spends AI quota and writes (MCP needs --allow-ai and --allow-writes).

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
