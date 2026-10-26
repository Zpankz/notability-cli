use incurs::schema::FieldMeta;
use nb_api::{NbError, Result};
use nb_domain::session::{
    ALIGNMENTS, AttrKind, CODE_LANGUAGES, FLAIRS, HIGHLIGHT_COLOR, STYLE_ATTRS, alignment,
    color_value, decorator, describe_paper, heading_attrs, make_paper, parse_color, parse_points,
};
use nb_domain::{Client, EditSession, Selection, export};
use serde_json::{Value, json};

use crate::handler;
use crate::spec::*;

const TEMPLATES: [&str; 4] = ["plain", "lines", "dots", "grid"];
const HEADINGS: [&str; 6] = ["title", "h1", "h2", "h3", "body", "caption"];
const LISTS: [&str; 6] = ["bullet", "number", "check", "quote", "code", "none"];
const SHAPES: [&str; 5] = ["rectangle", "ellipse", "triangle", "arrow", "line"];

fn field_opt() -> FieldMeta {
    text("field", "Text box id (default: the note's body text).")
}

/// Selection options shared by the text editing commands.
fn selection(s: Spec) -> Spec {
    s.opt(text("match", "Select the nth occurrence of this text."))
        .opt(num_def(
            "nth",
            1.0,
            "Which occurrence of --match (1-based).",
        ))
        .opt(text("range", "Character range START:END (end exclusive)."))
        .opt(text(
            "paragraphs",
            "Paragraph N or A-B (1-based, inclusive).",
        ))
}

fn parse_selection(c: &Ctx) -> Result<Selection> {
    let picked = [
        c.os("match").is_some(),
        c.os("range").is_some(),
        c.os("paragraphs").is_some(),
    ];
    if picked.iter().filter(|p| **p).count() > 1 {
        return Err(NbError::usage(
            "pass only one of --match, --range, --paragraphs",
        ));
    }
    let bad = |w: &str, v: &str| NbError::usage(format!("bad --{w} '{v}'"));
    if let Some(m) = c.os("match") {
        return Ok(Selection::Match(m, c.ou("nth")?.unwrap_or(1).max(1)));
    }
    if let Some(r) = c.os("range") {
        let (a, b) = r.split_once(':').ok_or_else(|| bad("range", &r))?;
        let (a, b) = (
            a.trim().parse().map_err(|_| bad("range", &r))?,
            b.trim().parse().map_err(|_| bad("range", &r))?,
        );
        return Ok(Selection::Range(a, b));
    }
    if let Some(p) = c.os("paragraphs") {
        let (a, b) = p.split_once('-').unwrap_or((&p, &p));
        let (a, b) = (
            a.trim().parse().map_err(|_| bad("paragraphs", &p))?,
            b.trim().parse().map_err(|_| bad("paragraphs", &p))?,
        );
        return Ok(Selection::Paragraphs(a, b));
    }
    Ok(Selection::All)
}

/// `none` clears a colour; otherwise `#RRGGBB[AA]`.
fn color_or_clear(v: &str, default: Option<(u8, u8, u8, u8)>) -> Result<Value> {
    match (v, default) {
        ("none", _) => Ok(Value::Null),
        ("default", Some(d)) => Ok(color_value(d)),
        _ => parse_color(v),
    }
}

fn opt_color(c: &Ctx, k: &str) -> Result<Option<Value>> {
    c.os(k).map(|v| parse_color(&v)).transpose()
}

async fn open<'a>(cl: &'a Client, c: &Ctx) -> Result<EditSession<'a>> {
    let n = cl.resolve_note(&c.s("note")?).await?;
    EditSession::open(cl, &n.id).await
}

