//! Integration tests: the full encryption matrix against a real `7zz`.
//!
//! Ground truth gathered 2026-07-26 by probing `7zz` directly:
//!
//! | archive              | list no-pw | list wrong-pw | extract no/wrong-pw | extract right-pw |
//! |----------------------|-----------|---------------|---------------------|------------------|
//! | 7z (AES, headers off)| OK        | OK            | PasswordRequired    | OK               |
//! | 7z (AES, -mhe=on)    | **PwReq** | **PwReq**     | PasswordRequired    | OK               |
//! | zip ZipCrypto        | OK        | OK            | PasswordRequired*   | OK               |
//! | zip AES-128/192/256  | OK        | OK            | PasswordRequired*   | OK               |
//!
//! Only 7z with encrypted headers (`-mhe=on`) hides filenames, so only it needs
//! a password to *list*. Everything else lists freely and needs the password at
//! *extract*. `7zz` puts "Enter password:" on stdout and "Wrong password" /
//! "Break signaled" on stderr, so detection must scan both streams.
//!
//! (*) A failed-password zip extract leaves a 0-byte placeholder file behind;
//! the normal prompt→retry path overwrites it (`-aoa`). Not asserted here.
//!
//! Ignored by default (spawns 7zz). Run with:
//!   cargo test -p septima-engine --test real_encrypt -- --ignored --nocapture

use septima_engine::{
    list_archive, new_cancel_token, run_add, run_extract, sevenzip_path, CompressionRequest,
    EngineError, ExtractRequest, OverwriteMode,
};

