//! Agent skills: one `SKILL.md` per noun plus a root router, rendered from the command specs.
//!
//! incurs' built-in `skills add` only knows the root description, so every noun skill would
//! fall back to a flat "Commands: a, b, c" trigger. This module renders curated files from the
//! spec table (when-to-use triggers, subgroups, flags and safety notes) and installs them with
//! incurs' own `sync_skills::sync`, passing each file through its `include` hook.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use incurs::sync_skills::{SyncOptions, sync};
use serde_json::{Value, json};

use crate::cmd::{self, GROUPS};
use crate::spec::{Effect, Spec};

pub const CLI: &str = "notability";

/// The always-loaded trigger for each noun: when an agent should reach for it.
const WHEN: [(&str, &str); 9] = [
    (
        "notes",
        "Use when you need to find a note (list, search, recent, by folder), open its details, \
         create, rename, move, duplicate, tag or delete notes, or restore something from the trash",
    ),
    (
        "folders",
        "Use when organising the library into folders, subjects and dividers: listing the tree, \
         creating, renaming, colouring, moving or deleting folders",
    ),
    (
        "content",
        "Use when reading or editing what is inside a note: typed text, headings, checklists, \
         code, LaTeX math, handwriting, shapes, images, comments and pages",
    ),
    (
        "files",
        "Use when moving files in or out of Notability: importing PDF, Office, images, audio, \
         .ntb, web PDFs, Google Drive files or YouTube transcripts, exporting PDF/markdown/text, \
         reading a PDF outline or rendering page previews",
    ),
    (
        "audio",
        "Use when working with recordings: listing and downloading audio, reading transcripts, \
         recording from this machine's microphone, or live-transcribing a recording or file",
    ),
    (
        "share",
        "Use when checking or changing who can see a note: share links, collaborators, access \
         requests and finding invitable people in a business workspace",
    ),
    (
        "learn",
        "Use when studying a note: AI summaries, Finish my notes, guided study sessions, quizzes \
         (incl. table-of-contents quizzes), flashcards, Brain Break discussions and syllabus import",
    ),
    (
        "chat",
        "Use when asking Notability AI questions about notes or reading and managing chat threads",
    ),
    (
        "account",
        "Use when checking the sign-in session, profile, quotas, survey and use type, settings, \
         calendar connections, or running a raw read-only GraphQL query",
    ),
];

fn when(noun: &str) -> &'static str {
    WHEN.iter().find(|(n, _)| *n == noun).map_or("", |(_, w)| w)
}

fn group_desc(path: &str) -> &'static str {
    GROUPS
        .iter()
        .find(|(p, _)| *p == path)
        .map_or("", |(_, h)| h)
}

/// Options every command shares; listed once in the body instead of per command.
const COMMON: [&str; 3] = ["cursor", "offset", "confirm"];

fn signature(s: &Spec) -> String {
    let mut sig = format!("{CLI} {}", s.path);
    for a in &s.args {
        if a.required {
            sig.push_str(&format!(" <{}>", a.cli_name));
        } else {
            sig.push_str(&format!(" [{}]", a.cli_name));
        }
    }
    sig
}

fn flags(s: &Spec) -> Vec<String> {
    s.opts
        .iter()
        .filter(|o| !COMMON.contains(&o.name))
        .map(|o| {
            let req = if o.required { " (required)" } else { "" };
            format!("`--{}`{req}", o.cli_name)
        })
        .collect()
}

fn safety(s: &Spec) -> Vec<&'static str> {
    let mut v = Vec::new();
    match s.effect {
        Effect::R => {}
        Effect::W => v.push("writes (MCP needs --allow-writes)"),
        Effect::D => v.push(
            "destructive: needs --yes (MCP: confirm=true, otherwise a dry-run preview; needs --allow-writes)",
        ),
        Effect::Ai => v.push("spends AI quota (MCP needs --allow-ai)"),
        Effect::Aiw => v.push("spends AI quota and writes (MCP needs --allow-ai and --allow-writes)"),
        Effect::Local => v.push("local only, CLI only (not over MCP)"),
    }
    if s.confirm && s.effect != Effect::D {
        v.push("changes the note or profile: needs --yes (MCP: confirm=true)");
    }
    if s.local_device {
        v.push("uses this machine's microphone; hidden over --http");
    }
    if s.cli_only && s.effect != Effect::Local {
        v.push("CLI only");
    }
    if s.unverified {
        v.push("mapped from the web app, not yet confirmed live");
    }
    v
}

fn line(s: &Spec) -> String {
    let mut out = format!("- `{}`: {}", signature(s), s.help.trim_end_matches('.'));
    let f = flags(s);
    if !f.is_empty() {
        out.push_str(&format!(". Flags: {}", f.join(", ")));
    }
    let sf = safety(s);
    if !sf.is_empty() {
        out.push_str(&format!(". Safety: {}", sf.join("; ")));
    }
    out.push('.');
    out
}

fn frontmatter(name: &str, description: &str, command: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {}\nrequires_bin: {CLI}\ncommand: {command}\n---\n",
        serde_json::to_string(description).unwrap_or_default()
    )
}

