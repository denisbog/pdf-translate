//! Background page renderer.
//!
//! Pages are rendered by the pool in [`crate::pdf`]: several worker threads over the
//! shared document, each request answered once, the most urgent first. The UI rebuilds
//! the pool's queue each frame from the pages it still needs, so a page scrolled away —
//! or a raster at an old zoom — is dropped instead of being rendered. (A single worker
//! thread with a plain request channel, as this used to be, renders one page at a time
//! and cannot cancel: after a zoom gesture it works through every stale request first.)

use std::sync::Arc;
use std::time::Duration;

use crate::pdf::{RenderConfig, RenderPool, RenderRequest, RenderedPage, RequestKind};

use crate::paragraphs::{self, Paragraph};

/// Everything the UI keeps for one page.
#[derive(Default)]
pub struct PageSlot {
    /// Raster as a GPU texture, at `texture_scale` device pixels per point. The raster at an
    /// older scale stays on screen (stretched) until the new one arrives.
    pub texture: Option<egui::TextureHandle>,
    pub texture_scale: f32,
    /// Paragraphs of this page, once its text layer has arrived.
    pub paragraphs: Option<Vec<Paragraph>>,
    /// Why the page could not be rendered.
    pub error: Option<String>,
    pending_pixels: Option<RenderRequest>,
    pending_text: Option<RenderRequest>,
}

pub struct RenderClient {
    pool: RenderPool,
    ctx: egui::Context,
    pub slots: Vec<PageSlot>,
    next_tag: u64,
    /// What this frame's `ensure_*` calls asked for, most urgent first.
    wanted: Vec<RenderRequest>,
    /// What was last handed to the pool, so an unchanged queue is not re-sent.
    sent: Vec<RenderRequest>,
    /// Whether a raster arrives differently this frame, so the UI can repaint.
    pub updated: bool,
}

/// As PdfCraft: every core but one (the UI thread, and the translation model), up to a cap.
fn render_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8) - 1
}

impl RenderClient {
    pub fn new(bytes: Arc<Vec<u8>>, pages: usize, config: RenderConfig, ctx: egui::Context) -> Self {
        Self {
            pool: RenderPool::new(bytes, render_threads(), config),
            ctx,
            slots: (0..pages).map(|_| PageSlot::default()).collect(),
            next_tag: 1,
            wanted: Vec::new(),
            sent: Vec::new(),
            updated: false,
        }
    }

    fn tag(&mut self) -> u64 {
        let tag = self.next_tag;
        self.next_tag += 1;
        tag
    }

    /// Ask for the page's text layer once (paragraphs are derived from it).
    pub fn ensure_text(&mut self, page: usize) {
        let Some(slot) = self.slots.get(page) else { return };
        if slot.paragraphs.is_none() && slot.pending_text.is_none() {
            let tag = self.tag();
            self.slots[page].pending_text = Some(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag });
        }
        if let Some(request) = self.slots[page].pending_text {
            self.wanted.push(request);
        }
    }

    /// Ask for a raster of the page at `scale` device pixels per point.
    pub fn ensure_pixels(&mut self, page: usize, scale: f32) {
        let Some(slot) = self.slots.get(page) else { return };
        let satisfied = slot.texture.is_some() && (slot.texture_scale - scale).abs() <= f32::EPSILON;
        let pending = slot.pending_pixels.is_some_and(|request| (request.scale - scale).abs() <= f32::EPSILON);
        if !satisfied && !pending {
            let tag = self.tag();
            self.slots[page].pending_pixels = Some(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag });
        }
        if let Some(request) = self.slots[page].pending_pixels {
            self.wanted.push(request);
        }
    }

    /// Drop a page's raster (it is far off screen); it is re-requested when needed.
    pub fn free_texture(&mut self, page: usize) {
        if let Some(slot) = self.slots.get_mut(page) {
            slot.texture = None;
            slot.texture_scale = 0.0;
        }
    }

    /// Hand this frame's requests to the pool, most urgent first: the page the reader is on,
    /// then the rest. An unchanged queue is left alone, so the pool is not woken for nothing.
    pub fn submit(&mut self, current: Option<usize>) {
        if self.wanted.is_empty() {
            return;
        }
        self.wanted.sort_by_key(|request| Some(request.page) != current);
        if self.wanted != self.sent {
            self.pool.set_queue(self.wanted.clone());
            self.sent.clone_from(&self.wanted);
        }
        // A worker is on it; keep frames coming so the answer is picked up even while idle.
        self.ctx.request_repaint_after(Duration::from_millis(16));
        self.wanted.clear();
    }

    /// Pick up finished work. Uploads rasters and stores paragraphs.
    pub fn poll(&mut self, ctx: &egui::Context) {
        // Inline pools (the web path) render one page per call, inside `try_recv`.
        let budget = if self.pool.is_inline() { 1 } else { usize::MAX };
        let mut taken = 0;
        while taken < budget {
            let Some(rendered) = self.pool.try_recv() else { break };
            taken += 1;
            self.accept(ctx, rendered);
        }
    }

    fn accept(&mut self, ctx: &egui::Context, rendered: RenderedPage) {
        let RenderedPage { request, width, height, rgba, error, text, .. } = rendered;
        let Some(slot) = self.slots.get_mut(request.page) else { return };
        match request.kind {
            RequestKind::Text => {
                if slot.pending_text.map(|pending| pending.tag) != Some(request.tag) {
                    return;
                }
                slot.pending_text = None;
                if let Some(text) = text {
                    slot.paragraphs = Some(paragraphs::extract(&text));
                } else if let Some(error) = error {
                    slot.error = Some(error);
                }
            }
            RequestKind::Pixels => {
                if slot.pending_pixels.map(|pending| pending.tag) != Some(request.tag) {
                    return;
                }
                slot.pending_pixels = None;
                if let Some(error) = error {
                    slot.error = Some(error);
                    return;
                }
                if width == 0 || height == 0 {
                    return;
                }
                let image = egui::ColorImage::from_rgba_premultiplied([width as usize, height as usize], &rgba);
                let texture = ctx.load_texture(format!("page-{}", request.page), image, egui::TextureOptions::LINEAR);
                slot.texture = Some(texture);
                slot.texture_scale = request.scale;
                slot.error = None;
                self.updated = true;
            }
        }
    }
}
