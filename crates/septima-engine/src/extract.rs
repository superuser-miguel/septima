use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::command::is_compressed_tar;
use crate::compress::existing_output_paths;
use crate::error::EngineError;
use crate::progress::{apply_fragment, ExtractProgress};
use crate::supervise::supervise;

/// Delete `archive` and any of its volume parts (`archive.001`, `archive.002`, …)
/// after a successful extract. `archive` may itself be any one part (whichever
/// the user opened) — the base name is recovered before scanning for siblings.
/// Best-effort: keeps going past a failed removal and returns the first error
/// encountered, if any.
pub fn delete_archive(archive: &Path) -> std::io::Result<()> {
    let mut first_err = None;
    for path in existing_output_paths(&volume_base(archive)) {
        if let Err(e) = std::fs::remove_file(&path) {
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// If `path` ends in a `-v`-style volume suffix (`.001`, `.002`, …), strip it
/// to recover the archive's base name; otherwise return `path` unchanged.
fn volume_base(path: &Path) -> PathBuf {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return path.to_path_buf();
    };
    let Some((stem, suffix)) = name.rsplit_once('.') else {
        return path.to_path_buf();
    };
    if suffix.len() >= 2 && suffix.bytes().all(|b| b.is_ascii_digit()) {
        path.with_file_name(stem)
    } else {
        path.to_path_buf()
    }
}

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

#[cfg(test)]
mod tests {
    use super::delete_archive;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A unique scratch dir; caller removes it.
    fn scratch(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("septima-test-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn deletes_a_single_file_archive() {
        let dir = scratch("delete-single");
        let archive = dir.join("out.7z");
        std::fs::write(&archive, b"an archive").unwrap();

        delete_archive(&archive).unwrap();

        assert!(!archive.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deletes_every_volume_part_given_the_base_name() {
        let dir = scratch("delete-volumes-base");
        let archive = dir.join("out.7z");
        std::fs::write(dir.join("out.7z.001"), b"vol1").unwrap();
        std::fs::write(dir.join("out.7z.002"), b"vol2").unwrap();
        std::fs::write(dir.join("other.7z"), b"keep").unwrap();

        delete_archive(&archive).unwrap();

        assert!(!dir.join("out.7z.001").exists());
        assert!(!dir.join("out.7z.002").exists());
        assert!(dir.join("other.7z").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The user opens (and Septima is handed) whichever part they picked, not
    /// the bare base name — e.g. `out.7z.001`. Every sibling part must still
    /// go, not just the one that was opened.
    #[test]
    fn deletes_every_volume_part_given_the_part_the_user_opened() {
        let dir = scratch("delete-volumes-part");
        std::fs::write(dir.join("out.7z.001"), b"vol1").unwrap();
        std::fs::write(dir.join("out.7z.002"), b"vol2").unwrap();
        std::fs::write(dir.join("out.7z.003"), b"vol3").unwrap();
        std::fs::write(dir.join("other.7z"), b"keep").unwrap();

        delete_archive(&dir.join("out.7z.001")).unwrap();

        assert!(!dir.join("out.7z.001").exists());
        assert!(!dir.join("out.7z.002").exists());
        assert!(!dir.join("out.7z.003").exists());
        assert!(dir.join("other.7z").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
