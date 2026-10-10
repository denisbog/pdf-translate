//! Background page rasterization and text extraction, straight on top of `hayro`.
//!
//! A `RenderPool` parses lazily once and shares that immutable parser between workers. Render
//! caches stay worker-local. Requests carry a caller tag that is echoed back.
//!
//! Every page render runs under `catch_unwind`, so a panic inside the renderer on a malformed
//! page yields an error instead of taking a worker down.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};

use crate::pdf::text::PageText;

/// Per-document rendering configuration.
#[derive(Clone, Debug, Default)]
pub struct RenderConfig {
    /// User or owner password for encrypted documents.
    pub password: Option<Arc<str>>,
}

impl RenderConfig {
    fn settings(&self) -> InterpreterSettings {
        InterpreterSettings::default()
    }
}

/// Hard cap on a rendered side, to bound memory at extreme zoom levels.
pub const MAX_SIDE: f32 = 8192.0;
/// Hard cap on rendered pixels per page (~64 MP ≈ 256 MB RGBA).
pub const MAX_PIXELS: f32 = 64.0e6;

/// What a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RequestKind {
    /// A raster of the page.
    #[default]
    Pixels,
    /// The page's text layer (glyphs with Unicode and boxes); `scale` is ignored.
    Text,
}

/// A region of a scaled page, in device pixels (for tiled rendering of large pages).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RenderRequest {
    pub page: usize,
    pub kind: RequestKind,
    /// Render only this region (device pixels at `scale`). `None` renders the whole page.
    pub tile: Option<Tile>,
    /// Device pixels per PDF point.
    pub scale: f32,
    /// Caller-defined tag, echoed back (used for zoom generations / thumbnail vs page).
    pub tag: u64,
}

#[derive(Debug)]
pub struct RenderedPage {
    pub request: RenderRequest,
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major. Empty when `error` is set.
    pub rgba: Vec<u8>,
    /// Why the page could not be rendered (renderer panic, empty page box, …).
    pub error: Option<String>,
    /// For `RequestKind::Text`.
    pub text: Option<Arc<PageText>>,
}

/// Clamp a requested scale so the output respects `MAX_SIDE` and `MAX_PIXELS`.
pub fn effective_scale(width_pt: f32, height_pt: f32, scale: f32) -> f32 {
    let (w, h) = (width_pt.max(1.0), height_pt.max(1.0));
    let by_side = MAX_SIDE / w.max(h);
    let by_area = (MAX_PIXELS / (w * h)).sqrt();
    let capped = scale.min(by_side).min(by_area);
    if capped.is_finite() && capped > 0.0 { capped } else { by_side.min(by_area).max(f32::MIN_POSITIVE) }
}

/// Device pixels that cover `pt` points at `scale`: rounded up, so the last partial row or
/// column of a page is drawn rather than cut off. At least 1; saturates instead of overflowing.
pub fn device_pixels(pt: f32, scale: f32) -> u32 {
    let px = pt * scale - 0.01;
    if px.is_finite() { px.ceil().max(1.0) as u32 } else { 1 }
}

type Output = (u32, u32, Vec<u8>, Option<Arc<PageText>>);

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into())
}

/// Render (or extract the text of) one page with a caller-owned parser. Panics inside the
/// renderer are caught and reported as `Err(message)`.
fn render_page(pdf: &Pdf, settings: &InterpreterSettings, req: RenderRequest) -> Result<Output, String> {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if req.kind == RequestKind::Text {
            let text = crate::pdf::text::extract_page(pdf, req.page, settings).ok_or_else(|| format!("page {} does not exist", req.page + 1))?;
            return Ok((0, 0, Vec::new(), Some(Arc::new(text))));
        }
        if req.tile.is_some() {
            return Err("tiled rendering is not implemented".to_string());
        }
        let pages = pdf.pages();
        let page = pages.get(req.page).ok_or_else(|| format!("page {} does not exist", req.page + 1))?;
        let (w, h) = page.render_dimensions();
        if !(w.is_finite() && h.is_finite()) || w < 0.5 || h < 0.5 {
            return Err(format!("page {} has an empty or invalid page box ({w}×{h} pt)", req.page + 1));
        }
        let scale = effective_scale(w, h, req.scale);
        let side = |pt: f32| device_pixels(pt, scale).min(MAX_SIDE as u32).max(1) as u16;
        let rs = RenderSettings { x_scale: scale, y_scale: scale, width: Some(side(w)), height: Some(side(h)), bg_color: WHITE };
        let cache = RenderCache::new();
        let pixmap = hayro::render(page, &cache, settings, &rs);
        let (pw, ph) = (pixmap.width(), pixmap.height());
        let rgba: Vec<u8> = pixmap.data().iter().flat_map(|p| p.to_u8_array()).collect();
        Ok((pw as u32, ph as u32, rgba, None))
    }));
    match result {
        Ok(r) => r,
        Err(p) => Err(format!("renderer crashed on page {}: {}", req.page + 1, panic_message(&p))),
    }
}

fn finish(req: RenderRequest, r: Result<Output, String>) -> RenderedPage {
    match r {
        Ok((width, height, rgba, text)) => RenderedPage { request: req, width, height, rgba, error: None, text },
        Err(error) => RenderedPage { request: req, width: 0, height: 0, rgba: Vec::new(), error: Some(error), text: None },
    }
}

/// A single-threaded renderer over one parsed document (used by the CLI).
pub struct PageRenderer {
    pdf: Option<Arc<Pdf>>,
    settings: InterpreterSettings,
}

impl PageRenderer {
    pub fn new(bytes: Arc<Vec<u8>>, config: RenderConfig) -> Self {
        let pdf = parse(&bytes, config.password.as_deref());
        let settings = config.settings();
        Self { pdf, settings }
    }

