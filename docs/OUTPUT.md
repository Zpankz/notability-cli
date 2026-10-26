# Output, envelopes and exit codes

The CLI is built on [incurs](https://crates.io/crates/incurs) 0.14 (features `http`,
`agent-plugins`), which provides the command parser, output formats (TOON, JSON, YAML, Markdown,
JSONL), `--llms` manifests, shell completions, skill files and the MCP server.

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

Codes 3 to 9 come from `nb_api::Kind::exit_code` and are set on the command error. Usage errors
raised by the argument parser itself are mapped from 1 to 2 by `envelope::adjust`.

## Envelopes

* `--json` prints the full envelope (`{ok, data, meta}` or `{ok: false, error}`), the same as
  `--format json --full-output`.
* List commands put the pagination cursor in `meta.next_cursor`; pass it back with `--cursor`.
  Over MCP, `next_cursor` is part of the tool result.
* Destructive commands need `--yes`. Without it, an interactive terminal is asked to confirm;
  scripted runs (no TTY, or `--format json`) fail with `CONFIRM_REQUIRED` and exit code 2.

## Notes

* **Buffered output.** To apply the exit-code mapping, output is written when the command ends
  rather than streamed.
* **In-process use.** The exit-code mapping and the `next_cursor` lift happen in the binary's
  `main`. Code that embeds the `Cli` directly sees plain incurs behaviour (argv errors exit 1,
  `next_cursor` stays in `data`).
* **Unknown commands under `--json`** print `COMMAND_NOT_FOUND` as a bare error object rather than
  an `{ok: false}` envelope. The exit code is still 2.
