//! Parser for the progress stream `7zz` writes under `-bsp1 -bb1`.
//!
//! 7zz redraws a single status line using carriage returns and backspaces, e.g.
//! (with `^H` = backspace): `` 37% 1^H^H^H^H^H^H      ^H^H^H^H^H^H- file_3.txt``.
//! We split the stream on `\r`/`\n`; within a fragment the percentage sits right
//! before the first `%`, and the current file name follows the *last* backspace.

/// A point-in-time view of an extraction's progress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractProgress {
    pub percent: Option<u8>,
    pub current_file: Option<String>,
}

/// Fold one `\r`/`\n`-delimited fragment into `state`; returns whether anything
/// user-visible changed (so callers can throttle UI updates).
pub fn apply_fragment(state: &mut ExtractProgress, fragment: &str) -> bool {
    let mut changed = false;

    if let Some(percent) = find_percent(fragment) {
        if state.percent != Some(percent) {
            state.percent = Some(percent);
            changed = true;
        }
    }

    if let Some(file) = current_file(fragment) {
        if state.current_file.as_deref() != Some(file.as_str()) {
            state.current_file = Some(file);
            changed = true;
        }
    }

    changed
}

/// The integer percentage immediately preceding the first `%`, if any.
fn find_percent(s: &str) -> Option<u8> {
    let bytes = s.as_bytes();
    let percent_pos = s.find('%')?;
    let mut start = percent_pos;
    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == percent_pos {
        return None;
    }
    s[start..percent_pos].parse().ok()
}

/// The current file name, taken from the `- <name>` marker after the final
/// backspace (so a `- ` inside the name itself can't confuse it).
fn current_file(fragment: &str) -> Option<String> {
    let tail = fragment.rsplit('\u{8}').next().unwrap_or(fragment);
    let name = tail.trim_start().strip_prefix("- ")?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_percent_and_file() {
        let mut s = ExtractProgress::default();
        assert!(apply_fragment(&mut s, "  0%\u{8}\u{8}\u{8}\u{8}    \u{8}\u{8}\u{8}\u{8}- file_1.txt"));
        assert_eq!(s.percent, Some(0));
        assert_eq!(s.current_file.as_deref(), Some("file_1.txt"));
    }

    #[test]
    fn parses_stray_digits_between_percent_and_name() {
        // ` 37% 1^H^H^H^H^H^H      ^H^H^H^H^H^H- file_3.txt`
        let mut s = ExtractProgress::default();
        apply_fragment(&mut s, " 37% 1\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}      \u{8}\u{8}\u{8}\u{8}\u{8}\u{8}- file_3.txt");
        assert_eq!(s.percent, Some(37));
        assert_eq!(s.current_file.as_deref(), Some("file_3.txt"));
    }

    #[test]
    fn name_with_dash_space_is_kept_whole() {
        let mut s = ExtractProgress::default();
        apply_fragment(&mut s, " 50%\u{8}\u{8}- my - file.txt");
        assert_eq!(s.current_file.as_deref(), Some("my - file.txt"));
    }

    #[test]
    fn ignores_non_progress_lines() {
        let mut s = ExtractProgress::default();
        assert!(!apply_fragment(&mut s, "  0M Scan"));
        assert!(!apply_fragment(&mut s, "Everything is Ok"));
        assert_eq!(s, ExtractProgress::default());
    }

    #[test]
    fn only_reports_change_on_new_values() {
        let mut s = ExtractProgress::default();
        assert!(apply_fragment(&mut s, " 10%\u{8}- a.txt"));
        assert!(!apply_fragment(&mut s, " 10%\u{8}- a.txt"));
        assert!(apply_fragment(&mut s, " 20%\u{8}- a.txt"));
    }
}
