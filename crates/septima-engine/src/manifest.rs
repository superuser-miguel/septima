//! Batch password manifest — the engine-side foundation of the batch-encrypt
//! feature: cryptographically-random password generation, the [`Manifest`] data
//! type, and RFC-4180 CSV read/write.
//!
//! Dependency-free by design (see the crate's A1 boundary): the CSPRNG is the
//! kernel's `/dev/urandom`, and the CSV is hand-rolled. The manifest is a
//! portable file — Septima writes it and reads it back to drive batch decrypt;
//! *where* it's stored (a vault, a `.gpg` file, plain disk) is the user's call,
//! so nothing here talks to a password manager.

use std::io::Read;

/// Character sets for generated passwords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    /// `A-Z a-z 0-9` (62 chars) — shell- and CSV-safe; the sensible default.
    Alphanumeric,
    /// Alphanumeric plus punctuation. Passed to `7zz` as `-p<pw>` (a direct
    /// argument, never a shell word), so symbols are safe there.
    AlphanumericSymbols,
}

impl Charset {
    fn bytes(self) -> &'static [u8] {
        const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        const SYM: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#%^&*()-_=+";
        match self {
            Charset::Alphanumeric => ALNUM,
            Charset::AlphanumericSymbols => SYM,
        }
    }
}

/// Generate a `len`-character password drawn uniformly from `charset`, using the
/// kernel CSPRNG (`/dev/urandom`) with rejection sampling so there is no modulo
/// bias toward the earlier characters of the set.
pub fn generate_password(len: usize, charset: Charset) -> std::io::Result<String> {
    let set = charset.bytes();
    let n = set.len(); // <= 75
    // Accept only bytes below the largest multiple of `n` that fits in a byte;
    // the rest would over-represent set[0..(256 % n)].
    let limit = (256 / n) * n;
    let mut urandom = std::fs::File::open("/dev/urandom")?;
    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 64];
    while out.len() < len {
        urandom.read_exact(&mut buf)?;
        for &b in &buf {
            if (b as usize) < limit {
                out.push(set[b as usize % n] as char);
                if out.len() == len {
                    break;
                }
            }
        }
    }
    Ok(out)
}

/// One archive's row in the manifest: the archive, what it was made from, its
/// password, and (optional) integrity + timestamp columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestEntry {
    pub archive: String,
    pub source: String,
    pub password: String,
    pub sha256: String,
    pub created: String,
}

/// A batch's archive→password mapping. Serialises to a CSV that doubles as a
/// password-manager import and an integrity record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

const HEADER: &[&str] = &["archive", "source", "password", "sha256", "created"];

impl Manifest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: ManifestEntry) {
        self.entries.push(entry);
    }

    /// Serialise to RFC-4180 CSV (CRLF line endings, header row first).
    pub fn to_csv(&self) -> String {
        let mut s = String::new();
        s.push_str(&csv_row(HEADER));
        for e in &self.entries {
            s.push_str(&csv_row(&[&e.archive, &e.source, &e.password, &e.sha256, &e.created]));
        }
        s
    }

    /// Parse a manifest CSV. A leading `archive,…` header row is skipped; rows
    /// with fewer than five fields are padded with empty strings, extras dropped.
    pub fn from_csv(text: &str) -> Manifest {
        // A spreadsheet (Excel/LibreOffice) may save the file with a UTF-8 BOM;
        // strip it so the first cell is "archive", not "\u{feff}archive".
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut rows = parse_csv(text).into_iter();
        // Skip a header row if present.
        let mut first = rows.next();
        if first.as_ref().and_then(|r| r.first()).map(|c| c.as_str()) == Some("archive") {
            first = rows.next();
        }
        let mut entries = Vec::new();
        for row in first.into_iter().chain(rows) {
            if row.iter().all(|f| f.is_empty()) {
                continue; // skip blank lines
            }
            let get = |i: usize| row.get(i).cloned().unwrap_or_default();
            entries.push(ManifestEntry {
                archive: get(0),
                source: get(1),
                password: get(2),
                sha256: get(3),
                created: get(4),
            });
        }
        Manifest { entries }
    }
}

// --- minimal RFC-4180 CSV ---------------------------------------------------

fn csv_field(f: &str) -> String {
    if f.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", f.replace('"', "\"\""))
    } else {
        f.to_string()
    }
}

fn csv_row(fields: &[&str]) -> String {
    let joined: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
    format!("{}\r\n", joined.join(","))
}

/// Split CSV `text` into rows of fields, honouring quoted fields that may
/// contain commas, doubled quotes, and embedded newlines.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => row.push(std::mem::take(&mut field)),
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                _ => field.push(c),
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_length_and_charset() {
        let pw = generate_password(64, Charset::Alphanumeric).unwrap();
        assert_eq!(pw.chars().count(), 64);
        assert!(pw.bytes().all(|b| b.is_ascii_alphanumeric()));
    }

    #[test]
    fn passwords_are_not_repeated() {
        let a = generate_password(32, Charset::Alphanumeric).unwrap();
        let b = generate_password(32, Charset::Alphanumeric).unwrap();
        assert_ne!(a, b, "two generated passwords should differ");
    }

    #[test]
    fn symbols_charset_stays_arg_safe() {
        let pw = generate_password(200, Charset::AlphanumericSymbols).unwrap();
        // No shell/CSV wrecking characters that we deliberately excluded.
        assert!(!pw.contains([' ', '\'', '"', '\\', '`', '$', ';', '\n']));
    }

    #[test]
    fn csv_round_trips() {
        let mut m = Manifest::new();
        m.push(ManifestEntry {
            archive: "photos.7z".into(),
            source: "photos/".into(),
            password: "aB3xY9".into(),
            sha256: "3f9a".into(),
            created: "2026-07-27T14:03:00Z".into(),
        });
        let back = Manifest::from_csv(&m.to_csv());
        assert_eq!(back, m);
    }

    #[test]
    fn csv_quotes_awkward_fields() {
        let mut m = Manifest::new();
        m.push(ManifestEntry {
            archive: "a,b\".7z".into(),           // comma + quote
            source: "with\nnewline".into(),        // embedded newline
            password: "p".into(),
            sha256: String::new(),
            created: String::new(),
        });
        let csv = m.to_csv();
        assert!(csv.contains("\"a,b\"\".7z\""), "field should be quoted+escaped: {csv}");
        assert_eq!(Manifest::from_csv(&csv), m);
    }

    #[test]
    fn from_csv_strips_utf8_bom() {
        // Excel/LibreOffice save with a leading BOM; the header must still skip.
        let csv = "\u{feff}archive,source,password,sha256,created\r\nx.7z,src,pw,,\r\n";
        let m = Manifest::from_csv(csv);
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].archive, "x.7z");
        assert_eq!(m.entries[0].password, "pw");
    }

    #[test]
    fn from_csv_tolerates_no_header_and_short_rows() {
        // no header, a row missing the trailing columns, no final newline
        let m = Manifest::from_csv("only.7z,src,pw");
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].archive, "only.7z");
        assert_eq!(m.entries[0].password, "pw");
        assert_eq!(m.entries[0].sha256, "");
    }
}
