//! Reading-order paragraphs taken from a page's text layer.
//!
//! The text layer (`crate::pdf::text`) already groups glyphs into lines in reading order (columns are
//! handled for us). A paragraph is a run of consecutive lines that belong together:
//! same type size, normal leading, and no indentation change. The paragraph keeps its
//! box in page view space (points, y down), its type size and its line spacing, so the
//! translation can be shown in the paragraph's own format.
//!
//! A sentence that runs past a paragraph break — a column break, a page break — is put back
//! together by [`spans`], so the unit a reader picks and the model translates is the sentence,
//! not the fragment.

use crate::pdf::PageText;

/// One line of a paragraph: its text and its box in view space.
#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    /// `[x0, y0, x1, y1]` in page view space (points, y down).
    pub rect: [f32; 4],
}

impl Line {
    pub fn height(&self) -> f32 {
        (self.rect[3] - self.rect[1]).max(0.1)
    }
}

/// What a paragraph is, as the reader and the layout see it. Only body text carries an
/// article's sentences; a display line, a caption or page furniture may sit between two body
/// paragraphs of the same sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Article text.
    Body,
    /// Display type: a heading, a pull quote, a drop cap.
    Display,
    /// A figure or photo caption.
    Caption,
}

/// A paragraph of the document: the unit a translation is requested for.
#[derive(Clone, Debug)]
pub struct Paragraph {
    pub text: String,
    /// `[x0, y0, x1, y1]` in page view space (points, y down).
    pub rect: [f32; 4],
    /// Type size in points (the line box height).
    pub size: f32,
    /// Baseline step divided by the type size (1.0 = solid setting, 1.4 = normal text).
    pub line_spacing: f32,
    pub lines: Vec<Line>,
    /// Body text, display type, a caption or page furniture.
    pub kind: Kind,
}

impl Paragraph {
    /// Whether this is article text (the only text a sentence is built from).
    pub fn is_body(&self) -> bool {
        self.kind == Kind::Body
    }
}

/// Whitespace-normalised text, so re-extraction with different line breaks still matches.
pub fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// FNV-1a 64 of the normalised text, as hex. Stable across runs and toolchains.
pub fn content_key(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalized(text).as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Lines of the page with their text and boxes.
///
/// The text layer folds glyphs into lines, but on a page with several columns it can fold two
/// columns into one line when the gutter is narrow. Each line is therefore split again at
/// gaps too wide to be a word space (a column gutter), and at jumps back to the left (the
/// next column of a folded line).
fn lines_of(page: &PageText) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut start = 0usize;
    for i in 1..=page.glyphs.len() {
        let boundary = i == page.glyphs.len() || page.line_of.get(i) != page.line_of.get(start);
        if !boundary {
            continue;
        }
        if i > start {
            push_line_pieces(page, start, i, &mut lines);
        }
        start = i;
    }
    lines
}

/// Split one `line_of` run wherever the glyphs leave a column gutter.
fn push_line_pieces(page: &PageText, start: usize, end: usize, out: &mut Vec<Line>) {
    let mut piece = start;
    for i in (start + 1)..end {
        let previous = &page.glyphs[i - 1];
        let glyph = &page.glyphs[i];
        let height = (previous.rect[3] - previous.rect[1]).max(glyph.rect[3] - glyph.rect[1]).max(0.1);
        let gap = glyph.rect[0] - previous.rect[2];
        // A wide forward gap is a gutter between columns; a large jump backwards is the
        // next column of a line the layout folded together (RTL advances are far smaller).
        let gutter = gap > (height * 0.8).max(4.5);
        let jump_back = previous.rect[0] - glyph.rect[0] > height * 3.0;
        if gutter || jump_back {
            push_piece(page, piece, i, out);
            piece = i;
        }
    }
    push_piece(page, piece, end, out);
}

fn push_piece(page: &PageText, start: usize, end: usize, out: &mut Vec<Line>) {
    if start >= end {
        return;
    }
    let mut rect = page.glyphs[start].rect;
    let mut text = String::new();
    for i in start..end {
        let glyph = &page.glyphs[i];
        rect[0] = rect[0].min(glyph.rect[0]);
        rect[1] = rect[1].min(glyph.rect[1]);
        rect[2] = rect[2].max(glyph.rect[2]);
        rect[3] = rect[3].max(glyph.rect[3]);
        if i > start && page.space_before.get(i).copied().unwrap_or(false) && !text.ends_with(' ') {
            text.push(' ');
        }
        text.push_str(&glyph.text);
    }
    if !text.trim().is_empty() {
        out.push(Line { text, rect });
    }
}

/// A page number or a stray numeral: not worth a translation.
fn is_furniture(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() > 8 {
        return false;
    }
    trimmed.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '.' || c == '/' || c == ' ')
}

/// Whether a line is set sideways: a tall, narrow strip of Latin text (a rotated photo credit).
/// A vertical CJK column has the same shape but is real text, so it is not matched.
fn is_rotated(line: &Line) -> bool {
    let width = line.rect[2] - line.rect[0];
    let height = line.rect[3] - line.rect[1];
    height > width * 3.0 && height > 30.0 && !line.text.chars().any(|c| matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0x20000..=0x3134F))
}

