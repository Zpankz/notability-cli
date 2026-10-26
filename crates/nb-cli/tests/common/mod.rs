#![allow(dead_code)]

use std::process::{Command, Output, Stdio};

use nb_cli::{Gate, build, prescan};

pub fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Runs the in-process CLI (no network unless a command needs it); returns (stdout, exit).
pub async fn run(a: &[&str]) -> (String, i32) {
    let (gate, argv) = prescan(argv(a)).expect("prescan");
    let cli = build(&gate);
    let mut out = Vec::new();
    let code = cli.serve_to(argv, &mut out, false).await.expect("serve");
    let (out, code) = nb_cli::envelope::adjust(out, code);
    (String::from_utf8(out).unwrap(), code.unwrap_or(0))
}

pub fn cli(gate: &Gate) -> incurs::cli::Cli {
    build(gate)
}

/// The real binary with an empty HOME, so no stored session or token is visible.
pub fn bin(home: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_notability"));
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .stdin(Stdio::null());
    c
}

pub fn out(o: &Output) -> (String, i32) {
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}
