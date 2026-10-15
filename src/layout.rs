//! Page layout: the column geometry of a page and the text regions inside it.
//!
//! Columns are found from the page's lines (their left margins and extents), the same way the
//! reading order is; paragraphs are then assigned to a column by where their box sits. A run of
//! consecutive paragraphs in one reading-order slot is a *region* — the block a reader sees as
//! "the text in this column", and the place where a sentence carries on into the next column or
//! the next page.
//!
//! This is what the debug overlay draws, and what tells a selection that a paragraph sits at a
//! column top, so a sentence cut at the previous column's bottom is joined to it.

use crate::paragraphs::{self, Paragraph};
use crate::pdf::PageText;

/// One column of a page: its horizontal extent and the vertical extent of the text in it.
#[derive(Clone, Debug, Default)]
pub struct Column {
    pub x0: f32,
    pub x1: f32,
    pub top: f32,
    pub bottom: f32,
}

/// A run of consecutive paragraphs that sit in one reading-order slot.
#[derive(Clone, Debug)]
pub struct Region {
    /// The column the region belongs to; `None` for text that reaches across columns (a
    /// headline or a caption spanning the page).
    pub column: Option<usize>,
    /// The union of the region's paragraph boxes, in page view space (points, y down).
    pub rect: [f32; 4],
    /// The paragraphs of the page in this region (`start..end`, end exclusive).
    pub paragraphs: std::ops::Range<usize>,
    /// Whether the region opens its column: nothing of that column comes before it on the page.
    pub opens_column: bool,
}

/// The column geometry of a page and its text regions, in reading order.
#[derive(Clone, Debug, Default)]
pub struct PageLayout {
    pub columns: Vec<Column>,
    pub regions: Vec<Region>,
}

impl PageLayout {
    /// The region a paragraph belongs to.
    pub fn region_of(&self, paragraph: usize) -> Option<&Region> {
        self.regions.iter().find(|region| region.paragraphs.contains(&paragraph))
    }

    /// Whether `paragraph` is the first paragraph of a column on this page — the place a
    /// sentence from the previous column or the previous page would carry on.
    pub fn opens_column(&self, paragraph: usize) -> bool {
        self.regions.iter().any(|region| region.opens_column && region.paragraphs.start == paragraph)
    }
}

/// Work out the columns and regions of a page from its text layer and its paragraphs.
///
/// The columns are the ones [`paragraphs::page_columns`] found for the page's lines, so the
/// line reader and the paragraph reader share one column model.
pub fn analyze(page: &PageText, paragraphs: &[Paragraph]) -> PageLayout {
    let columns = paragraphs::page_columns(page);
    analyze_with(paragraphs, &columns)
}

fn analyze_with(paragraphs: &[Paragraph], columns: &[[f32; 2]]) -> PageLayout {
    let mut layout = PageLayout {
        columns: columns.iter().map(|[x0, x1]| Column { x0: *x0, x1: *x1, top: f32::INFINITY, bottom: f32::NEG_INFINITY }).collect(),
        regions: Vec::new(),
    };
    // Consecutive paragraphs in the same slot are one region.
    let mut current: Option<Region> = None;
    for (index, paragraph) in paragraphs.iter().enumerate() {
        let column = paragraphs::column_of_rect(paragraph.rect, columns);
        match &mut current {
            Some(region) if region.column == column => {
                union(&mut region.rect, &paragraph.rect);
                region.paragraphs.end = index + 1;
            }
            _ => {
                if let Some(region) = current.take() {
                    layout.regions.push(region);
                }
                current = Some(Region { column, rect: paragraph.rect, paragraphs: index..index + 1, opens_column: false });
            }
        }
    }
    if let Some(region) = current {
        layout.regions.push(region);
    }
    // A region opens its column when nothing of that column comes before it on the page.
    let mut seen = vec![false; layout.columns.len()];
    for region in &mut layout.regions {
        if let Some(column) = region.column {
            if !seen[column] {
                seen[column] = true;
                region.opens_column = true;
            }
        }
    }
    // The vertical extent of a column is the union of its regions.
    for index in 0..layout.regions.len() {
        let Some(column) = layout.regions[index].column else { continue };
        let rect = layout.regions[index].rect;
        layout.columns[column].top = layout.columns[column].top.min(rect[1]);
        layout.columns[column].bottom = layout.columns[column].bottom.max(rect[3]);
    }
    for column in &mut layout.columns {
        if !column.top.is_finite() {
            column.top = 0.0;
            column.bottom = 0.0;
        }
    }
    layout
}

