//! Plain text of each PDF page (glyph Unicode via ToUnicode / encodings), in content order with
//! line breaks on baseline changes and spaces on wide gaps. Used for chat `note_read` (the web
//! client extracts PDF text with pdf.js for the same purpose).

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::kurbo::{Affine, BezPath, Point, Rect};

use crate::{Result, open};

#[derive(Default)]
struct Extract {
    out: String,
    last: Option<(f64, f64, f64)>, // x, y, em
}

impl Extract {
    fn push(&mut self, s: &str, t: Affine, adv: f64) {
        let o = t * Point::ZERO;
        // glyph space is 1/1000 em.
        let em = ((t * Point::new(0.0, 1000.0)) - o).hypot().max(0.1);
        if let Some((lx, ly, lem)) = self.last {
            let lem = lem.max(em);
            if (o.y - ly).abs() > 0.5 * lem {
                if !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
            } else if o.x - lx > 0.3 * lem
                && !self.out.ends_with([' ', '\n'])
                && !s.starts_with(' ')
            {
                self.out.push(' ');
            }
        }
        self.out.push_str(s);
        self.last = Some(((t * Point::new(adv, 0.0)).x, o.y, em));
    }
}

impl Device<'_> for Extract {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'_>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'_>>, _: BlendMode) {}
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, '_>, props: DrawProps<'_>, _: &DrawMode) {
        for g in run.glyphs() {
            let s = match g.as_unicode() {
                Some(BfString::Char(c)) => c.to_string(),
                Some(BfString::String(s)) => s,
                None => continue,
            };
            let adv = match &**g {
                Glyph::Outline(o) => o.advance_width(),
                Glyph::Type3(_) => None,
            }
            .map_or(500.0, f64::from);
            self.push(&s, props.transform * g.transform(), adv);
        }
    }
    fn draw_image(&mut self, _: Image<'_, '_>, _: ImageDrawProps<'_>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Text of every page (index = 0-based page).
pub fn page_texts(data: &[u8]) -> Result<Vec<String>> {
    let pdf = open(data)?;
    let cache = InterpreterCache::new();
    let mut out = Vec::new();
    for page in pdf.pages().iter() {
        let mut ctx = Context::new(
            Affine::IDENTITY,
            Rect::new(0.0, 0.0, 1.0, 1.0),
            &cache,
            pdf.xref(),
            InterpreterSettings::default(),
        );
        let mut ex = Extract::default();
        interpret_page(page, &mut ctx, &mut ex);
        let t: Vec<&str> = ex.out.lines().map(str::trim_end).collect();
        out.push(t.join("\n").trim().to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[test]
    fn extracts_composed_label() {
        let st = nb_domain::state::NoteState::new(&serde_json::json!({"operations": []}));
        let (pdf, _) = crate::compose::render(&st, &HashMap::new(), true).unwrap();
        let t = super::page_texts(&pdf).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0], crate::compose::LABEL);
    }
}
