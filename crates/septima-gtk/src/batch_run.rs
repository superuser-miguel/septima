//! One batch run (≥ 2 archives created or extracted at once), tallied as a
//! whole. Without it every archive raised its own toast — and its own error
//! dialog or password prompt — so a 50-item batch queued ~50–100 toasts that
//! kept marching past long after the work was done. Jobs report here instead,
//! and the window speaks once, when the last one lands.
//!
//! Pure bookkeeping: no GTK, so it is unit-tested directly. The window holds
//! it as `Rc<RefCell<BatchRun>>`, shared by the run's jobs.

use std::path::{Path, PathBuf};

/// What the batch is doing — picks the summary wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchKind {
    Compress,
    Extract,
}

/// How one archive's job ended, as far as the batch is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Finished; `output` is the archive created or the folder extracted into.
    Done { output: PathBuf },
    /// Failed with a message worth showing the user.
    Failed { message: String },
    /// Cancelled from its progress row — counted, never reported as a failure.
    Cancelled,
    /// Extract only: the archive is encrypted and the password given (if any)
    /// didn't open it. Held for one password prompt at the end of the round.
    NeedsPassword { dest: PathBuf },
    /// The user declined to give a password for it at the end-of-round prompt.
    Skipped,
}

/// A job that needs a password, with where it should extract to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPassword {
    pub archive: PathBuf,
    pub dest: PathBuf,
}

#[derive(Debug)]
pub struct BatchRun {
    kind: BatchKind,
    /// Jobs of the current round not yet recorded. Saturates at 0 so a stray
    /// extra `record` can't underflow or re-announce the end of the round.
    outstanding: usize,
    /// Each job's latest outcome, in the order recorded. A retried job's
    /// `NeedsPassword` entry is removed when its retry round starts, so every
    /// job appears here once and the final tally stays coherent. Kept as a
    /// list rather than a map keyed by path so two jobs that happen to share
    /// a path are still counted as two.
    records: Vec<(PathBuf, Outcome)>,
    /// Non-fatal problems, in the order reported.
    warnings: Vec<(PathBuf, String)>,
    /// Extract only: delete each archive once it's extracted. Held here, not
    /// passed along, so a password retry round honours the dialog's choice.
    delete_after: bool,
}

impl BatchRun {
    /// A run of `total` jobs, none finished yet.
    pub fn new(kind: BatchKind, total: usize) -> Self {
        Self { kind, outstanding: total, records: Vec::new(), warnings: Vec::new(), delete_after: false }
    }

    /// Set the run's delete-after choice (Extract only).
    pub fn with_delete_after(mut self, delete_after: bool) -> Self {
        self.delete_after = delete_after;
        self
    }

    pub fn delete_after(&self) -> bool {
        self.delete_after
    }

    /// True when at least one job failed outright — as opposed to a run whose
    /// report holds only warnings, which shouldn't be headed "failed".
    pub fn has_failures(&self) -> bool {
        self.records.iter().any(|(_, outcome)| matches!(outcome, Outcome::Failed { .. }))
    }

    /// Record the final outcome of the job for `archive` (the input archive
    /// for Extract, the created archive for Compress). Call it once per job,
    /// **after** its follow-up work (checksum file, delete-after) has settled.
    /// Returns true when this was the last outstanding job of the round.
    pub fn record(&mut self, archive: &Path, outcome: Outcome) -> bool {
        self.records.push((archive.to_path_buf(), outcome));
        // Only the 1 → 0 transition ends the round; over-recording past it
        // (a caller bug) is tallied but never announces a second end.
        if self.outstanding == 0 {
            return false;
        }
        self.outstanding -= 1;
        self.outstanding == 0
    }

    /// A non-fatal problem with a job that otherwise succeeded ("checksum
    /// file couldn't be written", "archive couldn't be deleted"). Listed in
    /// the report, doesn't turn the job into a failure. Call before `record`.
    pub fn warn(&mut self, archive: &Path, message: &str) {
        self.warnings.push((archive.to_path_buf(), message.to_owned()));
    }

    /// True once every job of the current round has been recorded.
    pub fn is_finished(&self) -> bool {
        self.outstanding == 0
    }

    /// Jobs recorded as `NeedsPassword` and not yet retried or skipped.
    pub fn pending_passwords(&self) -> Vec<PendingPassword> {
        self.records
            .iter()
            .filter_map(|(archive, outcome)| match outcome {
                Outcome::NeedsPassword { dest } => {
                    Some(PendingPassword { archive: archive.clone(), dest: dest.clone() })
                }
                _ => None,
            })
            .collect()
    }

