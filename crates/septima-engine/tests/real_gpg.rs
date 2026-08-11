//! Integration tests: the symmetric manifest encryption against a real `gpg`.
//!
//! Ignored by default (spawns gpg). Run with:
//!   cargo test -p septima-engine --test real_gpg -- --ignored --nocapture

use septima_engine::{
    decrypt_symmetric, encrypt_symmetric, gpg_available, looks_gpg_encrypted, GpgError, Manifest,
    ManifestEntry,
};

fn sample_manifest() -> Manifest {
    let mut m = Manifest::new();
    m.septima = "0.5.0".into();
    m.created = "2026-08-11T20:00:00Z".into();
    m.push(ManifestEntry {
        archive: "photos.7z".into(),
        source: "photos/".into(),
        password: "N0tAReal64CharPasswordButLongEnoughToCrossPipeBufferConcerns".into(),
        sha256: "3f9a".into(),
        created: "2026-08-11T20:00:01Z".into(),
        encryption: "7z, AES-256, encrypted headers".into(),
    });
    m
}

#[test]
#[ignore = "spawns real gpg"]
fn manifest_round_trips_through_gpg() {
    if !gpg_available() {
        eprintln!("skipping: no gpg on PATH");
        return;
    }
    let json = sample_manifest().to_json();
    let ct = encrypt_symmetric(json.as_bytes(), "hunter2 but longer").unwrap();
    assert!(looks_gpg_encrypted(&ct), "ciphertext should be detectable");
    assert!(
        !ct.windows(9).any(|w| w == b"photos.7z"),
        "plaintext must not leak into the ciphertext"
    );
    let pt = decrypt_symmetric(&ct, "hunter2 but longer").unwrap();
    let back = Manifest::from_json(std::str::from_utf8(&pt).unwrap()).unwrap();
    assert_eq!(back, sample_manifest());
}

#[test]
#[ignore = "spawns real gpg"]
fn wrong_passphrase_is_distinguished() {
    if !gpg_available() {
        eprintln!("skipping: no gpg on PATH");
        return;
    }
    let ct = encrypt_symmetric(b"secret data", "right").unwrap();
    match decrypt_symmetric(&ct, "wrong") {
        Err(GpgError::WrongPassphrase) => {}
        other => panic!("expected WrongPassphrase, got {other:?}"),
    }
}

#[test]
#[ignore = "spawns real gpg"]
fn large_manifest_does_not_deadlock() {
    if !gpg_available() {
        eprintln!("skipping: no gpg on PATH");
        return;
    }
    // Well past any pipe buffer (64 KiB on Linux) in both directions.
    let mut m = Manifest::new();
    for i in 0..4000 {
        m.push(ManifestEntry {
            archive: format!("archive_{i}.7z"),
            source: format!("dir_{i}/"),
            password: "x".repeat(64),
            ..Default::default()
        });
    }
    let json = m.to_json();
    assert!(json.len() > 512 * 1024);
    let ct = encrypt_symmetric(json.as_bytes(), "big pass").unwrap();
    let pt = decrypt_symmetric(&ct, "big pass").unwrap();
    assert_eq!(pt, json.as_bytes());
}
