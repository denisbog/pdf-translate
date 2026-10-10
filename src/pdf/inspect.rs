//! Document inspection on top of `hayro-syntax`.
//!
//! What the reader needs to open a document: its title, its page geometry, its bookmark tree and
//! the fonts its pages use. Rendering and text extraction live in [`super::raster`] and
//! [`super::text`].

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use hayro::hayro_interpret::hayro_syntax::object::{Dict, Name, Object, ObjectIdentifier};
use hayro::hayro_interpret::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};

/// One page's geometry.
#[derive(Clone, Debug, Default)]
pub struct PageInfo {
    /// Displayed size in points, after `/Rotate` and `/UserUnit`.
    pub width: f32,
    pub height: f32,
    /// Page label, falling back to the 1-based page number.
    pub label: String,
}

/// One bookmark.
#[derive(Clone, Debug, Default)]
pub struct OutlineItem {
    pub title: String,
    /// The 0-based page the bookmark points at, when it could be resolved.
    pub page: Option<usize>,
    pub children: Vec<OutlineItem>,
}

/// A font used by the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    /// BaseFont without the subset tag.
    pub name: String,
    /// Type1, TrueType, Type0, Type3, MMType1, CIDFontType0/2 …
    pub kind: String,
    pub embedded: bool,
    pub subset: bool,
    pub encoding: Option<String>,
}

/// Everything the reader keeps about an open document.
#[derive(Clone, Debug, Default)]
pub struct DocInfo {
    pub title: Option<String>,
    pub pages: Vec<PageInfo>,
    pub outline: Vec<OutlineItem>,
    pub fonts: Vec<FontInfo>,
}

/// Errors surfaced to the user when a document cannot be opened.
#[derive(Debug)]
pub enum OpenError {
    Invalid(String),
    NeedsPassword,
    WrongPassword,
    Unsupported(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Invalid(message) => write!(f, "the file is not a readable PDF: {message}"),
            OpenError::NeedsPassword => write!(f, "the document is protected by a password"),
            OpenError::WrongPassword => write!(f, "the password is incorrect"),
            OpenError::Unsupported(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Inspect a document. Fails only if the parser cannot open the file (or needs a password).
/// Never panics: parser crashes are caught and reported.
pub fn inspect(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocInfo, OpenError> {
    match catch_unwind(AssertUnwindSafe(|| inspect_inner(bytes, password))) {
        Ok(result) => result,
        Err(panic) => Err(OpenError::Invalid(format!("the parser crashed: {}", panic_message(&panic)))),
    }
}

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into())
}

fn inspect_inner(bytes: Arc<Vec<u8>>, password: Option<&str>) -> Result<DocInfo, OpenError> {
    let pdf = Pdf::new_with_password(bytes, password.unwrap_or("")).map_err(|error| match error {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
            if password.is_some() {
                OpenError::WrongPassword
            } else {
                OpenError::NeedsPassword
            }
        }
        LoadPdfError::Decryption(other) => OpenError::Unsupported(format!("encryption not supported: {other:?}")),
        LoadPdfError::Invalid => OpenError::Invalid("no readable page tree or cross-reference data was found".into()),
    })?;

    let title = pdf.metadata().title.as_deref().map(decode_string).filter(|title| !title.trim().is_empty());

    let mut pages = Vec::new();
    let mut page_of: HashMap<ObjectIdentifier, usize> = HashMap::new();
    let mut fonts: Vec<FontInfo> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (index, page) in pdf.pages().iter().enumerate() {
        let (width, height) = page.render_dimensions();
        pages.push(PageInfo { width, height, label: (index + 1).to_string() });
        if let Some(id) = page.raw().obj_id() {
            page_of.insert(id, index);
        }
        collect_fonts(page.resources(), &mut fonts, &mut seen);
    }

    let outline = pdf
        .xref()
        .get::<Dict>(pdf.xref().root_id())
        .and_then(|catalog| catalog.get::<Dict>("Outlines"))
        .map(|root| collect_outline(&pdf, &root, &page_of))
        .unwrap_or_default();

    Ok(DocInfo { title, pages, outline, fonts })
}

/// Collect the fonts of one page and its inherited resource dictionaries, deduplicated by name.
fn collect_fonts(resources: &hayro::hayro_interpret::hayro_syntax::page::Resources, fonts: &mut Vec<FontInfo>, seen: &mut HashSet<String>) {
    let mut current = Some(resources);
    while let Some(res) = current {
        for key in res.fonts.keys() {
            let Some(font) = res.fonts.get::<Dict>(key.as_ref()) else { continue };
            let Some(font) = font_info(&font) else { continue };
            if seen.insert(font.name.clone()) {
                fonts.push(font);
            }
        }
        current = res.parent();
    }
}

fn font_info(font: &Dict) -> Option<FontInfo> {
    let raw_name = font.get::<Name>("BaseFont")?.as_str().to_string();
    let subset = raw_name.split_once('+').is_some_and(|(prefix, _)| prefix.len() == 6);
    let name = raw_name.split_once('+').map_or(raw_name.clone(), |(_, rest)| rest.to_string());
    let kind = font.get::<Name>("Subtype").map(|subtype| subtype.as_str().to_string()).unwrap_or_default();
    let descriptor = font
        .get::<Dict>("FontDescriptor")
        .or_else(|| font.get::<hayro::hayro_interpret::hayro_syntax::object::Array>("DescendantFonts").and_then(|descendants| descendants.iter::<Dict>().next()).and_then(|descendant| descendant.get::<Dict>("FontDescriptor")));
    let embedded = descriptor.is_some_and(|d| d.contains_key("FontFile") || d.contains_key("FontFile2") || d.contains_key("FontFile3"));
    let encoding = font
        .get::<Name>("Encoding")
        .map(|encoding| encoding.as_str().to_string())
        .or_else(|| font.get::<Name>("BaseEncoding").map(|encoding| encoding.as_str().to_string()));
    Some(FontInfo { name, kind, embedded, subset, encoding })
}

fn collect_outline(pdf: &Pdf, root: &Dict, page_of: &HashMap<ObjectIdentifier, usize>) -> Vec<OutlineItem> {
    let mut out = Vec::new();
    let mut node = root.get::<Dict>("First");
    let mut guard = 0;
    while let Some(item) = node {
        guard += 1;
        if guard > 10_000 {
            break;
        }
        out.push(outline_item(pdf, &item, page_of, 0));
        node = item.get::<Dict>("Next");
    }
    out
}

fn outline_item(pdf: &Pdf, node: &Dict, page_of: &HashMap<ObjectIdentifier, usize>, depth: usize) -> OutlineItem {
    let title = node.get::<Object>("Title").and_then(Object::into_string).map(|string| decode_string(string.as_bytes())).unwrap_or_default();
    let page = destination(pdf, node, page_of);
    let children = if depth < 32 {
        let mut children = Vec::new();
        let mut child = node.get::<Dict>("First");
        let mut guard = 0;
        while let Some(item) = child {
            guard += 1;
            if guard > 10_000 {
                break;
            }
            children.push(outline_item(pdf, &item, page_of, depth + 1));
            child = item.get::<Dict>("Next");
        }
        children
    } else {
        Vec::new()
    };
    OutlineItem { title, page, children }
}

/// Where a bookmark goes: its `/Dest`, or the `/GoTo` action in `/A`.
fn destination<'a>(pdf: &'a Pdf, node: &Dict<'a>, page_of: &HashMap<ObjectIdentifier, usize>) -> Option<usize> {
    if let Some(dest) = node.get::<Object>("Dest") {
        return dest_page(pdf, dest, page_of);
    }
    let action = node.get::<Dict>("A")?;
    if action.get::<Name>("S")?.as_ref() != b"GoTo" {
        return None;
    }
    dest_page(pdf, action.get::<Object>("D")?, page_of)
}

