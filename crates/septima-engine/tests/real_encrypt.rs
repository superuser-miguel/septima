//! Integration test: zip encryption method (`-mem=`) against a real `7zz`.
//!
//! Ignored by default (spawns 7zz). Run with:
//!   cargo test -p septima-engine --test real_encrypt -- --ignored --nocapture

use septima_engine::{list_archive, new_cancel_token, run_add, sevenzip_path, CompressionRequest};

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-encrypt-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "spawns real 7zz"]
fn zip_aes256_uses_the_aes_cipher() {
    let dir = scratch("zip-aes");
    let input = dir.join("secret.txt");
    std::fs::write(&input, b"top secret contents that must be encrypted with AES").unwrap();

    let archive = dir.join("out.zip");
    let mut req = CompressionRequest::new(archive.clone(), vec![input], "zip");
    req.codec = Some("deflate".into());
    req.password = Some("hunter2".into());
    req.zip_encryption = Some("AES256".into());
    run_add(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();

    // List with the password; the entry's method should mention AES, not ZipCrypto.
    let listing = list_archive(&sevenzip_path(), &archive, Some("hunter2")).unwrap();
    let method = listing
        .entries
        .iter()
        .find(|e| e.path == "secret.txt")
        .and_then(|e| e.method.clone())
        .unwrap_or_default();
    assert!(
        method.contains("AES"),
        "expected an AES cipher, got method = {method:?}"
    );
    assert!(
        !method.contains("ZipCrypto"),
        "should not fall back to ZipCrypto, got {method:?}"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}
