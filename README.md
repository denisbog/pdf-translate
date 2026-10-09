# PDF Translate

A native PDF reader built on the renderer of
[PdfCraft](https://github.com/storytold/pdfcraft). Hover a paragraph and its
translation appears *inside the document*, next to the paragraph, set in the same
format as the paragraph it belongs to. Translations come from a local Qwen2.5 ONNX
model running in-process through ONNX Runtime — the same setup as
[`~/llm/transcribe`](../transcribe).

```
┌ tab strip ── ⌂ · document tab · + Open ────────────── theme · help · info ─┐
├ mode bar ── Menu │ Read · Outline · Translations · Log ── search, language, │
│             model, zoom                                                   │
│ ┌──┐┌─ page raster ─┐                              ┌─ translation card ─┐ │
│ │  ││  (pdfcraft)   │  ◀── hover a paragraph ──▶   │ same type size and │ │
│ │▐▌││               │   (blue highlight, mark)     │ leading, left or   │ │
│ │  │└───────────────┘                              │ right of the page  │ │
│ └──┘┌ rail ───────────────────────────────────────  └────────────────────┘ │
└──────────────────── pasteboard (gray), pages with a shadow ────────────────┘
```

The chrome — tab strip, mode bar, tool rails, panels, pasteboard, colour palette,
Inter typeface and Lucide icons — is PdfCraft's (`crates/ui-egui/src/theme.rs`,
`chrome.rs`, `widgets.rs`), so the two apps read as one family.

## What it does

