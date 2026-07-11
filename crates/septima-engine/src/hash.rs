use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;

use crate::error::EngineError;
use crate::extract::CancelToken;
use crate::progress::{apply_fragment, ExtractProgress};

/// A hash algorithm: its `7zz -scrc` switch name and a display label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashAlgo {
    pub switch: &'static str,
    pub label: &'static str,
}

/// The algorithms the calculator offers (a modern, useful spread).
pub fn hash_algorithms() -> &'static [HashAlgo] {
    &[
        HashAlgo { switch: "CRC32", label: "CRC-32" },
        HashAlgo { switch: "SHA256", label: "SHA-256" },
        HashAlgo { switch: "SHA512", label: "SHA-512" },
        HashAlgo { switch: "SHA3-256", label: "SHA3-256" },
        HashAlgo { switch: "BLAKE3", label: "BLAKE3" },
        HashAlgo { switch: "XXH64", label: "xxHash-64" },
    ]
}

/// One computed digest: the algorithm switch name and a lowercase hex string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    pub algo: String,
    pub hex: String,
}

/// Compute `algos` (switch names) for a single file via `7zz h`.
pub fn hash_file(sevenzip: &Path, file: &Path, algos: &[&str]) -> Result<Vec<Digest>, EngineError> {
    let mut cmd = Command::new(sevenzip);
    cmd.arg("h");
    for algo in algos {
        cmd.arg(format!("-scrc{algo}"));
    }
    let output = cmd
        .arg("--")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(EngineError::Spawn)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EngineError::SevenZip {
            code: output.status.code(),
            stderr: stderr.into_owned(),
        });
    }

    Ok(parse_digests(&String::from_utf8_lossy(&output.stdout)))
}

/// Like [`hash_file`], but streams progress (`0..=100`) as `7zz h` reads the
/// file — important for large inputs. Blocking; run on a worker thread.
pub fn hash_file_progress(
    sevenzip: &Path,
    file: &Path,
    algos: &[&str],
    cancel: &CancelToken,
    mut on_progress: impl FnMut(u8),
) -> Result<Vec<Digest>, EngineError> {
    let mut cmd = Command::new(sevenzip);
    cmd.arg("h").arg("-bsp1").arg("-bb1");
    for algo in algos {
        cmd.arg(format!("-scrc{algo}"));
    }
    let mut child = cmd
        .arg("--")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(EngineError::Spawn)?;

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut buf = [0u8; 8192];
    let mut full: Vec<u8> = Vec::new(); // accumulate for the final digest parse
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
        full.extend_from_slice(&buf[..n]);
        for &byte in &buf[..n] {
            if byte == b'\r' || byte == b'\n' {
                if !fragment.is_empty() {
                    let text = String::from_utf8_lossy(&fragment);
                    if apply_fragment(&mut state, &text) {
                        if let Some(percent) = state.percent {
                            on_progress(percent);
                        }
                    }
                    fragment.clear();
                }
            } else {
                fragment.push(byte);
            }
        }
    }

    let status = child.wait().map_err(EngineError::Spawn)?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        return Err(EngineError::SevenZip {
            code: status.code(),
            stderr,
        });
    }

    Ok(parse_digests(&String::from_utf8_lossy(&full)))
}

/// Parse `7zz h`'s `<ALGO> for data:  <hex>` summary lines (one input file, so
/// "data" is that file's content).
fn parse_digests(output: &str) -> Vec<Digest> {
    let mut digests = Vec::new();
    for line in output.lines() {
        let Some((name, rest)) = line.split_once(" for data:") else {
            continue;
        };
        let algo = name.trim();
        let hex = rest.trim();
        if !algo.is_empty() && !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            digests.push(Digest {
                algo: algo.to_string(),
                hex: hex.to_ascii_lowercase(),
            });
        }
    }
    digests
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Size: 14
CRC32  for data:              CE5B9440
SHA256 for data:              49f5819f475bf2c8e2ed80998789dba47a4a25ed19f97b6c8c6a4902eea0c1a1
XXH64  for data:              6DD738ACAB109C85
";

    #[test]
    fn parses_for_data_lines() {
        let d = parse_digests(SAMPLE);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0], Digest { algo: "CRC32".into(), hex: "ce5b9440".into() });
        assert_eq!(d[1].algo, "SHA256");
        assert_eq!(d[1].hex.len(), 64);
        // uppercase xxHash is normalised to lowercase
        assert_eq!(d[2], Digest { algo: "XXH64".into(), hex: "6dd738acab109c85".into() });
    }

    #[test]
    fn ignores_non_hash_lines() {
        assert!(parse_digests("Scanning...\n1 file, 14 bytes\nEverything is Ok\n").is_empty());
    }
}
