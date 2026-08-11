//! Batch password manifest — the engine-side foundation of the batch-encrypt
//! feature: cryptographically-random password generation, the [`Manifest`] data
//! type, JSON read/write (the manifest format), and RFC-4180 CSV (kept as a
//! password-manager export/import, since KeePassXC and Bitwarden bulk-import
//! CSV rather than arbitrary JSON).
//!
//! Dependency-free by design (see the crate's A1 boundary): the CSPRNG is the
//! kernel's `/dev/urandom`, and both serializers are hand-rolled. The manifest
//! is a portable file — Septima writes it and reads it back to drive batch
//! decrypt; *where* it's stored (a vault, a `.gpg` file, plain disk) is the
//! user's call, so nothing here talks to a password manager.

use std::io::{Read, Write};
use std::path::Path;

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
/// password, and informational integrity/context columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Archive **basename**, never a path — the manifest must stay portable.
    pub archive: String,
    /// What it was made from. Informational only; never used to decrypt.
    pub source: String,
    /// The exact secret. Never trimmed.
    pub password: String,
    /// Optional integrity record for the archive file itself.
    pub sha256: String,
    /// UTC ISO-8601, informational.
    pub created: String,
    /// Human-readable cipher note ("7z, AES-256, encrypted headers") — the
    /// context nobody remembers six months later.
    pub encryption: String,
}

/// A batch's archive→password mapping, plus a small informational header.
/// Serialises to JSON (the manifest proper) or CSV (password-manager export).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Septima version that wrote the file. Informational.
    pub septima: String,
    /// Batch timestamp, UTC ISO-8601. Informational; the GTK layer fills it.
    pub created: String,
    pub entries: Vec<ManifestEntry>,
}

/// Bumped only if the JSON layout changes incompatibly. Readers stay lenient.
const FORMAT_VERSION: u32 = 1;

/// A manifest file that could not be understood at all. Individual odd entries
/// never error — a wrong manifest means unrecoverable archives, so parsing is
/// as tolerant as it can be while still refusing non-manifests outright.
#[derive(Debug)]
pub struct ManifestParseError(pub String);

impl std::fmt::Display for ManifestParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a readable passwords file: {}", self.0)
    }
}

impl std::error::Error for ManifestParseError {}

