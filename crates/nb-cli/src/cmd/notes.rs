use nb_api::NbError;
use nb_domain::session::{duplicate_note, initial_ops};
use nb_domain::{EditSession, export, organize, search};
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

const TYPES: [&str; 5] = ["HANDWRITING", "TEXT", "PDF", "RECORDING", "IMAGE"];

pub fn specs() -> Vec<Spec> {
    vec![
        paging(
            Spec::new("notes list", Effect::R, "List notes (filter by folder, type, favourites; sort; paginate)", list)
                .direct()
                .opt(text("folder", "Folder id, path or title ('unfiled' for notes without a folder)."))
                .opt(choice("sort", &["updated", "created", "opened", "title"], Some("updated"), "Sort key."))
                .opt(flag("asc", "Ascending order."))
                .opt(flag("favorites", "Only favourited notes."))
                .opt(choice("type", &TYPES, None, "Only notes containing this content type.")),
            50.0,
        ),
        Spec::new("notes search", Effect::R, "Search titles, optionally typed/handwritten text", search_cmd)
            .direct()
            .arg(req("query", "Text to find."))
            .opt(flag("content", "Also search typed text inside notes (cached locally; run `notes index` to cache everything)."))
            .opt(num_def("content_limit", 40.0, "Max uncached notes to download per search."))
            .opt(alias(num_def("limit", 50.0, "Max results."), 'n')),
        Spec::new("notes get", Effect::R, "One note: metadata, pages, recordings, Learn status; --format md for markdown", get)
            .direct()
            .arg(note_arg())
            .opt(flag_on("learn", "Include transcripts / AI summaries (--no-learn skips the lookup)."))
            .opt(cli_name(choice("format", &["summary", "md", "text"], Some("summary"), "What to return (CLI: --as, because --format picks the output encoding)."), "as")),
        Spec::new("notes index", Effect::R, "Build/refresh the local full-text index (cache only)", index)
            .opt(num("limit", "Download at most this many uncached notes."))
            .opt(flag("refresh", "Re-download even cached notes."))
            .opt(flag("clear", "Delete the local text cache instead.")),
        Spec::new("notes create", Effect::W, "Create an empty, seeded note", create)
            .arg(req("title", "Note title."))
            .opt(text("folder", "Folder id, path or title.")),
        Spec::new("notes rename", Effect::W, "Rename a note", rename)
            .arg(note_arg())
            .arg(req("new_title", "New title.")),
        Spec::new("notes duplicate", Effect::W, "Copy a note", duplicate)
            .arg(note_arg())
            .opt(text("title", "Title of the copy (default: '<title> copy').")),
        Spec::new("notes move", Effect::W, "Move a note to a folder (or unfiled)", move_note)
            .arg(note_arg())
            .arg(text("folder", "Destination folder (omit for unfiled).")),
        Spec::new("notes favorite", Effect::W, "Star or unstar a note (--off)", favorite)
            .arg(note_arg())
            .opt(flag("off", "Remove from favourites.")),
        Spec::new("notes layout", Effect::W, "Switch paged / pageless", layout)
            .arg(note_arg())
            .arg(req_choice("mode", &["paged", "pageless"], "Layout mode.")),
        Spec::new("notes delete", Effect::D, "Move to trash, or --permanent", delete)
            .arg(note_arg())
            .opt(flag("permanent", "Also purge from Recently Deleted (irreversible).")),
        paging(
            Spec::new("notes trash list", Effect::R, "List trashed notes", trash_list)
                .opt(text("match", "Only titles matching this prefix or glob (e.g. 'Lecture*').")),
            50.0,
        ),
        Spec::new("notes trash restore", Effect::W, "Restore a trashed note", trash_restore).arg(note_arg()),
        Spec::new("notes trash purge", Effect::D, "Permanently delete one trashed note", trash_purge).arg(note_arg()),
        Spec::new("notes trash empty", Effect::D, "Purge trash (--match/--all, --dry-run)", trash_empty)
            .opt(text("match", "Only purge trashed notes whose title matches this prefix or glob."))
            .opt(flag("all", "Purge everything in Recently Deleted."))
            .opt(flag("dry_run", "Show what would be purged.")),
    ]
}

