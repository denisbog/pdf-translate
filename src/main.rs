//! PDF Translate — read a PDF and see each paragraph's translation from a local model,
//! rendered beside the paragraph it belongs to.
//!
//! Pages are rasterised and their text layer extracted by `pdfcraft-render` (the renderer
//! of [PdfCraft](https://github.com/storytold/pdfcraft)); paragraphs are grouped from the
//! reading-order text; translations come from a local Qwen2.5 ONNX model running in-process
//! through ONNX Runtime, the same setup the transcribe project uses.
//!
//! Debug helpers (no UI):
//!   pdf-translate --paragraphs <file.pdf> [page]
//!   pdf-translate --translate  <file.pdf> <page> <paragraph> [target] [model]

mod app;
mod icons;
mod llm;
mod paragraphs;
mod render;
mod store;
mod theme;
mod typeface;
mod widgets;

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--paragraphs") => return paragraphs_cli(&args),
        Some("--lines") => return lines_cli(&args),
        Some("--check") => return check_cli(&args),
        Some("--translate") => return translate_cli(&args),
        Some("--help" | "-h") => {
            println!("pdf-translate [file.pdf]\n\n  --paragraphs <file.pdf> [page]                     list detected paragraphs\n  --translate  <file.pdf> <page> <n> [target] [model] translate one paragraph");
            return Ok(());
        }
        _ => {}
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1360.0, 920.0]).with_min_inner_size([720.0, 480.0]).with_title("PDF Translate"),
        ..Default::default()
    };
    eframe::run_native("PDF Translate", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

fn open_pdf(path: &str) -> PageRenderer {
    let bytes = std::fs::read(path).unwrap_or_else(|error| {
        eprintln!("cannot read {path}: {error}");
        std::process::exit(1);
    });
    PageRenderer::new(Arc::new(bytes), RenderConfig::default())
}

fn page_paragraphs(renderer: &mut PageRenderer, page: usize) -> Vec<paragraphs::Paragraph> {
    let rendered = renderer.render(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0 });
    let Some(text) = rendered.text else {
        eprintln!("page {page} has no text: {:?}", rendered.error);
        std::process::exit(1);
    };
    paragraphs::extract(&text)
}

fn paragraphs_cli(args: &[String]) -> eframe::Result<()> {
    let Some(path) = args.get(2) else {
        eprintln!("usage: pdf-translate --paragraphs <file.pdf> [page]");
        std::process::exit(2);
    };
    let page: usize = args.get(3).and_then(|page| page.parse().ok()).unwrap_or(0);
    let mut renderer = open_pdf(path);
    for (index, paragraph) in page_paragraphs(&mut renderer, page).iter().enumerate() {
        println!("#{index} size={:.1} leading={:.2} box=[{:.1}, {:.1}, {:.1}, {:.1}]\n    {}", paragraph.size, paragraph.line_spacing, paragraph.rect[0], paragraph.rect[1], paragraph.rect[2], paragraph.rect[3], paragraph.text);
    }
    Ok(())
}

fn lines_cli(args: &[String]) -> eframe::Result<()> {
    let Some(path) = args.get(2) else {
        eprintln!("usage: pdf-translate --lines <file.pdf> [page]");
        std::process::exit(2);
    };
    let page: usize = args.get(3).and_then(|page| page.parse().ok()).unwrap_or(0);
    let mut renderer = open_pdf(path);
    let rendered = renderer.render(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0 });
    let Some(text) = rendered.text else {
        eprintln!("page {page} has no text");
        std::process::exit(1);
    };
    println!("columns: {:?}", paragraphs::page_columns(&text));
    for (index, line) in paragraphs::page_lines(&text).iter().enumerate() {
        println!("#{index:>3} box=[{:>6.1}, {:>6.1}, {:>6.1}, {:>6.1}] {:?}", line.rect[0], line.rect[1], line.rect[2], line.rect[3], line.text);
    }
    Ok(())
}