const CONVENTIONS: &str = "## Conventions\n\n\
- Reference a note by id, an id prefix (6+ characters), its exact title, or `folder/path/title`.\n\
- Output is TOON by default; pass `--json` (full envelope) or `--format json|yaml|md`. Large results take `--token-limit`; list commands page with `--limit` and `--cursor` (from `meta.next_cursor`).\n\
- Exit codes: 0 ok, 1 runtime error, 2 usage error or missing confirmation.\n\
- Destructive commands need `--yes`; without it a non-interactive run stops with CONFIRM_REQUIRED and changes nothing.\n\
- Over MCP (`notability --mcp`) tools are named after the path joined with `_` (e.g. `content_text_add`). Write tools appear only with `--allow-writes`, AI tools only with `--allow-ai`.\n";

/// One noun's skill file.
fn noun_skill(noun: &str, specs: &[&Spec]) -> String {
    let gd = group_desc(noun);
    let description = format!("{}. Run `{CLI} {noun} --help` for every flag.", when(noun));
    let mut body = frontmatter(
        &format!("{CLI}-{noun}"),
        &description,
        &format!("{CLI} {noun}"),
    );
    body.push_str(&format!("\n# {CLI} {noun}\n\n{gd}.\n\n{}.\n\n", when(noun)));

    // Subgroup → commands; leaves directly under the noun first.
    let mut groups: BTreeMap<String, Vec<&Spec>> = BTreeMap::new();
    for s in specs {
        let parts: Vec<&str> = s.path.split(' ').collect();
        let key = if parts.len() > 2 {
            parts[..parts.len() - 1].join(" ")
        } else {
            String::new()
        };
        groups.entry(key).or_default().push(s);
    }
    if let Some(top) = groups.remove("") {
        body.push_str("## Commands\n\n");
        for s in top {
            body.push_str(&line(s));
            body.push('\n');
        }
        body.push('\n');
    }
    for (g, list) in groups {
        let d = group_desc(&g);
        if d.is_empty() {
            body.push_str(&format!("## {g}\n\n"));
        } else {
            body.push_str(&format!("## {g}\n\n{d}.\n\n"));
        }
        for s in list {
            body.push_str(&line(s));
            body.push('\n');
        }
        body.push('\n');
    }
    body.push_str(CONVENTIONS);
    body
}

fn root_skill(nouns: &[String]) -> String {
    let description = format!(
        "Notability from the terminal or MCP: notes, folders, note content, files, audio, sharing, \
         Learn study tools, AI chat and account. Use when the user mentions Notability or their \
         notes there; this routes to the per-area {CLI}-* skills."
    );
    let mut body = frontmatter(CLI, &description, CLI);
    body.push_str(&format!(
        "\n# {CLI}\n\nPick the area, then load its skill (or run `{CLI} <area> --help`).\n\n## Areas\n\n"
    ));
    for n in nouns {
        body.push_str(&format!(
            "- `{CLI} {n}` (skill `{CLI}-{n}`): {}. {}.\n",
            group_desc(n).trim_end_matches('.'),
            when(n)
        ));
    }
    body.push_str(&format!(
        "\n## First steps\n\n\
- `{CLI} account login` signs in (stores the session locally); `{CLI} account status` checks it.\n\
- `{CLI} notes list`, `{CLI} notes search <query>` and `{CLI} notes get <note>` find and open notes.\n\
- `{CLI} --mcp` serves the same commands as MCP tools (read-only unless `--allow-writes` / `--allow-ai`).\n\
- `{CLI} skills add` installs these skills for your coding agents; `{CLI} skills list` shows them.\n\n"
    ));
    body.push_str(CONVENTIONS);
    body
}

/// Every skill file as `(dir, content)`, root first. Noun dirs are bare (`learn`) so they
/// replace the generic file incurs' sync generates for the same group.
pub fn render() -> Vec<(String, String)> {
    let specs = cmd::all();
    let mut by_noun: BTreeMap<String, Vec<&Spec>> = BTreeMap::new();
    for s in &specs {
        let noun = s.path.split(' ').next().unwrap_or_default().to_string();
        by_noun.entry(noun).or_default().push(s);
    }
    let order: Vec<String> = WHEN
        .iter()
        .map(|(n, _)| n.to_string())
        .filter(|n| by_noun.contains_key(n))
        .collect();
    let mut out = vec![(CLI.to_string(), root_skill(&order))];
    for n in &order {
        out.push((n.clone(), noun_skill(n, &by_noun[n])));
    }
    out
}

/// Writes `<dir>/<name>/SKILL.md` for every skill; returns the written paths.
pub fn write_all(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for (name, content) in render() {
        let p = dir.join(&name).join("SKILL.md");
        std::fs::create_dir_all(p.parent().unwrap_or(dir))?;
        std::fs::write(&p, content)?;
        paths.push(p);
    }
    Ok(paths)
}

fn description_of(content: &str) -> String {
    content
        .lines()
        .find_map(|l| l.strip_prefix("description: "))
        .and_then(|d| serde_json::from_str::<String>(d).ok())
        .unwrap_or_default()
}

