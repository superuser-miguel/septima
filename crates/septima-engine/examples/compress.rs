//! Dev tool: `cargo run -p septima-engine --example compress -- <format> <codec|-> <level|-> <output> <inputs...>`

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: compress <format> <codec|-> <level|-> <output> <inputs...>");
        std::process::exit(2);
    }

    let sevenzip = septima_engine::sevenzip_path();
    let inputs = args[4..].iter().map(PathBuf::from).collect();
    let mut req = septima_engine::CompressionRequest::new(&args[3], inputs, &args[0]);
    if args[1] != "-" {
        req.codec = Some(args[1].clone());
    }
    if args[2] != "-" {
        req.level = args[2].parse().ok();
    }

    let cancel = septima_engine::new_cancel_token();
    let progress = |p: &septima_engine::ExtractProgress| {
        print!("\r{:>3}%  {}    ", p.percent.unwrap_or(0), p.current_file.as_deref().unwrap_or(""));
        use std::io::Write;
        let _ = std::io::stdout().flush();
    };
    // tar + a real compressor -> two-step .tar.<ext>
    let result = if args[0] == "tar" && req.codec.as_deref().is_some_and(|c| c != "copy") {
        septima_engine::run_tar_and_compress(&sevenzip, &req, &cancel, progress)
    } else {
        septima_engine::run_add(&sevenzip, &req, &cancel, progress)
    };

    println!();
    match result {
        Ok(()) => println!("created {}", args[3]),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
