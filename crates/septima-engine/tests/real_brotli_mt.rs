//! Integration test: reading *foreign* multithreaded-brotli streams.
//!
//! `7zz` writes two mutually unreadable standalone brotli formats depending on
//! whether `-mmt` was passed at creation, and brotli has no header recording
//! which one a stream is. Septima never *writes* the mt form (compress.rs drops
//! `-mmt` for `-tbrotli`), but users open archives made elsewhere — so the read
//! paths retry once in the other mode.
//!
//! These fixtures are therefore built by invoking `7zz` directly, not through
//! the engine: the engine cannot produce the input this test needs.
//!
//! Ignored by default. Run with:
//!   cargo test -p septima-engine --test real_brotli_mt -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use septima_engine::{
    list_archive, new_cancel_token, run_extract, sevenzip_path, ExtractRequest, OverwriteMode,
};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("septima-brmt-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn payload() -> Vec<u8> {
    (0..300_000u32).map(|i| (i % 251) as u8).collect()
}

/// Build a standalone brotli stream with `7zz` directly. `mt` selects the
/// multithreaded encoding — the one Septima deliberately never writes.
fn make_brotli(archive: &Path, input: &Path, mt: bool) {
    let mut cmd = Command::new(sevenzip_path());
    cmd.arg("a").arg("-tbrotli").arg("-mx5");
    if mt {
        cmd.arg("-mmt4");
    }
    let status = cmd
        .arg("--")
        .arg(archive)
        .arg(input)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn 7zz");
    assert!(status.success(), "fixture creation failed (mt={mt})");
}

fn extract_to(archive: &Path, dest: &Path) -> Result<(), septima_engine::EngineError> {
    std::fs::create_dir_all(dest).unwrap();
    let req = ExtractRequest {
        archive: archive.to_path_buf(),
        dest_dir: dest.to_path_buf(),
        password: None,
        overwrite: OverwriteMode::default(),
    };
    run_extract(&sevenzip_path(), &req, &new_cancel_token(), |_| {})
}

/// A raw `.br` written with `-mmt` fails a default-mode decode; the engine must
/// recover it via the retry. Both encodings have to work through one call.
#[test]
#[ignore = "spawns real 7zz"]
fn raw_br_extracts_in_either_mode() {
    let dir = scratch("raw");
    let data = payload();
    let input = dir.join("data.bin");
    std::fs::write(&input, &data).unwrap();

    for (tag, mt) in [("mt", true), ("plain", false)] {
        let archive = dir.join(format!("{tag}.br"));
        make_brotli(&archive, &input, mt);

        let dest = dir.join(format!("out-{tag}"));
        extract_to(&archive, &dest).unwrap_or_else(|e| panic!("{tag}: extract failed: {e:?}"));

        let files: Vec<_> = std::fs::read_dir(&dest)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .collect();
        assert_eq!(files.len(), 1, "{tag}: expected one extracted file");
        assert_eq!(std::fs::read(files[0].path()).unwrap(), data, "{tag}: bytes differ");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The same for `.tar.br`, which goes through the two-stage pipe instead — and
/// covers browsing as well as extracting, since both decode the outer stream.
#[test]
#[ignore = "spawns real 7zz"]
fn mt_tar_br_browses_and_extracts() {
    let dir = scratch("tar");
    let data = payload();
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.bin"), &data).unwrap();
    std::fs::write(src.join("b.bin"), &data).unwrap();

    // tar it, then compress that tar with multithreaded brotli.
    let tar = dir.join("bundle.tar");
    let status = Command::new(sevenzip_path())
        .arg("a")
        .arg("-ttar")
        .arg("--")
        .arg(&tar)
        .arg(&src)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("spawn 7zz");
    assert!(status.success(), "tar fixture failed");

    let archive = dir.join("bundle.tar.br");
    make_brotli(&archive, &tar, true);

    // Browse: must show the real files, not an empty archive.
    let listing = list_archive(&sevenzip_path(), &archive, None).expect("mt .tar.br list failed");
    let names: Vec<_> = listing.entries.iter().map(|e| e.path.clone()).collect();
    assert!(
        names.iter().any(|n| n.ends_with("a.bin")) && names.iter().any(|n| n.ends_with("b.bin")),
        "mt .tar.br listed wrong contents: {names:?}"
    );

    // Extract: the files land, byte-identical.
    let dest = dir.join("out");
    extract_to(&archive, &dest).expect("mt .tar.br extract failed");
    assert_eq!(std::fs::read(dest.join("src/a.bin")).unwrap(), data);
    assert_eq!(std::fs::read(dest.join("src/b.bin")).unwrap(), data);

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Brotli inside a `.7z` splits the same way, but in the opposite direction: a
/// container decodes as the threaded format unless `-mmt=off` is passed, so an
/// archive written with `-t7z -m0=brotli -mmt=off` is unreadable without it.
/// Septima never writes those — brotli in a container keeps its threads — but
/// they exist in the wild, and upstream deliberately kept the fix behind the
/// flag rather than probing by default (mcmilk/7-Zip-zstd#538), so the retry
/// has to be ours. Both browsing and extracting must recover it.
#[test]
#[ignore = "spawns real 7zz"]
fn foreign_raw_brotli_in_a_7z_browses_and_extracts() {
    let dir = scratch("sevenz");
    let data = payload();
    let input = dir.join("data.bin");
    std::fs::write(&input, &data).unwrap();

    let archive = dir.join("foreign.7z");
    let status = Command::new(sevenzip_path())
        .args(["a", "-t7z", "-m0=brotli", "-mmt=off", "--"])
        .arg(&archive)
        .arg(&input)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawn 7zz");
    assert!(status.success(), "raw-brotli .7z fixture failed");

    let listing = list_archive(&sevenzip_path(), &archive, None)
        .expect("raw-brotli .7z list failed — the -mmt=off retry did not fire");
    assert!(
        listing.entries.iter().any(|e| e.path.ends_with("data.bin")),
        "listed wrong contents: {:?}",
        listing.entries.iter().map(|e| &e.path).collect::<Vec<_>>()
    );

    let dest = dir.join("out");
    extract_to(&archive, &dest).expect("raw-brotli .7z extract failed");
    assert_eq!(std::fs::read(dest.join("data.bin")).unwrap(), data);

    std::fs::remove_dir_all(&dir).unwrap();
}

/// A genuinely broken compressed tar must surface as an error. The inner
/// listing stage exits 0 on a truncated stream, so without checking the outer
/// decompressor's status this silently reported an empty archive.
#[test]
#[ignore = "spawns real 7zz"]
fn corrupt_tar_br_errors_rather_than_listing_empty() {
    let dir = scratch("corrupt");
    let archive = dir.join("broken.tar.br");
    std::fs::write(&archive, b"this is not a brotli stream at all").unwrap();

    let result = list_archive(&sevenzip_path(), &archive, None);
    assert!(result.is_err(), "corrupt .tar.br listed as {result:?} instead of erroring");

    std::fs::remove_dir_all(&dir).unwrap();
}
