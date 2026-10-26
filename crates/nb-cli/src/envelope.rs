//! Post-processing of incurs output that the stock crate does not configure: argv
//! validation errors exit 2 (usage), and `--json` envelopes carry `next_cursor` in `meta`.

use std::io::{IsTerminal, Write};

use incurs::cli::Cli;
use incurs::formatter::format;
use incurs::output::Format;
use serde_json::Value;

/// Exit code for usage errors (bad flags or values).
pub const USAGE_EXIT: i32 = 2;

/// The error codes incurs reports, with exit 1, for argv that fails validation or names
/// no command.
const USAGE_CODES: [&str; 2] = ["VALIDATION_ERROR", "COMMAND_NOT_FOUND"];

/// Runs `argv` through `cli`, writes the adjusted output to stdout and returns the exit code.
pub async fn run(cli: &Cli, argv: Vec<String>) -> Result<i32, Box<dyn std::error::Error>> {
    let human = std::io::stdout().is_terminal();
    let mut buf = Vec::new();
    let code = cli.serve_to(argv, &mut buf, human).await?;
    let (out, code) = adjust(buf, code);
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&out)?;
    stdout.flush()?;
    Ok(code.unwrap_or(0))
}

/// Applies the exit-code mapping and the meta lift to one captured run.
pub fn adjust(buf: Vec<u8>, code: Option<i32>) -> (Vec<u8>, Option<i32>) {
    let text = String::from_utf8_lossy(&buf);
    let parsed: Option<Value> = serde_json::from_str(text.trim()).ok();
    let code = match code {
        Some(1) if is_argv_error(&text, parsed.as_ref()) => Some(USAGE_EXIT),
        c => c,
    };
    match parsed.and_then(lift_cursor) {
        Some(v) => (format!("{}\n", format(&v, Format::Json)).into_bytes(), code),
        None => (buf, code),
    }
}

fn is_argv_error(text: &str, parsed: Option<&Value>) -> bool {
    match parsed {
        Some(v) => USAGE_CODES
            .iter()
            .any(|c| v["error"]["code"] == *c || v["code"] == *c),
        // Human, TOON, YAML or Markdown renderings of the same error.
        None => text.lines().map(str::trim).any(|l| {
            USAGE_CODES.iter().any(|c| {
                l.starts_with(&format!("Error ({c}):"))
                    || l.trim_start_matches(['-', '|', ' '])
                        .starts_with(&format!("code: {c}"))
                    || l.split('|')
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .windows(2)
                        .any(|w| w == ["code", c])
            })
        }),
    }
}

/// Moves `data.next_cursor` into `meta` in a full-output envelope (dropped when null).
fn lift_cursor(mut v: Value) -> Option<Value> {
    let meta_present = v.get("meta").is_some_and(Value::is_object);
    let cursor = v.get_mut("data")?.as_object_mut()?.remove("next_cursor")?;
    if !meta_present {
        return None;
    }
    if !cursor.is_null() {
        v["meta"]["next_cursor"] = cursor;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run_adjust(v: &Value, code: Option<i32>) -> (Value, Option<i32>) {
        let (out, code) = adjust(v.to_string().into_bytes(), code);
        (serde_json::from_slice(&out).unwrap(), code)
    }

    #[test]
    fn cursor_moves_into_meta() {
        let env = json!({"ok": true, "data": {"items": [], "next_cursor": "c1"}, "meta": {"command": "x"}});
        let (v, code) = run_adjust(&env, None);
        assert_eq!(v["meta"]["next_cursor"], "c1");
        assert!(v["data"].get("next_cursor").is_none());
        assert_eq!(code, None);
    }

    #[test]
    fn null_cursor_is_dropped() {
        let env = json!({"ok": true, "data": {"items": [], "next_cursor": null}, "meta": {}});
        let (v, _) = run_adjust(&env, None);
        assert!(v["meta"].get("next_cursor").is_none());
        assert!(v["data"].get("next_cursor").is_none());
    }

    #[test]
    fn validation_errors_exit_2() {
        let env = json!({"ok": false, "error": {"code": "VALIDATION_ERROR", "message": "m"}});
        assert_eq!(run_adjust(&env, Some(1)).1, Some(2));
        let (_, c) = adjust(
            b"Error (VALIDATION_ERROR): Unknown flag: --x\n".to_vec(),
            Some(1),
        );
        assert_eq!(c, Some(2));
        let (_, c) = adjust(b"code: VALIDATION_ERROR\nmessage: m\n".to_vec(), Some(1));
        assert_eq!(c, Some(2));
        let (_, c) = adjust(b"code: COMMAND_NOT_FOUND\nmessage: m\n".to_vec(), Some(1));
        assert_eq!(c, Some(2));
        let env = json!({"code": "COMMAND_NOT_FOUND", "message": "m"});
        assert_eq!(run_adjust(&env, Some(1)).1, Some(2));
    }

    #[test]
    fn other_errors_keep_their_code() {
        let env = json!({"ok": false, "error": {"code": "AUTH", "message": "m"}});
        assert_eq!(run_adjust(&env, Some(3)).1, Some(3));
        let env = json!({"ok": false, "error": {"code": "ERROR", "message": "m"}});
        assert_eq!(run_adjust(&env, Some(1)).1, Some(1));
    }
}