/// Installed skill name for a dir: `learn` → `notability-learn`.
pub fn skill_name(dir: &str) -> String {
    if dir == CLI {
        CLI.to_string()
    } else {
        format!("{CLI}-{dir}")
    }
}

fn installed(name: &str) -> bool {
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = incurs_home();
    [home.join(".agents/skills"), cwd.join(".agents/skills")]
        .iter()
        .any(|b| b.join(name).join("SKILL.md").exists())
}

fn incurs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// `skills list`.
pub fn list() -> Value {
    let rows: Vec<Value> = render()
        .into_iter()
        .map(|(dir, c)| {
            let name = skill_name(&dir);
            json!({"name": name, "description": description_of(&c), "installed": installed(&name)})
        })
        .collect();
    json!({"count": rows.len(), "skills": rows})
}

/// `skills add`: install the curated files through incurs' sync (which also records the
/// command hash so the "skills are out of date" hint keeps working).
pub async fn add(cli: &incurs::cli::Cli, global: bool) -> Result<Value, String> {
    let tmp = std::env::temp_dir().join(format!("{CLI}-skills-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let paths = write_all(&tmp).map_err(|e| e.to_string())?;
    let include: Vec<String> = paths
        .iter()
        .filter_map(|p| p.parent())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let res = sync(
        CLI,
        &cli.skill_command_info(),
        &SyncOptions {
            cwd: None,
            depth: Some(1),
            description: Some(crate::ABOUT.into()),
            global,
            include: Some(include),
        },
    )
    .await;
    let _ = std::fs::remove_dir_all(&tmp);
    let res = res.map_err(|e| e.to_string())?;
    let names: Vec<String> = res.skills.iter().map(|s| skill_name(&s.name)).collect();
    let paths: Vec<String> = res
        .paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let agents: Vec<&str> = res.agents.iter().map(|a| a.agent.as_str()).collect();
    Ok(json!({"synced": names.len(), "skills": names, "paths": paths, "agents": agents}))
}

/// Handles `skills add|list|generate` before incurs' builtin; `None` = not ours.
pub async fn intercept(cli: &incurs::cli::Cli, argv: &[String]) -> Option<i32> {
    // Positionals, skipping the values of global options that take one.
    const VALUED: [&str; 3] = ["--format", "--token-limit", "--out"];
    let mut pos: Vec<&str> = Vec::new();
    let mut skip = false;
    for a in argv.iter().map(String::as_str) {
        if std::mem::take(&mut skip) {
            continue;
        }
        if a.starts_with('-') {
            skip = VALUED.contains(&a);
        } else {
            pos.push(a);
        }
    }
    if pos.first() != Some(&"skills") || argv.iter().any(|a| a == "--help" || a == "-h") {
        return None;
    }
    let json_out = argv.iter().any(|a| a == "--json" || a == "--format=json")
        || argv
            .windows(2)
            .any(|w| w[0] == "--format" && w[1] == "json");
    let print = |v: &Value, human: String| {
        if json_out {
            println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
        } else {
            println!("{human}");
        }
    };
    match pos.get(1).copied() {
        Some("list" | "ls") => {
            let v = list();
            let rows = v["skills"].as_array().cloned().unwrap_or_default();
            let w = rows
                .iter()
                .map(|r| r["name"].as_str().unwrap_or_default().len())
                .max()
                .unwrap_or(0);
            let mut lines: Vec<String> = rows
                .iter()
                .map(|r| {
                    let n = r["name"].as_str().unwrap_or_default();
                    let mark = if r["installed"].as_bool() == Some(true) {
                        "\u{2713}"
                    } else {
                        "\u{2717}"
                    };
                    format!(
                        "  {mark} {n:w$}  {}",
                        r["description"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            let k = rows
                .iter()
                .filter(|r| r["installed"].as_bool() == Some(true))
                .count();
            lines.push(format!("\n{} skills ({k} installed)", rows.len()));
            print(&v, lines.join("\n"));
            Some(0)
        }
        Some("add") => {
            let global = !argv.iter().any(|a| a == "--no-global");
            match add(cli, global).await {
                Ok(v) => {
                    let mut human = format!("Synced {} skills", v["synced"]);
                    for n in v["skills"].as_array().into_iter().flatten() {
                        human.push_str(&format!("\n  {}", n.as_str().unwrap_or_default()));
                    }
                    print(&v, human);
                    Some(0)
                }
                Err(e) => {
                    eprintln!("error: skills add failed: {e}");
                    Some(1)
                }
            }
        }
        Some("generate") => {
            let out = argv
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| argv.get(i + 1).cloned())
                .or_else(|| {
                    argv.iter()
                        .find_map(|a| a.strip_prefix("--out=").map(String::from))
                })
                .unwrap_or_else(|| "skills".into());
            match write_all(Path::new(&out)) {
                Ok(p) => {
                    let v = json!({"written": p.len(), "dir": out});
                    print(&v, format!("Wrote {} skill files to {out}", p.len()));
                    Some(0)
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    Some(1)
                }
            }
        }
        _ => None,
    }
}
