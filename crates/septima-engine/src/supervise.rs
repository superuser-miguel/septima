use std::io::Read;
use std::process::Child;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crate::error::EngineError;
use crate::extract::CancelToken;
use crate::progress::{apply_fragment, ExtractProgress};

/// How often the supervisor wakes to check `cancel` while 7zz is silent.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// Drive a spawned `7zz` job (extract or add): stream its `-bsp1 -bb1` output to
/// `on_progress`, honour `cancel`, and turn the exit status into a result.
///
/// stdout is drained on a helper thread and handed over a channel so the
/// supervisor can poll `cancel` on a timer even when 7zz emits nothing for a
/// while — scanning a large tree, storing a big file, or walking portal document
/// paths (`/run/user/.../doc/...`). Reading stdout inline would block that whole
/// time and make the Cancel button do nothing until the next byte arrived.
///
/// `child` must have been spawned with piped stdout/stderr and null stdin.
pub(crate) fn supervise(
    mut child: Child,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&ExtractProgress),
) -> Result<(), EngineError> {
    let pid = child.id();
    if debug_enabled() {
        eprintln!("[septima] job: 7zz started (pid {pid})");
    }
    let mut stdout = child.stdout.take().expect("piped stdout");

    // Helper thread: blocking-read stdout, forward chunks. It ends on EOF — which
    // includes the pipe closing after we kill the child on cancel.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut fragment: Vec<u8> = Vec::new();
    let mut state = ExtractProgress::default();

    let cancelled = loop {
        if cancel.load(Ordering::Relaxed) {
            break true;
        }
        match rx.recv_timeout(CANCEL_POLL) {
            Ok(chunk) => {
                for &byte in &chunk {
                    if byte == b'\r' || byte == b'\n' {
                        flush(&mut fragment, &mut state, &mut on_progress);
                    } else {
                        fragment.push(byte);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {} // loop back and re-check cancel
            Err(RecvTimeoutError::Disconnected) => break false, // reader hit EOF
        }
    };

    if cancelled {
        if debug_enabled() {
            eprintln!("[septima] job: cancel flag set — killing pid {pid}");
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        if debug_enabled() {
            eprintln!("[septima] job: pid {pid} killed");
        }
        return Err(EngineError::Cancelled);
    }

    flush(&mut fragment, &mut state, &mut on_progress);
    let _ = reader.join();

    let status = child.wait().map_err(EngineError::Spawn)?;
    if debug_enabled() {
        eprintln!("[septima] job: pid {pid} exited (status {:?})", status.code());
    }
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

/// Whether `SEPTIMA_DEBUG` is set — gates the job-lifecycle trace consumed by
/// `build-aux/debug-run.sh`.
pub(crate) fn debug_enabled() -> bool {
    std::env::var_os("SEPTIMA_DEBUG").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_cancel_token;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    /// A silent, long-running child must still cancel promptly. Old code blocked
    /// on `stdout.read()` and only checked `cancel` between reads, so a process
    /// that emits nothing (like `sleep`, or 7zz scanning) could never be
    /// cancelled. Here the supervisor polls `cancel` on a timer instead.
    #[test]
    fn cancel_is_prompt_even_with_no_output() {
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sleep");

        let cancel = new_cancel_token();
        cancel.store(true, Ordering::Relaxed);

        let start = Instant::now();
        let result = supervise(child, &cancel, |_| {});

        assert!(matches!(result, Err(EngineError::Cancelled)));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "cancel took too long: {:?}",
            start.elapsed()
        );
    }
}
