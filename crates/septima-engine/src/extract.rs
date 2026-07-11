use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::command::is_compressed_tar;
use crate::error::EngineError;
use crate::progress::{apply_fragment, ExtractProgress};
use crate::supervise::supervise;

/// How `7zz` should treat files that already exist at the destination (`-ao*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverwriteMode {
    #[default]
    Overwrite,
    Skip,
    RenameExtracted,
    RenameExisting,
}

impl OverwriteMode {
    fn flag(self) -> &'static str {
        match self {
            OverwriteMode::Overwrite => "-aoa",
            OverwriteMode::Skip => "-aos",
            OverwriteMode::RenameExtracted => "-aou",
            OverwriteMode::RenameExisting => "-aot",
        }
    }
}

/// A request to extract an archive to a destination directory.
#[derive(Debug, Clone)]
pub struct ExtractRequest {
    pub archive: PathBuf,
    pub dest_dir: PathBuf,
    pub password: Option<String>,
    pub overwrite: OverwriteMode,
}

impl ExtractRequest {
    pub fn new(archive: impl Into<PathBuf>, dest_dir: impl Into<PathBuf>) -> Self {
        Self {
            archive: archive.into(),
            dest_dir: dest_dir.into(),
            password: None,
            overwrite: OverwriteMode::default(),
        }
    }
}

/// A shared flag the caller can set to request cancellation.
pub type CancelToken = Arc<AtomicBool>;

pub fn new_cancel_token() -> CancelToken {
    Arc::new(AtomicBool::new(false))
}

/// Extract `req` via `7zz x`, invoking `on_progress` as the status line updates.
///
/// Blocking — run it on a worker thread. `cancel` is checked between reads;
/// setting it kills `7zz` and returns [`EngineError::Cancelled`]. Preserves the
/// full path layout (no `-spf`/`-e` collapsing); path-mode options come later.
pub fn run_extract(
    sevenzip: &Path,
    req: &ExtractRequest,
    cancel: &CancelToken,
    on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
    // Transparently peel a compressed tar so the files land, not the .tar.
    if is_compressed_tar(&req.archive) {
        return extract_compressed_tar(sevenzip, req, cancel, on_progress);
    }

    let mut cmd = Command::new(sevenzip);
    cmd.arg("x")
        .arg("-bsp1") // progress to stdout
        .arg("-bb1") // report each file name
        .arg("-y") // no interactive queries
        .arg(req.overwrite.flag())
        .arg(format!("-o{}", req.dest_dir.display()));
    if let Some(password) = &req.password {
        cmd.arg(format!("-p{password}"));
    }
    cmd.arg("--")
        .arg(&req.archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(EngineError::Spawn)?;
    supervise(child, cancel, on_progress)
}

/// Extract a compressed tar to `dest_dir` by piping the decompressed outer
/// stream into `7zz x -si -ttar` — so the files land, not the intermediate tar.
/// Decompression progress arrives on the first process's stderr (`-bsp2`).
fn extract_compressed_tar(
    sevenzip: &Path,
    req: &ExtractRequest,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
    let mut decompress = Command::new(sevenzip)
        .arg("x")
        .arg("-so")
        .arg("-bsp2") // progress to stderr, so stdout stays the tar stream
        .arg("-bb1")
        .arg("--")
        .arg(&req.archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(EngineError::Spawn)?;
    let tar_stream = decompress.stdout.take().expect("piped stdout");

    let mut untar = Command::new(sevenzip)
        .arg("x")
        .arg("-si")
        .arg("-ttar")
        .arg("-y")
        .arg(req.overwrite.flag())
        .arg(format!("-o{}", req.dest_dir.display()))
        .stdin(Stdio::from(tar_stream))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(EngineError::Spawn)?;

    // Stream the decompressor's progress (its stderr), honouring cancellation.
    let mut stderr = decompress.stderr.take().expect("piped stderr");
    let mut buf = [0u8; 4096];
    let mut fragment: Vec<u8> = Vec::new();
    let mut state = ExtractProgress::default();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = decompress.kill();
            let _ = untar.kill();
            let _ = decompress.wait();
            let _ = untar.wait();
            return Err(EngineError::Cancelled);
        }
        let n = stderr.read(&mut buf).map_err(EngineError::Spawn)?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            if byte == b'\r' || byte == b'\n' {
                if !fragment.is_empty() {
                    let text = String::from_utf8_lossy(&fragment);
                    if apply_fragment(&mut state, &text) {
                        on_progress(&state);
                    }
                    fragment.clear();
                }
            } else {
                fragment.push(byte);
            }
        }
    }

    let decompress_status = decompress.wait().map_err(EngineError::Spawn)?;
    let untar_status = untar.wait().map_err(EngineError::Spawn)?;
    let mut untar_err = String::new();
    if let Some(mut e) = untar.stderr.take() {
        let _ = e.read_to_string(&mut untar_err);
    }

    if decompress_status.success() && untar_status.success() {
        return Ok(());
    }
    let code = if untar_status.success() {
        decompress_status.code()
    } else {
        untar_status.code()
    };
    Err(EngineError::SevenZip {
        code,
        stderr: untar_err,
    })
}