    pub fn page_count(&self) -> usize {
        self.pdf.as_ref().map(|p| p.pages().len()).unwrap_or(0)
    }

    /// Render one page. Never panics.
    pub fn render(&mut self, req: RenderRequest) -> RenderedPage {
        let r = match self.pdf.as_ref() {
            Some(pdf) => render_page(pdf, &self.settings, req),
            None => Err("the document could not be parsed".to_string()),
        };
        finish(req, r)
    }
}

/// Parse a document, catching panics in the parser.
fn parse(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Option<Arc<Pdf>> {
    catch_unwind(AssertUnwindSafe(|| Pdf::new_with_password(bytes.clone(), password.unwrap_or("")).ok())).ok().flatten().map(Arc::new)
}

/// The work a pool has been asked for, plus what is in flight and already finished. One entry per
/// request (its tag makes it unique), so a request is never rendered twice before it is picked up.
#[derive(Default)]
struct Queue {
    desired: Vec<RenderRequest>,
    active: HashSet<u64>,
    done: HashSet<u64>,
}

impl Queue {
    fn replace(&mut self, requests: Vec<RenderRequest>) {
        self.desired = requests;
        // A finished request that is no longer wanted is forgotten, so it can be asked for again.
        let desired: HashSet<u64> = self.desired.iter().map(|request| request.tag).collect();
        self.done.retain(|tag| desired.contains(tag));
    }

    fn pop(&mut self) -> Option<RenderRequest> {
        let request = self.desired.iter().find(|request| !self.active.contains(&request.tag) && !self.done.contains(&request.tag)).copied()?;
        self.active.insert(request.tag);
        Some(request)
    }

    /// Mark a request finished; returns whether its result is still wanted.
    fn finish(&mut self, request: RenderRequest) -> bool {
        self.active.remove(&request.tag);
        if self.desired.iter().any(|wanted| wanted.tag == request.tag) {
            self.done.insert(request.tag);
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    parser: OnceLock<Option<Arc<Pdf>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Renders pages on worker threads, most urgent request first.
///
/// The UI rebuilds the queue each frame from the pages it still needs; a request that is no
/// longer in the queue has its (late) result dropped instead of uploaded.
pub struct RenderPool {
    shared: Arc<Shared>,
    bytes: Arc<Vec<u8>>,
    config: RenderConfig,
    results: Mutex<Receiver<RenderedPage>>,
    wake: Mutex<Vec<SyncSender<()>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    stop: Arc<AtomicBool>,
}

impl RenderPool {
    pub fn new(bytes: Arc<Vec<u8>>, threads: usize, config: RenderConfig) -> Self {
        let shared = Arc::new(Shared::default());
        let (results_tx, results_rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let pool = Self {
            shared,
            bytes,
            config,
            results: Mutex::new(results_rx),
            wake: Mutex::new(Vec::new()),
            workers: Mutex::new(Vec::new()),
            stop,
        };
        for id in 0..threads.max(1) {
            pool.spawn_worker(id, results_tx.clone());
        }
        pool
    }

    fn spawn_worker(&self, id: usize, results: Sender<RenderedPage>) {
        let (wake_tx, wake_rx) = sync_channel::<()>(1);
        let (shared, bytes, config, stop) = (self.shared.clone(), self.bytes.clone(), self.config.clone(), self.stop.clone());
        let spawned = std::thread::Builder::new().name(format!("pdf-translate-render-{id}")).spawn(move || worker(shared, bytes, config, wake_rx, results, stop));
        match spawned {
            Ok(handle) => {
                lock(&self.wake).push(wake_tx);
                lock(&self.workers).push(handle);
            }
            Err(error) => log_warning(&format!("could not spawn render worker {id}: {error}")),
        }
    }

    /// `true` when rendering happens on the caller's thread (no worker threads available).
    pub fn is_inline(&self) -> bool {
        false
    }

    /// Replace this frame's desired work (most urgent first). In-flight work is not killed.
    pub fn set_queue(&self, requests: Vec<RenderRequest>) {
        lock(&self.shared.queue).replace(requests);
        self.wake_workers();
    }

    fn wake_workers(&self) {
        for wake in lock(&self.wake).iter() {
            // One pending wakeup is enough; UI polling must not grow wake channels.
            let _ = wake.try_send(());
        }
    }

    /// Pick up finished work (uploads rasters and stores paragraphs in the caller).
    pub fn try_recv(&self) -> Option<RenderedPage> {
        let page = lock(&self.results).try_recv().ok();
        self.wake_workers();
        page
    }
}

impl Drop for RenderPool {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake_workers();
    }
}

fn log_warning(message: &str) {
    eprintln!("{message}");
}

fn worker(shared: Arc<Shared>, bytes: Arc<Vec<u8>>, config: RenderConfig, wake: Receiver<()>, results: Sender<RenderedPage>, stop: Arc<AtomicBool>) {
    let settings = config.settings();
    while !stop.load(Ordering::Acquire) {
        let Some(request) = lock(&shared.queue).pop() else {
            // Wait for work; the short timeout also lets a dropped pool's workers exit.
            let _ = wake.recv_timeout(Duration::from_millis(50));
            continue;
        };
        // Parsed once per pool, on first use, and shared by every worker.
        let pdf = shared.parser.get_or_init(|| parse(&bytes, config.password.as_deref()));
        let rendered = match pdf {
            Some(pdf) => render_page(pdf, &settings, request),
            None => Err("the document could not be parsed".to_string()),
        };
        let page = finish(request, rendered);
        if lock(&shared.queue).finish(request) {
            let _ = results.send(page);
        }
    }
}
