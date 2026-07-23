//! Integration test: `write_checksum_file` against a *real* `7zz`.
//!
//! Ignored by default (spawns 7zz). Run with:
//!   cargo test -p septima-engine --test real_checksum -- --ignored --nocapture

use septima_engine::{hash_file, sevenzip_path, write_checksum_file};

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-checksum-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "spawns real 7zz"]
fn writes_a_matching_sha256_for_a_single_file_archive() {
    let dir = scratch("single");
    let archive = dir.join("out.7z");
    std::fs::write(&archive, b"not a real archive, just needs bytes to hash").unwrap();

    let sevenzip = sevenzip_path();
    let checksum_path = write_checksum_file(&sevenzip, &archive).unwrap();
    assert_eq!(checksum_path, dir.join("out.7z.sha256"));

    let expected = hash_file(&sevenzip, &archive, &["SHA256"]).unwrap();
    let expected_hex = &expected.iter().find(|d| d.algo == "SHA256").unwrap().hex;

    let contents = std::fs::read_to_string(&checksum_path).unwrap();
    assert_eq!(contents, format!("{expected_hex}  out.7z\n"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn writes_one_line_per_volume_part() {
    let dir = scratch("volumes");
    let base = dir.join("out.7z");
    std::fs::write(dir.join("out.7z.001"), b"part one bytes").unwrap();
    std::fs::write(dir.join("out.7z.002"), b"part two bytes").unwrap();

    let sevenzip = sevenzip_path();
    let checksum_path = write_checksum_file(&sevenzip, &base).unwrap();
    assert_eq!(checksum_path, dir.join("out.7z.sha256"));

    let contents = std::fs::read_to_string(&checksum_path).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("  out.7z.001"));
    assert!(lines[1].ends_with("  out.7z.002"));

    std::fs::remove_dir_all(&dir).unwrap();
}
