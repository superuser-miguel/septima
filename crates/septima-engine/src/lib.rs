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

pub mod capabilities;
mod command;
mod compress;
mod error;
mod extract;
mod hash;
mod listing;
mod progress;
mod supervise;

pub use capabilities::{formats, Codec, Format};
pub use command::{list_archive, sevenzip_path};
pub use compress::{estimate_add_memory, run_add, run_tar_and_compress, CompressionRequest};
pub use hash::{hash_algorithms, hash_file, hash_file_progress, Digest, HashAlgo};
pub use error::EngineError;
pub use extract::{new_cancel_token, run_extract, CancelToken, ExtractRequest, OverwriteMode};
pub use listing::{parse_listing, ArchiveEntry, ArchiveListing};
pub use progress::ExtractProgress;

/// The `7zz` binary this engine drives by default (resolved via `PATH`).
pub const SEVENZIP_BIN: &str = "7zz";
