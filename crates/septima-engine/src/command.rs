use std::io::Read;
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
        ".tar.zst", ".tar.xz", ".tar.gz", ".tar.bz2", ".tar.lz4", ".tar.lz5", ".tar.br",
        ".tar.liz", ".tzst", ".txz", ".tgz", ".tbz2", ".tbz", ".tlz4", ".tlz5", ".tbr", ".tliz",
    ];
    SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// The decode flag that selects `7zz`'s *other* standalone-brotli format.
///
/// `7zz` writes two mutually unreadable `-tbrotli` streams depending on whether
/// `-mmt` was passed when the archive was created. On decode `-mmt` is a boolean
/// selector and its value is ignored (`-mmt=on` reads a stream written with
/// `-mmt4` just as well as `-mmt=32` does), so one retry always covers the other
/// mode. See [`should_retry_brotli`].
pub(crate) const BROTLI_MT_RETRY: &str = "-mmt=on";

/// Whether `path` is a standalone brotli stream — raw `.br` or a brotli-compressed
/// tar — i.e. one of the formats subject to the two-encoding split above. Brotli
/// *inside* a `.7z` container is unaffected and never reaches this path.
pub(crate) fn is_brotli_stream(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name.ends_with(".br") || name.ends_with(".tbr")
}

/// Whether a failed decode of `archive` should be retried in the other brotli
/// mode. Brotli carries no header recording which encoding it is, so a mode
/// mismatch is indistinguishable up front and surfaces only as a bare
/// `E_FAIL` — the retry is the detection. Bounded to a single extra run, on the
/// failure path only, and only for standalone brotli.
pub(crate) fn should_retry_brotli(archive: &Path, err: &EngineError) -> bool {
    is_brotli_stream(archive) && matches!(err, EngineError::SevenZip { .. })
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
    match list_compressed_tar_once(sevenzip, archive, None) {
        Err(e) if should_retry_brotli(archive, &e) => {
            list_compressed_tar_once(sevenzip, archive, Some(BROTLI_MT_RETRY)).map_err(|_| e)
        }
        other => other,
    }
}

/// One attempt at [`list_compressed_tar`], with an optional extra flag for the
/// outer decompressor.
fn list_compressed_tar_once(
    sevenzip: &Path,
    archive: &Path,
    extra: Option<&str>,
) -> Result<ArchiveListing, EngineError> {
    let mut decompress = Command::new(sevenzip)
        .arg("x")
        .arg("-so")
        .args(extra)
        .arg("--")
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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

    // The inner listing exits 0 on a truncated/empty stream, so a failed outer
    // decompressor would otherwise surface as an empty archive rather than an
    // error. Its status has to be checked explicitly. (Safe to read its stderr
    // only now: the inner `output()` has returned, so the pipe is drained and
    // the decompressor has run to completion.)
    let decompress_status = decompress.wait().map_err(EngineError::Spawn)?;
    if !decompress_status.success() {
        let mut stderr = String::new();
        if let Some(mut e) = decompress.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        return Err(EngineError::SevenZip {
            code: decompress_status.code(),
            stderr,
        });
    }

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

#[cfg(test)]
mod tests {
    use super::{is_brotli_stream, is_compressed_tar, should_retry_brotli};
    use crate::error::EngineError;
    use std::path::Path;

    #[test]
    fn recognises_standalone_brotli_streams() {
        for name in ["out.br", "bundle.tar.br", "OUT.BR", "archive.tbr"] {
            assert!(is_brotli_stream(Path::new(name)), "{name} should be brotli");
        }
        for name in ["out.tar.zst", "out.7z", "notes.abr", "out.br.txt", "out.bz2"] {
            assert!(!is_brotli_stream(Path::new(name)), "{name} should not be brotli");
        }
    }

    #[test]
    fn brotli_tars_are_still_compressed_tars() {
        assert!(is_compressed_tar(Path::new("bundle.tar.br")));
        assert!(!is_compressed_tar(Path::new("out.br"))); // raw stream, not a tar
    }

    #[test]
    fn retries_only_brotli_and_only_on_sevenzip_errors() {
        let fail = EngineError::SevenZip { code: Some(2), stderr: "E_FAIL".into() };
        assert!(should_retry_brotli(Path::new("out.br"), &fail));
        assert!(should_retry_brotli(Path::new("b.tar.br"), &fail));
        // Wrong format: a zstd failure is a real failure, not a mode mismatch.
        assert!(!should_retry_brotli(Path::new("out.tar.zst"), &fail));
        // Wrong error: never burn a second run on cancellation or a password.
        assert!(!should_retry_brotli(Path::new("out.br"), &EngineError::Cancelled));
        assert!(!should_retry_brotli(Path::new("out.br"), &EngineError::PasswordRequired));
    }
}
