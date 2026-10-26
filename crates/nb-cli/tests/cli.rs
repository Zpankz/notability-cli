//! Command tree, help, envelope, and exit-code contract.

mod common;

use std::collections::BTreeSet;

use common::{argv, bin, out, run};
use nb_cli::prescan;
use serde_json::Value;

const NOUNS: [&str; 9] = [
    "notes", "folders", "content", "files", "audio", "share", "learn", "chat", "account",
];

#[tokio::test]
async fn help_per_noun() {
    let (root, code) = run(&["--help"]).await;
    assert_eq!(code, 0);
    insta::assert_snapshot!("root", root);
    for noun in NOUNS {
        let (help, code) = run(&[noun, "--help"]).await;
        assert_eq!(code, 0, "{noun}");
        insta::assert_snapshot!(noun, help);
    }
}

/// The shipped command tree is a contract: adding, renaming or removing a command must update
/// `tests/data/commands.txt`.
#[tokio::test]
async fn command_tree() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/commands.txt");
    let want: BTreeSet<String> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    let (manifest, code) = run(&["--llms", "--format", "json"]).await;
    assert_eq!(code, 0);
    let manifest: Value = serde_json::from_str(&manifest).unwrap();
    let ours: BTreeSet<String> = manifest["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ours, want);
}

#[test]
fn prescan_strips_mcp_switches() {
    let (gate, rest) = prescan(argv(&[
        "--mcp",
        "--allow-writes",
        "--mcp-tools=notes_*,folders_list",
        "--mcp-flat",
        "--http",
        "127.0.0.1:8765",
    ]))
    .unwrap();
    assert!(gate.allow_writes && gate.flat && !gate.allow_ai);
    assert_eq!(gate.include, ["notes_*", "folders_list"]);
    assert_eq!(gate.http.unwrap().port(), 8765);
    assert_eq!(rest, ["--mcp"]);

    let (_, rest) = prescan(argv(&["notes", "list", "--json"])).unwrap();
    assert_eq!(rest, ["notes", "list", "--json", "--full-output"]);
    let (_, rest) = prescan(argv(&["notes", "get", "--", "--allow-ai"])).unwrap();
    assert_eq!(rest, ["notes", "get", "--", "--allow-ai"]);
    assert!(prescan(argv(&["--http", "nope"])).is_err());
    assert!(prescan(argv(&["--mcp-tools"])).is_err());
}

fn envelope(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
}

#[test]
fn not_signed_in_is_exit_3_with_cta() {
    let home = tempfile::tempdir().unwrap();
    let (stdout, code) = out(&bin(home.path())
        .args(["notes", "list", "--json"])
        .output()
        .unwrap());
    assert_eq!(code, 3);
    let env = envelope(&stdout);
    assert_eq!(env["ok"], false);
    assert_eq!(env["error"]["code"], "AUTH");
    assert_eq!(env["meta"]["command"], "notes list");
    assert_eq!(
        env["meta"]["cta"]["commands"][0]["command"],
        "notability account login"
    );
}

#[test]
fn destructive_without_tty_needs_yes() {
    let home = tempfile::tempdir().unwrap();
    let (stdout, code) = out(&bin(home.path())
        .args(["notes", "delete", "abcdef", "--json"])
        .output()
        .unwrap());
    assert_eq!(code, 2);
    let env = envelope(&stdout);
    assert_eq!(env["error"]["code"], "CONFIRM_REQUIRED");
    // With --yes the confirmation passes and the command reaches auth.
    let (_, code) = out(&bin(home.path())
        .args(["notes", "delete", "abcdef", "--yes", "--json"])
        .output()
        .unwrap());
    assert_eq!(code, 3);
}

#[test]
fn usage_errors_exit_2() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["notes", "get"][..],
        &["notes", "list", "--limit", "x"],
        &["content", "text", "style", "abcdef", "--bogus"],
    ] {
        let (_, code) = out(&bin(home.path()).args(args).output().unwrap());
        assert_eq!(code, 2, "{args:?}");
    }
}

#[test]
fn garbage_cursor_is_usage_before_any_request() {
    let home = tempfile::tempdir().unwrap();
    let (stdout, code) = out(&bin(home.path())
        .args(["notes", "list", "--cursor", "!!!", "--json"])
        .output()
        .unwrap());
    assert_eq!(code, 2, "{stdout}");
    assert_eq!(envelope(&stdout)["error"]["code"], "USAGE");
}

#[test]
fn offline_status_and_logout() {
    let home = tempfile::tempdir().unwrap();
    let (stdout, code) = out(&bin(home.path())
        .args(["account", "status", "--offline", "--json"])
        .output()
        .unwrap());
    assert_eq!(code, 3, "{stdout}");
    let (stdout, code) = out(&bin(home.path())
        .args(["account", "logout", "--json"])
        .output()
        .unwrap());
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(envelope(&stdout)["data"]["removed"], false);
}

#[tokio::test]
async fn graphql_refuses_mutations() {
    let (stdout, code) = run(&["account", "graphql", "mutation { x }", "--json"]).await;
    assert_eq!(code, 2, "{stdout}");
}
