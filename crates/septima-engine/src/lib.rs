//! # septima-engine
//!
//! UI-free core for Septima: spawns and supervises the `7zz` (7-Zip ZS) CLI,
//! parses `7z l -slt` listings and `-bsp1 -bb1` progress streams.
//!
//! ## Boundary
//!
//! This crate MUST NOT depend on GTK, GLib, or libadwaita (Kickoff plan A1).
//! All portal interaction lives in `septima-gtk`; the engine accepts plain
//! paths — including opaque doc-portal paths under `/run/user/*/doc/` — without
//! normalization (Work Area C1).

mod command;
mod error;
mod extract;
mod listing;
mod progress;

pub use command::{list_archive, sevenzip_path};
pub use error::EngineError;
pub use extract::{new_cancel_token, run_extract, CancelToken, ExtractRequest, OverwriteMode};
pub use listing::{parse_listing, ArchiveEntry, ArchiveListing};
pub use progress::ExtractProgress;

/// The `7zz` binary this engine drives by default (resolved via `PATH`).
pub const SEVENZIP_BIN: &str = "7zz";
