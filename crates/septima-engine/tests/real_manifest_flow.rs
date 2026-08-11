//! Integration test: the whole batch-encrypt loop, end to end, exactly as the
//! GTK layer drives it — generate a password per item, compress each into its
//! own encrypted archive, record everything in the JSON manifest (optionally
//! GPG-protected), then come back the other way: parse the manifest and
//! extract every archive with its recorded password.
//!
//! Ignored by default (spawns 7zz and gpg). Run with:
//!   cargo test -p septima-engine --test real_manifest_flow -- --ignored --nocapture

use std::path::{Path, PathBuf};

use septima_engine::{
    decrypt_symmetric, encrypt_symmetric, generate_password, gpg_available, hash_file,
    new_cancel_token, run_add, run_extract, sevenzip_path, write_atomic, Charset,
    CompressionRequest, ExtractRequest, Manifest, ManifestEntry, OverwriteMode,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-mflow-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Three source folders with distinct contents, like a staged batch.
fn stage_sources(dir: &Path) -> Vec<PathBuf> {
    (0..3)
        .map(|i| {
            let src = dir.join(format!("project_{i}"));
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("data.txt"), format!("contents of project {i}")).unwrap();
            src
        })
        .collect()
}

/// The write side, as the window does it: passwords first, archives second.
fn run_batch(dir: &Path, items: &[PathBuf]) -> Manifest {
    let mut manifest = Manifest::new();
    manifest.septima = "test".into();
    manifest.created = "2026-08-11T00:00:00Z".into();
    let mut jobs = Vec::new();
    for item in items {
        let output = item.with_extension("7z");
        let password = generate_password(64, Charset::Alphanumeric).unwrap();
        manifest.push(ManifestEntry {
            archive: output.file_name().unwrap().to_string_lossy().into_owned(),
            source: item.file_name().unwrap().to_string_lossy().into_owned(),
            password: password.clone(),
            sha256: String::new(),
            created: String::new(),
            encryption: "7z, AES-256, encrypted headers".into(),
        });
        jobs.push((item.clone(), output, password));
    }
    // Manifest hits disk before any archive exists.
    let manifest_path = dir.join("passwords.json");
    write_atomic(&manifest_path, manifest.to_json().as_bytes()).unwrap();

    for (item, output, password) in jobs {
        let mut req = CompressionRequest::new(output.clone(), vec![item], "7z");
        req.password = Some(password);
        req.encrypt_headers = true;
        run_add(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();
        let digest = hash_file(&sevenzip_path(), &output, &["SHA256"]).unwrap();
        let name = output.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(e) = manifest.entries.iter_mut().find(|e| e.archive == name) {
            e.sha256 = digest.into_iter().next().map(|d| d.hex).unwrap_or_default();
        }
        write_atomic(&manifest_path, manifest.to_json().as_bytes()).unwrap();
    }
    manifest
}

/// The read side: parse what's on disk, extract each with its recorded password.
fn extract_from_manifest(dir: &Path, manifest_text: &str) {
    let manifest = Manifest::parse(manifest_text).unwrap();
    assert_eq!(manifest.entries.len(), 3);
    for entry in &manifest.entries {
        assert_eq!(entry.password.len(), 64);
        assert_eq!(entry.sha256.len(), 64, "sha256 should have been filled in");
        let archive = dir.join(&entry.archive);
        assert!(archive.is_file(), "{} missing", archive.display());
        let dest = dir.join(format!("out-{}", entry.source));
        std::fs::create_dir_all(&dest).unwrap();
        let req = ExtractRequest {
            archive,
            dest_dir: dest.clone(),
            password: Some(entry.password.clone()),
            overwrite: OverwriteMode::default(),
            entries: Vec::new(),
        };
        run_extract(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();
        let extracted = dest.join(&entry.source).join("data.txt");
        let original = dir.join(&entry.source).join("data.txt");
        assert_eq!(
            std::fs::read(&extracted).unwrap(),
            std::fs::read(&original).unwrap(),
            "round-trip diff for {}",
            entry.source
        );
    }
}

#[test]
#[ignore = "spawns real 7zz"]
fn plain_manifest_batch_round_trips() {
    let dir = scratch("plain");
    let items = stage_sources(&dir);
    run_batch(&dir, &items);
    let text = std::fs::read_to_string(dir.join("passwords.json")).unwrap();
    extract_from_manifest(&dir, &text);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "spawns real 7zz and gpg"]
fn protected_manifest_batch_round_trips() {
    if !gpg_available() {
        eprintln!("skipping: no gpg on PATH");
        return;
    }
    let dir = scratch("gpg");
    let items = stage_sources(&dir);
    let manifest = run_batch(&dir, &items);

    // Protected mode: the encrypted file is what lands on disk.
    let protected = dir.join("passwords.json.gpg");
    let ct = encrypt_symmetric(manifest.to_json().as_bytes(), "batch master pass").unwrap();
    write_atomic(&protected, &ct).unwrap();
    std::fs::remove_file(dir.join("passwords.json")).unwrap();

    let bytes = std::fs::read(&protected).unwrap();
    assert!(septima_engine::looks_gpg_encrypted(&bytes));
    let plain = decrypt_symmetric(&bytes, "batch master pass").unwrap();
    extract_from_manifest(&dir, &String::from_utf8_lossy(&plain));
    let _ = std::fs::remove_dir_all(&dir);
}