    /// Start a retry round for every pending-password job: they leave the
    /// pending list and count as outstanding again (the caller then starts
    /// one job for each, and each records normally). Returns them.
    pub fn begin_password_retry(&mut self) -> Vec<PendingPassword> {
        let pending = self.pending_passwords();
        // Drop the old entries outright: the retry's own `record` is the
        // archive's outcome from here on, and must not count twice.
        self.records.retain(|(_, outcome)| !matches!(outcome, Outcome::NeedsPassword { .. }));
        self.outstanding = self.outstanding.saturating_add(pending.len());
        pending
    }

    /// The user declined the end-of-round password prompt: every pending job
    /// becomes `Skipped`.
    pub fn skip_pending(&mut self) {
        for (_, outcome) in &mut self.records {
            if matches!(outcome, Outcome::NeedsPassword { .. }) {
                *outcome = Outcome::Skipped;
            }
        }
    }

    /// The single summary toast, e.g. "Created 50 archives",
    /// "Extracted 48 archives · 2 failed", "Extracted 3 archives · 1 skipped".
    /// Wording by kind; plural-correct via `gettextrs::ngettext`. When nothing
    /// succeeded and nothing failed (all cancelled), `None` — say nothing.
    ///
    /// A job still waiting on a password when this is asked for wasn't opened,
    /// so it reads as skipped rather than vanishing from the count.
    pub fn summary(&self) -> Option<String> {
        let (mut done, mut failed, mut skipped) = (0usize, 0usize, 0usize);
        for (_, outcome) in &self.records {
            match outcome {
                Outcome::Done { .. } => done += 1,
                Outcome::Failed { .. } => failed += 1,
                Outcome::Skipped | Outcome::NeedsPassword { .. } => skipped += 1,
                Outcome::Cancelled => {}
            }
        }

        // Lead with the biggest thing that happened: what was made, else what
        // failed, else what was skipped. The rest trail as " · n failed" etc.
        let mut text = if done > 0 {
            counted(done, match self.kind {
                BatchKind::Compress => gettextrs::ngettext("Created {} archive", "Created {} archives", done as u32),
                BatchKind::Extract => {
                    gettextrs::ngettext("Extracted {} archive", "Extracted {} archives", done as u32)
                }
            })
        } else if failed > 0 {
            let n = std::mem::take(&mut failed);
            counted(n, match self.kind {
                BatchKind::Compress => {
                    gettextrs::ngettext("Couldn't create {} archive", "Couldn't create {} archives", n as u32)
                }
                BatchKind::Extract => {
                    gettextrs::ngettext("Couldn't extract {} archive", "Couldn't extract {} archives", n as u32)
                }
            })
        } else if skipped > 0 {
            let n = std::mem::take(&mut skipped);
            counted(n, gettextrs::ngettext("Skipped {} archive", "Skipped {} archives", n as u32))
        } else {
            return None;
        };

        if failed > 0 {
            text.push_str(&counted(failed, gettextrs::ngettext(" · {} failed", " · {} failed", failed as u32)));
        }
        if skipped > 0 {
            text.push_str(&counted(skipped, gettextrs::ngettext(" · {} skipped", " · {} skipped", skipped as u32)));
        }
        Some(text)
    }

    /// Body for one "Some archives failed" dialog listing each failure and
    /// warning as "<file name>: <message>", or `None` when there are none.
    pub fn report(&self) -> Option<String> {
        // File names only: the batch's archives usually share a folder, and
        // full paths would bury the part that differs.
        let failures = self.records.iter().filter_map(|(archive, outcome)| match outcome {
            Outcome::Failed { message } => Some((archive, message)),
            _ => None,
        });
        let warnings = self.warnings.iter().map(|(archive, message)| (archive, message));
        let lines: Vec<String> = failures
            .chain(warnings)
            .map(|(archive, message)| format!("{}: {message}", display_name(archive)))
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The folder for the summary toast's "Show in Files": the parent shared
    /// by every successful output, or `None` if there are none or they
    /// don't share one. (Extract outputs are sibling folders of each archive;
    /// Compress outputs are archives, so their parent is the folder.)
    pub fn common_folder(&self) -> Option<PathBuf> {
        let mut parents = self.records.iter().filter_map(|(_, outcome)| match outcome {
            Outcome::Done { output } => Some(output.parent()),
            _ => None,
        });
        let first = parents.next()??;
        // An output with no parent (`/`) can't share one, hence the `Some` compare.
        parents.all(|p| p == Some(first)).then(|| first.to_path_buf())
    }
}

/// Fill the count into an `ngettext` result, as the window's `n_*` helpers do.
fn counted(n: usize, template: String) -> String {
    template.replacen("{}", &n.to_string(), 1)
}

/// The last path component for the report, falling back to the whole path
/// for the odd one without a file name (`/`, `..`).
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn done(out: &str) -> Outcome {
        Outcome::Done { output: p(out) }
    }

