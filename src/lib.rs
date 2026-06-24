//! High-level, owned API for reading and editing OneNote documents.
//!
//! `libonenote` deliberately separates its public document model from the
//! binary parser. Applications can depend on this crate without exposing
//! parser-specific types throughout their codebase.
//!
//! Unchanged `.one` and `.onepkg` documents can be saved byte-for-byte through
//! [`Document::save_native`]. Modified `.one` sections and `.onepkg` packages
//! currently support verified page-title and paragraph changes that fit their
//! existing property allocations. Other native edits return a hard error
//! instead of silently losing data.

#![forbid(unsafe_code)]

mod error;
mod graph;
mod loader;
mod model;
mod native;

pub use error::{Error, Result};
pub use graph::*;
pub use loader::{BinaryDataPolicy, InkDataPolicy, LoadOptions, Loader, open};
pub use model::*;
