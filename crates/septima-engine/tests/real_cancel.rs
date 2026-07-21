//! Integration test: cancelling a *real* `7zz` job kills it promptly.
//!
//! Ignored by default (spawns 7zz on a large input, needs `SEPTIMA_7ZZ` or a
//! built-in path). Run with:
//!   SEPTIMA_7ZZ=/usr/local/sbin/7zz cargo test -p septima-engine --test real_cancel -- --ignored --nocapture

use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use septima_engine::{new_cancel_token, run_add, sevenzip_path, CompressionRequest, EngineError};

#[test]
#[ignore = "spawns real 7zz on a large input; run with --ignored"]
fn real_7zz_creation_cancels_promptly() {
    // A big, incompressible input so 7zz stays busy long enough to cancel mid-run.
    let dir = std::env::temp_dir().join(format!("septima-cancel-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("big.bin");
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&input).unwrap());
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut buf = vec![0u8; 1 << 20]; // 1 MiB
        for _ in 0..256 {
            // 256 MiB of xorshift bytes (incompressible enough to keep lzma2 busy)
            for b in buf.iter_mut() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = x as u8;
            }
            f.write_all(&buf).unwrap();
        }
        f.flush().unwrap();
    }

    let output = dir.join("out.7z");
    let mut req = CompressionRequest::new(output.clone(), vec![input], "7z");
    req.codec = Some("lzma2".into());
    req.level = Some(6);
    req.threads = Some(2);

    let sevenzip = sevenzip_path();
    let cancel = new_cancel_token();
    let cancel_bg = cancel.clone();

    let handle = std::thread::spawn(move || run_add(&sevenzip, &req, &cancel_bg, |_| {}));

    // Let 7zz get going, then cancel and time how long it takes to unwind.
    std::thread::sleep(Duration::from_millis(400));
    let t = Instant::now();
    cancel.store(true, Ordering::Relaxed);
    let result = handle.join().unwrap();
    let elapsed = t.elapsed();

    let partial_left = output.exists();
    let _ = std::fs::remove_dir_all(&dir);

    println!("cancel -> return in {elapsed:?}; result: {result:?}");
    assert!(
        matches!(result, Err(EngineError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "cancel took too long: {elapsed:?}"
    );
    assert!(
        !partial_left,
        "cancel left a partial archive behind at {}",
        output.display()
    );
}
