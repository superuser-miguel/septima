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
pub fn list_archive(
    sevenzip: &Path,
    archive: &Path,
    password: Option<&str>,
) -> Result<ArchiveListing, EngineError> {
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
