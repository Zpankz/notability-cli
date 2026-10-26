use std::collections::HashMap;

use incurs::schema::FieldMeta;
use nb_api::NbError;
use nb_domain::client::resolve_folder;
use nb_domain::organize::DEFAULT_COLOR;
use nb_domain::{Folder, Library};
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

const POSITIONS: [&str; 4] = ["first", "last", "before", "after"];
const KINDS: [&str; 2] = ["subject", "divider"];

pub fn specs() -> Vec<Spec> {
    vec![
        paging(
            Spec::new(
                "folders list",
                Effect::R,
                "List folders (--parent, --tree)",
                list,
            )
            .opt(text(
                "parent",
                "Only direct children of this folder (id, path or title).",
            ))
            .opt(flag("tree", "Return an indented tree instead of rows.")),
            0.0,
        ),
        Spec::new(
            "folders order",
            Effect::R,
            "Children of a folder in display order",
            order,
        )
        .arg(text("parent", "Parent folder (omit for the top level).")),
        Spec::new("folders create", Effect::W, "Create a folder", create)
            .arg(req("title", "Folder title."))
            .opt(text(
                "parent",
                "Parent folder (id, path or title). Default: top level.",
            ))
            .opt(text_def(
                "color",
                DEFAULT_COLOR,
                "Hex colour or palette name.",
            )),
        Spec::new("folders rename", Effect::W, "Rename a folder", rename)
            .arg(req("folder", "Folder id, path or title."))
            .arg(req("new_title", "New title.")),
        Spec::new(
            "folders edit",
            Effect::W,
            "Edit title, colour, emoji, lock",
            edit,
        )
        .arg(req("folder", "Folder id, path or title."))
        .opt(text("title", "New title."))
        .opt(text(
            "color",
            "#RRGGBB or a palette name (crimson, red, orange, ...).",
        ))
        .opt(text(
            "emoji",
            "Folder emoji (replaces the colour dot in the sidebar).",
        ))
        .opt(cli_name(
            flag("clear_emoji", "Remove the emoji."),
            "no-emoji",
        ))
        .opt(flag("lock", "Lock (Face ID / passcode on iOS)."))
        .opt(flag("unlock", "Unlock.")),
        Spec::new("folders move", Effect::W, "Re-parent a folder", move_folder)
            .arg(req("folder", "Folder id, path or title."))
            .arg(text("parent", "New parent folder."))
            .opt(flag("root", "Move to the top level."))
            .opt(choice(
                "position",
                &POSITIONS,
                Some("last"),
                "Position among the new siblings.",
            ))
            .opt(text(
                "anchor",
                "Sibling folder for --position before/after.",
            )),
        Spec::new(
            "folders reorder",
            Effect::W,
            "Change position among siblings",
            reorder,
        )
        .arg(req("folder", "Folder id, path or title."))
        .opt(FieldMeta {
            required: true,
            ..choice("position", &POSITIONS, None, "New position.")
        })
        .opt(text(
            "anchor",
            "Sibling folder for --position before/after.",
        )),
        Spec::new(
            "folders delete",
            Effect::D,
            "Delete a folder (--recursive)",
            delete,
        )
        .arg(req("folder", "Folder id, path or title."))
        .opt(flag(
            "recursive",
            "Also delete contained folders and notes (moved to Recently Deleted).",
        )),
        Spec::new(
            "folders organizers list",
            Effect::R,
            "List subjects/dividers",
            org_list,
        )
        .opt(choice("type", &KINDS, None, "Only this kind.")),
        Spec::new(
            "folders organizers create",
            Effect::W,
            "Create a subject or divider",
            org_create,
        )
        .arg(req_choice("kind", &KINDS, "subject or divider."))
        .arg(req("title", "Title."))
        .opt(text("parent", "Parent folder/organizer."))
        .opt(text_def(
            "color",
            DEFAULT_COLOR,
            "Subject colour (#RRGGBB or palette name).",
        )),
        Spec::new(
            "folders organizers edit",
            Effect::W,
            "Edit a subject or divider",
            org_edit,
        )
        .arg(req("organizer", "Organizer id or title."))
        .opt(text("title", "New title."))
        .opt(text("color", "Subjects only."))
        .opt(text("emoji", "Subjects only."))
        .opt(flag("lock", "Subjects only: lock."))
        .opt(flag("unlock", "Subjects only: unlock.")),
        Spec::new(
            "folders organizers delete",
            Effect::D,
            "Delete a subject or divider",
            org_delete,
        )
        .arg(req("organizer", "Organizer id or title.")),
    ]
}

fn is_folder(f: &Folder) -> bool {
    f.organizer_type
        .as_deref()
        .is_none_or(|t| t.eq_ignore_ascii_case("folder"))
}

fn note_counts(lib: &Library) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for n in lib.all_notes(false) {
        if let Some(f) = &n.folder_id {
            *m.entry(f.clone()).or_insert(0) += 1;
        }
    }
    m
}

