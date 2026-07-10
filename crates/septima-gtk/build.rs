//! Build script for `septima-gtk` (plain-cargo path).
//!
//! Does what Meson will do in Task A3, so the app is runnable via `cargo run`
//! today:
//!   1. compiles Blueprint `.blp` -> `.ui` (Work Area B1),
//!   2. bundles `.ui` + `style.css` into a `.gresource`,
//!   3. generates `config.rs` from `config.rs.in`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // crates/septima-gtk -> workspace root
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("crate is expected two levels below the workspace root")
        .to_path_buf();
    let data_dir = workspace_root.join("data");
    let ui_dir = data_dir.join("ui");

    // --- 1. Blueprint: window.blp -> $OUT_DIR/window.ui ---
    let blp = ui_dir.join("window.blp");
    let ui_out = out_dir.join("window.ui");
    run(
        Command::new("blueprint-compiler")
            .arg("compile")
            .arg(&blp)
            .arg("--output")
            .arg(&ui_out),
        "blueprint-compiler",
    );

    // --- 2. gresource bundle: stage inputs in $OUT_DIR, then compile ---
    let style_src = data_dir.join("style.css");
    fs::copy(&style_src, out_dir.join("style.css")).expect("copy style.css");
    let gresource_xml = out_dir.join("resources.gresource.xml");
    fs::copy(data_dir.join("resources.gresource.xml"), &gresource_xml)
        .expect("copy resources.gresource.xml");
    let gresource_out = out_dir.join("septima.gresource");
    run(
        Command::new("glib-compile-resources")
            .arg("--sourcedir")
            .arg(&out_dir)
            .arg("--target")
            .arg(&gresource_out)
            .arg(&gresource_xml),
        "glib-compile-resources",
    );

    // --- 3. config.rs from config.rs.in ---
    // Base identity; the `.Devel` app-id suffix arrives with Task A4.
    let app_id = "io.github.superuser_miguel.Septima";
    let gresource_prefix = "/io/github/superuser_miguel/Septima";
    let gettext_package = "septima";
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let profile = env::var("SEPTIMA_PROFILE").unwrap_or_else(|_| "Devel".to_string());
    let localedir = env::var("SEPTIMA_LOCALEDIR").unwrap_or_else(|_| "/usr/share/locale".to_string());

    let template_path = manifest_dir.join("src/config.rs.in");
    let config = fs::read_to_string(&template_path)
        .expect("read config.rs.in")
        .replace("@APP_ID@", app_id)
        .replace("@VERSION@", &version)
        .replace("@PROFILE@", &profile)
        .replace("@GRESOURCE_PREFIX@", gresource_prefix)
        .replace("@GETTEXT_PACKAGE@", gettext_package)
        .replace("@LOCALEDIR@", &localedir);
    fs::write(out_dir.join("config.rs"), config).expect("write config.rs");

    // --- rerun triggers ---
    for p in [&blp, &style_src, &data_dir.join("resources.gresource.xml"), &template_path] {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    println!("cargo:rerun-if-env-changed=SEPTIMA_PROFILE");
    println!("cargo:rerun-if-env-changed=SEPTIMA_LOCALEDIR");
}

fn run(cmd: &mut Command, tool: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {tool}: {e}"));
    assert!(status.success(), "{tool} exited with {status}");
}