    fn failed(msg: &str) -> Outcome {
        Outcome::Failed { message: msg.to_owned() }
    }

    fn needs_pw(dest: &str) -> Outcome {
        Outcome::NeedsPassword { dest: p(dest) }
    }

    #[test]
    fn clean_compress_run() {
        let mut run = BatchRun::new(BatchKind::Compress, 3);
        assert!(!run.record(&p("/a/x.7z"), done("/a/x.7z")));
        assert!(!run.record(&p("/a/y.7z"), done("/a/y.7z")));
        assert!(run.record(&p("/a/z.7z"), done("/a/z.7z")));
        assert_eq!(run.summary().as_deref(), Some("Created 3 archives"));
        assert_eq!(run.report(), None);
        assert_eq!(run.common_folder(), Some(p("/a")));
    }

    #[test]
    fn mixed_done_failed_cancelled() {
        let mut run = BatchRun::new(BatchKind::Extract, 5);
        run.record(&p("/a/1.7z"), done("/a/1"));
        run.record(&p("/a/2.7z"), failed("Data error"));
        run.record(&p("/a/3.7z"), Outcome::Cancelled);
        run.record(&p("/a/4.7z"), done("/a/4"));
        assert!(run.record(&p("/a/5.7z"), failed("CRC failed")));
        assert_eq!(run.summary().as_deref(), Some("Extracted 2 archives · 2 failed"));
        assert_eq!(run.report().as_deref(), Some("2.7z: Data error\n5.7z: CRC failed"));
    }

    #[test]
    fn is_finished_tracks_record() {
        let mut run = BatchRun::new(BatchKind::Compress, 2);
        assert!(!run.is_finished());
        assert!(!run.record(&p("/a/x.7z"), done("/a/x.7z")));
        assert!(!run.is_finished());
        assert!(run.record(&p("/a/y.7z"), done("/a/y.7z")));
        assert!(run.is_finished());
    }

    #[test]
    fn over_recording_saturates() {
        let mut run = BatchRun::new(BatchKind::Compress, 1);
        assert!(run.record(&p("/a/x.7z"), done("/a/x.7z")));
        assert!(!run.record(&p("/a/y.7z"), done("/a/y.7z")));
        assert!(!run.record(&p("/a/z.7z"), failed("boom")));
        assert!(run.is_finished());

        let mut empty = BatchRun::new(BatchKind::Extract, 0);
        assert!(empty.is_finished());
        assert!(!empty.record(&p("/a/x.7z"), Outcome::Cancelled));
    }

    #[test]
    fn password_round_counts_each_archive_once() {
        let mut run = BatchRun::new(BatchKind::Extract, 4);
        run.record(&p("/a/plain.7z"), done("/a/plain"));
        run.record(&p("/a/lock1.7z"), needs_pw("/a/lock1"));
        run.record(&p("/a/lock2.7z"), needs_pw("/a/lock2"));
        assert!(run.record(&p("/a/bad.7z"), failed("Unsupported method")));
        assert_eq!(
            run.pending_passwords(),
            vec![
                PendingPassword { archive: p("/a/lock1.7z"), dest: p("/a/lock1") },
                PendingPassword { archive: p("/a/lock2.7z"), dest: p("/a/lock2") },
            ]
        );

        let retry = run.begin_password_retry();
        assert_eq!(retry.len(), 2);
        assert!(run.pending_passwords().is_empty());
        assert!(!run.is_finished());

        assert!(!run.record(&p("/a/lock1.7z"), done("/a/lock1")));
        assert!(run.record(&p("/a/lock2.7z"), needs_pw("/a/lock2")));
        assert_eq!(run.pending_passwords().len(), 1);

        run.skip_pending();
        assert!(run.pending_passwords().is_empty());
        assert!(run.is_finished());
        // 4 archives: 2 extracted, 1 failed, 1 skipped — none counted twice.
        assert_eq!(run.summary().as_deref(), Some("Extracted 2 archives · 1 failed · 1 skipped"));
        assert_eq!(run.report().as_deref(), Some("bad.7z: Unsupported method"));
    }

    #[test]
    fn retry_with_nothing_pending_is_a_no_op() {
        let mut run = BatchRun::new(BatchKind::Extract, 1);
        assert!(run.record(&p("/a/x.7z"), done("/a/x")));
        assert!(run.begin_password_retry().is_empty());
        assert!(run.is_finished());
    }