fn tree_lines(
    lib: &Library,
    parent: Option<&str>,
    depth: usize,
    counts: &HashMap<String, usize>,
    out: &mut Vec<String>,
) {
    let mut kids: Vec<&Folder> = lib
        .all_folders()
        .into_iter()
        .filter(|f| is_folder(f) && f.parent_id.as_deref() == parent)
        .collect();
    kids.sort_by(|a, b| {
        a.sibling_order
            .unwrap_or(0.0)
            .total_cmp(&b.sibling_order.unwrap_or(0.0))
    });
    for f in kids {
        let n = counts.get(&f.id).copied().unwrap_or(0);
        out.push(format!("{}{} ({n})", "  ".repeat(depth), f.title));
        tree_lines(lib, Some(&f.id), depth + 1, counts, out);
    }
}

/// Lock tri-state from `--lock` / `--unlock`.
fn lock(c: &Ctx) -> nb_api::Result<Option<bool>> {
    match (c.b("lock"), c.b("unlock")) {
        (true, true) => Err(NbError::usage("pass only one of --lock / --unlock")),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        _ => Ok(None),
    }
}

handler!(list, |c| {
    let cl = c.client()?;
    let lib = cl.library().await?;
    let counts = note_counts(&lib);
    let parent = c
        .os("parent")
        .map(|p| resolve_folder(&lib, &p))
        .transpose()?;
    if c.b("tree") {
        let mut lines = Vec::new();
        tree_lines(
            &lib,
            parent.as_ref().map(|f| f.id.as_str()),
            0,
            &counts,
            &mut lines,
        );
        return Ok(json!({"tree": lines.join("\n"), "count": lines.len()}));
    }
    let mut rows: Vec<Value> = lib
        .all_folders()
        .into_iter()
        .filter(|f| is_folder(f))
        .filter(|f| {
            parent
                .as_ref()
                .is_none_or(|p| f.parent_id.as_deref() == Some(p.id.as_str()))
        })
        .map(|f| {
            f.summary(
                Some(counts.get(&f.id).copied().unwrap_or(0)),
                lib.folder_path(Some(&f.id)),
            )
        })
        .collect();
    rows.sort_by_key(|r| r["path"].as_str().unwrap_or_default().to_lowercase());
    let snap = format!(
        "{}:{}",
        lib.sequence_id.unwrap_or_default(),
        lib.all_folders().len()
    );
    let page = Paging::from(&c, &snap)?;
    Ok(page.apply(rows, &snap))
});

handler!(order, |c| {
    let cl = c.client()?;
    let items = cl.ordered_children(c.os("parent").as_deref()).await?;
    Ok(json!({"count": items.len(), "items": items}))
});

handler!(create, |c| {
    let cl = c.client()?;
    let color = c.os("color").unwrap_or_else(|| DEFAULT_COLOR.into());
    cl.create_folder(&c.s("title")?, c.os("parent").as_deref(), &color)
        .await
});

handler!(rename, |c| {
    let cl = c.client()?;
    cl.rename_folder(&c.s("folder")?, &c.s("new_title")?).await
});

handler!(edit, |c| {
    let cl = c.client()?;
    let (title, color, emoji) = (c.os("title"), c.os("color"), c.os("emoji"));
    cl.folder_edit(
        &c.s("folder")?,
        title.as_deref(),
        color.as_deref(),
        emoji.as_deref(),
        c.b("clear_emoji"),
        lock(&c)?,
    )
    .await
});

handler!(move_folder, |c| {
    let cl = c.client()?;
    let parent = if c.b("root") { None } else { c.os("parent") };
    if parent.is_none() && !c.b("root") {
        return Err(NbError::usage("give a PARENT or --root"));
    }
    let pos = c.os("position").unwrap_or_else(|| "last".into());
    cl.folder_move(
        &c.s("folder")?,
        parent.as_deref(),
        &pos,
        c.os("anchor").as_deref(),
    )
    .await
});

handler!(reorder, |c| {
    let cl = c.client()?;
    cl.folder_reorder(
        &c.s("folder")?,
        &c.s("position")?,
        c.os("anchor").as_deref(),
    )
    .await
});

handler!(delete, |c| {
    let cl = c.client()?;
    cl.delete_folder(&c.s("folder")?, c.b("recursive")).await
});

handler!(org_list, |c| {
    let cl = c.client()?;
    let items = cl.organizers(c.os("type").as_deref()).await?;
    Ok(json!({"count": items.len(), "items": items}))
});

handler!(org_create, |c| {
    let cl = c.client()?;
    let color = c.os("color").unwrap_or_else(|| DEFAULT_COLOR.into());
    cl.organizer_create(
        &c.s("kind")?,
        &c.s("title")?,
        c.os("parent").as_deref(),
        &color,
    )
    .await
});

handler!(org_edit, |c| {
    let cl = c.client()?;
    let (title, color, emoji) = (c.os("title"), c.os("color"), c.os("emoji"));
    cl.organizer_edit(
        &c.s("organizer")?,
        title.as_deref(),
        color.as_deref(),
        lock(&c)?,
        emoji.as_deref(),
    )
    .await
});

handler!(org_delete, |c| {
    let cl = c.client()?;
    cl.organizer_delete(&c.s("organizer")?).await
});
