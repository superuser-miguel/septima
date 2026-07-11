use std::path::PathBuf;

/// One entry (file or directory) inside an archive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Archive-relative path, e.g. `tree/sub/a file.txt`.
    pub path: String,
    pub is_dir: bool,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Compressed size in bytes. `None` when `7zz` reports it empty (e.g. members
    /// sharing a solid block, or directories).
    pub packed_size: Option<u64>,
    /// Raw `Modified` string as printed by `7zz` (e.g. `2026-07-10 15:09:36.959`).
    pub modified: Option<String>,
    /// Compression method for this entry, e.g. `LZMA2:6k` or `Store`.
    pub method: Option<String>,
    /// CRC as an uppercase hex string, when present.
    pub crc: Option<String>,
    /// Whether this specific entry's data is encrypted.
    pub encrypted: bool,
    /// Raw `Attributes` string, e.g. `A -rw-r--r--` or `D drwxr-xr-x`.
    pub attributes: Option<String>,
}

/// The parsed result of `7z l -slt <archive>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveListing {
    /// Path to the archive on disk (filled in by the caller).
    pub path: PathBuf,
    /// Archive format from the header `Type` field, e.g. `7z`, `zip`, `tar`.
    pub format: Option<String>,
    pub entries: Vec<ArchiveEntry>,
}

impl ArchiveListing {
    pub fn file_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_dir).count()
    }

    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }
}

/// Parse the technical listing produced by `7z l -slt`.
///
/// The output has a header block (archive-level properties) followed by a line
/// of dashes, then one `Key = Value` block per entry, blank-line separated. We
/// take `Type` from the header and one [`ArchiveEntry`] per block after the
/// separator; unknown keys are ignored, so new `7zz` fields don't break parsing.
pub fn parse_listing(output: &str) -> ArchiveListing {
    let mut listing = ArchiveListing::default();
    let mut in_entries = false;
    let mut current: Option<ArchiveEntry> = None;

    for line in output.lines() {
        if line == "----------" {
            in_entries = true;
            continue;
        }

        if !in_entries {
            if let Some(("Type", value)) = split_kv(line) {
                if listing.format.is_none() {
                    listing.format = non_empty(value);
                }
            }
            continue;
        }

        if line.is_empty() {
            flush(&mut listing, &mut current);
            continue;
        }

        if let Some((key, value)) = split_kv(line) {
            apply_field(current.get_or_insert_with(ArchiveEntry::default), key, value);
        }
    }
    flush(&mut listing, &mut current);
    listing
}

fn flush(listing: &mut ArchiveListing, current: &mut Option<ArchiveEntry>) {
    if let Some(entry) = current.take() {
        if !entry.path.is_empty() {
            listing.entries.push(entry);
        }
    }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    line.split_once(" = ").map(|(k, v)| (k.trim(), v.trim()))
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn apply_field(entry: &mut ArchiveEntry, key: &str, value: &str) {
    match key {
        "Path" => entry.path = value.to_string(),
        "Size" => entry.size = value.parse().unwrap_or(0),
        "Packed Size" => entry.packed_size = value.parse().ok(),
        "Modified" => entry.modified = non_empty(value),
        "Attributes" => {
            entry.attributes = non_empty(value);
            // The `Attributes` field leads with `D` for directories on every
            // format 7zz emits; `Folder = +` (zip) is handled separately below.
            if value.starts_with('D') {
                entry.is_dir = true;
            }
        }
        "Folder" => {
            if value == "+" {
                entry.is_dir = true;
            }
        }
        "CRC" => entry.crc = non_empty(value),
        "Encrypted" => entry.encrypted = value == "+",
        "Method" => entry.method = non_empty(value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_7Z: &str = include_str!("../tests/fixtures/list_7z.slt.txt");
    const LIST_ZIP: &str = include_str!("../tests/fixtures/list_zip.slt.txt");

    #[test]
    fn parses_7z_header_and_entries() {
        let l = parse_listing(LIST_7Z);
        assert_eq!(l.format.as_deref(), Some("7z"));
        // tree, tree/sub, tree/data.bin, tree/readme.txt, tree/sub/a file.txt
        assert_eq!(l.entries.len(), 5);
        assert_eq!(l.file_count(), 3);

        let dirs: Vec<_> = l.entries.iter().filter(|e| e.is_dir).map(|e| &e.path).collect();
        assert_eq!(dirs, ["tree", "tree/sub"]);
    }

    #[test]
    fn parses_entry_fields() {
        let l = parse_listing(LIST_7Z);
        let bin = l.entries.iter().find(|e| e.path == "tree/data.bin").unwrap();
        assert!(!bin.is_dir);
        assert_eq!(bin.size, 4096);
        assert_eq!(bin.packed_size, Some(4136));
        assert_eq!(bin.crc.as_deref(), Some("597445BE"));
        assert_eq!(bin.method.as_deref(), Some("LZMA2:6k"));
        assert!(!bin.encrypted);
        assert!(bin.modified.is_some());
    }

    #[test]
    fn empty_packed_size_is_none() {
        // readme.txt shares a solid block, so 7zz prints an empty Packed Size.
        let l = parse_listing(LIST_7Z);
        let readme = l.entries.iter().find(|e| e.path == "tree/readme.txt").unwrap();
        assert_eq!(readme.packed_size, None);
        assert_eq!(readme.size, 12);
    }

    #[test]
    fn handles_paths_with_spaces() {
        let l = parse_listing(LIST_7Z);
        assert!(l.entries.iter().any(|e| e.path == "tree/sub/a file.txt"));
    }

    #[test]
    fn detects_compressed_tars() {
        use crate::command::is_compressed_tar;
        use std::path::Path;
        for yes in ["a.tar.zst", "b.TAR.GZ", "c.tgz", "d.tar.xz", "e.tbz2"] {
            assert!(is_compressed_tar(Path::new(yes)), "{yes}");
        }
        for no in ["a.7z", "b.zip", "c.tar", "d.zst"] {
            assert!(!is_compressed_tar(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn parses_zip_folder_flag_and_store_method() {
        let l = parse_listing(LIST_ZIP);
        assert_eq!(l.format.as_deref(), Some("zip"));
        // zip marks directories with `Folder = +`.
        assert!(l.entries.iter().find(|e| e.path == "tree").unwrap().is_dir);
        assert!(l.entries.iter().any(|e| e.method.as_deref() == Some("Store")));
    }
}