pub fn specs() -> Vec<Spec> {
    let mut style = Spec::new(
        "content text style",
        Effect::W,
        "Style a text range: bold/italic/underline/strike/super/subscript/inline code (unverified)/highlight/colour/font/size/heading/link/clear",
        text_style,
    )
    .arg(note_arg())
    .opt(field_opt());
    style = selection(style);
    for (name, _, kind) in STYLE_ATTRS {
        style = style.opt(match kind {
            AttrKind::Bool if name == "code" => {
                flag(name, "Inline code style (unverified); --no-code turns off.")
            }
            AttrKind::Bool => flag(name, "Turn on (--no-* turns off)."),
            AttrKind::Color if name == "highlight" => {
                text(name, "#RRGGBB[AA], 'default' for yellow, or none to clear.")
            }
            AttrKind::Color => text(name, "#RRGGBB[AA], or none to clear."),
            AttrKind::String => text(name, "Font family name, or none to reset."),
            AttrKind::Float => num(name, "Font size in points."),
        });
    }
    style = style
        .opt(choice(
            "heading",
            &HEADINGS,
            None,
            "Apply a heading preset (font size + bold).",
        ))
        .opt(text("link", "Make the selection a link to this URL."))
        .opt(flag("clear", "Remove character styles from the selection."))
        .opt(flag(
            "clear_paragraph",
            "Also reset paragraph styles (lists, alignment, indent).",
        ));

    vec![
        Spec::new(
            "content show",
            Effect::R,
            "Decoded edit log: pages, text boxes, math, code, ink, shapes, comments, images",
            show,
        )
        .arg(note_arg()),
        Spec::new(
            "content text list",
            Effect::R,
            "List text and text boxes with ids and styles",
            text_list,
        )
        .arg(note_arg()),
        Spec::new(
            "content text add",
            Effect::W,
            "Add typed text or a text box (--code LANG for a code block)",
            text_add,
        )
        .arg(note_arg())
        .arg(req("content", "Text to add."))
        .opt(flag(
            "box",
            "Create a text box instead of appending to the body text.",
        ))
        .opt(flag(
            "at_start",
            "Insert at the start of the body instead of the end.",
        ))
        .opt(num_def("page", 1.0, "Page for --box (1-based)."))
        .opt(num_def("x", 72.0, "Box origin x (points)."))
        .opt(num_def("y", 72.0, "Box origin y (points)."))
        .opt(num("width", "Fixed box width (default: auto-fit text)."))
        .opt(choice(
            "code",
            &CODE_LANGUAGES,
            None,
            "Append as a code block in this language.",
        )),
        style,
        selection(
            Spec::new(
                "content text paragraph",
                Effect::W,
                "Paragraph style: list kind, alignment, indent, line spacing, code language",
                text_paragraph,
            )
            .arg(note_arg())
            .opt(field_opt()),
        )
        .opt(choice(
            "list",
            &LISTS,
            None,
            "List / block kind (none removes it).",
        ))
        .opt(choice(
            "align",
            &ALIGNMENTS.map(|a| a.0),
            None,
            "Alignment.",
        ))
        .opt(num("indent", "Indent level (0 = none)."))
        .opt(num("spacing", "Line spacing multiplier."))
        .opt(choice(
            "language",
            &CODE_LANGUAGES,
            None,
            "Code-block language.",
        )),
        Spec::new(
            "content text check",
            Effect::W,
            "Tick or untick a checklist item",
            text_check,
        )
        .arg(note_arg())
        .arg(req_num("paragraph", "Paragraph number (1-based)."))
        .opt(field_opt())
        .opt(flag("off", "Untick instead.")),
        Spec::new(
            "content math list",
            Effect::R,
            "List math blocks with their LaTeX",
            math_list,
        )
        .arg(note_arg()),
        Spec::new(
            "content math add",
            Effect::W,
            "Insert a LaTeX math block",
            math_add,
        )
        .arg(note_arg())
        .arg(req("latex", "LaTeX source (without $$)."))
        .opt(num_def("page", 1.0, "Page (1-based)."))
        .opt(num_def("x", 72.0, "x (points)."))
        .opt(num_def("y", 72.0, "y (points)."))
        .opt(text("color", "#RRGGBB.")),
        Spec::new(
            "content math edit",
            Effect::W,
            "Change a math block's LaTeX or colour",
            math_edit,
        )
        .arg(note_arg())
        .arg(req("math_id", "Math block id (from content math list)."))
        .opt(text("latex", "New LaTeX."))
        .opt(text("color", "New colour #RRGGBB.")),
        Spec::new(
            "content comments list",
            Effect::R,
            "List comments and replies",
            comments_list,
        )
        .arg(note_arg()),
        Spec::new(
            "content comments add",
            Effect::W,
            "Comment on a page (or reply)",
            comments_add,
        )
        .arg(note_arg())
        .arg(req("content", "Comment text."))
        .opt(num_def("page", 1.0, "Page (1-based)."))
        .opt(num_def("x", 100.0, "Pin x (points)."))
        .opt(num_def("y", 100.0, "Pin y (points)."))
        .opt(text(
            "reply_to",
            "Reply to the thread with this comment id (SITE:TS).",
        )),
        Spec::new(
            "content comments edit",
            Effect::W,
            "Change a comment's text",
            comments_edit,
        )
        .arg(note_arg())
        .arg(req("comment_id", "Comment id."))
        .arg(req("content", "New text.")),
        Spec::new(
            "content comments move",
            Effect::W,
            "Move a comment pin",
            comments_move,
        )
        .arg(note_arg())
        .arg(req("comment_id", "Comment id."))
        .opt(num_def("page", 1.0, "Page (1-based)."))
        .opt(FieldMeta {
            required: true,
            ..num("x", "Pin x (points).")
        })
        .opt(FieldMeta {
            required: true,
            ..num("y", "Pin y (points).")
        }),
        Spec::new(
            "content comments delete",
            Effect::D,
            "Delete a comment",
            comments_delete,
        )
        .arg(note_arg())
        .arg(req("comment_id", "Comment id.")),
        Spec::new(
            "content pages list",
            Effect::R,
            "List pages with size and template",
            pages_list,
        )
        .arg(note_arg()),
        Spec::new(
            "content pages add",
            Effect::W,
            "Insert blank pages",
            pages_add,
        )
        .arg(note_arg())
        .opt(num(
            "after",
            "Insert after this page (0 = first; default: at the end).",
        ))
        .opt(num_def("count", 1.0, "How many pages."))
        .opt(choice(
            "template",
            &TEMPLATES,
            None,
            "Give the new page(s) their own paper.",
        ))
        .opt(num("spacing", "Line/dot/grid spacing in points."))
        .opt(text("color", "Paper colour #RRGGBB.")),
        Spec::new(
            "content pages delete",
            Effect::D,
            "Delete a page",
            pages_delete,
        )
        .arg(note_arg())
        .arg(req_num("page", "Page number (1-based).")),
        Spec::new(
            "content pages reorder",
            Effect::W,
            "Move a page to a new index",
            pages_reorder,
        )
        .arg(note_arg())
        .arg(req_num("page", "Page number (1-based)."))
        .arg(req_num("to", "New position (1-based).")),
        Spec::new(
            "content pages template",
            Effect::W,
            "Change paper template, spacing, colour",
            pages_template,
        )
        .arg(note_arg())
        .arg(req_choice("template", &TEMPLATES, "Paper style."))
        .opt(num("page", "Only this page (default: the whole note)."))
        .opt(num("spacing", "Line/dot/grid spacing in points."))
        .opt(text("color", "Paper colour #RRGGBB (default white).")),
        Spec::new(
            "content pages templates",
            Effect::R,
            "List known paper templates",
            pages_templates,
        ),
        Spec::new(
            "content ink list",
            Effect::R,
            "List ink strokes with ids, tool and bounds",
            ink_list,
        )
        .arg(note_arg()),
        Spec::new(
            "content ink draw",
            Effect::W,
            "Draw a pen or highlighter stroke through points",
            ink_draw,
        )
        .arg(note_arg())
        .opt(FieldMeta {
            required: true,
            ..text("points", "Stroke points 'x,y;x,y;...' in page points.")
        })
        .opt(num_def("page", 1.0, "Page (1-based)."))
        .opt(flag(
            "highlighter",
            "Draw with the highlighter (translucent, wide).",
        ))
        .opt(text(
            "color",
            "#RRGGBB[AA] (default: ink black / highlighter yellow).",
        ))
        .opt(num("width", "Stroke width in points.")),
        Spec::new(
            "content ink erase",
            Effect::W,
            "Erase ink strokes or shapes by id (the eraser)",
            ink_erase,
        )
        .arg(note_arg())
        .arg(req_multi(
            "ids",
            "Ink or shape ids (from content ink list / content shapes list).",
        )),
        Spec::new(
            "content shapes list",
            Effect::R,
            "List shapes with ids, kind and bounds",
            shapes_list,
        )
        .arg(note_arg()),
        Spec::new(
            "content shapes add",
            Effect::W,
            "Draw a rectangle, ellipse, triangle, arrow or line",
            shapes_add,
        )
        .arg(note_arg())
        .arg(req_choice("kind", &SHAPES, "Shape kind."))
        .opt(num_def("page", 1.0, "Page (1-based)."))
        .opt(num_def("x", 72.0, "Left (points)."))
        .opt(num_def("y", 72.0, "Top (points)."))
        .opt(num_def("width", 100.0, "Width (points)."))
        .opt(num_def("height", 100.0, "Height (points)."))
        .opt(text("color", "Stroke colour #RRGGBB[AA]."))
        .opt(num_def("border", 2.0, "Stroke width (points).")),
        Spec::new(
            "content move",
            Effect::W,
            "Move a selection of ink, shapes, images or blocks",
            move_sel,
        )
        .arg(note_arg())
        .arg(req_multi("ids", "Entity ids to move together."))
        .opt(num_def("dx", 0.0, "Horizontal offset (points)."))
        .opt(num_def("dy", 0.0, "Vertical offset (points)."))
        .opt(num("to_page", "Move onto this page (1-based).")),
        Spec::new(
            "content images list",
            Effect::R,
            "List images with ids, page and frame",
            images_list,
        )
        .arg(note_arg()),
        Spec::new(
            "content images crop",
            Effect::W,
            "Crop an image to a rectangle of its own frame",
            images_crop,
        )
        .arg(note_arg())
        .arg(req("image_id", "Image id (from content images list)."))
        .opt(FieldMeta {
            required: true,
            ..num("x", "Crop left (points).")
        })
        .opt(FieldMeta {
            required: true,
            ..num("y", "Crop top (points).")
        })
        .opt(FieldMeta {
            required: true,
            ..num("width", "Crop width (points).")
        })
        .opt(FieldMeta {
            required: true,
            ..num("height", "Crop height (points).")
        }),
        Spec::new(
            "content images flip",
            Effect::W,
            "Flip an image horizontally and/or vertically",
            images_flip,
        )
        .arg(note_arg())
        .arg(req("image_id", "Image id (from content images list)."))
        .opt(flag("horizontal", "Mirror left-right."))
        .opt(flag("vertical", "Mirror top-bottom.")),
    ]
}