/// The page's usual distance between two wrapped lines, in points: a paragraph break (and
/// the space under a subheading) is a gap clearly wider than this.
fn typical_gap(lines: &[Line]) -> f32 {
    let mut gaps: Vec<f32> = Vec::new();
    for pair in lines.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        let overlap = first.rect[2].min(second.rect[2]) - first.rect[0].max(second.rect[0]);
        if overlap > (first.rect[2] - first.rect[0]).min(second.rect[2] - second.rect[0]) * 0.3 {
            let gap = second.rect[1] - first.rect[3];
            if gap >= 0.0 && gap < (first.height() + second.height()) * 1.5 {
                gaps.push(gap);
            }
        }
    }
    if gaps.is_empty() {
        return 2.0;
    }
    gaps.sort_by(f32::total_cmp);
    gaps[gaps.len() / 2]
}

/// Whether `line` starts a new paragraph after the lines collected in `current`.
fn starts_new_paragraph(current: &[Line], line: &Line, typical: f32) -> bool {
    let Some(prev) = current.last() else {
        return true;
    };
    let (ph, lh) = (prev.height(), line.height());
    if !(ph > 0.0 && lh > 0.0) {
        return true;
    }
    let para_left = current.iter().map(|l| l.rect[0]).fold(f32::INFINITY, f32::min);
    let para_right = current.iter().map(|l| l.rect[2]).fold(f32::NEG_INFINITY, f32::max);
    // Two lines on the same visual line, the second continuing to the right, are one line split
    // in two (a bold run-in heading, or a line cut at a wide gap). The two must be set at the
    // same size: a rotated photo credit is a tall, narrow line *beside* a body line and must not
    // swallow it.
    if lh * 2.0 >= ph && ph * 2.0 >= lh && (line.rect[1] - prev.rect[1]).abs() <= lh.min(ph) * 0.5 && line.rect[0] >= prev.rect[2] - lh {
        return false;
    }
    // Text wrapped beside a drop cap: the previous line is much taller than this one (it
    // carries the cap) but still a wide text line. A narrow, tall box is instead rotated
    // text (a photo credit) and must not swallow the line beside it.
    if ph > lh * 1.6 && prev.rect[2] - prev.rect[0] > ph && line.rect[1] < prev.rect[3] - lh * 0.4 && line.rect[1] > prev.rect[1] - ph {
        return false;
    }
    // Extra leading between lines (a paragraph break, not a wrapped line). A small tolerance
    // keeps a block set a hair looser than the page's median leading from breaking apart.
    let gap = line.rect[1] - prev.rect[3];
    // A line far above the one before it is a jump to another column or block, never a wrapped
    // line (the reading order goes top to bottom within a column).
    if gap < -(ph.max(lh)) {
        return true;
    }
    if gap > (typical * 1.8).max(ph * 0.25) {
        return true;
    }
    // A change of type size (a heading, a caption).
    if (lh - ph).abs() > ph * 0.28 {
        return true;
    }
    // The previous line was the short last line of a paragraph. The next line may start
    // flush left, or with a first-line indent of any size up to a line height (the indent
    // rule below covers the larger ones), so both conditions together break the paragraph.
    if prev.rect[2] < para_right - lh * 2.0 && line.rect[0] <= para_left + lh * 1.0 {
        return true;
    }
    // A first-line indent (a paragraph set without extra leading), or a line pulled out to
    // the left (a new column or list).
    if line.rect[0] > para_left + lh {
        return true;
    }
    // A line pulled out to the left of the paragraph's edge starts a new one (a new column,
    // a list). Not while the paragraph is just its indented first line: the body's first
    // line then correctly sits to the left of that indent.
    if current.len() >= 2 && line.rect[0] < para_left - lh * 1.2 {
        return true;
    }
    // No horizontal overlap with the paragraph at all.
    let overlap = line.rect[2].min(para_right) - line.rect[0].max(para_left);
    overlap <= 0.0
}

fn join_lines(lines: &[Line]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let piece = line.text.trim();
        if i > 0 {
            let dehyphenate = out.ends_with('-') && piece.chars().next().is_some_and(|c| c.is_lowercase());
            if dehyphenate {
                out.pop();
            } else {
                out.push(' ');
            }
        }
        out.push_str(piece);
    }
    out
}

fn to_paragraph(lines: Vec<Line>) -> Option<Paragraph> {
    let text = join_lines(&lines);
    if text.trim().chars().count() < 2 {
        return None;
    }
    let mut rect = lines[0].rect;
    for line in &lines[1..] {
        rect[0] = rect[0].min(line.rect[0]);
        rect[1] = rect[1].min(line.rect[1]);
        rect[2] = rect[2].max(line.rect[2]);
        rect[3] = rect[3].max(line.rect[3]);
    }
    let size = {
        let mut heights: Vec<f32> = lines.iter().map(Line::height).collect();
        heights.sort_by(f32::total_cmp);
        heights[heights.len() / 2]
    };
    // Baseline step (line box top to line box top) over the type size.
    let line_spacing = if lines.len() >= 2 {
        let step = (lines[lines.len() - 1].rect[1] - lines[0].rect[1]) / (lines.len() - 1) as f32;
        (step / size).clamp(1.0, 2.5)
    } else {
        1.35
    };
    Some(Paragraph { text, rect, size, line_spacing, lines, kind: Kind::Body })
}

