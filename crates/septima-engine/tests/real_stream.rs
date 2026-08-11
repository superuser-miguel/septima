//! Integration test: raw single-file stream creation (parity #5) against real
//! `7zz`. Mirrors what the create dialog builds for the "Single file (raw
//! stream)" format — `-t<codec>` on one file, no container.
//!
//! Ignored by default. Run with:
//!   cargo test -p septima-engine --test real_stream -- --ignored --nocapture

use septima_engine::{
    new_cancel_token, run_add, run_extract, run_tar_and_compress, sevenzip_path, stream_extension,
    CompressionRequest, ExtractRequest, OverwriteMode,
};

const STREAM_CODECS: &[&str] = &["zstd", "xz", "gzip", "bzip2", "brotli", "lz4", "lz5"];

#[test]
#[ignore = "spawns real 7zz"]
fn every_stream_codec_round_trips_one_file() {
    let dir = std::env::temp_dir().join(format!("septima-stream-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let payload: Vec<u8> = (0..50_000u32).flat_map(|i| i.to_le_bytes()).collect();
    let input = dir.join("data.bin");
    std::fs::write(&input, &payload).unwrap();

    for codec in STREAM_CODECS {
        let ext = stream_extension(codec);
        let archive = dir.join(format!("out.{ext}"));

        // Exactly how compression_request builds a raw stream: format = codec.
        let mut req = CompressionRequest::new(archive.clone(), vec![input.clone()], *codec);
        req.level = Some(3.min(level_cap(codec)));
        req.threads = Some(2);
        run_add(&sevenzip_path(), &req, &new_cancel_token(), |_| {})
            .unwrap_or_else(|e| panic!("{codec}: create failed: {e:?}"));
        assert!(archive.exists(), "{codec}: no output written");
        let compressed = std::fs::metadata(&archive).unwrap().len();
        assert!(compressed > 0, "{codec}: empty output");

        // Extract it back and confirm the bytes match the original.
        let dest = dir.join(format!("out-{codec}"));
        std::fs::create_dir_all(&dest).unwrap();
        let ereq = ExtractRequest {
            archive: archive.clone(),
            dest_dir: dest.clone(),
            password: None,
            overwrite: OverwriteMode::default(),
            entries: Vec::new(),
        };
        run_extract(&sevenzip_path(), &ereq, &new_cancel_token(), |_| {})
            .unwrap_or_else(|e| panic!("{codec}: extract failed: {e:?}"));
        // Exactly one file lands; its name varies (gzip restores the original
        // name from its header, others use the archive stem), so match by content.
        let files: Vec<_> = std::fs::read_dir(&dest)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .collect();
        assert_eq!(files.len(), 1, "{codec}: expected one extracted file, got {}", files.len());
        let got = std::fs::read(files[0].path()).unwrap();
        assert_eq!(got, payload, "{codec}: round-trip mismatch");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

fn level_cap(codec: &str) -> u8 {
    match codec {
        "xz" | "gzip" | "bzip2" => 9,
        _ => 12,
    }
}

/// tar→brotli goes through the same `-tbrotli` path as a raw stream, so with
/// threads it used to produce a corrupt `.tar.br` (the v0.3.0 shipped bug).
/// Building with threads set must now yield an archive that passes integrity.
#[test]
#[ignore = "spawns real 7zz"]
fn tar_brotli_with_threads_is_not_corrupt() {
    let dir = std::env::temp_dir().join(format!("septima-tarbr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 256) as u8).collect();
    std::fs::write(dir.join("a.bin"), &payload).unwrap();
    std::fs::write(dir.join("b.bin"), &payload).unwrap();

    let archive = dir.join("out.tar.br");
    let mut req = CompressionRequest::new(archive.clone(), vec![dir.join("a.bin"), dir.join("b.bin")], "tar");
    req.codec = Some("brotli".into());
    req.level = Some(5);
    req.threads = Some(8); // the corrupting condition
    run_tar_and_compress(&sevenzip_path(), &req, &new_cancel_token(), |_| {}).unwrap();

    // Extract and confirm the inner tar comes back whole.
    let dest = dir.join("out");
    std::fs::create_dir_all(&dest).unwrap();
    let ereq = ExtractRequest {
        archive,
        dest_dir: dest.clone(),
        password: None,
        overwrite: OverwriteMode::default(),
        entries: Vec::new(),
    };
    run_extract(&sevenzip_path(), &ereq, &new_cancel_token(), |_| {})
        .expect("tar.br extract failed — corrupt stream");
    assert_eq!(std::fs::read(dest.join("a.bin")).unwrap(), payload);
    assert_eq!(std::fs::read(dest.join("b.bin")).unwrap(), payload);

    std::fs::remove_dir_all(&dir).unwrap();
}