const PW: &str = "CorrectPass123";

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-encrypt-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build an encrypted archive. `mem` = cipher (`-mem=`); `headers` = 7z
/// encrypted headers (`-mhe=on`).
fn build(dir: &std::path::Path, name: &str, format: &str, mem: Option<&str>, headers: bool) -> std::path::PathBuf {
    let input = dir.join("payload.txt");
    std::fs::write(&input, b"confidential contents for encryption testing").unwrap();
    let archive = dir.join(name);
    let mut req = CompressionRequest::new(archive.clone(), vec![input], format);
    req.password = Some(PW.into());
    req.encryption_method = mem.map(String::from);
    req.encrypt_headers = headers;
    run_add(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();
    archive
}

fn try_list(archive: &std::path::Path, pw: Option<&str>) -> Result<(), EngineError> {
    list_archive(&sevenzip_path(), archive, pw).map(|_| ())
}

fn try_extract(archive: &std::path::Path, dest: &std::path::Path, pw: Option<&str>) -> Result<(), EngineError> {
    std::fs::create_dir_all(dest).unwrap();
    let req = ExtractRequest {
        archive: archive.to_path_buf(),
        dest_dir: dest.to_path_buf(),
        password: pw.map(String::from),
        overwrite: OverwriteMode::default(),
    };
    run_extract(&sevenzip_path(), &req, &new_cancel_token(), |_| {})
}

// --- Listing: only 7z-with-headers needs a password -----------------------

#[test]
#[ignore = "spawns real 7zz"]
fn plain_encryption_lists_without_a_password() {
    // 7z (headers off) and every zip cipher list fine with no password.
    let dir = scratch("list-free");
    for (name, fmt, mem) in [
        ("a.7z", "7z", None),
        ("a.zip", "zip", None),           // ZipCrypto
        ("aes.zip", "zip", Some("AES256")),
    ] {
        let archive = build(&dir, name, fmt, mem, false);
        assert!(try_list(&archive, None).is_ok(), "{name} should list without a password");
        assert!(try_list(&archive, Some("wrong")).is_ok(), "{name} lists even with a wrong password");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn encrypted_headers_require_a_password_to_list() {
    let dir = scratch("list-hdr");
    let archive = build(&dir, "hdr.7z", "7z", None, true);
    assert!(
        matches!(try_list(&archive, None), Err(EngineError::PasswordRequired)),
        "encrypted-header 7z must ask for a password to list"
    );
    assert!(
        matches!(try_list(&archive, Some("wrong")), Err(EngineError::PasswordRequired)),
        "a wrong password on an encrypted-header 7z must map to PasswordRequired"
    );
    assert!(try_list(&archive, Some(PW)).is_ok(), "the right password lists it");
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- Extraction: everything needs the password; no/wrong → PasswordRequired ---

#[test]
#[ignore = "spawns real 7zz"]
fn extract_without_password_asks_across_all_ciphers() {
    let dir = scratch("ex-nopw");
    for (name, fmt, mem, hdr) in [
        ("a.7z", "7z", None, false),
        ("hdr.7z", "7z", None, true),
        ("zc.zip", "zip", None, false),
        ("a128.zip", "zip", Some("AES128"), false),
        ("a256.zip", "zip", Some("AES256"), false),
    ] {
        let archive = build(&dir, name, fmt, mem, hdr);
        let dest = dir.join(format!("out-{name}"));
        assert!(
            matches!(try_extract(&archive, &dest, None), Err(EngineError::PasswordRequired)),
            "{name}: extract with no password must be PasswordRequired"
        );
        assert!(
            matches!(try_extract(&archive, &dest, Some("wrong")), Err(EngineError::PasswordRequired)),
            "{name}: extract with a wrong password must be PasswordRequired"
        );
        // No 0-byte placeholder should be left behind after the failure.
        assert!(
            !dest.join("payload.txt").exists(),
            "{name}: a failed-password extract must not leave a leftover file"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn correct_password_extracts_every_variant() {
    let dir = scratch("ex-right");
    for (name, fmt, mem, hdr) in [
        ("a.7z", "7z", None, false),
        ("hdr.7z", "7z", None, true),
        ("zc.zip", "zip", None, false),
        ("a128.zip", "zip", Some("AES128"), false),
        ("a192.zip", "zip", Some("AES192"), false),
        ("a256.zip", "zip", Some("AES256"), false),
    ] {
        let archive = build(&dir, name, fmt, mem, hdr);
        let dest = dir.join(format!("out-{name}"));
        try_extract(&archive, &dest, Some(PW)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let got = std::fs::read(dest.join("payload.txt")).unwrap();
        assert_eq!(got, b"confidential contents for encryption testing", "{name} content");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- The specific cipher actually applied ---------------------------------

// --- AES-256-GCM + Argon2id (Septima engine extension) --------------------
//
// Skipped automatically when the `7zz` under test is stock (no GCM codec), so
// the suite still passes against a distro 7-Zip.

#[test]
#[ignore = "spawns real 7zz"]
fn sevenz_gcm_roundtrips_and_rejects_wrong_passwords() {
    if !septima_engine::capabilities::aes256gcm_available() {
        eprintln!("skipping: this 7zz has no AES256GCM codec");
        return;
    }
    let dir = scratch("gcm");
    for (name, hdr) in [("gcm.7z", false), ("gcm-hdr.7z", true)] {
        let archive = build(&dir, name, "7z", Some("AES256GCM"), hdr);
        let dest = dir.join(format!("out-{name}"));

        assert!(
            matches!(try_extract(&archive, &dest, None), Err(EngineError::PasswordRequired)),
            "{name}: extract with no password must be PasswordRequired"
        );
        assert!(
            matches!(try_extract(&archive, &dest, Some("wrong")), Err(EngineError::PasswordRequired)),
            "{name}: a wrong password must be PasswordRequired, not a corruption error"
        );

        try_extract(&archive, &dest, Some(PW)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let got = std::fs::read(dest.join("payload.txt")).unwrap();
        assert_eq!(got, b"confidential contents for encryption testing", "{name} content");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn sevenz_gcm_is_opt_in_and_reported_in_the_listing() {
    if !septima_engine::capabilities::aes256gcm_available() {
        eprintln!("skipping: this 7zz has no AES256GCM codec");
        return;
    }
    let dir = scratch("gcm-method");
    let method_of = |archive: &std::path::Path| {
        list_archive(&sevenzip_path(), archive, Some(PW))
            .unwrap()
            .entries
            .iter()
            .find(|e| e.path == "payload.txt")
            .and_then(|e| e.method.clone())
            .unwrap_or_default()
    };

    let gcm = method_of(&build(&dir, "gcm.7z", "7z", Some("AES256GCM"), false));
    assert!(gcm.contains("AES256GCM"), "expected AES256GCM, got {gcm:?}");

    // Without an explicit request, 7z must stay on the interoperable default.
    let default = method_of(&build(&dir, "plain.7z", "7z", None, false));
    assert!(default.contains("7zAES"), "default 7z should be 7zAES, got {default:?}");
    assert!(!default.contains("GCM"), "default 7z must not silently use GCM, got {default:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "spawns real 7zz"]
fn zip_aes256_uses_the_aes_cipher_not_zipcrypto() {
    let dir = scratch("cipher");
    let archive = build(&dir, "aes.zip", "zip", Some("AES256"), false);
    let listing = list_archive(&sevenzip_path(), &archive, Some(PW)).unwrap();
    let method = listing
        .entries
        .iter()
        .find(|e| e.path == "payload.txt")
        .and_then(|e| e.method.clone())
        .unwrap_or_default();
    assert!(method.contains("AES"), "expected AES, got {method:?}");
    assert!(!method.contains("ZipCrypto"), "should not be ZipCrypto, got {method:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
