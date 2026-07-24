//! Integration tests: `run_test` / `run_delete` / `run_rename` against a
//! *real* `7zz`.
//!
//! Ignored by default (spawns 7zz). Run with:
//!   cargo test -p septima-engine --test real_edit -- --ignored --nocapture

use septima_engine::{
    list_archive, new_cancel_token, run_add, run_delete, run_rename, run_test, sevenzip_path,
    CompressionRequest,
};

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-edit-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A real `.7z` with two files, built the same way the app would.
fn build_archive(dir: &std::path::Path) -> std::path::PathBuf {
    let a = dir.join("a.txt");
    let b = dir.join("b.txt");
    std::fs::write(&a, b"file a").unwrap();
    std::fs::write(&b, b"file b").unwrap();

    let archive = dir.join("test.7z");
    let cancel = septima_engine::new_cancel_token();
    let req = CompressionRequest::new(archive.clone(), vec![a, b], "7z");
    run_add(&sevenzip_path(), &req, &cancel, |_| {}).unwrap();
    archive
}

#[test]
#[ignore = "spawns real 7zz"]
fn test_passes_on_a_good_archive() {
    let dir = scratch("test-good");
    let archive = build_archive(&dir);

    run_test(&sevenzip_path(), &archive, None, &new_cancel_token(), |_| {}).unwrap();

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn test_fails_on_a_corrupt_archive() {
    let dir = scratch("test-corrupt");
    let archive = dir.join("corrupt.7z");
    std::fs::write(&archive, b"not a real archive").unwrap();

    let result = run_test(&sevenzip_path(), &archive, None, &new_cancel_token(), |_| {});
    assert!(result.is_err());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn delete_removes_the_entry() {
    let dir = scratch("delete");
    let archive = build_archive(&dir);

    run_delete(&sevenzip_path(), &archive, &["a.txt".to_string()], None).unwrap();

    let listing = list_archive(&sevenzip_path(), &archive, None).unwrap();
    let names: Vec<_> = listing.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(!names.contains(&"a.txt"));
    assert!(names.contains(&"b.txt"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn rename_changes_the_entry_name() {
    let dir = scratch("rename");
    let archive = build_archive(&dir);

    run_rename(
        &sevenzip_path(),
        &archive,
        &[("a.txt".to_string(), "renamed.txt".to_string())],
        None,
    )
    .unwrap();

    let listing = list_archive(&sevenzip_path(), &archive, None).unwrap();
    let names: Vec<_> = listing.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(!names.contains(&"a.txt"));
    assert!(names.contains(&"renamed.txt"));
    assert!(names.contains(&"b.txt"));

    std::fs::remove_dir_all(&dir).unwrap();
}