fn head(s: &EditSession<'_>) -> Value {
    json!({"note_id": s.note_id, "title": s.state.title()})
}

fn with_head(s: &EditSession<'_>, k: &str, items: Vec<Value>) -> Value {
    let mut h = head(s);
    h["count"] = json!(items.len());
    h[k] = json!(items);
    h
}

handler!(show, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    let mut v = export::content(&s.state);
    v["note_id"] = json!(s.note_id);
    Ok(v)
});

handler!(text_list, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    let paras: Vec<Value> = s
        .state
        .paragraphs(None)
        .iter()
        .enumerate()
        .map(|(i, p)| json!({"paragraph": i + 1, "text": p.text.trim_start_matches('\n'), "style": p.style.to_json()}))
        .collect();
    let v = export::content(&s.state);
    let mut out = head(&s);
    out["body"] = json!(paras);
    out["text_boxes"] = v["text_boxes"].clone();
    out["styles"] = json!(s.state.style_ops(None).len());
    Ok(out)
});

handler!(text_add, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let body = c.s("content")?;
    if let Some(lang) = c.os("code") {
        return s.add_code(&body, &lang).await;
    }
    if c.b("box") {
        let (x, y) = (c.of("x").unwrap_or(72.0), c.of("y").unwrap_or(72.0));
        return s
            .add_text_box(&body, c.ou("page")?.unwrap_or(1), x, y, c.of("width"), 30.0)
            .await;
    }
    s.add_text(&body, c.b("at_start")).await
});

