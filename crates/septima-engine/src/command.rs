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

/// The decode flag that reaches `7zz`'s *other* brotli format for `archive`, or
/// `None` if brotli can't be hiding in that kind of file.
///
/// Brotli records nothing about which of the two formats a stream is, so a
/// mismatch can't be seen up front — it surfaces as a bare `E_FAIL`, and the
/// retry *is* the detection. Which way to retry depends on the container, and
/// the two point in opposite directions:
///
/// * **Standalone** (`.br`, `.tbr`, `.tar.br`) — `7zz` writes a plain stream by
///   default and its multithreaded chain format when `-mmt` is passed, so a
///   failed read retries with `-mmt=on`. On decode `-mmt` is a boolean selector
///   whose value is ignored (`-mmt=on` reads an `-mmt4` stream as happily as
///   `-mmt=32` does), so a single retry covers every thread count.
/// * **Inside a `.7z`** — the reverse, so the retry is `-mmt=off`. A container
///   decodes as the multithreaded format unless `-mmt=off` is passed, which
///   leaves archives written with `-t7z -m0=brotli -mmt=off` unreadable without
///   it. Septima never writes those (brotli in a container keeps its threads),
///   but they exist in the wild and the bundled `7zz` can't open them otherwise.
///   Upstream deliberately kept this behind the flag rather than probing by
///   default, so the retry has to live on our side:
///   <https://github.com/mcmilk/7-Zip-zstd/issues/538>
pub(crate) fn brotli_retry_flag(archive: &Path) -> Option<&'static str> {
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.ends_with(".br") || name.ends_with(".tbr") {
        Some("-mmt=on")
    } else if name.ends_with(".7z") {
        Some("-mmt=off")
    } else {
        None
    }
}

/// Whether a failed decode of `archive` should be retried in the other brotli
/// mode. Bounded to a single extra run, on the failure path only, and never for
/// a wrong password or a cancel — those are already unambiguous.
pub(crate) fn should_retry_brotli(archive: &Path, err: &EngineError) -> bool {
    brotli_retry_flag(archive).is_some() && matches!(err, EngineError::SevenZip { .. })
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

    match list_archive_once(sevenzip, archive, password, None) {
        Err(e) if should_retry_brotli(archive, &e) => {
            list_archive_once(sevenzip, archive, password, brotli_retry_flag(archive))
                .map_err(|_| e)
        }
        other => other,
    }
}

/// One `7zz l -slt` attempt, with an optional extra flag (see
/// [`brotli_retry_flag`]).
fn list_archive_once(
    sevenzip: &Path,
    archive: &Path,
    password: Option<&str>,
    extra: Option<&str>,
) -> Result<ArchiveListing, EngineError> {
    let mut cmd = Command::new(sevenzip);
    cmd.arg("l").arg("-slt");
    if let Some(password) = password {
        cmd.arg(format!("-p{password}"));
    }
    if let Some(extra) = extra {
        cmd.arg(extra);
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
            list_compressed_tar_once(sevenzip, archive, brotli_retry_flag(archive)).map_err(|_| e)
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
    use super::{brotli_retry_flag, is_compressed_tar, should_retry_brotli};
    use crate::error::EngineError;
    use std::path::Path;

    #[test]
    fn standalone_brotli_retries_towards_the_threaded_format() {
        for name in ["out.br", "bundle.tar.br", "OUT.BR", "archive.tbr"] {
            assert_eq!(brotli_retry_flag(Path::new(name)), Some("-mmt=on"), "{name}");
        }
        for name in ["out.tar.zst", "notes.abr", "out.br.txt", "out.bz2"] {
            assert_eq!(brotli_retry_flag(Path::new(name)), None, "{name}");
        }
    }

    #[test]
    fn a_7z_retries_the_other_way() {
        // A container decodes as the threaded format unless -mmt=off is passed,
        // so the .7z retry is the mirror image of the standalone one. Getting
        // these backwards would silently do nothing, hence the explicit check.
        for name in ["foreign.7z", "FOREIGN.7Z"] {
            assert_eq!(brotli_retry_flag(Path::new(name)), Some("-mmt=off"), "{name}");
        }
        assert_ne!(
            brotli_retry_flag(Path::new("a.7z")),
            brotli_retry_flag(Path::new("a.br")),
        );
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
        assert!(should_retry_brotli(Path::new("foreign.7z"), &fail));
        // Wrong format: a zstd failure is a real failure, not a mode mismatch.
        assert!(!should_retry_brotli(Path::new("out.tar.zst"), &fail));
        // Wrong error: never burn a second run on cancellation or a password.
        assert!(!should_retry_brotli(Path::new("out.br"), &EngineError::Cancelled));
        assert!(!should_retry_brotli(Path::new("out.br"), &EngineError::PasswordRequired));
    }
}
