//! Translation cache and background translation worker.
//!
//! Translations live in `~/.cache/pdf-translate/<document>-<target>.json`, keyed by the
//! normalised paragraph text, so a second reader (or a re-open) never asks the model
//! again. Requests run on one worker thread that owns the local ONNX model, and report
//! their stage back to the UI while they work.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use crate::llm::{self, Llm};

/// Which part of a request is running, shown next to the paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Queued,
    LoadingModel,
    Translating,
    Failed(String),
}

impl Stage {
    pub fn label(&self) -> &str {
        match self {
            Stage::Queued => "queued",
            Stage::LoadingModel => "loading the model…",
            Stage::Translating => "translating…",
            Stage::Failed(_) => "failed",
        }
    }
}

#[derive(Clone)]
pub struct Job {
    /// The document the paragraph belongs to, as its tab id.
    pub doc: u64,
    /// That document's cache: the entry is written here when the translation is ready, and
    /// checked here first, so one worker serves every open document.
    pub store: Arc<Mutex<Store>>,
    pub key: String,
    pub text: String,
    pub target: String,
    /// The page the paragraph is on, so the translations list can jump back to it.
    pub page: usize,
}

#[derive(Clone, Debug)]
pub enum Update {
    Stage(u64, String, Stage),
    Done { doc: u64, key: String },
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Entry {
    pub source: String,
    pub translation: String,
    /// The page the paragraph was on, when it was translated.
    #[serde(default)]
    pub page: Option<usize>,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    #[serde(default)]
    source_file: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

/// The on-disk translation store of one document in one target language.
pub struct Store {
    path: PathBuf,
    target: String,
    model: String,
    source_file: String,
    entries: HashMap<String, Entry>,
}

fn fnv(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `$XDG_CACHE_HOME/pdf-translate`, else `~/.cache/pdf-translate`.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("pdf-translate")
}

/// The document's cache file: the canonical path, size and mtime pin the exact bytes,
/// so editing or replacing the PDF gets a fresh cache.
pub fn cache_path(pdf: &Path, target: &str) -> PathBuf {
    let canonical = std::fs::canonicalize(pdf).unwrap_or_else(|_| pdf.to_path_buf());
    let mut key = canonical.to_string_lossy().into_owned();
    if let Ok(meta) = std::fs::metadata(pdf) {
        key.push_str(&format!(
            ":{}:{}",
            meta.len(),
            meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
        ));
    }
    let stem = pdf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    let stem: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    cache_dir().join(format!("{stem}-{:016x}-{target}.json", fnv(&key)))
}

impl Store {
    /// Load the document's cache, starting empty when it does not exist yet.
    pub fn load(pdf: &Path, target: &str, model: &str) -> Self {
        let path = cache_path(pdf, target);
        let file: CacheFile = std::fs::read_to_string(&path).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
        Self {
            path,
            target: target.to_string(),
            model: model.to_string(),
            source_file: pdf.to_string_lossy().into_owned(),
            entries: file.entries,
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|entry| entry.translation.as_str())
    }

    /// Every translation of this document, as `(key, source, translation, page)`, sorted by source.
    pub fn entries(&self) -> Vec<(String, String, String, Option<usize>)> {
        let mut out: Vec<(String, String, String, Option<usize>)> =
            self.entries.iter().map(|(key, entry)| (key.clone(), entry.source.clone(), entry.translation.clone(), entry.page)).collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }

    /// Forget every translation of this document and drop its cache file.
    pub fn clear(&mut self) -> Result<(), String> {
        self.entries.clear();
        let _ = std::fs::remove_file(&self.path);
        Ok(())
    }


    fn insert(&mut self, key: &str, source: String, translation: String, page: usize) -> Result<(), String> {
        self.entries.insert(key.to_string(), Entry { source, translation, page: Some(page) });
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let file = CacheFile {
            source_file: self.source_file.clone(),
            target: self.target.clone(),
            model: self.model.clone(),
            entries: self.entries.clone(),
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&file).map_err(|error| error.to_string())?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, json).map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, &self.path).map_err(|error| format!("cannot write {}: {error}", self.path.display()))
    }
}

/// Where the ONNX models live: the same folder the transcribe project uses, so the
/// models already downloaded there are reused (`PDF_TRANSLATE_MODELS` overrides).
pub fn models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PDF_TRANSLATE_MODELS") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("transcribe-models/onnx")
}