fn column_root(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

/// The columns of a page, as their horizontal extents, left to right.
///
/// Column margins are the x positions where many lines start. Lines are grouped by their
/// nearest margin, and groups whose extents overlap (an indented block, a paragraph that
/// starts further in) are merged, so only genuinely side-by-side columns survive. Groups
/// with only a line or two — a marginal note, vertical text — are dropped and their lines
/// fall back to the nearest column.
fn detect_columns(lines: &[Line]) -> Vec<[f32; 2]> {
    if lines.is_empty() {
        return Vec::new();
    }
    // Frequent left edges.
    let mut starts: Vec<f32> = lines.iter().map(|line| line.rect[0]).collect();
    starts.sort_by(f32::total_cmp);
    let mut lefts: Vec<f32> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut index = 0;
    while index < starts.len() {
        let begin = index;
        while index < starts.len() && starts[index] - starts[begin] <= 3.0 {
            index += 1;
        }
        lefts.push(starts[begin..index].iter().sum::<f32>() / (index - begin) as f32);
        counts.push(index - begin);
    }
    let min_count = ((lines.len() as f32) * 0.05).ceil().max(2.0) as usize;
    let mut margins: Vec<f32> = Vec::new();
    let mut slot_counts: Vec<usize> = Vec::new();
    for (x, count) in lefts.iter().zip(&counts) {
        if *count >= min_count {
            margins.push(*x);
            slot_counts.push(*count);
        }
    }
    if margins.is_empty() {
        margins.push(starts[0]);
        slot_counts.push(lines.len());
    }

    let nearest = |line: &Line| -> usize {
        let mut best = 0;
        let mut best_distance = f32::INFINITY;
        for (slot, x) in margins.iter().enumerate() {
            let distance = (line.rect[0] - x).abs();
            if distance < best_distance {
                best_distance = distance;
                best = slot;
            }
        }
        best
    };

    // The extent of a column is its *typical* line, not the widest one: a caption or a
    // credit line that reaches across two columns must not stretch the column it starts
    // in (it would then swallow the next column's body lines).
    let slots = margins.len();
    let mut starts_per_slot: Vec<Vec<f32>> = vec![Vec::new(); slots];
    let mut ends_per_slot: Vec<Vec<f32>> = vec![Vec::new(); slots];
    for line in lines {
        let slot = nearest(line);
        starts_per_slot[slot].push(line.rect[0]);
        ends_per_slot[slot].push(line.rect[2]);
    }
    let mut range: Vec<[f32; 2]> = vec![[f32::INFINITY, f32::NEG_INFINITY]; slots];
    for slot in 0..slots {
        if starts_per_slot[slot].is_empty() {
            continue;
        }
        starts_per_slot[slot].sort_by(f32::total_cmp);
        ends_per_slot[slot].sort_by(f32::total_cmp);
        range[slot] = [starts_per_slot[slot][starts_per_slot[slot].len() / 2], ends_per_slot[slot][ends_per_slot[slot].len() / 2]];
    }
    let mut parent: Vec<usize> = (0..slots).collect();
    for a in 0..slots {
        for b in (a + 1)..slots {
            if !range[a][0].is_finite() || !range[b][0].is_finite() {
                continue;
            }
            let overlap = range[a][1].min(range[b][1]) - range[a][0].max(range[b][0]);
            let narrow = (range[a][1] - range[a][0]).min(range[b][1] - range[b][0]);
            if narrow > 0.0 && overlap > narrow * 0.5 {
                let (root_a, root_b) = (column_root(&mut parent, a), column_root(&mut parent, b));
                if root_a != root_b {
                    parent[root_a] = root_b;
                }
            }
        }
    }

    let mut merged: Vec<[f32; 2]> = Vec::new();
    let mut merged_count: Vec<usize> = Vec::new();
    let mut which: Vec<usize> = vec![usize::MAX; slots];
    for slot in 0..slots {
        if !range[slot][0].is_finite() {
            continue;
        }
        let root = column_root(&mut parent, slot);
        if which[root] == usize::MAX {
            which[root] = merged.len();
            merged.push([f32::INFINITY, f32::NEG_INFINITY]);
            merged_count.push(0);
        }
        let target = which[root];
        merged[target][0] = merged[target][0].min(range[slot][0]);
        merged[target][1] = merged[target][1].max(range[slot][1]);
        merged_count[target] += slot_counts[slot];
    }

    // A column needs more than a line or two to be a column.
    let strong: Vec<[f32; 2]> = merged.iter().zip(&merged_count).filter(|(_, count)| **count >= 3).map(|(range, _)| *range).collect();
    let mut columns = if strong.is_empty() { merged } else { strong };
    columns.sort_by(|a, b| a[0].total_cmp(&b[0]));
    columns
}

/// The column a box belongs to, or `None` when it reaches across two columns (a heading or an
/// image caption spanning the page).
pub fn column_of_rect(rect: [f32; 4], columns: &[[f32; 2]]) -> Option<usize> {
    let mut covered = 0;
    for column in columns {
        let overlap = rect[2].min(column[1]) - rect[0].max(column[0]);
        let width = (column[1] - column[0]).max(1.0);
        if overlap > width * 0.3 {
            covered += 1;
        }
    }
    if covered >= 2 {
        return None;
    }
    let mut best = 0;
    let mut best_distance = f32::INFINITY;
    for (slot, column) in columns.iter().enumerate() {
        let distance = (rect[0] - column[0]).abs();
        if distance < best_distance {
            best_distance = distance;
            best = slot;
        }
    }
    Some(best)
}

/// The column a line belongs to.
fn assign_column(line: &Line, columns: &[[f32; 2]]) -> Option<usize> {
    column_of_rect(line.rect, columns)
}

/// Whether two boxes overlap horizontally at all.
fn overlaps_x(a: &Line, b: &Line) -> bool {
    a.rect[2].min(b.rect[2]) - a.rect[0].max(b.rect[0]) > 0.0
}

/// Vertical centre of a line.
fn center_y(line: &Line) -> f32 {
    (line.rect[1] + line.rect[3]) / 2.0
}

/// Reading order for a page's lines. The text layer groups glyphs into lines but, on a page
/// whose columns were folded into one block, it emits them baseline by baseline (left,
/// right, left, right…), which would fuse two columns into a single paragraph.
///
/// Lines are read column by column, top to bottom. A line that spans several columns (a
/// headline, a caption) only separates the columns it actually covers: a column beside it
/// keeps flowing, so an article that runs past a headline stays one paragraph.
pub fn order_lines(lines: Vec<Line>) -> Vec<Line> {
    let count = lines.len();
    if count < 4 {
        return lines;
    }
    let columns = detect_columns(&lines);
    if columns.len() < 2 {
        let mut out = lines;
        out.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]).then(a.rect[0].total_cmp(&b.rect[0])));
        return out;
    }
    let column_of: Vec<Option<usize>> = lines.iter().map(|line| assign_column(line, &columns)).collect();
    let separator: Vec<bool> = column_of.iter().map(Option::is_none).collect();
    let spanning: Vec<usize> = (0..count).filter(|index| separator[*index]).collect();

    // A spanning line in the top band of the page (a headline or standfirst over the columns)
    // opens the page: it separates *every* column, because the text beside it belongs to the
    // article under it, not before it. A mid-page spanning line only separates the columns its
    // box actually covers, so a column beside it keeps flowing.
    let top = lines.iter().map(|line| line.rect[1]).fold(f32::INFINITY, f32::min);
    let bottom = lines.iter().map(|line| line.rect[3]).fold(f32::NEG_INFINITY, f32::max);
    let header = |index: usize| separator[index] && lines[index].rect[1] <= top + (bottom - top) * 0.15;
    let band_of: Vec<usize> = (0..count)
        .map(|index| {
            let line = &lines[index];
            spanning
                .iter()
                .filter(|other| {
                    let other = **other;
                    if other == index {
                        return false;
                    }
                    let span = &lines[other];
                    if header(other) {
                        // Never push another separator: a headline and the standfirst under it
                        // belong together, ahead of the columns.
                        !separator[index]
                    } else {
                        overlaps_x(line, span) && span.rect[3] <= center_y(line)
                    }
                })
                .count()
        })
        .collect();
    let bands = band_of.iter().copied().max().map_or(0, |max| max + 1);

    let mut out = Vec::with_capacity(count);
    for band in 0..bands {
        let mut members: Vec<usize> = (0..count).filter(|index| band_of[*index] == band && !separator[*index]).collect();
        members.sort_by(|a, b| {
            column_of[*a].cmp(&column_of[*b]).then(lines[*a].rect[1].total_cmp(&lines[*b].rect[1])).then(lines[*a].rect[0].total_cmp(&lines[*b].rect[0]))
        });
        out.extend(members.into_iter().map(|index| lines[index].clone()));

        // The spanning lines that open the next band close this one, so they stay between the
        // text above and the text below.
        let mut separators: Vec<usize> = (0..count).filter(|index| band_of[*index] == band && separator[*index]).collect();
        separators.sort_by(|a, b| lines[*a].rect[1].total_cmp(&lines[*b].rect[1]).then(lines[*a].rect[0].total_cmp(&lines[*b].rect[0])));
        out.extend(separators.into_iter().map(|index| lines[index].clone()));
    }
    out
}

