//! Reading-order paragraphs taken from a page's text layer.
//!
//! The text layer (`crate::pdf::text`) already groups glyphs into lines in reading order (columns are
//! handled for us). A paragraph is a run of consecutive lines that belong together:
//! same type size, normal leading, and no indentation change. The paragraph keeps its
//! box in page view space (points, y down), its type size and its line spacing, so the
//! translation can be shown in the paragraph's own format.

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
}

impl Paragraph {
    /// Stable identity of a paragraph's content: the cache and the translation store key.
    pub fn key(&self) -> String {
        content_key(&self.text)
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

    // Two lines on the same visual line, the second continuing to the right, are one line
    // split in two (a bold run-in heading, or a line cut at a wide gap).
    if (line.rect[1] - prev.rect[1]).abs() <= lh.min(ph) * 0.5 && line.rect[0] >= prev.rect[2] - lh {
        return false;
    }
    // Text wrapped beside a drop cap: the previous line is much taller than this one (it
    // carries the cap) but still a wide text line. A narrow, tall box is instead rotated
    // text (a photo credit) and must not swallow the line beside it.
    if ph > lh * 1.6 && prev.rect[2] - prev.rect[0] > ph && line.rect[1] < prev.rect[3] - lh * 0.4 {
        return false;
    }
    // Extra leading between lines (a paragraph break, not a wrapped line).
    let gap = line.rect[1] - prev.rect[3];
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
    Some(Paragraph { text, rect, size, line_spacing, lines })
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

/// The column a line belongs to, or `None` when it reaches across two columns (a heading
/// or an image caption spanning the page).
fn assign_column(line: &Line, columns: &[[f32; 2]]) -> Option<usize> {
    let mut covered = 0;
    for column in columns {
        let overlap = line.rect[2].min(column[1]) - line.rect[0].max(column[0]);
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
        let distance = (line.rect[0] - column[0]).abs();
        if distance < best_distance {
            best_distance = distance;
            best = slot;
        }
    }
    Some(best)
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

    // A line sits in the band below every spanning line that covers it horizontally and ends
    // above its centre. Lines a spanning line does not cover stay in the earlier band.
    let band_of: Vec<usize> = (0..count)
        .map(|index| {
            let line = &lines[index];
            spanning
                .iter()
                .filter(|other| **other != index && overlaps_x(line, &lines[**other]) && lines[**other].rect[3] <= center_y(line))
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
