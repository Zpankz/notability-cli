//! The `notability` CLI and MCP server, built on incurs.

pub mod cmd;
pub mod cursor;
pub mod envelope;
pub mod mcp;
pub mod skills;
pub mod spec;

use std::collections::BTreeMap;
use std::net::SocketAddr;

use incurs::cli::Cli;
use nb_api::{NbError, Result};

pub use mcp::Gate;
use spec::Spec;

pub const ABOUT: &str = "Notability from the terminal and over MCP: notes, folders, content, files, audio, sharing, Learn, chat, account";

fn env_on(k: &str) -> bool {
    std::env::var(k).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

/// Strips the MCP startup switches incurs does not know, applying env fallbacks, and makes
/// `--json` print the full envelope.
pub fn prescan(argv: Vec<String>) -> Result<(Gate, Vec<String>)> {
    let mut gate = Gate {
        allow_writes: env_on("NOTABILITY_ALLOW_WRITES"),
        allow_ai: env_on("NOTABILITY_ALLOW_AI"),
        ..Gate::default()
    };
    let mut out = Vec::with_capacity(argv.len() + 1);
    let mut it = argv.into_iter();
    let mut json = false;
    let mut full = false;
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| NbError::usage(format!("{name} needs a value")))
        };
        match flag.as_str() {
            "--" => {
                out.push(a);
                out.extend(it.by_ref());
            }
            "--allow-writes" => gate.allow_writes = true,
            "--allow-ai" => gate.allow_ai = true,
            "--mcp-flat" => gate.flat = true,
            "--mcp-tools" => gate.include.extend(
                value("--mcp-tools")?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from),
            ),
            "--http" => {
                let v = value("--http")?;
                gate.http = Some(
                    v.parse::<SocketAddr>()
                        .map_err(|e| NbError::usage(format!("--http {v}: {e}")))?,
                );
            }
            _ => {
                json |= a == "--json";
                full |= a == "--full-output";
                out.push(a);
            }
        }
    }
    if json && !full {
        out.push("--full-output".into());
    }
    Ok((gate, out))
}

#[derive(Default)]
struct Node {
    leaf: Option<Spec>,
    children: BTreeMap<String, Node>,
}

fn group_help(path: &str) -> &'static str {
    cmd::GROUPS
        .iter()
        .find(|(p, _)| *p == path)
        .map_or("", |(_, h)| h)
}

fn mount(name: &str, path: &str, node: Node) -> Cli {
    let mut g = Cli::create(name).description(group_help(path));
    for (child, n) in node.children {
        let sub = format!("{path} {child}");
        g = match n.leaf {
            Some(s) => g.command(child, s.into_def()),
            None => g.group(mount(&child, &sub, n)),
        };
    }
    g
}

/// The full command tree with the MCP surface shaped by `gate`.
pub fn build(gate: &Gate) -> Cli {
    let specs = cmd::all();
    let options = gate.options(&specs);
    let mut root = Node::default();
    for s in specs {
        let mut n = &mut root;
        for part in s.path.split(' ') {
            n = n.children.entry(part.to_string()).or_default();
        }
        n.leaf = Some(s);
    }
    let mut cli = Cli::create("notability")
        .description(ABOUT)
        .version(env!("CARGO_PKG_VERSION"))
        .mcp(options);
    for (noun, n) in root.children {
        cli = cli.group(mount(&noun, &noun, n));
    }
    cli
}