/// The page's lines in the order the paragraphs are built from (diagnostics).
pub fn page_lines(page: &PageText) -> Vec<Line> {
    order_lines(lines_of(page))
}

/// The columns detected on a page, left to right (diagnostics).
pub fn page_columns(page: &PageText) -> Vec<[f32; 2]> {
    detect_columns(&lines_of(page))
}

/// Paragraphs of one page, in reading order.
pub fn extract(page: &PageText) -> Vec<Paragraph> {
    let mut out: Vec<Paragraph> = Vec::new();
    let mut current: Vec<Line> = Vec::new();
    let raw = lines_of(page);
    let columns = detect_columns(&raw);
    let lines = order_lines(raw);
    let typical = typical_gap(&lines);
    for line in lines {
        // A rotated credit is not prose, and it must not break the sentence it sits beside.
        if is_rotated(&line) {
            continue;
        }
        if is_furniture(&line.text) {
            if let Some(paragraph) = to_paragraph(std::mem::take(&mut current)) {
                out.push(paragraph);
            }
            continue;
        }
        if !current.is_empty() && starts_new_paragraph(&current, &line, typical) {
            if let Some(paragraph) = to_paragraph(std::mem::take(&mut current)) {
                out.push(paragraph);
            }
        }
        current.push(line);
    }
    if let Some(paragraph) = to_paragraph(current) {
        out.push(paragraph);
    }
    align_to_columns(&mut out, &columns);
    classify(&mut out);
    out
}

/// A paragraph's box starts at the ink of its first glyph, which jitters by a fraction of a
/// point (`C` begins further left than `L`). Snapping the left edge to the column margin it
/// sits on keeps the markers beside consecutive paragraphs in line.
fn align_to_columns(paragraphs: &mut [Paragraph], columns: &[[f32; 2]]) {
    const SNAP: f32 = 2.5;
    for paragraph in paragraphs {
        let left = paragraph.rect[0];
        let nearest = columns.iter().map(|column| column[0]).min_by(|a, b| (left - *a).abs().total_cmp(&(left - *b).abs()));
        if let Some(margin) = nearest.filter(|margin| (left - margin).abs() <= SNAP) {
            paragraph.rect[0] = margin;
        }
    }
}