    #[test]
    fn warnings_are_reported_not_failed() {
        let mut run = BatchRun::new(BatchKind::Compress, 2);
        run.warn(&p("/a/x.7z"), "Checksum file couldn't be written");
        run.record(&p("/a/x.7z"), done("/a/x.7z"));
        run.record(&p("/a/y.7z"), failed("Disk full"));
        assert_eq!(run.summary().as_deref(), Some("Created 1 archive · 1 failed"));
        assert_eq!(
            run.report().as_deref(),
            Some("y.7z: Disk full\nx.7z: Checksum file couldn't be written")
        );
    }

    #[test]
    fn all_cancelled_says_nothing() {
        let mut run = BatchRun::new(BatchKind::Extract, 2);
        run.record(&p("/a/x.7z"), Outcome::Cancelled);
        assert!(run.record(&p("/a/y.7z"), Outcome::Cancelled));
        assert_eq!(run.summary(), None);
        assert_eq!(run.report(), None);
        assert_eq!(run.common_folder(), None);
    }

    #[test]
    fn wording_singular_plural_and_no_successes() {
        let mut one = BatchRun::new(BatchKind::Extract, 1);
        one.record(&p("/a/x.7z"), done("/a/x"));
        assert_eq!(one.summary().as_deref(), Some("Extracted 1 archive"));

        let mut failures = BatchRun::new(BatchKind::Compress, 2);
        failures.record(&p("/a/x.7z"), failed("a"));
        failures.record(&p("/a/y.7z"), failed("b"));
        assert_eq!(failures.summary().as_deref(), Some("Couldn't create 2 archives"));

        let mut ext = BatchRun::new(BatchKind::Extract, 3);
        ext.record(&p("/a/x.7z"), failed("a"));
        ext.record(&p("/a/y.7z"), needs_pw("/a/y"));
        ext.record(&p("/a/z.7z"), Outcome::Cancelled);
        ext.skip_pending();
        assert_eq!(ext.summary().as_deref(), Some("Couldn't extract 1 archive · 1 skipped"));

        let mut skipped = BatchRun::new(BatchKind::Extract, 2);
        skipped.record(&p("/a/x.7z"), needs_pw("/a/x"));
        skipped.record(&p("/a/y.7z"), needs_pw("/a/y"));
        skipped.skip_pending();
        assert_eq!(skipped.summary().as_deref(), Some("Skipped 2 archives"));
    }

    #[test]
    fn common_folder_needs_a_shared_parent() {
        let mut same = BatchRun::new(BatchKind::Extract, 3);
        same.record(&p("/a/x.7z"), done("/a/x"));
        same.record(&p("/a/y.7z"), failed("nope"));
        same.record(&p("/a/z.7z"), done("/a/z"));
        assert_eq!(same.common_folder(), Some(p("/a")));

        let mut diff = BatchRun::new(BatchKind::Compress, 2);
        diff.record(&p("/a/x.7z"), done("/a/x.7z"));
        diff.record(&p("/b/y.7z"), done("/b/y.7z"));
        assert_eq!(diff.common_folder(), None);

        let mut none = BatchRun::new(BatchKind::Compress, 1);
        none.record(&p("/a/x.7z"), failed("nope"));
        assert_eq!(none.common_folder(), None);
    }

    #[test]
    fn report_uses_file_names_only() {
        let mut run = BatchRun::new(BatchKind::Extract, 2);
        run.record(&p("/home/me/deep/dir/one.7z"), failed("Wrong password"));
        run.warn(&p("/home/me/other/two.zip"), "Archive couldn't be deleted");
        run.record(&p("/home/me/other/two.zip"), done("/home/me/other/two"));
        let report = run.report().unwrap();
        assert_eq!(report, "one.7z: Wrong password\ntwo.zip: Archive couldn't be deleted");
        assert!(!report.contains("/home"));
    }

    #[test]
    fn warnings_alone_are_not_failures() {
        let mut run = BatchRun::new(BatchKind::Compress, 2);
        run.warn(Path::new("/a/x.7z"), "created, but the checksum file couldn't be written");
        run.record(Path::new("/a/x.7z"), Outcome::Done { output: PathBuf::from("/a/x.7z") });
        run.record(Path::new("/a/y.7z"), Outcome::Done { output: PathBuf::from("/a/y.7z") });
        assert!(!run.has_failures());
        assert!(run.report().is_some());

        let mut run = BatchRun::new(BatchKind::Compress, 1);
        run.record(Path::new("/a/x.7z"), Outcome::Failed { message: "disk full".into() });
        assert!(run.has_failures());
    }

    #[test]
    fn delete_after_is_held_by_the_run() {
        assert!(!BatchRun::new(BatchKind::Extract, 2).delete_after());
        assert!(BatchRun::new(BatchKind::Extract, 2).with_delete_after(true).delete_after());
    }
}
