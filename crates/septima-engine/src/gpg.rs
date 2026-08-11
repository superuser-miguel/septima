//! Symmetric GPG encryption for the passwords manifest (`gpg -c`).
//!
//! Uses the `gpg` on PATH — inside the Flatpak that's the GNOME runtime's
//! `/usr/bin/gpg`, so nothing is bundled and no sandbox hole is opened: no
//! keys, no keyring, no host-spawn. The output is a standard OpenPGP file any
//! `gpg` anywhere decrypts with the passphrase.
//!
//! Secret handling: the passphrase and the plaintext both travel over stdin
//! (`--passphrase-fd 0` reads the first line, the `-` input reads the rest),
//! so neither ever appears in argv, the environment, or a file. The plaintext
//! manifest in particular must never touch disk unencrypted on this path —
//! that residue is the reason the encrypt-or-not choice happens *before* the
//! batch runs.

use std::io::Write;
use std::process::{Command, Stdio};

/// Errors from driving `gpg`.
#[derive(Debug)]
pub enum GpgError {
    /// The `gpg` binary could not be spawned (missing, permissions).
    Spawn(std::io::Error),
    /// Decryption failed because the passphrase is wrong ("Bad session key").
    WrongPassphrase,
    /// The passphrase can't be transported over the first-line protocol.
    /// The UI should refuse these up front; this is the backstop.
    UnusablePassphrase,
    /// `gpg` exited non-zero for another reason.
    Gpg { code: Option<i32>, stderr: String },
    /// Plumbing I/O around the child process failed.
    Io(std::io::Error),
}

impl std::fmt::Display for GpgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpgError::Spawn(e) => write!(f, "failed to run gpg: {e}"),
            GpgError::WrongPassphrase => write!(f, "wrong passphrase for this file"),
            GpgError::UnusablePassphrase => {
                write!(f, "the passphrase must not be empty or contain line breaks")
            }
            GpgError::Gpg { code, stderr } => match code {
                Some(c) => write!(f, "gpg exited with code {c}: {}", stderr.trim()),
                None => write!(f, "gpg was terminated: {}", stderr.trim()),
            },
            GpgError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for GpgError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GpgError::Spawn(e) | GpgError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Whether a usable `gpg` is on PATH. Probed once (`gpg --version`), cached.
/// The "protect with a password" option should simply not appear without it.
pub fn gpg_available() -> bool {
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("gpg")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Whether `bytes` look like an OpenPGP file (binary packet or ASCII armor)
/// rather than a plain manifest. Reliable here because the alternatives are
/// JSON (`{`) and CSV (printable ASCII), while every binary OpenPGP packet
/// starts with the high bit set.
pub fn looks_gpg_encrypted(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|b| b & 0x80 != 0)
        || bytes.starts_with(b"-----BEGIN PGP MESSAGE-----")
}

/// Encrypt `plaintext` with `gpg --symmetric` (AES-256) under `passphrase`.
pub fn encrypt_symmetric(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>, GpgError> {
    run_gpg(
        &["--batch", "--yes", "--quiet", "--symmetric", "--cipher-algo", "AES256"],
        plaintext,
        passphrase,
    )
}

/// Decrypt a `gpg --symmetric` file. Wrong passphrase maps to
/// [`GpgError::WrongPassphrase`] so the UI can re-prompt instead of failing.
pub fn decrypt_symmetric(ciphertext: &[u8], passphrase: &str) -> Result<Vec<u8>, GpgError> {
    run_gpg(&["--batch", "--quiet", "--decrypt"], ciphertext, passphrase)
}

fn run_gpg(args: &[&str], data: &[u8], passphrase: &str) -> Result<Vec<u8>, GpgError> {
    if passphrase.is_empty() || passphrase.contains(['\n', '\r']) {
        return Err(GpgError::UnusablePassphrase);
    }
    let mut child = Command::new("gpg")
        .args(args)
        // Loopback keeps pinentry out of it; --no-symkey-cache keeps the
        // passphrase out of the agent's cache. Both streams ride stdin:
        // first line passphrase, remainder data.
        .args(["--pinentry-mode", "loopback", "--passphrase-fd", "0", "--no-symkey-cache"])
        .args(["--output", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(GpgError::Spawn)?;

    // Feed stdin from a thread so a large manifest can't deadlock against a
    // full stdout pipe.
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let mut feed = Vec::with_capacity(passphrase.len() + 1 + data.len());
    feed.extend_from_slice(passphrase.as_bytes());
    feed.push(b'\n');
    feed.extend_from_slice(data);
    let writer = std::thread::spawn(move || {
        // A write error here surfaces as gpg exiting non-zero below.
        let _ = stdin.write_all(&feed);
    });

    let out = child.wait_with_output().map_err(GpgError::Io)?;
    let _ = writer.join();

    if out.status.success() {
        return Ok(out.stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if stderr.contains("Bad session key") || stderr.contains("decryption failed") {
        return Err(GpgError::WrongPassphrase);
    }
    Err(GpgError::Gpg { code: out.status.code(), stderr })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_unusable_passphrases() {
        assert!(matches!(
            encrypt_symmetric(b"data", ""),
            Err(GpgError::UnusablePassphrase)
        ));
        assert!(matches!(
            encrypt_symmetric(b"data", "two\nlines"),
            Err(GpgError::UnusablePassphrase)
        ));
    }

    #[test]
    fn detects_openpgp_bytes() {
        assert!(looks_gpg_encrypted(&[0x8c, 0x0d, 0x04])); // symmetric session packet
        assert!(looks_gpg_encrypted(b"-----BEGIN PGP MESSAGE-----\n"));
        assert!(!looks_gpg_encrypted(b"{\n  \"septima_manifest\": 1"));
        assert!(!looks_gpg_encrypted(b"archive,source,password"));
        assert!(!looks_gpg_encrypted(b""));
    }
}