/// Resolve a destination to a page index: an explicit array (page reference or page number), or
/// a named destination looked up in `/Dests` and the `/Names /Dests` name tree.
fn dest_page<'a>(pdf: &'a Pdf, dest: Object<'a>, page_of: &HashMap<ObjectIdentifier, usize>) -> Option<usize> {
    match dest {
        Object::Array(array) => {
            let first = array.iter::<Object>().next()?;
            if let Some(index) = first.clone().into_i32() {
                return usize::try_from(index.max(0)).ok();
            }
            page_of.get(&first.into_dict()?.obj_id()?).copied()
        }
        Object::Name(name) => named_dest(pdf, name.as_ref()).and_then(|found| dest_page(pdf, found, page_of)),
        Object::String(string) => named_dest(pdf, string.as_bytes()).and_then(|found| dest_page(pdf, found, page_of)),
        // A destination dictionary (`<< /D [...] >>`) is what a name tree maps a name to.
        Object::Dict(dict) => dest_page(pdf, dict.get::<Object>("D")?, page_of),
        _ => None,
    }
}

fn named_dest<'a>(pdf: &'a Pdf, name: &[u8]) -> Option<Object<'a>> {
    let catalog = pdf.xref().get::<Dict>(pdf.xref().root_id())?;
    if let Some(dests) = catalog.get::<Dict>("Dests") {
        if let Some(dest) = dests.get::<Object>(name) {
            return Some(dest);
        }
    }
    let tree = catalog.get::<Dict>("Names")?.get::<Dict>("Dests")?;
    let mut visited = 0usize;
    search_name_tree(&tree, name, &mut visited)
}

fn search_name_tree<'a>(node: &Dict<'a>, name: &[u8], visited: &mut usize) -> Option<Object<'a>> {
    *visited += 1;
    if *visited > 1000 {
        return None;
    }
    if let Some(names) = node.get::<hayro::hayro_interpret::hayro_syntax::object::Array>("Names") {
        let mut items = names.iter::<Object>();
        while let Some(key) = items.next() {
            let value = items.next()?;
            let matches = match key {
                Object::String(string) => string.as_bytes() == name,
                Object::Name(key_name) => key_name.as_ref() == name,
                _ => false,
            };
            if matches {
                return Some(value);
            }
        }
    }
    for kid in node.get::<hayro::hayro_interpret::hayro_syntax::object::Array>("Kids").map(|kids| kids.iter::<Dict>().collect::<Vec<_>>()).unwrap_or_default() {
        if let Some(found) = search_name_tree(&kid, name, visited) {
            return Some(found);
        }
    }
    None
}

/// Decode a PDF text string: UTF-16BE when it carries a BOM, otherwise best-effort UTF-8.
fn decode_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}