handler!(text_style, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let sel = parse_selection(&c)?;
    let field = c.os("field");
    let mut attrs: Vec<(String, Value)> = Vec::new();
    for (name, op_field, kind) in STYLE_ATTRS {
        let v = match kind {
            AttrKind::Bool => c.ob(name).map(Value::Bool),
            AttrKind::Color => {
                let def = (name == "highlight").then_some(HIGHLIGHT_COLOR);
                c.os(name).map(|v| color_or_clear(&v, def)).transpose()?
            }
            AttrKind::String => c
                .os(name)
                .map(|v| if v == "none" { Value::Null } else { json!(v) }),
            AttrKind::Float => c.of(name).map(|f| json!(f)),
        };
        if let Some(v) = v {
            attrs.push((op_field.to_string(), v));
        }
    }
    if let Some(h) = c.os("heading") {
        attrs.extend(heading_attrs(&h)?);
    }
    let mut results = Vec::new();
    if c.b("clear") || c.b("clear_paragraph") {
        results.push(
            s.clear_style(field.as_deref(), &sel, c.b("clear_paragraph"))
                .await?,
        );
    }
    if !attrs.is_empty() {
        results.push(s.style(field.as_deref(), &sel, &attrs).await?);
    }
    if let Some(url) = c.os("link") {
        results.push(s.link(field.as_deref(), &sel, &url).await?);
    }
    if results.is_empty() {
        return Err(NbError::usage(
            "nothing to apply: pass a style flag, --heading, --link or --clear",
        ));
    }
    Ok(if results.len() == 1 {
        results.remove(0)
    } else {
        json!({"results": results})
    })
});

