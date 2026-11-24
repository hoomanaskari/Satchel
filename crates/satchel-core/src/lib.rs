//! Platform-independent core of Satchel.
//!
//! This crate has no Tauri or OS dependencies so it can be unit-tested fast.
//! The public API in these modules is a CONTRACT shared with `src-tauri` and
//! the front end (see `docs/ARCHITECTURE.md`). Implement bodies freely; do not
//! change public signatures or serialized field names without updating the
//! contract document.

#![forbid(unsafe_code)]

pub mod classify;
pub mod error;
pub mod link;
pub mod model;
pub mod privacy;
pub mod store;

pub use error::{Error, Result};
pub use model::*;
pub use store::Store;
