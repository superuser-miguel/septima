//! Dev tool: `cargo run -p septima-engine --example list -- <archive>`
//! Lists an archive via the engine (drives the real `7zz`).

use std::path::PathBuf;

fn main() {
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: list <archive>");
        std::process::exit(2);
    };
    let sevenzip = septima_engine::sevenzip_path();
    let password = std::env::args().nth(2);
    match septima_engine::list_archive(&sevenzip, &PathBuf::from(arg), password.as_deref()) {
        Ok(listing) => {
            println!(
                "format={:?}  files={}  total={} bytes",
                listing.format,
                listing.file_count(),
                listing.total_size()
            );
            for e in &listing.entries {
                println!(
                    "  {}{}\tsize={} packed={:?} method={:?} crc={:?}",
                    if e.is_dir { "[dir] " } else { "" },
                    e.path,
                    e.size,
                    e.packed_size,
                    e.method,
                    e.crc
                );
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