handler!(text_paragraph, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let sel = parse_selection(&c)?;
    let mut attrs: Vec<(String, Value)> = Vec::new();
    if let Some(l) = c.os("list") {
        attrs.push(("decoratorStyle".into(), decorator(&l)?));
    }
    if let Some(a) = c.os("align") {
        attrs.push(("alignment".into(), json!(alignment(&a)?)));
    }
    if let Some(i) = c.ou("indent")? {
        attrs.push(("indentLevel".into(), json!(i)));
    }
    if let Some(sp) = c.of("spacing") {
        attrs.push(("lineSpacing".into(), json!(sp)));
    }
    if let Some(lang) = c.os("language") {
        attrs.push(("programmingLanguage".into(), json!(lang)));
    }
    s.paragraph_style(c.os("field").as_deref(), &sel, &attrs)
        .await
});

handler!(text_check, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.set_checked(c.os("field").as_deref(), c.u("paragraph")?, !c.b("off"))
        .await
});

fn blocks_of(s: &EditSession<'_>, kind: &str) -> Vec<Value> {
    s.state
        .blocks()
        .into_iter()
        .filter(|b| b["kind"] == kind)
        .collect()
}

handler!(math_list, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    let v = export::content(&s.state);
    Ok(with_head(
        &s,
        "items",
        v["math"].as_array().cloned().unwrap_or_default(),
    ))
});

handler!(math_add, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let color = opt_color(&c, "color")?;
    s.add_math(
        &c.s("latex")?,
        c.ou("page")?.unwrap_or(1),
        c.of("x").unwrap_or(72.0),
        c.of("y").unwrap_or(72.0),
        color,
    )
    .await
});

handler!(math_edit, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let color = opt_color(&c, "color")?;
    s.edit_math(&c.s("math_id")?, c.os("latex").as_deref(), color)
        .await
});

handler!(comments_list, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    Ok(with_head(&s, "items", s.state.comments()))
});