impl Manifest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: ManifestEntry) {
        self.entries.push(entry);
    }

    /// Serialise to the manifest format: pretty-printed JSON, one entry object
    /// per archive, self-describing keys so the file reads well in any editor.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        s.push_str("{\n");
        s.push_str(&format!("  \"septima_manifest\": {FORMAT_VERSION},\n"));
        s.push_str(&format!("  \"septima\": {},\n", json_str(&self.septima)));
        s.push_str(&format!("  \"created\": {},\n", json_str(&self.created)));
        s.push_str("  \"entries\": [\n");
        let last = self.entries.len().saturating_sub(1);
        for (i, e) in self.entries.iter().enumerate() {
            s.push_str("    {\n");
            s.push_str(&format!("      \"archive\": {},\n", json_str(&e.archive)));
            s.push_str(&format!("      \"source\": {},\n", json_str(&e.source)));
            s.push_str(&format!("      \"password\": {},\n", json_str(&e.password)));
            s.push_str(&format!("      \"sha256\": {},\n", json_str(&e.sha256)));
            s.push_str(&format!("      \"created\": {},\n", json_str(&e.created)));
            s.push_str(&format!("      \"encryption\": {}\n", json_str(&e.encryption)));
            s.push_str(if i == last { "    }\n" } else { "    },\n" });
        }
        s.push_str("  ]\n}\n");
        s
    }

    /// Parse a manifest from JSON. Lenient wherever safe: unknown keys are
    /// ignored, missing fields become empty strings, a newer `septima_manifest`
    /// number still parses. Errors only when the text isn't JSON or has no
    /// `entries` array — i.e. isn't a passwords file at all.
    pub fn from_json(text: &str) -> Result<Manifest, ManifestParseError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let root = json::parse(text).map_err(ManifestParseError)?;
        let obj = match &root {
            json::Value::Object(pairs) => pairs,
            _ => return Err(ManifestParseError("top level is not an object".into())),
        };
        let get_str = |pairs: &[(String, json::Value)], key: &str| -> String {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| match v {
                    json::Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        };
        let entries_val = obj.iter().find(|(k, _)| k == "entries").map(|(_, v)| v);
        let arr = match entries_val {
            Some(json::Value::Array(a)) => a,
            _ => return Err(ManifestParseError("no \"entries\" array".into())),
        };
        let mut entries = Vec::new();
        for item in arr {
            if let json::Value::Object(pairs) = item {
                let e = ManifestEntry {
                    archive: get_str(pairs, "archive"),
                    source: get_str(pairs, "source"),
                    password: get_str(pairs, "password"),
                    sha256: get_str(pairs, "sha256"),
                    created: get_str(pairs, "created"),
                    encryption: get_str(pairs, "encryption"),
                };
                if e != ManifestEntry::default() {
                    entries.push(e);
                }
            }
        }
        Ok(Manifest {
            septima: get_str(obj, "septima"),
            created: get_str(obj, "created"),
            entries,
        })
    }

    /// Parse a manifest of either format: JSON (sniffed by a leading `{`) or
    /// the CSV shape — covering both the export and any manifest written by
    /// the 0.4.x engine.
    pub fn parse(text: &str) -> Result<Manifest, ManifestParseError> {
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        if body.trim_start().starts_with('{') {
            Manifest::from_json(body)
        } else {
            let m = Manifest::from_csv(body);
            if m.entries.is_empty() {
                return Err(ManifestParseError("no rows recognised".into()));
            }
            Ok(m)
        }
    }

    /// Serialise to RFC-4180 CSV (CRLF line endings, header row first) — the
    /// **password-manager export**: KeePassXC/Bitwarden map `archive` to the
    /// entry title and `password` to the password on import.
    pub fn to_csv(&self) -> String {
        let mut s = String::new();
        s.push_str(&csv_row(HEADER));
        for e in &self.entries {
            s.push_str(&csv_row(&[
                &e.archive,
                &e.source,
                &e.password,
                &e.sha256,
                &e.created,
                &e.encryption,
            ]));
        }
        s
    }

    /// Parse a manifest CSV. A leading `archive,…` header row is skipped; rows
    /// with fewer than six fields are padded with empty strings (0.4.x wrote
    /// five columns), extras dropped.
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
                encryption: get(5),
            });
        }
        Manifest {
            septima: String::new(),
            created: String::new(),
            entries,
        }
    }
}