* **Reads PDFs natively** — pages are rasterised and their text layer extracted by
  `pdfcraft-render` (hayro + vello_cpu, with PdfCraft's vendored patches), so text
  is available in reading order (columns, RTL and vertical CJK included).
* **Paragraphs, not pages** — the text layer is grouped into paragraphs by type size,
  leading, indentation and short last lines. A bold run-in heading and a line wrapped
  beside a drop cap stay with their paragraph; each paragraph keeps its box, type size
  and line spacing.
* **Hover → translation** — the hovered paragraph is highlighted and its card is
  shown **on the side of the paragraph that has the most room** (left or right of
  the page). The translation is set at the paragraph's own size, leading and typeface
  family (serif, sans or mono, from the document's fonts), and a bold run-in heading
  such as *Müller:* stays bold — so it reads like the paragraph it translates.
* **Marked when translated** — a translated paragraph carries an accent bar in the
  margin, so you can see at a glance what is already done.
* **PdfCraft's chrome** — tab strip, mode bar, tool rails and a pasteboard with white,
  shadowed pages; light, dark or system theme; and a command palette on ⌘K. The right
  rail opens a panel: **Outline** (click a bookmark to jump), **Translations** (the
  document's translations, click one to jump to its page) and **Log**.
* **Request one paragraph** — click a paragraph (or the *Translate this paragraph*
  button on its card) to translate just that one. Nothing is translated until you
  ask.
* **Live stage** — while a request runs, a small animated chip next to the
  paragraph says what is happening: `queued`, `loading the model…`, `translating…`.
  The same stage is shown inside the card.
* **Close a translation** — the `x` on the card hides it for that paragraph;
  hovering stays quiet until you click the paragraph again.
* **Cache** — every translation is written to disk and reused; opening the same
  PDF again (or another reader) never asks the model twice.

## Requirements

* Rust (edition 2021). The dependency uses PdfCraft, which needs a recent
  toolchain (it is built with the workspace's `rust-version = 1.90`).
* A local checkout of PdfCraft. The build expects it at **`~/llm/pdfcraft`**
  (the `path` in `Cargo.toml`). PdfCraft's `[patch.crates-io]` for hayro/lopdf is
  mirrored here, because `pdfcraft-render` relies on those patches.
* ONNX Runtime with the CUDA provider. On this machine the copy bundled with
  sherpa-onnx is used (`~/sherpa-onnx/.../lib/libonnxruntime.so`); it is found
  automatically. Override with `PDF_TRANSLATE_ORT_DIR`.
* The Qwen2.5 ONNX models, reusing the transcribe folder
  `~/transcribe-models/onnx` (`qwen2-5-1-5b-q4f16`, `qwen2-5-3b-fp16`). Missing
  models are downloaded from Hugging Face on first use. Override the folder with
  `PDF_TRANSLATE_MODELS`.

## Build & run

```bash
cd ~/llm/pdf-translate
cargo run --release -- "/path/to/document.pdf"
```

Without a file argument, **Open…** (or ⌘/Ctrl+O) picks a PDF.

**Chrome.** The tab strip holds the document tab; the mode bar has the **Menu**,
the panel tabs (*Read · Outline · Translations · Log*) and, on the right, the target
language, the model and the zoom. The left rail holds the reading tools (pan, zoom,
fit to width, actual size), the right rail the panel toggles, the theme and help.
The theme follows **Light gray / Dark gray / Use system setting** (the button in the
tab strip); ⌘K opens the command palette.

Shortcuts: ⌘O open · ⌘K palette · ⌘+ / ⌘− zoom · ⌘0 actual size · ⌘2 fit to width ·
Esc close the palette.

### Debug helpers (no UI)

```bash
# list the paragraphs detected on a page (size, leading, box, text)
pdf-translate --paragraphs document.pdf 0

# list the page's lines in the reading order paragraphs are built from
pdf-translate --lines document.pdf 0

# sanity-check every page: no paragraph may mix two columns
pdf-translate --check document.pdf

# translate one paragraph and print it
pdf-translate --translate document.pdf 0 14 en qwen2.5:1.5b
```

Debug environment variables: `PDF_TRANSLATE_PAGE=<n>` opens the document at that
page, `PDF_TRANSLATE_MODELS` and `PDF_TRANSLATE_ORT_DIR` override the model folder
and the ONNX Runtime library directory.

## Cache

`$XDG_CACHE_HOME/pdf-translate`, else `~/.cache/pdf-translate`. One file per
document and target language, e.g.
`2304_07193v2-9aa388b6faab01b6-en.json`:

```json
{
  "source_file": "/home/me/paper.pdf",
  "target": "en",
  "model": "qwen2.5:1.5b",
  "entries": {
    "6a315b7dbb1d75f1": { "source": "The recent …", "translation": "Recent …", "page": 0 }
  }
}
```

Entries are keyed by a hash of the paragraph's normalised text, so a translated
paragraph keeps its translation even if the document is re-opened or the paragraph
grouping shifts slightly. The file name pins the document's canonical path, size
and mtime, so replacing the PDF starts a fresh cache.

## How it is built

| file | role |
| --- | --- |
| `src/main.rs` | entry point and the debug CLI |
| `src/app.rs` | the reader: chrome, page stack, hover, translation cards, panels |
| `src/theme.rs` | PdfCraft's design tokens, egui style and fonts |
| `src/icons.rs` | PdfCraft's Lucide SVGs, tinted icon buttons |
| `src/widgets.rs` | mode tabs, ghost buttons, pills, search box, section titles |
| `src/render.rs` | background page renderer (one thread owns `PageRenderer`) |
| `src/paragraphs.rs` | reading-order lines → columns → paragraphs, content keys |
| `src/typeface.rs` | the document's body family (serif/sans/mono) from its font names |
| `src/store.rs` | translation cache + background translation worker |
| `src/llm.rs` | local Qwen2.5 ONNX decoder (KV-cache loop) — adapted from `transcribe` |
| `tools/xctl.py` | tiny X11 helper used to script hover/click during development |

The interface icons are PdfCraft's Lucide SVGs (ISC) read from the PdfCraft checkout;
the interface typefaces are PdfCraft's Inter and JetBrains Mono (OFL), and the document
text is set in Source Serif Pro (OFL, `assets/fonts`); the theme tokens, widget metrics
and chrome layout are copied from `crates/ui-egui`.

Rendering and text extraction both go through
`pdfcraft_render::PageRenderer`; a request for `RequestKind::Pixels` produces the
page texture, `RequestKind::Text` the `PageText` the paragraphs are grouped from.
The renderer runs on its own thread so the UI never blocks, and only pages near
the viewport are requested (rasters far off screen are freed).

Translation runs on a second thread that owns the model (loaded lazily on the
first request), reports stages over a channel, and wakes the UI with
`Context::request_repaint`. A paragraph is translated sentence by sentence in small
numbered batches, which keeps a small model from drifting or summarising.

## Notes and limitations

* The default `qwen2.5:1.5b` is fast and good enough for gist reading; switch to
  `qwen2.5:3b` for quality (it runs on the CPU path here, so it is slower).
* Multi-column pages are read column by column: the columns are found from where lines
  start, and a line that runs across columns (a heading or a caption) only separates the
  columns it covers, so a column beside it keeps flowing and paragraphs never mix two
  columns.
* Paragraph grouping is heuristic: dense tables of contents and slide decks merge
  some lines; running headers, page numbers and equation numbers may show up as
  paragraphs.
* The translation is set in the document's *family* (serif, sans or mono, inferred from
  the document's font names), not its exact embedded face: PDFs ship subset fonts whose
  programs cannot be reused for other text. Only the run-in heading is mirrored in bold;
  italic runs inside a paragraph are not.
* Only Latin faces are bundled (Inter, JetBrains Mono, Source Serif Pro). A translation
  into a script they do not cover (e.g. CJK) would render as replacement boxes; adding a
  face (PdfCraft's optional `craft-fonts` input) would fix that.
* Rastering happens at one scale per zoom step; the cache key does not include the
  zoom, so panning/zooming re-renders as expected.
