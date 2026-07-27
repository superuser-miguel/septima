//! Integration test: the create-dialog filters produce valid archives that
//! actually store the chosen filter, against a real `7zz`.
//!
//! Ignored by default. Run with:
//!   cargo test -p septima-engine --test real_filter -- --ignored --nocapture

use septima_engine::{filters, list_archive, new_cancel_token, run_add, sevenzip_path, CompressionRequest};

#[test]
#[ignore = "spawns real 7zz"]
fn every_offered_filter_creates_a_valid_archive() {
    let dir = std::env::temp_dir().join(format!("septima-filter-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A binary-ish payload so the executable filters have something to reorder.
    let input = dir.join("bin.dat");
    let bytes: Vec<u8> = (0..20_000u32).flat_map(|i| i.to_le_bytes()).collect();
    std::fs::write(&input, &bytes).unwrap();

    for f in filters().iter().filter(|f| !f.id.is_empty()) {
        let archive = dir.join(format!("f_{}.7z", f.id));
        let mut req = CompressionRequest::new(archive.clone(), vec![input.clone()], "7z");
        req.codec = Some("lzma2".into());
        req.filter = Some(f.id.to_string());
        run_add(&sevenzip_path(), &req, &new_cancel_token(), |_| {})
            .unwrap_or_else(|e| panic!("filter {} failed to create: {e:?}", f.id));

        // The archive must list, and the stored method must mention the filter.
        let listing = list_archive(&sevenzip_path(), &archive, None).unwrap();
        let method = listing
            .entries
            .iter()
            .find(|e| e.path == "bin.dat")
            .and_then(|e| e.method.clone())
            .unwrap_or_default();
        assert!(
            method.contains(f.id),
            "filter {} not reflected in stored method {method:?}",
            f.id
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