/// Label every paragraph of a page and drop the page furniture, so the article's text is what
/// remains. A running head or a folio sits in the margin in type smaller than the body; a
/// section label is short and all caps; a vertical photo credit is a degenerate strip.
fn classify(paragraphs: &mut Vec<Paragraph>) {
    if paragraphs.is_empty() {
        return;
    }
    let body = body_size(paragraphs);
    // The text block's extent, from the body-sized paragraphs only: a running head in small
    // type must not drag the top margin down past a section label that sits at the block's top.
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    let mut block = false;
    for paragraph in paragraphs.iter().filter(|paragraph| paragraph.size >= body * 0.92 && paragraph.size <= body * 1.25) {
        block = true;
        top = top.min(paragraph.rect[1]);
        bottom = bottom.max(paragraph.rect[3]);
    }
    if !block {
        for paragraph in paragraphs.iter() {
            top = top.min(paragraph.rect[1]);
            bottom = bottom.max(paragraph.rect[3]);
        }
    }
    let margin = body * 2.0;
    let mut out = Vec::with_capacity(paragraphs.len());
    for mut paragraph in paragraphs.drain(..) {
        let width = paragraph.rect[2] - paragraph.rect[0];
        let height = paragraph.rect[3] - paragraph.rect[1];
        let characters = paragraph.text.chars().count();
        let letters_are_caps = paragraph.text.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
        let at_margin = paragraph.rect[1] <= top + margin || paragraph.rect[3] >= bottom - margin;
        // A vertical strip: a photo credit set sideways. A big headline is wide, so it is kept
        // (and labelled Display below); only a tall, narrow strip is furniture.
        let degenerate = width < body * 0.9 && height > body * 4.0;
        // A short line of capitals is a section label or a kicker ("FDP-KRISE"), never body text.
        let label = characters <= 60 && letters_are_caps;
        // A one-line heading set a little larger than the body ("Journalismus, der Lösungen
        // vorstellt", "Teil 2: Annäherung") is display type: a sentence must not run through it.
        let kicker = paragraph.lines.len() <= 1 && characters <= 80 && paragraph.size - body > (body * 0.02).max(0.15);
        let furniture = degenerate
            || (at_margin && characters <= 60 && (paragraph.size < body * 0.9 || letters_are_caps));
        if furniture {
            continue;
        }
        paragraph.kind = if label || kicker || paragraph.size > body * 1.25 {
            Kind::Display
        } else if body - paragraph.size > (body * 0.05).max(0.45) {
            Kind::Caption
        } else {
            Kind::Body
        };
        out.push(paragraph);
    }
    *paragraphs = out;
}

/// The page's body type size: the size that carries most of the text. Sizes are bucketed to
/// half a point and weighted by characters, so a page's body dominates its headings, captions
/// and furniture.
fn body_size(paragraphs: &[Paragraph]) -> f32 {
    // Sizes are bucketed to half a point and weighted by characters, so the page's body wins.
    // The bucket's *mean* size is returned (not its rounded key), so a body set at 9.25 is not
    // reported as 9.5 and its 9.5 pt headings still stand out.
    let mut buckets: Vec<(i32, usize, f32)> = Vec::new();
    for paragraph in paragraphs {
        let key = (paragraph.size * 2.0).round() as i32;
        let weight = paragraph.text.chars().count().max(1);
        match buckets.iter_mut().find(|(bucket, _, _)| *bucket == key) {
            Some((_, total, sum)) => {
                *total += weight;
                *sum += paragraph.size * weight as f32;
            }
            None => buckets.push((key, weight, paragraph.size * weight as f32)),
        }
    }
    buckets.into_iter().max_by_key(|(_, weight, _)| *weight).map_or(10.0, |(_, weight, sum)| sum / weight as f32)
}

/// One flag per paragraph: whether it is article text (the only text a sentence is built from).
pub fn body_mask(paragraphs: &[Paragraph]) -> Vec<bool> {
    paragraphs.iter().map(Paragraph::is_body).collect()
}

/// Punctuation that closes a sentence. A paragraph ending with one of these has finished the
/// last sentence it carries, so nothing runs on into the paragraph after it.
const TERMINATORS: [char; 7] = ['.', '!', '?', '…', '。', '！', '？'];

/// Quotes and brackets that belong to the sentence ending behind them.
const CLOSERS: [char; 12] = ['"', '\'', '”', '’', '»', '«', '›', ')', ']', '}', '」', '』'];

/// Opening quotes and brackets that belong to the sentence starting behind them. The guillemets
/// are listed in both sets: German sets them `»…«`, French `«…»`, and the extractor is not told
/// which language the document is in.
const OPENERS: [char; 12] = ['"', '\'', '“', '‘', '„', '»', '«', '‹', '(', '[', '「', '『'];

/// The last character of `text`, ignoring trailing quotes and brackets.
fn last_significant(text: &str) -> Option<char> {
    let mut chars = text.trim_end().chars().rev().peekable();
    while chars.peek().is_some_and(|c| CLOSERS.contains(c)) {
        chars.next();
    }
    chars.next()
}

/// The first character of `text`, ignoring a leading quote or bracket.
fn first_significant(text: &str) -> Option<char> {
    text.trim_start().trim_start_matches(|c| OPENERS.contains(&c)).chars().next()
}

/// The text without a trailing author's byline — two to four capitals set after the last full
/// stop ("… forderte. NIE"). The sentence ends at the stop before them.
fn without_byline(text: &str) -> &str {
    let trimmed = text.trim_end();
    let Some((head, tail)) = trimmed.rsplit_once(char::is_whitespace) else { return trimmed };
    let size = tail.chars().count();
    if (2..=4).contains(&size) && tail.chars().all(|c| c.is_ascii_uppercase()) {
        head.trim_end()
    } else {
        trimmed
    }
}

