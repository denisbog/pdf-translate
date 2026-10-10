//! Text extraction on top of `hayro`.
//!
//! A hayro `Device` that ignores paint and records every glyph: its Unicode value (ToUnicode →
//! encoding → glyph name fallbacks, handled by hayro) and its box in *page view space*, i.e.
//! points with the origin at the top-left of the displayed page, rotation applied, y down. The UI
//! maps view space to the screen with a single scale.
//!
//! On top of the glyph list: line grouping (baseline clustering), word gaps and reading-order
//! selection.

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    interpret_page,
};
use kurbo::{Affine, BezPath, Rect, Shape};

/// One glyph on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct TextGlyph {
    /// The glyph's Unicode text: one character, or several for a ligature.
    pub text: String,
    /// Box in page view space (points, y down): [x0, y0, x1, y1].
    pub rect: [f32; 4],
}

/// The text of one page, in content order, with line structure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub glyphs: Vec<TextGlyph>,
    /// For each glyph: its line index (lines are in reading order, top to bottom).
    pub line_of: Vec<u32>,
    /// `true` when a word break precedes the glyph on the same line.
    pub space_before: Vec<bool>,
}

#[allow(dead_code)]
impl PageText {
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Text of glyphs `range` with spaces and line breaks reconstructed.
    pub fn text_of(&self, range: std::ops::Range<usize>) -> String {
        let mut s = String::new();
        let mut prev_line = None;
        for i in range.clone() {
            let Some(g) = self.glyphs.get(i) else { break };
            if i != range.start {
                if prev_line != Some(self.line_of[i]) {
                    s.push('\n');
                } else if self.space_before[i] {
                    s.push(' ');
                }
            }
            s.push_str(&g.text);
            prev_line = Some(self.line_of[i]);
        }
        s
    }

    pub fn plain_text(&self) -> String {
        self.text_of(0..self.glyphs.len())
    }
}

struct TextDevice {
    /// Separate flows by writing direction: right, down, left, up.
    glyphs: [Vec<TextGlyph>; 4],
}

impl TextDevice {
    fn new() -> Self {
        Self { glyphs: Default::default() }
    }
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        let Some(u) = glyph.as_unicode() else { return };
        let text = match u {
            hayro::hayro_interpret::hayro_cmap::BfString::Char(c) => c.to_string(),
            hayro::hayro_interpret::hayro_cmap::BfString::String(s) => s,
        };
        if text.chars().all(|c| c.is_control()) {
            return;
        }
        // Glyph space → view space. Glyph space uses 1000 units per em for outline glyphs; the em
        // box spans descender (−200) to ascender (800), the advance gives the width.
        let t = transform * glyph_transform;
        let advance = match glyph {
            Glyph::Outline(g) => g.advance_width().filter(|a| *a > 0.0).map(f64::from).unwrap_or_else(|| g.outline().bounding_box().width().max(500.0)),
            Glyph::Type3(_) => 600.0,
        };
        // The baseline direction in view space (y down), to the nearest quarter turn.
        let coefficients = t.as_coeffs();
        let (dx, dy) = (coefficients[0], coefficients[1]);
        let dir = if dx.abs() >= dy.abs() {
            if dx >= 0.0 { 0 } else { 2 }
        } else if dy > 0.0 {
            1
        } else {
            3
        };
        let em = Rect::new(0.0, -200.0, advance, 800.0);
        let b = (t * em.to_path(0.1)).bounding_box();
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) || b.width() > 10_000.0 || b.height() > 10_000.0 {
            return;
        }
        // A glyph whose Unicode is several characters (a ligature such as "ffi") stays one glyph
        // with its own box: the PDF gives no position for the characters inside it.
        self.glyphs[dir].push(TextGlyph { text, rect: [b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32] });
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Extract the text of one page.
pub(crate) fn extract_page(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> Option<PageText> {
    let pages = pdf.pages();
    let p = pages.get(page)?;
    let (w, h) = p.render_dimensions();
    let cache = InterpreterCache::new();
    // Annotation appearances are included: form-field values and comment text are searchable,
    // as in Acrobat, and match what other extractors report.
    let settings = settings.clone();
    let initial = p.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, w as f64, h as f64), &cache, p.xref(), settings);
    let mut dev = TextDevice::new();
    interpret_page(p, &mut ctx, &mut dev);
    // A horizontal heading must not force a vertical body back into horizontal line grouping.
    // Group each writing direction independently, then join flows from their topmost position.
    let mut flows = Vec::new();
    for (quarter, mut glyphs) in dev.glyphs.into_iter().enumerate() {
        if glyphs.is_empty() {
            continue;
        }
        let top = glyphs.iter().map(|g| g.rect[1]).fold(f32::INFINITY, f32::min);
        for g in &mut glyphs {
            g.rect = to_upright(g.rect, quarter as u8, w, h);
        }
        let mut text = layout(glyphs);
        for g in &mut text.glyphs {
            g.rect = from_upright(g.rect, quarter as u8, w, h);
        }
        flows.push((top, text));
    }
    flows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut text = PageText::default();
    for (_, flow) in flows {
        let offset = text.line_of.last().map_or(0, |line| line.saturating_add(1));
        text.line_of.extend(flow.line_of.into_iter().map(|line| line.saturating_add(offset)));
        text.glyphs.extend(flow.glyphs);
        text.space_before.extend(flow.space_before);
    }
    Some(text)
}

