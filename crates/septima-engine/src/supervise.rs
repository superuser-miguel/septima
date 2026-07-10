use std::io::Read;
use std::process::Child;
use std::sync::atomic::Ordering;

use crate::error::EngineError;
use crate::extract::CancelToken;
use crate::progress::{apply_fragment, ExtractProgress};

/// Drive a spawned `7zz` job (extract or add): stream its `-bsp1 -bb1` output to
/// `on_progress`, honour `cancel`, and turn the exit status into a result.
///
/// `child` must have been spawned with piped stdout/stderr and null stdin.
pub(crate) fn supervise(
    mut child: Child,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
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