/// Whether a paragraph closes the last sentence it holds: it ends with a full stop, an
/// exclamation or a question mark, perhaps behind a closing quote or bracket (and perhaps
/// behind an author's byline).
pub fn ends_sentence(text: &str) -> bool {
    match last_significant(without_byline(text)) {
        Some(last) => TERMINATORS.contains(&last),
        None => true,
    }
}

/// Whether a paragraph leaves its last sentence open: it has text and does not end on terminal
/// punctuation. A heading and a mid-sentence fragment both look like this.
fn unfinished(paragraph: &Paragraph) -> bool {
    !paragraph.text.trim().is_empty() && !ends_sentence(&paragraph.text)
}

/// Whether the sentence of `above` runs on into `below`, two paragraphs adjacent in reading
/// order (the paragraph below, the top of the next column, or the top of the next page).
///
/// Ending on a comma, a dash or a colon plainly leaves the sentence open. Ending on a
/// lower-case (or caseless, as in CJK) word is only taken for a cut when `below` opens
/// mid-sentence too: a heading carries no full stop either, but the body under it starts a new
/// sentence with a capital.
pub fn continues_into(above: &Paragraph, below: &Paragraph) -> bool {
    if !unfinished(above) {
        return false;
    }
    let Some(last) = last_significant(&above.text) else {
        return false;
    };
    if matches!(last, ',' | ';' | ':' | '-' | '–' | '—' | '(' | '[' | '«') {
        return true;
    }
    first_significant(&below.text).is_some_and(|first| !first.is_uppercase())
}

/// Whether the sentence of `above` runs on into `below` across a column or page break, where
/// `below` sits at the top of a column.
///
/// Such a break is a stronger hint than a paragraph break: a sentence cut at a column's bottom
/// carries on at the next column's top even when its next word is a name that begins with a
/// capital. The heading case is only a risk *within* a column, where the next paragraph after a
/// title starts a new sentence — at a column top the text is the tail of the column before.
pub fn continues_at_boundary(above: &Paragraph, below: &Paragraph) -> bool {
    if !unfinished(above) || below.text.trim().is_empty() {
        return false;
    }
    let Some(last) = last_significant(&above.text) else {
        return false;
    };
    matches!(last, ',' | ';' | ':' | '-' | '–' | '—' | '(' | '[' | '«') || last.is_alphabetic()
}

/// Whether the sentence of `above` runs on into `below` across a **page** break.
///
/// A lower-case opening is the continuation. A capital is one too when the next page carries on
/// at the same type size — German capitalises its nouns and names (`… um ein paar` / `Ecken …`),
/// so the case alone cannot decide. A bigger size is a new article's standfirst or an advert
/// (`Als Einzelheft oder im Abo…`, `Sicherheit Wie der Wiener…`) and is rejected. A clause cut at
/// a comma, dash or colon is taken for a continuation whatever follows.
pub fn continues_across_page(above: &Paragraph, below: &Paragraph) -> bool {
    if !unfinished(above) || below.text.trim().is_empty() {
        return false;
    }
    let Some(last) = last_significant(&above.text) else {
        return false;
    };
    let first = first_significant(&below.text);
    let same_size = (below.size - above.size).abs() <= above.size * 0.08;
    match last {
        '-' | '‐' => first.is_some_and(|first| first.is_lowercase()) || same_size,
        ',' | ';' | ':' | '–' | '—' | '(' | '[' | '«' => true,
        // A word or sentence carried over the page break is set at the same size; a different
        // size is another article's text (or an advert) that merely starts lower-case by chance.
        _ => last.is_alphabetic() && !last.is_uppercase() && same_size,
    }
}


/// The sentence every paragraph belongs to, over only the paragraphs `keep` selects: a
/// caption, a figure or page furniture between two body paragraphs of one sentence does not
/// break the run. A paragraph `keep` leaves out is a run of its own.
///
/// `boundary[i]` marks a paragraph that sits at the top of a column: the step into it uses the
/// column-break rule, which lets a name that begins with a capital continue the sentence.
pub fn spans_filtered(paragraphs: &[Paragraph], keep: &[bool], boundary: &[bool]) -> Vec<(usize, usize)> {
    let selected: Vec<usize> = (0..paragraphs.len()).filter(|index| keep.get(*index).copied().unwrap_or(false)).collect();
    let runs = spans_of(paragraphs, &selected, boundary);
    let mut spans: Vec<(usize, usize)> = (0..paragraphs.len()).map(|index| (index, index)).collect();
    for (slot, &index) in selected.iter().enumerate() {
        spans[index] = runs[slot];
    }
    spans
}

/// The runs over a selected sequence of paragraphs, as `(first, last)` paragraph indices.
fn spans_of(paragraphs: &[Paragraph], selected: &[usize], boundary: &[bool]) -> Vec<(usize, usize)> {
    let count = selected.len();
    let mut runs: Vec<(usize, usize)> = (0..count).map(|slot| (selected[slot], selected[slot])).collect();
    let runs_on: Vec<bool> = (0..count)
        .map(|slot| {
            slot + 1 < count && {
                let above = &paragraphs[selected[slot]];
                let below = &paragraphs[selected[slot + 1]];
                if boundary.get(selected[slot + 1]).copied().unwrap_or(false) {
                    continues_at_boundary(above, below)
                } else {
                    continues_into(above, below)
                }
            }
        })
        .collect();
    // A paragraph starts a run unless the one before it runs on into it.
    let mut start = 0;
    for slot in 0..count {
        if slot > 0 && !runs_on[slot - 1] {
            start = slot;
        }
        runs[slot].0 = selected[start];
    }
    // Walking from the right, a paragraph that runs on shares the end of the one after it.
    let mut end = count.saturating_sub(1);
    for slot in (0..count).rev() {
        if runs_on[slot] {
            runs[slot].1 = selected[end];
        } else {
            end = slot;
            runs[slot].1 = selected[slot];
        }
    }
    runs
}

