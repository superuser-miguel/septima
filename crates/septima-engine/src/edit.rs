use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::EngineError;
use crate::extract::CancelToken;
use crate::progress::ExtractProgress;
use crate::supervise::supervise;

/// Verify an archive's integrity via `7zz t`. `Ok(())` means "Everything is Ok";
/// a corrupt archive or bad password surfaces as an `Err`. Streams the same
/// `-bsp1 -bb1` progress `7zz x` does, and honours `cancel` the same way.
pub fn run_test(
    sevenzip: &Path,
    archive: &Path,
    password: Option<&str>,
    cancel: &CancelToken,
    on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
    let child = test_command(sevenzip, archive, password)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(EngineError::Spawn)?;
    supervise(child, cancel, on_progress)
}

/// Delete `entries` (files or folders — a folder removes its contents too)
/// from `archive` via `7zz d`. A no-op if `entries` is empty.
pub fn run_delete(
    sevenzip: &Path,
    archive: &Path,
    entries: &[String],
    password: Option<&str>,
) -> Result<(), EngineError> {
    if entries.is_empty() {
        return Ok(());
    }
    run(delete_command(sevenzip, archive, entries, password))
}

/// Rename entries in `archive` via `7zz rn`, given `(old_path, new_path)`
/// pairs. A no-op if `renames` is empty.
pub fn run_rename(
    sevenzip: &Path,
    archive: &Path,
    renames: &[(String, String)],
    password: Option<&str>,
) -> Result<(), EngineError> {
    if renames.is_empty() {
        return Ok(());
    }
    run(rename_command(sevenzip, archive, renames, password))
}

/// Run `cmd` (stdin closed so a password prompt can't block) and map a
/// non-zero exit to the right `EngineError`.
fn run(mut cmd: Command) -> Result<(), EngineError> {
    let output = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(EngineError::Spawn)?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains("Enter password") || stdout.contains("Wrong password") || stderr.contains("Wrong password") {
        return Err(EngineError::PasswordRequired);
    }
    Err(EngineError::SevenZip {
        code: output.status.code(),
        stderr: stderr.into_owned(),
    })
}

/// `7zz t`, not yet run (see [`run_test`]).
pub(crate) fn test_command(sevenzip: &Path, archive: &Path, password: Option<&str>) -> Command {
    let mut cmd = Command::new(sevenzip);
    cmd.arg("t").arg("-bsp1").arg("-bb1");
    if let Some(password) = password {
        cmd.arg(format!("-p{password}"));
    }
    cmd.arg("--").arg(archive);
    cmd
}

/// `7zz d`, not yet run (see [`run_delete`]).
pub(crate) fn delete_command(sevenzip: &Path, archive: &Path, entries: &[String], password: Option<&str>) -> Command {
    let mut cmd = Command::new(sevenzip);
    cmd.arg("d");
    if let Some(password) = password {
        cmd.arg(format!("-p{password}"));
    }
    cmd.arg("--").arg(archive);
    for entry in entries {
        cmd.arg(entry);
    }
    cmd
}

/// `7zz rn`, not yet run (see [`run_rename`]).
pub(crate) fn rename_command(
    sevenzip: &Path,
    archive: &Path,
    renames: &[(String, String)],
    password: Option<&str>,
) -> Command {
    let mut cmd = Command::new(sevenzip);
    cmd.arg("rn");
    if let Some(password) = password {
        cmd.arg(format!("-p{password}"));
    }
    cmd.arg("--").arg(archive);
    for (old, new) in renames {
        cmd.arg(old).arg(new);
    }
    cmd
}
