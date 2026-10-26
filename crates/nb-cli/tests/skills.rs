//! The committed `skills/` directory must match what `notability skills generate` renders.

use nb_cli::skills::{CLI, render};

fn repo_skills() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills")
}

#[test]
fn committed_skills_match_generated() {
    for (dir, content) in render() {
        let p = repo_skills().join(&dir).join("SKILL.md");
        let have = std::fs::read_to_string(&p).unwrap_or_else(|_| {
            panic!("missing {}; run `notability skills generate`", p.display())
        });
        assert_eq!(
            have, content,
            "{dir}/SKILL.md is stale; run `notability skills generate --out skills`"
        );
    }
    let dirs = std::fs::read_dir(repo_skills()).unwrap().count();
    assert_eq!(dirs, render().len(), "skills/ has extra directories");
}

fn description(content: &str) -> &str {
    content
        .lines()
        .find_map(|l| l.strip_prefix("description: "))
        .unwrap()
}

#[test]
fn descriptions_are_triggers_not_command_lists() {
    for (dir, content) in render() {
        let d = description(&content);
        assert!(d.contains("Use when"), "{dir}: {d}");
        assert!(!d.contains("Commands:"), "{dir}: {d}");
        assert!(d.len() < 400, "{dir}: description too long");
    }
}

#[test]
fn every_command_listed_once_under_its_noun() {
    let specs = nb_cli::cmd::all();
    let all = render();
    for s in &specs {
        let noun = s.path.split(' ').next().unwrap();
        let needle = format!("- `{CLI} {}", s.path);
        let hits = |c: &str| {
            c.lines()
                .filter(|l| l.starts_with(&needle) && l[needle.len()..].starts_with([' ', '`']))
                .count()
        };
        for (dir, content) in &all {
            let n = hits(content);
            if dir == noun {
                assert_eq!(n, 1, "{} listed {n} times in {dir}", s.path);
            } else if dir != CLI {
                assert_eq!(n, 0, "{} leaked into {dir}", s.path);
            }
        }
    }
}

#[test]
fn root_routes_to_every_noun() {
    let all = render();
    let (dir, root) = &all[0];
    assert_eq!(dir, CLI);
    for (d, _) in &all[1..] {
        assert!(
            root.contains(&format!("skill `{CLI}-{d}`")),
            "root misses {d}"
        );
    }
    // The router names a few first steps, not the whole command list.
    let leaves = root
        .lines()
        .filter(|l| l.starts_with(&format!("- `{CLI} ")))
        .count();
    assert!(leaves <= all.len() + 4, "root lists {leaves} commands");
}