handler!(comments_add, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let (x, y) = (c.of("x").unwrap_or(100.0), c.of("y").unwrap_or(100.0));
    s.add_comment(
        &c.s("content")?,
        c.ou("page")?.unwrap_or(1),
        x,
        y,
        c.os("reply_to").as_deref(),
    )
    .await
});

handler!(comments_edit, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.edit_comment(&c.s("comment_id")?, &c.s("content")?).await
});

handler!(comments_move, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.move_comment(
        &c.s("comment_id")?,
        c.ou("page")?.unwrap_or(1),
        c.f("x")?,
        c.f("y")?,
    )
    .await
});

handler!(comments_delete, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.delete_comment(&c.s("comment_id")?).await
});

handler!(pages_list, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    let pages: Vec<Value> = s
        .state
        .pages()
        .iter()
        .map(|p| {
            let mut v = p.to_json();
            v["number"] = json!(p.index + 1);
            v["paper"] = describe_paper(p.background.as_ref());
            v
        })
        .collect();
    let mut out = with_head(&s, "pages", pages);
    out["layout"] = json!(s.state.layout());
    Ok(out)
});

fn paper(c: &Ctx, style: Option<String>) -> Result<Option<Value>> {
    style
        .map(|t| make_paper(&t, c.of("spacing"), c.os("color").as_deref()))
        .transpose()
}

handler!(pages_add, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let tpl = paper(&c, c.os("template"))?;
    s.add_pages(c.ou("after")?, c.ou("count")?.unwrap_or(1), tpl)
        .await
});

handler!(pages_delete, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.delete_page(c.u("page")?).await
});

handler!(pages_reorder, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.move_page(c.u("page")?, c.u("to")?).await
});

handler!(pages_template, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let tpl = paper(&c, Some(c.s("template")?))?.unwrap_or_default();
    s.set_template(tpl, c.ou("page")?).await
});

handler!(pages_templates, |_c| {
    let items: Vec<Value> = FLAIRS
        .iter()
        .map(|(name, flair)| json!({"template": name, "flair": flair, "spacing": name != &"plain", "color": true}))
        .collect();
    Ok(json!({"count": items.len(), "items": items}))
});

handler!(ink_list, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    Ok(with_head(&s, "items", s.state.entities("CreateInk")))
});

handler!(ink_draw, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let pts = parse_points(&c.s("points")?)?;
    let color = opt_color(&c, "color")?;
    s.draw_ink(
        c.ou("page")?.unwrap_or(1),
        &pts,
        c.b("highlighter"),
        color,
        c.of("width"),
    )
    .await
});

handler!(ink_erase, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.erase(&c.ids("ids"), &["ink", "shape"]).await
});

handler!(shapes_list, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    Ok(with_head(&s, "items", s.state.entities("CreateShape")))
});

handler!(shapes_add, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let color = opt_color(&c, "color")?;
    let (x, y) = (c.of("x").unwrap_or(72.0), c.of("y").unwrap_or(72.0));
    let (w, h) = (
        c.of("width").unwrap_or(100.0),
        c.of("height").unwrap_or(100.0),
    );
    s.add_shape(
        &c.s("kind")?,
        c.ou("page")?.unwrap_or(1),
        x,
        y,
        w,
        h,
        color,
        c.of("border").unwrap_or(2.0),
    )
    .await
});

handler!(move_sel, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    let (dx, dy) = (c.of("dx").unwrap_or(0.0), c.of("dy").unwrap_or(0.0));
    s.move_entities(&c.ids("ids"), dx, dy, c.ou("to_page")?)
        .await
});

handler!(images_list, |c| {
    let cl = c.client()?;
    let s = open(&cl, &c).await?;
    let items = blocks_of(&s, "image");
    Ok(with_head(&s, "items", items))
});

handler!(images_crop, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.crop_image(
        &c.s("image_id")?,
        c.f("x")?,
        c.f("y")?,
        c.f("width")?,
        c.f("height")?,
    )
    .await
});

handler!(images_flip, |c| {
    let cl = c.client()?;
    let mut s = open(&cl, &c).await?;
    s.flip_image(&c.s("image_id")?, c.b("horizontal"), c.b("vertical"))
        .await
});
