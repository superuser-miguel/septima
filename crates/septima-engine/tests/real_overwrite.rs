//! Integration tests: what each overwrite mode does to a file already at the
//! destination, for a plain archive and for a compressed tarball (whose inner
//! `7zz x -si` step takes the same switch). These are the promises the extract
//! dialog's Keep both / Skip / Replace choice makes.
//!
//! Ignored by default (spawns real 7zz). Run with:
//!   cargo test -p septima-engine --test real_overwrite -- --ignored --nocapture

use std::path::{Path, PathBuf};

use septima_engine::{
    new_cancel_token, run_add, run_extract, run_tar_and_compress, sevenzip_path, CompressionRequest,
    ExtractRequest, OverwriteMode,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-overwrite-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An archive holding `report.txt` = "new", made with `format` ("7z" or a
/// tar compressor like "zstd").
fn archive(dir: &Path, format: &str) -> PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("report.txt"), b"new").unwrap();
    let cancel = new_cancel_token();
    if format == "7z" {
        let out = dir.join("a.7z");
        let req = CompressionRequest::new(&out, vec![src.join("report.txt")], "7z");
        run_add(&sevenzip_path(), &req, &cancel, |_| {}).unwrap();
        out
    } else {
        let out = dir.join("a.tar.zst");
        let mut req = CompressionRequest::new(&out, vec![src.join("report.txt")], "tar");
        req.codec = Some(format.into());
        run_tar_and_compress(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();
        out
    }
}

/// Extract `archive` with `mode` into a folder that already has
/// `report.txt` = "old"; return the folder's files and contents, sorted.
fn extract_over(dir: &Path, archive: &Path, mode: OverwriteMode) -> Vec<(String, String)> {
    let dest = dir.join(format!("dest-{mode:?}"));
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("report.txt"), b"old").unwrap();
    let mut req = ExtractRequest::new(archive, &dest);
    req.overwrite = mode;
    run_extract(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dest)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap())
        })
        .collect();
    files.sort();
    files
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

fn check_all_modes(tag: &str, format: &str) {
    let dir = scratch(tag);
    let a = archive(&dir, format);
    assert_eq!(
        extract_over(&dir, &a, OverwriteMode::RenameExtracted),
        pairs(&[("report.txt", "old"), ("report_1.txt", "new")]),
        "Keep both: the old file stays, the new one gets a number"
    );
    assert_eq!(
        extract_over(&dir, &a, OverwriteMode::Skip),
        pairs(&[("report.txt", "old")]),
        "Skip: the old file stays, the new one isn't extracted"
    );
    assert_eq!(
        extract_over(&dir, &a, OverwriteMode::Overwrite),
        pairs(&[("report.txt", "new")]),
        "Replace: the new file takes the name"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "spawns real 7zz"]
fn overwrite_modes_on_a_7z() {
    check_all_modes("7z", "7z");
}

#[test]
#[ignore = "spawns real 7zz"]
fn overwrite_modes_on_a_compressed_tarball() {
    check_all_modes("tzst", "zstd");
}