/// View-space box → the frame where text runs left to right, for text running `quarter` × 90°
/// clockwise from that in a `w` × `h` view (1: text runs downwards).
fn to_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [x0, y0, x1, y1] = r;
    match quarter {
        1 => [y0, w - x1, y1, w - x0],
        2 => [w - x1, h - y1, w - x0, h - y0],
        3 => [h - y1, x0, h - y0, x1],
        _ => r,
    }
}

/// The inverse of [`to_upright`].
fn from_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [u0, v0, u1, v1] = r;
    match quarter {
        1 => [w - v1, u0, w - v0, u1],
        2 => [w - u1, h - v1, w - u0, h - v0],
        3 => [v0, h - u1, v1, h - u0],
        _ => r,
    }
}

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// Scripts written with explicit spaces whose combining signs can sit apart from their base
/// (Indic, Thai, Lao, Myanmar, Khmer): never infer a space inside a tight cluster.
fn is_complex(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3134F | 0x1100..=0x11FF)
}

struct Seg {
    idx: Vec<usize>,
    bbox: [f32; 4],
    h: f32,
}

impl Seg {
    fn cy(&self) -> f32 {
        (self.bbox[1] + self.bbox[3]) / 2.0
    }
}

fn union(a: &mut [f32; 4], b: &[f32; 4]) {
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

/// Reading-order layout. Glyphs are reordered so that `glyphs` is in reading order, lines are
/// numbered in that order, and word gaps are inferred per line.
///
/// Heuristics:
/// 1. *Segments:* consecutive glyphs (content order) on one baseline with no column-sized gap.
///    Segments on the same baseline that nearly touch are merged (text drawn out of order).
/// 2. *Blocks:* segments stacked with line-sized gaps and overlapping horizontally.
/// 3. *Block order:* repeatedly take the top-most remaining block, preferring a block to its left
///    that overlaps it vertically (so columns read left, then right).
/// 4. Within a segment glyphs run left to right; runs of right-to-left script are reversed so the
///    text comes out in logical order.
pub fn layout(glyphs: Vec<TextGlyph>) -> PageText {
    // Drop "fake bold" duplicates: the same character redrawn at (almost) the same place.
    let mut glyphs = glyphs;
    let mut keep = vec![true; glyphs.len()];
    for i in 1..glyphs.len() {
        let g = &glyphs[i];
        let h = (g.rect[3] - g.rect[1]).max(0.1);
        for j in (i.saturating_sub(4)..i).rev() {
            let p = &glyphs[j];
            if keep[j] && p.text == g.text && (p.rect[0] - g.rect[0]).abs() < h * 0.2 && (p.rect[1] - g.rect[1]).abs() < h * 0.2 {
                keep[i] = false;
                break;
            }
        }
    }
    if keep.iter().any(|k| !k) {
        let mut it = keep.iter();
        glyphs.retain(|_| *it.next().unwrap_or(&true));
    }
    let n = glyphs.len();
    if n == 0 {
        return PageText::default();
    }
    let height = |i: usize| (glyphs[i].rect[3] - glyphs[i].rect[1]).max(0.1);
    let cy = |i: usize| (glyphs[i].rect[1] + glyphs[i].rect[3]) / 2.0;

    // 1. Segments in content order.
    let mut segs: Vec<Seg> = Vec::new();
    for i in 0..n {
        let g = glyphs[i].rect;
        let cont = segs.last().is_some_and(|s| {
            let Some(&p) = s.idx.last() else { return false };
            let ph = height(p).min(height(i));
            let gap = g[0] - glyphs[p].rect[2];
            (cy(i) - cy(p)).abs() < ph * 0.5 && gap < ph * 3.0 && g[0] > glyphs[p].rect[0] - ph * 2.0
        });
        if cont {
            if let Some(s) = segs.last_mut() {
                s.idx.push(i);
                union(&mut s.bbox, &g);
                s.h = s.h.max(height(i));
            }
        } else {
            segs.push(Seg { idx: vec![i], bbox: g, h: height(i) });
        }
    }
    // Merge same-baseline segments that nearly touch.
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for a in 0..segs.len() {
            for b in 0..segs.len() {
                if a == b {
                    continue;
                }
                let (sa, sb) = (&segs[a], &segs[b]);
                let h = sa.h.min(sb.h);
                let gap = (sb.bbox[0] - sa.bbox[2]).max(sa.bbox[0] - sb.bbox[2]);
                if (sa.cy() - sb.cy()).abs() < h * 0.3 && (sa.h / sb.h - 1.0).abs() < 0.35 && gap < h * 1.2 {
                    let sb = segs.remove(b);
                    let a = if b < a { a - 1 } else { a };
                    let sa = &mut segs[a];
                    sa.idx.extend(sb.idx);
                    union(&mut sa.bbox, &sb.bbox);
                    sa.h = sa.h.max(sb.h);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    for s in &mut segs {
        s.idx.sort_by(|a, b| glyphs[*a].rect[0].total_cmp(&glyphs[*b].rect[0]));
    }

    // 2. Blocks.
    let mut by_top: Vec<usize> = (0..segs.len()).collect();
    by_top.sort_by(|a, b| segs[*a].bbox[1].total_cmp(&segs[*b].bbox[1]));
    let mut blocks: Vec<(Vec<usize>, [f32; 4])> = Vec::new();
    for si in by_top {
        let s = &segs[si];
        let target = blocks.iter().position(|(members, bb)| {
            let Some(&m) = members.last() else { return false };
            let last = &segs[m];
            let vgap = s.bbox[1] - last.bbox[3];
            let overlap = s.bbox[2].min(bb[2]) - s.bbox[0].max(bb[0]);
            let minw = (s.bbox[2] - s.bbox[0]).min(bb[2] - bb[0]).max(1.0);
            vgap > -last.h * 0.5 && vgap < last.h.max(s.h) * 1.1 && overlap > minw * 0.3 && (last.h / s.h - 1.0).abs() < 0.6
        });
        match target {
            Some(b) => {
                blocks[b].0.push(si);
                let bb = s.bbox;
                union(&mut blocks[b].1, &bb);
            }
            None => blocks.push((vec![si], s.bbox)),
        }
    }

    // 3. Block order.
    let mut remaining: Vec<usize> = (0..blocks.len()).collect();
    let mut block_order = Vec::with_capacity(blocks.len());
    while let Some(&top) = remaining.iter().min_by(|a, b| blocks[**a].1[1].total_cmp(&blocks[**b].1[1])) {
        let tb = blocks[top].1;
        let pick = remaining
            .iter()
            .copied()
            .filter(|c| {
                let cb = blocks[*c].1;
                let v = cb[3].min(tb[3]) - cb[1].max(tb[1]);
                cb[2] <= tb[0] + 1.0 && v > 0.5 * (cb[3] - cb[1]).min(tb[3] - tb[1])
            })
            .min_by(|a, b| blocks[*a].1[0].total_cmp(&blocks[*b].1[0]))
            .unwrap_or(top);
        remaining.retain(|r| *r != pick);
        block_order.push(pick);
    }

    // 4. Emit glyphs in reading order with line numbers and word gaps.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut line_of = Vec::with_capacity(n);
    let mut space_before = Vec::with_capacity(n);
    let mut line = 0u32;
    for b in block_order {
        for si in &blocks[b].0 {
            let s = &segs[*si];
            let mut idx = s.idx.clone();
            // Reverse right-to-left runs (visual → logical).
            let rtl = |i: usize| glyphs[i].text.chars().any(is_rtl);
            let mut k = 0;
            while k < idx.len() {
                if rtl(idx[k]) {
                    let mut e = k;
                    while e + 1 < idx.len() && (rtl(idx[e + 1]) || (glyphs[idx[e + 1]].text.trim().is_empty() && e + 2 < idx.len() && rtl(idx[e + 2]))) {
                        e += 1;
                    }
                    idx[k..=e].reverse();
                    k = e + 1;
                } else {
                    k += 1;
                }
            }
            // Word gaps relative to this line's typical letter gap (handles tracking).
            let mut gaps: Vec<f32> = s.idx.windows(2).map(|w| glyphs[w[1]].rect[0] - glyphs[w[0]].rect[2]).collect();
            gaps.sort_by(f32::total_cmp);
            let typical = gaps.get(gaps.len() / 3).copied().unwrap_or(0.0).max(0.0);
            let threshold = (typical + s.h * 0.15).max(s.h * 0.15);
            let mut spaces = vec![false; idx.len()];
            for w in 1..s.idx.len() {
                let (p, c) = (s.idx[w - 1], s.idx[w]);
                let gap = glyphs[c].rect[0] - glyphs[p].rect[2];
                let (pt, ct) = (&glyphs[p].text, &glyphs[c].text);
                let cjk = pt.chars().any(is_cjk) && ct.chars().any(is_cjk) && gap < s.h * 0.5;
                let tight_cluster = (pt.chars().any(is_complex) || ct.chars().any(is_complex)) && gap < s.h * 0.6;
                if gap > threshold && !cjk && !tight_cluster && !pt.trim().is_empty() && !ct.trim().is_empty() {
                    // Mark the space before glyph `c` wherever it ended up after RTL reversal.
                    if let Some(pos) = idx.iter().position(|x| *x == c.max(p)) {
                        spaces[pos] = true;
                    }
                }
            }
            for (k, g) in idx.iter().enumerate() {
                order.push(*g);
                line_of.push(line);
                space_before.push(k > 0 && spaces[k]);
            }
            line += 1;
        }
    }
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(Some).collect();
    // Each glyph is emitted exactly once.
    let glyphs = order.iter().filter_map(|i| slots.get_mut(*i).and_then(Option::take)).collect();
    PageText { glyphs, line_of, space_before }
}