/// Split a paragraph into sentences, so the model translates one short unit at a time and
/// a long paragraph cannot drift or get summarised.
pub fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            current.clear();
            continue;
        }
        current.push(c);
        let ends = matches!(c, '.' | '!' | '?' | '…');
        // A '.' inside "3.5" or after an initial must not end a sentence.
        let next_is_space = chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        let number = c == '.' && chars.get(i.wrapping_sub(1)).is_some_and(|p| p.is_ascii_digit()) && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
        if ends && next_is_space && !number {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            current.clear();
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

/// Translate one paragraph: sentence by sentence in small numbered batches.
pub fn translate_paragraph(llm: &mut Llm, text: &str, target: &str) -> Result<String, String> {
    let sentences = split_sentences(text);
    if sentences.is_empty() {
        return Ok(String::new());
    }
    let target_name = llm::language_name(target);
    let mut out: Vec<String> = Vec::with_capacity(sentences.len());
    for batch in sentences.chunks(5) {
        let numbered = batch.iter().enumerate().map(|(i, line)| format!("{}. {}", i + 1, line)).collect::<Vec<_>>().join("\n");
        let prompt = format!(
            "You are a professional translator. Translate each numbered line into {target_name}. \
             Keep the numbering: output exactly {} lines, one translation per line, and nothing else. \
             Never copy the source text, never merge or split lines. If a line is already in {target_name}, copy it unchanged.\n\n{numbered}",
            batch.len(),
        );
        let max_new = 200 + 140 * batch.len();
        let answer = llm.generate(&prompt, max_new)?;
        match llm::parse_numbered(&answer, batch.len()) {
            Some(lines) => out.extend(lines),
            None => {
                // Fall back to one line at a time, which the small model handles more reliably.
                for sentence in batch {
                    let one = format!(
                        "Translate this into {target_name}. Output only the translation, nothing else.\n\n{sentence}"
                    );
                    let answer = llm.generate(&one, 300)?;
                    out.push(answer.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    Ok(out.join(" ").split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Translation worker: owns the model, answers jobs in order, reports stages.
pub struct Worker {
    jobs: Option<Sender<Job>>,
    updates: Receiver<Update>,
    pub log: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
    /// Set when the worker is dropped: the running job stops at the next token.
    cancel: Arc<AtomicBool>,
}

/// Shutting down: close the queue, ask the running job to stop, and *wait for the thread*.
///
/// The thread owns the model, and its ONNX session must be destroyed while the process is
/// still alive: releasing the CUDA execution provider during process teardown (which is what
/// happens when the thread is left detached) segfaults inside `libonnxruntime_providers_cuda`.
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Worker {
    /// Start the worker. `ctx` is only used to wake the UI when an update is ready.
    pub fn spawn(model: &str, ctx: egui::Context) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (update_tx, update_rx) = mpsc::channel::<Update>();
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let worker_log = log.clone();
        let thread_model = model.to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("translator".into())
            .spawn(move || {
                let mut llm: Option<Llm> = None;
                while let Ok(job) = job_rx.recv() {
                    if worker_cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let notify = |update: Update| {
                        let _ = update_tx.send(update);
                        ctx.request_repaint();
                    };
                    if job.store.lock().map(|s| s.get(&job.key).is_some()).unwrap_or(false) {
                        notify(Update::Done { doc: job.doc, key: job.key });
                        continue;
                    }
                    notify(Update::Stage(job.doc, job.key.clone(), Stage::Queued));
                    if llm.is_none() {
                        notify(Update::Stage(job.doc, job.key.clone(), Stage::LoadingModel));
                        let runtime = llm::prepare_runtime();
                        if let Some(dir) = runtime {
                            if let Ok(mut log) = worker_log.lock() {
                                log.push(format!("onnx runtime: {}", dir.display()));
                            }
                        }
                        let mut sink = {
                            let log = worker_log.clone();
                            move |message: String| {
                                if let Ok(mut log) = log.lock() {
                                    log.push(message.clone());
                                }
                            }
                        };
                        match Llm::load(&thread_model, &models_dir(), true, &mut sink) {
                            Ok(mut loaded) => {
                                sink(format!("model ready: {}", loaded.summary()));
                                loaded.set_cancel(worker_cancel.clone());
                                llm = Some(loaded);
                            }
                            Err(error) => {
                                sink(format!("model failed: {error}"));
                                notify(Update::Stage(job.doc, job.key.clone(), Stage::Failed(error)));
                                continue;
                            }
                        }
                    }
                    notify(Update::Stage(job.doc, job.key.clone(), Stage::Translating));
                    let result = llm.as_mut().map(|l| translate_paragraph(l, &job.text, &job.target));
                    if worker_cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    match result {
                        Some(Ok(translation)) if !translation.trim().is_empty() => {
                            let doc = job.doc;
                            let key = job.key.clone();
                            let source = job.text.clone();
                            let saved = job
                                .store
                                .lock()
                                .map_err(|_| "cache lock poisoned".to_string())
                                .and_then(|mut store| store.insert(&key, source.clone(), translation.clone(), job.page));
                            match saved {
                                Ok(()) => notify(Update::Done { doc, key }),
                                Err(error) => notify(Update::Stage(doc, key, Stage::Failed(error))),
                            }
                        }
                        Some(Ok(_)) => notify(Update::Stage(job.doc, job.key.clone(), Stage::Failed("the model returned nothing".into()))),
                        Some(Err(error)) => notify(Update::Stage(job.doc, job.key.clone(), Stage::Failed(error))),
                        None => notify(Update::Stage(job.doc, job.key.clone(), Stage::Failed("the model is not loaded".into()))),
                    }
                }
                let _ = update_tx;
            })
            .expect("spawn translator");
        Self { jobs: Some(job_tx), updates: update_rx, log, thread: Some(thread), cancel }
    }


    pub fn submit(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }

    pub fn try_recv(&self) -> Option<Update> {
        self.updates.try_recv().ok()
    }
}

/// Command-line helper: load the local model and translate one piece of text.
/// Used by `pdf-translate --translate …` for testing without the UI.
pub fn translate_with_model(model: &str, text: &str, target: &str) -> Result<String, String> {
    let runtime = llm::prepare_runtime();
    if let Some(dir) = runtime {
        eprintln!("onnx runtime: {}", dir.display());
    }
    let mut llm = Llm::load(model, &models_dir(), true, &mut |message| eprintln!("{message}"))?;
    eprintln!("using {}", llm.summary());
    translate_paragraph(&mut llm, text, target)
}
