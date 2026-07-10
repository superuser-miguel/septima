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
//!
//! ## Modules to come (Phase 2)
//!
//! - listing parser — `7z l -slt`
//! - progress parser — `-bsp1 -bb1`
//! - subprocess supervisor — spawn/cancel/wait around `7zz`

/// The `7zz` binary this engine drives, as invoked by default.
///
/// Placeholder constant that also proves the crate is linkable from
/// `septima-gtk` before any real API exists. Discovery/override logic lands
/// with the supervisor in Phase 2.
pub const SEVENZIP_BIN: &str = "7zz";