fn ts(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

handler!(list, |c| {
    let cl = c.client()?;
    let lib = cl.library().await?;
    let fm = lib.folder_map();
    let all = cl.notes(c.os("folder").as_deref(), true).await?;
    let snap = format!(
        "{}:{}",
        lib.sequence_id.unwrap_or_default(),
        lib.all_notes(false).len()
    );
    let page = Paging::from(&c, &snap)?;
    let ty = c.os("type");
    let mut rows: Vec<Value> = all
        .iter()
        .filter(|n| !c.b("favorites") || n.is_favorited)
        .filter(|n| ty.as_ref().is_none_or(|t| n.content_types().contains(t)))
        .map(|n| n.summary(&fm))
        .collect();
    let key = match c.os("sort").as_deref() {
        Some("created") => "created_at",
        Some("opened") => "last_opened",
        Some("title") => "title",
        _ => "updated_at",
    };
    rows.sort_by_key(|r| {
        if key == "title" {
            ts(r, key).to_lowercase()
        } else {
            ts(r, key)
        }
    });
    let asc = c.b("asc") || (key == "title" && c.ob("asc").is_none());
    if !asc {
        rows.reverse();
    }
    Ok(page.apply(rows, &snap))
});

handler!(search_cmd, |c| {
    let cl = c.client()?;
    let q = c.s("query")?;
    let limit = c.ou("limit")?.unwrap_or(50);
    if c.b("content") {
        search::index(&cl, Some(c.ou("content_limit")?.unwrap_or(40)), false).await?;
    }
    let items = search::search(&cl, &q, c.b("content"), limit).await?;
    Ok(json!({"query": q, "count": items.len(), "items": items}))
});

handler!(get, |c| {
    let cl = c.client()?;
    let learn = c.ob("learn").unwrap_or(true);
    let d = export::note_details(&cl, &c.s("note")?, learn, learn).await?;
    Ok(match c.os("format").as_deref() {
        Some("md") => {
            json!({"id": d["id"], "title": d["title"], "format": "md", "content": export::to_markdown(&d)})
        }
        Some("text") => {
            json!({"id": d["id"], "title": d["title"], "format": "text", "content": export::to_text(&d)})
        }
        _ => d,
    })
});

handler!(index, |c| {
    if c.b("clear") {
        return Ok(json!({"cleared": search::clear()}));
    }
    let cl = c.client()?;
    search::index(&cl, c.ou("limit")?, c.b("refresh")).await
});

handler!(create, |c| {
    let cl = c.client()?;
    let title = c.s("title")?;
    cl.create_note(&title, c.os("folder").as_deref(), initial_ops(&title))
        .await
});

handler!(rename, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let mut s = EditSession::open(&cl, &n.id).await?;
    s.rename(&c.s("new_title")?).await
});

handler!(duplicate, |c| {
    let cl = c.client()?;
    duplicate_note(&cl, &c.s("note")?, c.os("title").as_deref()).await
});

handler!(move_note, |c| {
    let cl = c.client()?;
    cl.move_note(&c.s("note")?, c.os("folder").as_deref()).await
});

handler!(favorite, |c| {
    let cl = c.client()?;
    cl.favorite_note(&c.s("note")?, !c.b("off")).await
});

handler!(layout, |c| {
    let cl = c.client()?;
    let n = cl.resolve_note(&c.s("note")?).await?;
    let mut s = EditSession::open(&cl, &n.id).await?;
    s.set_layout(&c.s("mode")?).await
});

handler!(delete, |c| {
    let cl = c.client()?;
    cl.delete_note(&c.s("note")?, c.b("permanent")).await
});

handler!(trash_list, |c| {
    let cl = c.client()?;
    let lib = cl.library().await?;
    let rows = organize::trash_rows(&lib, c.os("match").as_deref());
    let snap = format!("trash:{}", lib.deleted_notes().len());
    let page = Paging::from(&c, &snap)?;
    Ok(page.apply(rows, &snap))
});

handler!(trash_restore, |c| {
    let cl = c.client()?;
    cl.trash_restore(&c.s("note")?).await
});

handler!(trash_purge, |c| {
    let cl = c.client()?;
    cl.trash_purge(&c.s("note")?).await
});

handler!(trash_empty, |c| {
    let cl = c.client()?;
    let (pat, all) = (c.os("match"), c.b("all"));
    if pat.is_none() && !all {
        return Err(NbError::usage("pass --match PATTERN or --all"));
    }
    if c.b("dry_run") {
        let lib = cl.library().await?;
        let plan = organize::trash_plan(&lib, pat.as_deref(), all)?;
        let ids: Vec<Value> = plan
            .iter()
            .map(|n| json!({"id": n.id, "title": n.title}))
            .collect();
        return Ok(json!({"dry_run": true, "would_purge": ids.len(), "notes": ids}));
    }
    cl.trash_empty(pat.as_deref(), all).await
});
