use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::EngineError;
use crate::progress::{apply_fragment, ExtractProgress};

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
    mut on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
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

    let mut child = cmd.spawn().map_err(EngineError::Spawn)?;
    let mut stdout = child.stdout.take().expect("piped stdout");

    let mut buf = [0u8; 4096];
    let mut fragment: Vec<u8> = Vec::new();
    let mut state = ExtractProgress::default();

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EngineError::Cancelled);
        }
        let n = stdout.read(&mut buf).map_err(EngineError::Spawn)?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            if byte == b'\r' || byte == b'\n' {
                flush(&mut fragment, &mut state, &mut on_progress);
            } else {
                fragment.push(byte);
            }
        }
    }
    flush(&mut fragment, &mut state, &mut on_progress);

    let status = child.wait().map_err(EngineError::Spawn)?;
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }

    if status.success() {
        return Ok(());
    }
    if stderr.contains("Wrong password")
        || stderr.contains("Enter password")
        || stderr.contains("Data Error in encrypted")
    {
        return Err(EngineError::PasswordRequired);
    }
    Err(EngineError::SevenZip {
        code: status.code(),
        stderr,
    })
}

fn flush(
    fragment: &mut Vec<u8>,
    state: &mut ExtractProgress,
    on_progress: &mut impl FnMut(&ExtractProgress),
) {
    if fragment.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(fragment);
    if apply_fragment(state, &text) {
        on_progress(state);
    }
    fragment.clear();
}
