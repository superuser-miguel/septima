//! Integration tests: extracting a subset of entries (ExtractRequest::entries),
//! the engine side of drag-out. Covers the direct path, a directory subtree,
//! and the compressed-tar two-process path where the patterns apply to the
//! inner tar.
//!
//! Ignored by default (spawns real 7zz). Run with:
//!   cargo test -p septima-engine --test real_subset_extract -- --ignored --nocapture

use std::path::{Path, PathBuf};

use septima_engine::{
    new_cancel_token, run_add, run_extract, sevenzip_path, CompressionRequest, ExtractRequest,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-subset-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A source tree with two top-level files and a folder with a child.
fn stage(dir: &Path) -> PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.txt"), b"alpha").unwrap();
    std::fs::write(src.join("b.txt"), b"beta").unwrap();
    std::fs::write(src.join("sub").join("c.txt"), b"gamma").unwrap();
    src
}

fn build(dir: &Path, name: &str, format: &str) -> PathBuf {
    let archive = dir.join(name);
    let req = CompressionRequest::new(archive.clone(), vec![dir.join("src")], format);
    run_add(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();
    archive
}

fn extract_subset(archive: &Path, dest: &Path, entries: &[&str]) {
    std::fs::create_dir_all(dest).unwrap();
    let mut req = ExtractRequest::new(archive, dest);
    req.entries = entries.iter().map(|s| s.to_string()).collect();
    run_extract(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn one_file_of_three_from_a_7z() {
    let dir = scratch("7z");
    stage(&dir);
    let archive = build(&dir, "t.7z", "7z");
    let dest = dir.join("out");
    extract_subset(&archive, &dest, &["src/a.txt"]);
    assert_eq!(std::fs::read(dest.join("src/a.txt")).unwrap(), b"alpha");
    assert!(!dest.join("src/b.txt").exists(), "b.txt was not asked for");
    assert!(!dest.join("src/sub").exists(), "sub/ was not asked for");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "spawns real 7zz"]
fn a_directory_entry_brings_its_subtree() {
    let dir = scratch("dir");
    stage(&dir);
    let archive = build(&dir, "t.zip", "zip");
    let dest = dir.join("out");
    extract_subset(&archive, &dest, &["src/sub"]);
    assert_eq!(std::fs::read(dest.join("src/sub/c.txt")).unwrap(), b"gamma");
    assert!(!dest.join("src/a.txt").exists(), "a.txt was not asked for");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "spawns real 7zz"]
fn subset_from_a_compressed_tar() {
    let dir = scratch("tarzst");
    stage(&dir);
    // Build the .tar.zst the same two-step way the app does.
    let archive = dir.join("t.tar.zst");
    let mut req = CompressionRequest::new(archive.clone(), vec![dir.join("src")], "tar");
    req.codec = Some("zstd".into());
    septima_engine::run_tar_and_compress(&sevenzip_path(), &req, &new_cancel_token(), |_| {})
        .unwrap();
    let dest = dir.join("out");
    extract_subset(&archive, &dest, &["src/b.txt"]);
    assert_eq!(std::fs::read(dest.join("src/b.txt")).unwrap(), b"beta");
    assert!(!dest.join("src/a.txt").exists(), "a.txt was not asked for");
    let _ = std::fs::remove_dir_all(&dir);
}