/// Sanity check over every page: a paragraph of more than one line must sit inside one
/// column. Prints any that reach across two columns.
fn check_cli(args: &[String]) -> eframe::Result<()> {
    let Some(path) = args.get(2) else {
        eprintln!("usage: pdf-translate --check <file.pdf>");
        std::process::exit(2);
    };
    let mut renderer = open_pdf(path);
    let pages = renderer.page_count();
    let mut multi_column = 0usize;
    let mut spanning = 0usize;
    let mut reported = 0usize;
    let mut total_paragraphs = 0usize;
    let mut longest = 0usize;
    let mut longest_at = String::new();
    for page in 0..pages {
        let rendered = renderer.render(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: 0 });
        let Some(text) = rendered.text else { continue };
        let columns = paragraphs::page_columns(&text);
        if columns.len() >= 2 {
            multi_column += 1;
        }
        // Display type (pull quotes, headlines, ads) legitimately spans columns; only body
        // lines are checked.
        let mut heights: Vec<f32> = paragraphs::page_lines(&text).iter().map(|line| line.height()).collect();
        heights.sort_by(f32::total_cmp);
        let body_height = heights.get(heights.len() / 2).copied().unwrap_or(9.0).max(1.0);
        let paragraphs = paragraphs::extract(&text);
        total_paragraphs += paragraphs.len();
        for paragraph in &paragraphs {
            let length = paragraph.text.chars().count();
            if length > longest {
                longest = length;
                longest_at = format!("page {page}");
            }
        }
        for paragraph in paragraphs {
            if paragraph.lines.len() < 2 {
                continue;
            }
            if paragraph.lines.iter().any(|line| line.height() > body_height * 1.3) {
                continue;
            }
            // The bug is body text from two columns in one paragraph: its real line starts
            // (a line whose top differs from the previous one) then sit at two different
            // column margins. Continuations on the same visual line (a run-in heading, a
            // line wrapped beside a drop cap) are skipped.
            let mut starts: Vec<f32> = Vec::new();
            for (index, line) in paragraph.lines.iter().enumerate() {
                let previous = index.checked_sub(1).map(|i| &paragraph.lines[i]);
                if previous.is_none_or(|previous| (line.rect[1] - previous.rect[1]).abs() > line.height().min(previous.height()) * 0.5) {
                    starts.push(line.rect[0]);
                }
            }
            starts.sort_by(f32::total_cmp);
            let mut groups = 0;
            let mut last = f32::NEG_INFINITY;
            for x in &starts {
                if *x - last > 20.0 {
                    groups += 1;
                    last = *x;
                }
            }
            if groups >= 2 {
                let left = starts.first().copied().unwrap_or(0.0);
                let right = starts.last().copied().unwrap_or(0.0);
                spanning += 1;
                if reported < 25 {
                    reported += 1;
                    let excerpt: String = paragraph.text.chars().take(70).collect();
                    println!(
                        "page {page}: box=[{:.1}, {:.1}, {:.1}, {:.1}] lefts {left:.1}..{right:.1} {excerpt:?}",
                        paragraph.rect[0], paragraph.rect[1], paragraph.rect[2], paragraph.rect[3]
                    );
                }
            }
        }
    }
    println!("{pages} pages, {multi_column} multi-column, {spanning} multi-line paragraphs spanning columns");
    println!("{total_paragraphs} paragraphs, longest {longest} characters ({longest_at})");
    Ok(())
}

fn translate_cli(args: &[String]) -> eframe::Result<()> {
    let (Some(path), Some(page)) = (args.get(2), args.get(3)) else {
        eprintln!("usage: pdf-translate --translate <file.pdf> <page> <paragraph> [target] [model]");
        std::process::exit(2);
    };
    let page: usize = page.parse().unwrap_or_else(|_| {
        eprintln!("page must be a number");
        std::process::exit(2);
    });
    let index: usize = args.get(4).and_then(|index| index.parse().ok()).unwrap_or(0);
    let target = args.get(5).map(String::as_str).unwrap_or("en");
    let model = args.get(6).map(String::as_str).unwrap_or("qwen2.5:1.5b");

    let mut renderer = open_pdf(path);
    let found = page_paragraphs(&mut renderer, page);
    let Some(paragraph) = found.get(index) else {
        eprintln!("page {page} has {} paragraphs", found.len());
        std::process::exit(1);
    };
    println!("source [{}]: {}", target, paragraph.text);
    match store::translate_with_model(model, &paragraph.text, target) {
        Ok(translation) => println!("\ntranslation: {translation}"),
        Err(error) => {
            eprintln!("translation failed: {error}");
            std::process::exit(1);
        }
    }
    Ok(())
}
