//! Measure a staged selection (files + folders) before compressing it.
//!
//! Blocking and potentially slow — a deep tree over portal FUSE mounts costs a
//! `stat` per entry — so callers run this off the UI thread and pass a cancel
//! token so a superseded measurement stops early.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use crate::extract::CancelToken;

/// Stop after this many entries. A selection this large is already far past the
/// point where an exact byte count changes anyone's mind, and we would rather
/// answer "at least this much" quickly than walk a runaway tree.
const ENTRY_LIMIT: u64 = 200_000;

/// What a staged selection adds up to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    /// Regular files found (directories are not counted as files).
    pub files: u64,
    /// Total apparent size of those files, in bytes.
    pub bytes: u64,
    /// The walk hit [`ENTRY_LIMIT`] or was cancelled: the real totals are
    /// larger, so present these as a floor ("more than …"), not a total.
    pub truncated: bool,
}

/// Walk `paths` and total up the regular files beneath them.
///
/// Symlinks are counted but never followed — 7-Zip stores the link itself by
/// default, and following them risks cycles and double-counting.
pub fn measure_selection(paths: &[PathBuf], cancel: &CancelToken) -> Selection {
    let mut total = Selection::default();
    let mut seen: u64 = 0;
    let mut stack: Vec<PathBuf> = paths.to_vec();

    while let Some(path) = stack.pop() {
        if cancel.load(Ordering::Relaxed) || seen >= ENTRY_LIMIT {
            total.truncated = true;
            break;
        }
        seen += 1;

        // symlink_metadata: describe the link, don't follow it.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue; // vanished or unreadable — skip, don't fail the whole walk
        };
        if meta.is_dir() {
            push_children(&path, &mut stack);
        } else {
            total.files += 1;
            total.bytes += meta.len();
        }
    }
    total
}

fn push_children(dir: &Path, stack: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        stack.push(entry.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::new_cancel_token;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("septima-measure-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn totals_files_recursively() {
        let dir = scratch("recursive");
        std::fs::write(dir.join("a.txt"), b"12345").unwrap();
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("b.txt"), b"1234567890").unwrap();
        std::fs::create_dir(sub.join("empty")).unwrap();

        let got = measure_selection(&[dir.clone()], &new_cancel_token());
        assert_eq!(got.files, 2, "directories must not count as files");
        assert_eq!(got.bytes, 15);
        assert!(!got.truncated);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_single_file_selection_works() {
        let dir = scratch("single");
        let file = dir.join("a.bin");
        std::fs::write(&file, vec![0u8; 1024]).unwrap();

        let got = measure_selection(&[file], &new_cancel_token());
        assert_eq!(got, Selection { files: 1, bytes: 1024, truncated: false });

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreadable_paths_are_skipped_not_fatal() {
        let dir = scratch("missing");
        std::fs::write(dir.join("real.txt"), b"abc").unwrap();

        let got = measure_selection(
            &[dir.join("real.txt"), dir.join("does-not-exist")],
            &new_cancel_token(),
        );
        assert_eq!(got.files, 1);
        assert_eq!(got.bytes, 3);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_stops_the_walk_and_marks_it_truncated() {
        let dir = scratch("cancel");
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();

        let cancel = new_cancel_token();
        cancel.store(true, Ordering::Relaxed);
        let got = measure_selection(&[dir.clone()], &cancel);
        assert!(got.truncated, "a cancelled walk must not look complete");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
