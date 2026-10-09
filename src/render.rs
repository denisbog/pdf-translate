//! Background page renderer.
//!
//! One worker thread owns the `pdfcraft_render::PageRenderer` (rasterising and text
//! extraction both go through it) and answers requests from the UI. The UI never
//! blocks: it asks for the pages that are on screen and picks the answers up on the
//! next frames.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};

use crate::paragraphs::{self, Paragraph};

/// Everything the UI keeps for one page.
#[derive(Default)]
pub struct PageSlot {
    /// Raster as a GPU texture, at `texture_scale` device pixels per point.
    pub texture: Option<egui::TextureHandle>,
    pub texture_scale: f32,
    /// Paragraphs of this page, once its text layer has arrived.
    pub paragraphs: Option<Vec<Paragraph>>,
    /// Why the page could not be rendered.
    pub error: Option<String>,
    pending_pixels: Option<(u64, f32)>,
    pending_text: Option<(u64,)>,
    text_requested: bool,
}

pub struct RenderClient {
    tx: Sender<RenderRequest>,
    rx: Receiver<RenderedPage>,
    pub slots: Vec<PageSlot>,
    next_tag: u64,
    /// Pages whose rasters changed this frame, so the UI can repaint.
    pub updated: bool,
}

fn spawn(bytes: Arc<Vec<u8>>, config: RenderConfig, ctx: egui::Context) -> (Sender<RenderRequest>, Receiver<RenderedPage>) {
    let (tx_req, rx_req) = mpsc::channel::<RenderRequest>();
    let (tx_out, rx_out) = mpsc::channel::<RenderedPage>();
    std::thread::Builder::new()
        .name("page-renderer".into())
        .spawn(move || {
            let mut renderer = PageRenderer::new(bytes, config);
            while let Ok(request) = rx_req.recv() {
                let rendered = renderer.render(request);
                if tx_out.send(rendered).is_err() {
                    break;
                }
                // Wake the UI so it picks the finished page up even while idle.
                ctx.request_repaint();
            }
        })
        .expect("spawn page renderer");
    (tx_req, rx_out)
}

impl RenderClient {
    pub fn new(bytes: Arc<Vec<u8>>, pages: usize, config: RenderConfig, ctx: egui::Context) -> Self {
        let (tx, rx) = spawn(bytes, config, ctx);
        Self { tx, rx, slots: (0..pages).map(|_| PageSlot::default()).collect(), next_tag: 1, updated: false }
    }

    /// Ask for the page's text layer once (paragraphs are derived from it).
    pub fn ensure_text(&mut self, page: usize) {
        let Some(slot) = self.slots.get_mut(page) else { return };
        if slot.paragraphs.is_some() || slot.text_requested {
            return;
        }
        let tag = self.next_tag;
        self.next_tag += 1;
        slot.text_requested = true;
        slot.pending_text = Some((tag,));
        let _ = self.tx.send(RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag });
    }

    /// Ask for a raster of the page at `scale` device pixels per point.
    pub fn ensure_pixels(&mut self, page: usize, scale: f32) {
        let Some(slot) = self.slots.get_mut(page) else { return };
        if slot.texture.is_some() && (slot.texture_scale - scale).abs() < f32::EPSILON {
            return;
        }
        if slot.pending_pixels.is_some_and(|(_, pending)| (pending - scale).abs() < f32::EPSILON) {
            return;
        }
        let tag = self.next_tag;
        self.next_tag += 1;
        slot.pending_pixels = Some((tag, scale));
        let _ = self.tx.send(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag });
    }

    /// Drop a page's raster (it is far off screen); it is re-requested when needed.
    pub fn free_texture(&mut self, page: usize) {
        if let Some(slot) = self.slots.get_mut(page) {
            slot.texture = None;
            slot.texture_scale = 0.0;
        }
    }

    /// Pick up finished work. Uploads rasters and stores paragraphs.
    pub fn poll(&mut self, ctx: &egui::Context) {
        loop {
            match self.rx.try_recv() {
                Ok(rendered) => self.accept(ctx, rendered),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn accept(&mut self, ctx: &egui::Context, rendered: RenderedPage) {
        let RenderedPage { request, width, height, rgba, error, text, .. } = rendered;
        let Some(slot) = self.slots.get_mut(request.page) else { return };
        match request.kind {
            RequestKind::Text => {
                if slot.pending_text.map(|(tag,)| tag) != Some(request.tag) {
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
                if slot.pending_pixels.map(|(tag, _)| tag) != Some(request.tag) {
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
