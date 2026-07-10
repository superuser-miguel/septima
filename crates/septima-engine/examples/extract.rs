//! Dev tool: `cargo run -p septima-engine --example extract -- <archive> <dest>`
//! Extracts an archive, printing live progress.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(archive), Some(dest)) = (args.next(), args.next()) else {
        eprintln!("usage: extract <archive> <dest-dir>");
        std::process::exit(2);
    };

    let sevenzip = septima_engine::sevenzip_path();
    let req = septima_engine::ExtractRequest::new(PathBuf::from(archive), PathBuf::from(dest));
    let cancel = septima_engine::new_cancel_token();

    let result = septima_engine::run_extract(&sevenzip, &req, &cancel, |p| {
        print!(
            "\r{:>3}%  {}          ",
            p.percent.unwrap_or(0),
            p.current_file.as_deref().unwrap_or("")
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();
    });

    println!();
    match result {
        Ok(()) => println!("done"),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
