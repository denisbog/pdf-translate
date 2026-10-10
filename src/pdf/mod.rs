//! PDF backend, straight on top of the `hayro` crate.
//!
//! hayro parses and rasterizes one page at a time. This module adds what the reader needs around
//! it: a background render pool with a cancellable queue ([`raster`]), glyph extraction with
//! reading-order line layout ([`text`]) and document inspection ([`inspect`]).

mod inspect;
mod raster;
mod text;

pub use inspect::{DocInfo, FontInfo, OutlineItem, inspect};
pub use raster::{PageRenderer, RenderConfig, RenderPool, RenderRequest, RenderedPage, RequestKind};
pub use text::PageText;