fn union(a: &mut [f32; 4], b: &[f32; 4]) {
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

/// One page's loaded text, for the sentence walk below: its paragraphs and their layout.
/// `None` when that page's text layer has not loaded, which ends the walk at that page.
pub type LoadedPage<'a> = Option<(&'a [Paragraph], &'a PageLayout)>;

fn loaded<'a>(pages: &[LoadedPage<'a>], index: usize) -> Option<(&'a [Paragraph], &'a PageLayout)> {
    pages.get(index).copied().flatten()
}

/// The first body paragraph of a page.
fn first_body(paragraphs: &[Paragraph]) -> Option<usize> {
    paragraphs.iter().position(Paragraph::is_body)
}

/// The last body paragraph of a page.
fn last_body(paragraphs: &[Paragraph]) -> Option<usize> {
    paragraphs.iter().rposition(Paragraph::is_body)
}
/// Which paragraphs of a page sit at the top of a column, as `spans_filtered` wants them.
fn boundaries(paragraphs: &[Paragraph], layout: &PageLayout) -> Vec<bool> {
    (0..paragraphs.len()).map(|index| layout.opens_column(index)).collect()
}

/// The sentence a paragraph belongs to: the run of `(page, paragraph index)` pairs in reading
/// order, from the paragraph that opens it to the one that closes it, across column and page
/// boundaries.
///
/// Only body text carries a sentence. Page furniture is dropped before this and a caption, a
/// pull quote or a figure between two body fragments of one sentence is stepped over, so a
/// figure never cuts a sentence in two.
///
/// Within a page a step onto a column top uses the permissive rule (a name that begins with a
/// capital may carry the sentence on). Across a page break the text must read on by itself — a
/// lower-case open or a comma, dash or colon — because a capital at the top of a new page is
/// usually the deck of the next article, not the tail of this one.
pub fn sentence_run(pages: &[LoadedPage<'_>], page: usize, index: usize) -> Vec<(usize, usize)> {
    let Some((paragraphs, layout)) = loaded(pages, page) else { return vec![(page, index)] };
    if index >= paragraphs.len() {
        return vec![(page, index)];
    }
    // Within the page: only body text carries the sentence, so a caption or a line of display
    // type between two body fragments of one sentence does not break the run. A step onto a
    // column top allows a capital to continue the sentence, as the column before was cut.
    let mask = paragraphs::body_mask(paragraphs);
    if !mask.get(index).copied().unwrap_or(false) {
        // A caption or a line of display type is a run of its own.
        return vec![(page, index)];
    }
    let (start, end) = paragraphs::spans_filtered(paragraphs, &mask, &boundaries(paragraphs, layout))[index];
    let mut run: Vec<(usize, usize)> = (start..=end).filter(|slot| mask[*slot]).map(|index| (page, index)).collect();
    // Backwards across the page break: only when the run opens this page's body text.
    while run.len() < 256 {
        let &(first_page, first_index) = run.first().unwrap();
        if first_page == 0 {
            break;
        }
        let Some((first_paragraphs, _)) = loaded(pages, first_page) else { break };
        if first_body(first_paragraphs) != Some(first_index) {
            break;
        }
        let previous_page = first_page - 1;
        let Some((previous_paragraphs, previous_layout)) = loaded(pages, previous_page) else { break };
        let Some(previous_index) = last_body(previous_paragraphs) else { break };
        if !paragraphs::continues_across_page(&previous_paragraphs[previous_index], &first_paragraphs[first_index]) {
            break;
        }
        let previous_mask = paragraphs::body_mask(previous_paragraphs);
        let (previous_start, _) = paragraphs::spans_filtered(previous_paragraphs, &previous_mask, &boundaries(previous_paragraphs, previous_layout))[previous_index];
        let addition: Vec<(usize, usize)> = (previous_start..=previous_index).filter(|slot| previous_mask[*slot]).map(|index| (previous_page, index)).collect();
        run.splice(0..0, addition);
    }
    // Forwards across the page break: only when the run closes this page's body text.
    while run.len() < 256 {
        let &(last_page, last_index) = run.last().unwrap();
        let Some((last_paragraphs, _)) = loaded(pages, last_page) else { break };
        if last_body(last_paragraphs) != Some(last_index) {
            break;
        }
        let next_page = last_page + 1;
        let Some((next_paragraphs, next_layout)) = loaded(pages, next_page) else { break };
        let Some(next_index) = first_body(next_paragraphs) else { break };
        if !paragraphs::continues_across_page(&last_paragraphs[last_index], &next_paragraphs[next_index]) {
            break;
        }
        let next_mask = paragraphs::body_mask(next_paragraphs);
        let (_, next_end) = paragraphs::spans_filtered(next_paragraphs, &next_mask, &boundaries(next_paragraphs, next_layout))[next_index];
        let addition: Vec<(usize, usize)> = (next_index..=next_end).filter(|slot| next_mask[*slot]).map(|index| (next_page, index)).collect();
        run.extend(addition);
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph(rect: [f32; 4]) -> Paragraph {
        Paragraph { text: "text".to_string(), rect, size: 10.0, line_spacing: 1.35, lines: Vec::new(), kind: crate::paragraphs::Kind::Body }
    }

    #[test]
    fn paragraphs_in_one_column_are_one_region() {
        let paragraphs = vec![paragraph([40.0, 100.0, 200.0, 110.0]), paragraph([40.0, 120.0, 200.0, 130.0]), paragraph([220.0, 100.0, 380.0, 110.0])];
        let layout = analyze_with(&paragraphs, &[[40.0, 200.0], [220.0, 380.0]]);
        assert_eq!(layout.regions.len(), 2);
        assert_eq!(layout.regions[0].column, Some(0));
        assert_eq!(layout.regions[0].paragraphs, 0..2);
        assert!(layout.regions[0].opens_column);
        assert_eq!(layout.regions[1].column, Some(1));
        assert!(layout.opens_column(2));
        assert!(!layout.opens_column(1));
    }

    #[test]
    fn a_box_across_two_columns_is_a_spanning_region() {
        let paragraphs = vec![paragraph([40.0, 100.0, 200.0, 110.0]), paragraph([40.0, 120.0, 380.0, 130.0]), paragraph([40.0, 140.0, 200.0, 150.0])];
        let layout = analyze_with(&paragraphs, &[[40.0, 200.0], [220.0, 380.0]]);
        assert_eq!(layout.regions.len(), 3);
        assert_eq!(layout.regions[1].column, None);
        assert_eq!(layout.regions[1].paragraphs, 1..2);
    }

    #[test]
    fn a_run_steps_over_display_text_without_taking_it() {
        let text = |text: &str, kind: crate::paragraphs::Kind| Paragraph { text: text.to_string(), rect: [40.0, 0.0, 200.0, 10.0], size: 9.0, line_spacing: 1.35, lines: Vec::new(), kind };
        let paragraphs = vec![text("the sentence is cut here", crate::paragraphs::Kind::Body), text("Sven Jaax", crate::paragraphs::Kind::Display), text("and carries on", crate::paragraphs::Kind::Body)];
        let layout = analyze_with(&paragraphs, &[[40.0, 200.0]]);
        let pages = [Some((paragraphs.as_slice(), &layout))];
        // The body fragments of the sentence are joined, and the display line between them is
        // not part of the run (so it is never translated with the sentence).
        assert_eq!(sentence_run(&pages, 0, 0), vec![(0, 0), (0, 2)]);
        assert_eq!(sentence_run(&pages, 0, 1), vec![(0, 1)]);
    }

    #[test]
    fn a_new_article_on_the_next_page_is_not_joined() {
        let paragraph = |text: &str, kind, size: f32| Paragraph { text: text.to_string(), rect: [40.0, 0.0, 200.0, 10.0], size, line_spacing: 1.35, lines: Vec::new(), kind };
        let first = vec![paragraph("the interview stops here and", crate::paragraphs::Kind::Body, 9.0)];
        let layout = analyze_with(&first, &[[40.0, 200.0]]);
        // The next page opens a new article with a big headline and a larger deck: not a
        // continuation, even though the previous sentence stops mid-way.
        let article = vec![
            paragraph("Weidel macht den Siegmund", crate::paragraphs::Kind::Display, 20.0),
            paragraph("Sicherheit Wie der Wiener", crate::paragraphs::Kind::Body, 10.5),
        ];
        let article_layout = analyze_with(&article, &[[40.0, 200.0]]);
        let pages = [Some((first.as_slice(), &layout)), Some((article.as_slice(), &article_layout))];
        assert_eq!(sentence_run(&pages, 0, 0), vec![(0, 0)]);
        // A lower-case opening on the next page is the tail of the same sentence.
        let tail = vec![paragraph("and carries on here.", crate::paragraphs::Kind::Body, 9.0)];
        let tail_layout = analyze_with(&tail, &[[40.0, 200.0]]);
        let pages = [Some((first.as_slice(), &layout)), Some((tail.as_slice(), &tail_layout))];
        assert_eq!(sentence_run(&pages, 0, 0), vec![(0, 0), (1, 0)]);
        // A German noun in capitals, at the same size, carries the sentence on.
        let noun = vec![paragraph("Ecken verbandelt sein soll.", crate::paragraphs::Kind::Body, 9.0)];
        let noun_layout = analyze_with(&noun, &[[40.0, 200.0]]);
        let pages = [Some((first.as_slice(), &layout)), Some((noun.as_slice(), &noun_layout))];
        assert_eq!(sentence_run(&pages, 0, 0), vec![(0, 0), (1, 0)]);
    }
}
