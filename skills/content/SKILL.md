---
name: notability-content
description: "Use when reading or editing what is inside a note: typed text, headings, checklists, code, LaTeX math, handwriting, shapes, images, comments and pages. Run `notability content --help` for every flag."
requires_bin: notability
command: notability content
---

# notability content

Read and edit what is inside a note: text, math, code, ink, shapes, comments, pages.

Use when reading or editing what is inside a note: typed text, headings, checklists, code, LaTeX math, handwriting, shapes, images, comments and pages.

## Commands

- `notability content show <note>`: Decoded edit log: pages, text boxes, math, code, ink, shapes, comments, images.
- `notability content move <note> <ids>`: Move a selection of ink, shapes, images or blocks. Flags: `--dx`, `--dy`, `--to-page`. Safety: writes (MCP needs --allow-writes).

## content comments

Page comments and replies.

- `notability content comments list <note>`: List comments and replies.
- `notability content comments add <note> <content>`: Comment on a page (or reply). Flags: `--page`, `--x`, `--y`, `--reply-to`. Safety: writes (MCP needs --allow-writes).
- `notability content comments edit <note> <comment-id> <content>`: Change a comment's text. Safety: writes (MCP needs --allow-writes).
- `notability content comments move <note> <comment-id>`: Move a comment pin. Flags: `--page`, `--x` (required), `--y` (required). Safety: writes (MCP needs --allow-writes).
- `notability content comments delete <note> <comment-id>`: Delete a comment. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).

## content images

Images placed on pages.

- `notability content images list <note>`: List images with ids, page and frame.
- `notability content images crop <note> <image-id>`: Crop an image to a rectangle of its own frame. Flags: `--x` (required), `--y` (required), `--width` (required), `--height` (required). Safety: writes (MCP needs --allow-writes).
- `notability content images flip <note> <image-id>`: Flip an image horizontally and/or vertically. Flags: `--horizontal`, `--vertical`. Safety: writes (MCP needs --allow-writes).

## content ink

Handwriting: pen and highlighter strokes, eraser.

- `notability content ink list <note>`: List ink strokes with ids, tool and bounds.
- `notability content ink draw <note>`: Draw a pen or highlighter stroke through points. Flags: `--points` (required), `--page`, `--highlighter`, `--color`, `--width`. Safety: writes (MCP needs --allow-writes).
- `notability content ink erase <note> <ids>`: Erase ink strokes or shapes by id (the eraser). Safety: writes (MCP needs --allow-writes).

## content math

Math blocks (LaTeX).

- `notability content math list <note>`: List math blocks with their LaTeX.
- `notability content math add <note> <latex>`: Insert a LaTeX math block. Flags: `--page`, `--x`, `--y`, `--color`. Safety: writes (MCP needs --allow-writes).
- `notability content math edit <note> <math-id>`: Change a math block's LaTeX or colour. Flags: `--latex`, `--color`. Safety: writes (MCP needs --allow-writes).

## content pages

Pages: list, add, delete, reorder, paper templates.

- `notability content pages list <note>`: List pages with size and template.
- `notability content pages add <note>`: Insert blank pages. Flags: `--after`, `--count`, `--template`, `--spacing`, `--color`. Safety: writes (MCP needs --allow-writes).
- `notability content pages delete <note> <page>`: Delete a page. Safety: destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes).
- `notability content pages reorder <note> <page> <to>`: Move a page to a new index. Safety: writes (MCP needs --allow-writes).
- `notability content pages template <note> <template>`: Change paper template, spacing, colour. Flags: `--page`, `--spacing`, `--color`. Safety: writes (MCP needs --allow-writes).
- `notability content pages templates`: List known paper templates.

## content shapes

Shapes: rectangle, ellipse, triangle, arrow, line.

- `notability content shapes list <note>`: List shapes with ids, kind and bounds.
- `notability content shapes add <note> <kind>`: Draw a rectangle, ellipse, triangle, arrow or line. Flags: `--page`, `--x`, `--y`, `--width`, `--height`, `--color`, `--border`. Safety: writes (MCP needs --allow-writes).

## content text

Typed text and text boxes: styles, paragraphs, checklists, code blocks.

- `notability content text list <note>`: List text and text boxes with ids and styles.
- `notability content text add <note> <content>`: Add typed text or a text box (--code LANG for a code block). Flags: `--box`, `--at-start`, `--page`, `--x`, `--y`, `--width`, `--code`. Safety: writes (MCP needs --allow-writes).
- `notability content text style <note>`: Style a text range: bold/italic/underline/strike/super/subscript/inline code (unverified)/highlight/colour/font/size/heading/link/clear. Flags: `--field`, `--match`, `--nth`, `--range`, `--paragraphs`, `--bold`, `--italic`, `--underline`, `--strike`, `--superscript`, `--subscript`, `--code`, `--highlight`, `--color`, `--font`, `--size`, `--heading`, `--link`, `--clear`, `--clear-paragraph`. Safety: writes (MCP needs --allow-writes).
- `notability content text paragraph <note>`: Paragraph style: list kind, alignment, indent, line spacing, code language. Flags: `--field`, `--match`, `--nth`, `--range`, `--paragraphs`, `--list`, `--align`, `--indent`, `--spacing`, `--language`. Safety: writes (MCP needs --allow-writes).
- `notability content text check <note> <paragraph>`: Tick or untick a checklist item. Flags: `--field`, `--off`. Safety: writes (MCP needs --allow-writes).

## Conventions

- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.