/// Join paragraphs translated as one, the way a paragraph's own lines are joined: a word broken
/// over the break is put back together, otherwise a space separates them.
pub fn join_paragraph_text(paragraphs: &[&Paragraph]) -> String {
    let mut out = String::new();
    for paragraph in paragraphs {
        let piece = paragraph.text.trim();
        if out.is_empty() {
            out.push_str(piece);
            continue;
        }
        if out.ends_with('-') && piece.chars().next().is_some_and(|c| c.is_lowercase()) {
            out.pop();
        } else {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

/// The text of a run of paragraphs translated as one.
pub fn span_text(paragraphs: &[Paragraph], span: (usize, usize)) -> String {
    let refs: Vec<&Paragraph> = paragraphs[span.0..=span.1].iter().collect();
    join_paragraph_text(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(text: &str) -> Paragraph {
        Paragraph { text: text.to_string(), rect: [0.0, 0.0, 100.0, 10.0], size: 10.0, line_spacing: 1.35, lines: Vec::new(), kind: Kind::Body }
    }

    fn paragraph_at(text: &str, rect: [f32; 4], size: f32) -> Paragraph {
        Paragraph { text: text.to_string(), rect, size, line_spacing: 1.35, lines: Vec::new(), kind: Kind::Body }
    }

    /// The page's runs with nothing filtered out, as the sentence tests want.
    fn spans(paragraphs: &[Paragraph]) -> Vec<(usize, usize)> {
        spans_filtered(paragraphs, &vec![true; paragraphs.len()], &vec![false; paragraphs.len()])
    }

    #[test]
    fn furniture_is_dropped_and_kinds_are_labelled() {
        let mut page = vec![
            paragraph_at("DER SPIEGEL 39 | 2026", [40.0, 5.0, 120.0, 12.0], 7.5),
            paragraph_at("Body text that starts here", [40.0, 40.0, 200.0, 130.0], 9.2),
            paragraph_at("A caption under a photo", [40.0, 300.0, 200.0, 320.0], 8.0),
            paragraph_at("and the sentence carries on", [40.0, 340.0, 200.0, 400.0], 9.2),
            paragraph_at("A Heading", [40.0, 500.0, 200.0, 520.0], 14.0),
        ];
        classify(&mut page);
        // The tiny running head in the top margin is gone; the rest is labelled.
        assert_eq!(page.len(), 4);
        assert_eq!(page[0].kind, Kind::Body);
        assert_eq!(page[1].kind, Kind::Caption);
        assert_eq!(page[2].kind, Kind::Body);
        assert_eq!(page[3].kind, Kind::Display);
        // A caption between two body fragments of one sentence does not break the run.
        let spans = spans_filtered(&page, &body_mask(&page), &vec![false; page.len()]);
        assert_eq!(spans, vec![(0, 2), (1, 1), (0, 2), (3, 3)]);
    }

    #[test]
    fn a_closed_sentence_ends_the_run() {
        assert!(ends_sentence("The sentence stops here."));
        assert!(ends_sentence("Did it?"));
        assert!(ends_sentence("A quotation.\""));
        // German guillemets close on « (SPIEGEL sets »…«); French on ».
        assert!(ends_sentence("»Eine Frau muss eigenes Geld haben.«"));
        assert!(!ends_sentence("»Eine Frau muss eigenes Geld haben«"));
        assert!(ends_sentence("entmutigt mich das dermaßen.«"));
        assert!(ends_sentence("«La phrase est finie.»"));
        assert!(!ends_sentence("A heading without a stop"));
        assert!(!ends_sentence("A clause,"));
    }

    #[test]
    fn an_advertisement_is_not_a_page_continuation() {
        let article = |text: &str| paragraph_at(text, [0.0, 0.0, 100.0, 10.0], 9.2);
        let cut = article("in schweren Fällen wengis-");
        // A page that opens an advert or a new article is set larger: not the tail of the cut word.
        assert!(!continues_across_page(&cut, &paragraph_at("Als Einzelheft oder im Abo erhältlich", [0.0, 0.0, 100.0, 14.0], 12.9)));
        assert!(!continues_across_page(&cut, &paragraph_at("Sicherheit zuerst.", [0.0, 0.0, 100.0, 11.0], 10.0)));
        assert!(continues_across_page(&cut, &article("tens weiter.")));
        // A capitalised German noun at the same size carries the sentence on across the page.
        assert!(continues_across_page(&article("mit der Benhamous Frau um ein paar"), &article("Ecken verbandelt sein soll.")));
        // A clause cut at a comma still carries on whatever the next page opens with.
        assert!(continues_across_page(&article("wie folgt,"), &article("Ecken zuerst.")));
        // A lower-case opening only continues the sentence at the same type size; a different
        // size is another article (or the line under an advert) that happens to start lower-case.
        assert!(continues_across_page(&article("Am Ende der Geschichte"), &article("geschuetzte Stadtsilhouette")));
        assert!(!continues_across_page(&article("Am Ende der Geschichte"), &paragraph_at("geschuetzte Stadtsilhouette", [0.0, 0.0, 100.0, 14.0], 12.9)));
    }

    #[test]
    fn a_sentence_broken_over_a_column_is_one_run() {
        let page = vec![
            paragraph("The report ends here and"),
            paragraph("continues in the second column,"),
            paragraph("where the sentence finally stops."),
            paragraph("A new paragraph starts here."),
        ];
        let spans = spans(&page);
        assert_eq!(spans, vec![(0, 2), (0, 2), (0, 2), (3, 3)]);
        assert_eq!(span_text(&page, spans[0]), "The report ends here and continues in the second column, where the sentence finally stops.");
    }

    #[test]
    fn a_heading_is_not_glued_to_its_body() {
        let page = vec![paragraph("Introduction"), paragraph("The body begins with a capital.")];
        assert_eq!(spans(&page), vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn a_kicker_a_hair_larger_than_the_body_is_display() {
        let body = "Ein laengerer Absatz Fliesstext, damit die Grundschrift die Seite bestimmt. ".repeat(8);
        let mut page = vec![
            paragraph_at(&body, [40.0, 100.0, 200.0, 300.0], 9.25),
            paragraph_at("Journalismus, der Loesungen vorstellt", [40.0, 320.0, 200.0, 330.0], 9.5),
            paragraph_at("Ein Bildtext unter einer Grafik", [40.0, 500.0, 200.0, 509.0], 8.6),
            paragraph_at("Noch ein Absatz Fliesstext.", [40.0, 520.0, 200.0, 530.0], 9.25),
        ];
        classify(&mut page);
        // The one-line advert/subhead is display type and the small photo caption a caption, so
        // neither takes part in the article's sentences.
        let kinds: Vec<Kind> = page.iter().map(|paragraph| paragraph.kind).collect();
        assert_eq!(kinds, vec![Kind::Body, Kind::Display, Kind::Caption, Kind::Body]);
    }

    #[test]
    fn a_column_break_takes_a_capital_next_word() {
        let above = paragraph("The sentence is cut here");
        let below = paragraph("Müller carries on.");
        // Within one column this is read as a title and a new sentence …
        assert!(!continues_into(&above, &below));
        // … but at a column top it is the tail of the sentence the column before was cut on.
        assert!(continues_at_boundary(&above, &below));
        assert!(!continues_at_boundary(&paragraph("A finished sentence."), &below));
    }


    #[test]
    fn a_word_broken_over_the_break_is_rejoined() {
        let page = vec![paragraph("The word contin-"), paragraph("ues without a stop.")];
        assert_eq!(span_text(&page, (0, 1)), "The word continues without a stop.");
    }

    #[test]
    fn a_byline_does_not_leave_the_sentence_open() {
        assert!(ends_sentence("Das forderte der Abgeordnete. NIE"));
        assert!(!ends_sentence("Das forderte der Abgeordnete"));
        // A trailing acronym is not a byline.
        assert!(!ends_sentence("Er lebt in den USA"));
    }

    #[test]
    fn a_section_kicker_is_display_not_body() {
        let mut page = vec![
            paragraph_at("DER SPIEGEL 39 | 2026", [40.0, 5.0, 120.0, 12.0], 7.5),
            paragraph_at("DEMOKRATIECHECK", [55.0, 40.0, 144.0, 50.0], 10.0),
            paragraph_at("Body text of the article", [40.0, 300.0, 200.0, 400.0], 9.2),
            paragraph_at("FDP-KRISE", [40.0, 500.0, 90.0, 510.0], 10.0),
            paragraph_at("Another body paragraph", [40.0, 520.0, 200.0, 600.0], 9.2),
        ];
        classify(&mut page);
        // The running head and the section label at the top of the text block are furniture; the
        // mid-page kicker stays, but as display type, never as body text a sentence is built from.
        let kinds: Vec<(Kind, &str)> = page.iter().map(|paragraph| (paragraph.kind, paragraph.text.as_str())).collect();
        assert_eq!(kinds, vec![(Kind::Body, "Body text of the article"), (Kind::Display, "FDP-KRISE"), (Kind::Body, "Another body paragraph")]);
    }

    #[test]
    fn a_headline_at_the_top_opens_the_page() {
        let line = |x0: f32, y0: f32, x1: f32, text: &str| Line { text: text.to_string(), rect: [x0, y0, x1, y0 + 10.0] };
        let mut lines = Vec::new();
        // Column 3 is full height, beside the headline.
        for i in 0..6 {
            lines.push(line(400.0, 60.0 + i as f32 * 12.0, 560.0, "c3"));
        }
        // A headline spans columns 1-2 at the top, with their text below it.
        lines.push(line(40.0, 60.0, 380.0, "headline"));
        for i in 0..3 {
            lines.push(line(40.0, 200.0 + i as f32 * 12.0, 190.0, "c1"));
        }
        for i in 0..3 {
            lines.push(line(220.0, 200.0 + i as f32 * 12.0, 370.0, "c2"));
        }
        let ordered = order_lines(lines);
        assert_eq!(ordered[0].text, "headline");
        assert_eq!(ordered[1].text, "c1");
        assert_eq!(ordered[4].text, "c2");
        assert_eq!(ordered[7].text, "c3");
    }

    #[test]
    fn a_rotated_credit_does_not_swallow_a_body_line() {
        let credit = Line { text: "Foto: DER SPIEGEL".to_string(), rect: [40.0, 100.0, 44.5, 206.0] };
        let body = Line { text: "Ziel: sie dazu zu bringen".to_string(), rect: [52.0, 104.0, 200.0, 113.0] };
        assert!(is_rotated(&credit));
        assert!(!is_rotated(&body));
        // Even if the drop did not happen, the same-visual-line rule must not merge the strip
        // beside a body line into it.
        assert!(starts_new_paragraph(&[credit], &body, 2.0));
    }
}
