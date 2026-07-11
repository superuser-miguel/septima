//! Dev tool: `cargo run -p septima-engine --example hash -- <file>`

use std::path::PathBuf;

fn main() {
    let Some(file) = std::env::args().nth(1) else {
        eprintln!("usage: hash <file>");
        std::process::exit(2);
    };
    let sevenzip = septima_engine::sevenzip_path();
    let algos: Vec<&str> = septima_engine::hash_algorithms().iter().map(|a| a.switch).collect();

    match septima_engine::hash_file(&sevenzip, &PathBuf::from(file), &algos) {
        Ok(digests) => {
            for d in digests {
                println!("{:<10} {}", d.algo, d.hex);
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