/// Write `bytes` to `path` atomically: a hidden sibling temp file (created
/// owner-read/write only — this file holds passwords), fsynced, then renamed
/// over the destination. A crash mid-write can never leave a truncated
/// manifest where a complete one should be.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("manifest path has no file name"))?;
    let tmp = dir.join(format!(".{}.tmp", name.to_string_lossy()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    // Make the rename itself durable; best-effort, some filesystems refuse.
    if let Ok(d) = std::fs::File::open(&dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

// --- minimal JSON ------------------------------------------------------------

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

mod json {
    //! Just enough of a JSON parser for the flat manifest schema — objects,
    //! arrays, strings (with escapes), numbers, booleans, null. Rejects
    //! trailing garbage. No serde: the crate is deliberately dependency-free,
    //! and the input is a file this module (or a careful human) wrote.

    #[derive(Debug, Clone, PartialEq)]
    pub enum Value {
        Object(Vec<(String, Value)>),
        Array(Vec<Value>),
        String(String),
        Number(f64),
        Bool(bool),
        Null,
    }

    pub fn parse(text: &str) -> Result<Value, String> {
        let chars: Vec<char> = text.chars().collect();
        let mut p = Parser { c: &chars, i: 0 };
        p.ws();
        let v = p.value()?;
        p.ws();
        if p.i != p.c.len() {
            return Err(format!("trailing data at character {}", p.i));
        }
        Ok(v)
    }

    struct Parser<'a> {
        c: &'a [char],
        i: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<char> {
            self.c.get(self.i).copied()
        }

        fn ws(&mut self) {
            while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                self.i += 1;
            }
        }

        fn expect(&mut self, ch: char) -> Result<(), String> {
            if self.peek() == Some(ch) {
                self.i += 1;
                Ok(())
            } else {
                Err(format!("expected '{ch}' at character {}", self.i))
            }
        }

        fn value(&mut self) -> Result<Value, String> {
            match self.peek() {
                Some('{') => self.object(),
                Some('[') => self.array(),
                Some('"') => Ok(Value::String(self.string()?)),
                Some('t') => self.literal("true", Value::Bool(true)),
                Some('f') => self.literal("false", Value::Bool(false)),
                Some('n') => self.literal("null", Value::Null),
                Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
                _ => Err(format!("unexpected input at character {}", self.i)),
            }
        }

        fn literal(&mut self, word: &str, v: Value) -> Result<Value, String> {
            for ch in word.chars() {
                self.expect(ch)?;
            }
            Ok(v)
        }

        fn object(&mut self) -> Result<Value, String> {
            self.expect('{')?;
            let mut pairs = Vec::new();
            self.ws();
            if self.peek() == Some('}') {
                self.i += 1;
                return Ok(Value::Object(pairs));
            }
            loop {
                self.ws();
                let key = self.string()?;
                self.ws();
                self.expect(':')?;
                self.ws();
                let val = self.value()?;
                pairs.push((key, val));
                self.ws();
                match self.peek() {
                    Some(',') => self.i += 1,
                    Some('}') => {
                        self.i += 1;
                        return Ok(Value::Object(pairs));
                    }
                    _ => return Err(format!("expected ',' or '}}' at character {}", self.i)),
                }
            }
        }

        fn array(&mut self) -> Result<Value, String> {
            self.expect('[')?;
            let mut items = Vec::new();
            self.ws();
            if self.peek() == Some(']') {
                self.i += 1;
                return Ok(Value::Array(items));
            }
            loop {
                self.ws();
                items.push(self.value()?);
                self.ws();
                match self.peek() {
                    Some(',') => self.i += 1,
                    Some(']') => {
                        self.i += 1;
                        return Ok(Value::Array(items));
                    }
                    _ => return Err(format!("expected ',' or ']' at character {}", self.i)),
                }
            }
        }

        fn string(&mut self) -> Result<String, String> {
            self.expect('"')?;
            let mut out = String::new();
            loop {
                match self.peek() {
                    None => return Err("unterminated string".into()),
                    Some('"') => {
                        self.i += 1;
                        return Ok(out);
                    }
                    Some('\\') => {
                        self.i += 1;
                        let esc = self.peek().ok_or("unterminated escape")?;
                        self.i += 1;
                        match esc {
                            '"' => out.push('"'),
                            '\\' => out.push('\\'),
                            '/' => out.push('/'),
                            'n' => out.push('\n'),
                            'r' => out.push('\r'),
                            't' => out.push('\t'),
                            'b' => out.push('\u{8}'),
                            'f' => out.push('\u{c}'),
                            'u' => {
                                let mut code = 0u32;
                                for _ in 0..4 {
                                    let h = self.peek().and_then(|c| c.to_digit(16)).ok_or_else(
                                        || format!("bad \\u escape at character {}", self.i),
                                    )?;
                                    code = code * 16 + h;
                                    self.i += 1;
                                }
                                // Surrogate pairs: only if a low surrogate follows.
                                if (0xD800..0xDC00).contains(&code)
                                    && self.c.get(self.i) == Some(&'\\')
                                    && self.c.get(self.i + 1) == Some(&'u')
                                {
                                    let save = self.i;
                                    self.i += 2;
                                    let mut low = 0u32;
                                    let mut ok = true;
                                    for _ in 0..4 {
                                        match self.peek().and_then(|c| c.to_digit(16)) {
                                            Some(h) => {
                                                low = low * 16 + h;
                                                self.i += 1;
                                            }
                                            None => {
                                                ok = false;
                                                break;
                                            }
                                        }
                                    }
                                    if ok && (0xDC00..0xE000).contains(&low) {
                                        code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                    } else {
                                        self.i = save;
                                    }
                                }
                                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                            other => return Err(format!("bad escape '\\{other}'")),
                        }
                    }
                    Some(c) => {
                        out.push(c);
                        self.i += 1;
                    }
                }
            }
        }

        fn number(&mut self) -> Result<Value, String> {
            let start = self.i;
            if self.peek() == Some('-') {
                self.i += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
            {
                self.i += 1;
            }
            let text: String = self.c[start..self.i].iter().collect();
            text.parse::<f64>()
                .map(Value::Number)
                .map_err(|_| format!("bad number '{text}'"))
        }
    }
}

// --- minimal RFC-4180 CSV ----------------------------------------------------

const HEADER: &[&str] = &["archive", "source", "password", "sha256", "created", "encryption"];

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

    fn sample() -> Manifest {
        let mut m = Manifest::new();
        m.septima = "0.5.0".into();
        m.created = "2026-08-11T20:00:00Z".into();
        m.push(ManifestEntry {
            archive: "photos.7z".into(),
            source: "photos/".into(),
            password: "aB3xY9".into(),
            sha256: "3f9a".into(),
            created: "2026-08-11T20:00:01Z".into(),
            encryption: "7z, AES-256, encrypted headers".into(),
        });
        m
    }

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
        // No shell/CSV/JSON wrecking characters that we deliberately excluded.
        assert!(!pw.contains([' ', '\'', '"', '\\', '`', '$', ';', '\n']));
    }

    #[test]
    fn json_round_trips() {
        let m = sample();
        let back = Manifest::from_json(&m.to_json()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn json_escapes_awkward_strings() {
        let mut m = Manifest::new();
        m.push(ManifestEntry {
            archive: "a\"b\\c.7z".into(),
            source: "tab\there\nand a line".into(),
            password: "p".into(),
            ..Default::default()
        });
        let back = Manifest::from_json(&m.to_json()).unwrap();
        assert_eq!(back.entries, m.entries);
    }

    #[test]
    fn json_tolerates_hand_edits() {
        // Unknown keys, missing fields, reordered keys, extra whitespace —
        // everything a careful human might leave behind in an editor.
        let text = r#"
        {
          "septima_manifest": 99,
          "note": "I moved this file to my vault",
          "entries": [
            { "password": "pw1", "archive": "one.7z" },
            { "archive": "two.7z", "password": "pw2", "color": "blue" }
          ]
        }"#;
        let m = Manifest::from_json(text).unwrap();
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.entries[0].archive, "one.7z");
        assert_eq!(m.entries[0].password, "pw1");
        assert_eq!(m.entries[1].password, "pw2");
    }

    #[test]
    fn json_rejects_non_manifests() {
        assert!(Manifest::from_json("not json").is_err());
        assert!(Manifest::from_json("{\"foo\": 1}").is_err(), "no entries array");
        assert!(Manifest::from_json("[1,2,3]").is_err(), "not an object");
    }

    #[test]
    fn json_unicode_escapes_decode() {
        let text = r#"{"entries":[{"archive":"café.7z","password":"😀"}]}"#;
        let m = Manifest::from_json(text).unwrap();
        assert_eq!(m.entries[0].archive, "café.7z");
        assert_eq!(m.entries[0].password, "😀");
    }

    #[test]
    fn parse_sniffs_json_and_csv() {
        let m = sample();
        assert_eq!(Manifest::parse(&m.to_json()).unwrap(), m);
        let from_csv = Manifest::parse(&m.to_csv()).unwrap();
        assert_eq!(from_csv.entries, m.entries);
        assert!(Manifest::parse("").is_err());
    }

    #[test]
    fn csv_round_trips() {
        let m = sample();
        let back = Manifest::from_csv(&m.to_csv());
        assert_eq!(back.entries, m.entries);
    }

    #[test]
    fn csv_quotes_awkward_fields() {
        let mut m = Manifest::new();
        m.push(ManifestEntry {
            archive: "a,b\".7z".into(),    // comma + quote
            source: "with\nnewline".into(), // embedded newline
            password: "p".into(),
            ..Default::default()
        });
        let csv = m.to_csv();
        assert!(csv.contains("\"a,b\"\".7z\""), "field should be quoted+escaped: {csv}");
        assert_eq!(Manifest::from_csv(&csv).entries, m.entries);
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
        // no header, a 0.4.x five-column row, a row missing trailing columns,
        // no final newline
        let m = Manifest::from_csv("only.7z,src,pw");
        assert_eq!(m.entries.len(), 1);
        assert_eq!(m.entries[0].archive, "only.7z");
        assert_eq!(m.entries[0].password, "pw");
        assert_eq!(m.entries[0].sha256, "");
        assert_eq!(m.entries[0].encryption, "");
    }

    #[test]
    fn write_atomic_replaces_and_restricts() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("septima-manifest-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("passwords.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "passwords file must be owner-only");
        assert!(
            !dir.join(".passwords.json.tmp").exists(),
            "temp file should be gone after rename"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
