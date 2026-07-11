use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::EngineError;
use crate::listing::{parse_listing, ArchiveListing};

/// Resolve which `7zz` binary to run.
///
/// `SEPTIMA_7ZZ` overrides (useful for tests / dev); otherwise the bare name,
/// resolved via `PATH` — inside the Flatpak that is `/app/bin/7zz`.
pub fn sevenzip_path() -> PathBuf {
    std::env::var_os("SEPTIMA_7ZZ")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::SEVENZIP_BIN))
}

/// List an archive's contents via `7z l -slt`.
///
/// stdin is closed so an encrypted archive returns promptly (7zz would otherwise
/// block on an interactive password prompt); a missing/wrong password maps to
/// [`EngineError::PasswordRequired`]. Pass `password` for archives with
/// encrypted headers.
/// Whether `path` is a compressed tarball (`.tar.zst`, `.tgz`, …) that must be
/// descended into two layers to show its files.
pub fn is_compressed_tar(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    const SUFFIXES: &[&str] = &[
        ".tar.zst", ".tar.xz", ".tar.gz", ".tar.bz2", ".tar.lz4", ".tzst", ".txz", ".tgz",
        ".tbz2", ".tbz",
    ];
    SUFFIXES.iter().any(|s| name.ends_with(s))
}

pub fn list_archive(
    sevenzip: &Path,
    archive: &Path,
    password: Option<&str>,
) -> Result<ArchiveListing, EngineError> {
    // Transparently descend a compressed tar so its files show, not the tar.
    if is_compressed_tar(archive) {
        return list_compressed_tar(sevenzip, archive);
    }

    let mut cmd = Command::new(sevenzip);
    cmd.arg("l").arg("-slt");
    if let Some(password) = password {
        cmd.arg(format!("-p{password}"));
    }
    let output = cmd
        .arg("--")
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(EngineError::Spawn)?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stdout.contains("Enter password") || stdout.contains("Wrong password") || stderr.contains("Wrong password") {
            return Err(EngineError::PasswordRequired);
        }
        return Err(EngineError::SevenZip {
            code: output.status.code(),
            stderr: stderr.into_owned(),
        });
    }

    let mut listing = parse_listing(&stdout);
    listing.path = archive.to_path_buf();
    Ok(listing)
}

/// List the contents of a compressed tar by piping the decompressed outer
/// stream (`7zz x -so`) into a tar listing (`7zz l -slt -si -ttar`) — no temp
/// file, and the real files show instead of the intermediate `.tar`.
fn list_compressed_tar(sevenzip: &Path, archive: &Path) -> Result<ArchiveListing, EngineError> {
    let mut decompress = Command::new(sevenzip)
        .arg("x")
        .arg("-so")
        .arg("--")
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(EngineError::Spawn)?;

    let stdout = decompress.stdout.take().expect("piped stdout");
    let output = Command::new(sevenzip)
        .arg("l")
        .arg("-slt")
        .arg("-si")
        .arg("-ttar")
        .stdin(Stdio::from(stdout))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(EngineError::Spawn)?;
    let _ = decompress.wait();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EngineError::SevenZip {
            code: output.status.code(),
            stderr: stderr.into_owned(),
        });
    }

    let mut listing = parse_listing(&String::from_utf8_lossy(&output.stdout));
    listing.path = archive.to_path_buf();
    Ok(listing)
}
