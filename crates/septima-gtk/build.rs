//! Build script for `septima-gtk`.
//!
//! Two modes, distinguished by whether Meson set `SEPTIMA_PKGDATADIR`:
//!
//!  * Plain `cargo` (dev): compiles Blueprint `.blp` -> `.ui`, bundles the
//!    gresource into `OUT_DIR`, and generates `config.rs` with `PKGDATADIR`
//!    pointing at `OUT_DIR`, so the app loads its resources from there.
//!  * Under Meson: Meson compiles/installs the gresource itself, so this script
//!    only generates `config.rs` from the Meson-provided environment.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE_ID: &str = "io.github.superuser_miguel.Septima";
const GRESOURCE_PREFIX: &str = "/io/github/superuser_miguel/Septima";
const GETTEXT_PACKAGE: &str = "septima";

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("crate is two levels below the workspace root")
        .to_path_buf();
    let data_dir = workspace_root.join("data");

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let profile = env::var("SEPTIMA_PROFILE").unwrap_or_else(|_| "Devel".to_string());
    let localedir =
        env::var("SEPTIMA_LOCALEDIR").unwrap_or_else(|_| "/usr/share/locale".to_string());
    let app_id = env::var("SEPTIMA_APP_ID").unwrap_or_else(|_| {
        if profile == "Devel" {
            format!("{BASE_ID}.Devel")
        } else {
            BASE_ID.to_string()
        }
    });

    // Meson tells us where it installs resources; its absence means cargo-dev.
    let meson_pkgdatadir = env::var("SEPTIMA_PKGDATADIR").ok();
    let pkgdatadir = match &meson_pkgdatadir {
        Some(dir) => dir.clone(),
        None => {
            compile_resources(&data_dir, &out_dir);
            out_dir.to_string_lossy().into_owned()
        }
    };

    // config.rs is always generated here (single `include!` path for main.rs).
    let template = fs::read_to_string(manifest_dir.join("src/config.rs.in")).unwrap();
    let config = template
        .replace("@APP_ID@", &app_id)
        .replace("@VERSION@", &version)
        .replace("@PROFILE@", &profile)
        .replace("@GRESOURCE_PREFIX@", GRESOURCE_PREFIX)
        .replace("@GETTEXT_PACKAGE@", GETTEXT_PACKAGE)
        .replace("@LOCALEDIR@", &localedir)
        .replace("@PKGDATADIR@", &pkgdatadir);
    fs::write(out_dir.join("config.rs"), config).expect("write config.rs");

    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/config.rs.in").display()
    );
    for p in ["ui/window.blp", "style.css", "resources.gresource.xml"] {
        println!("cargo:rerun-if-changed={}", data_dir.join(p).display());
    }
    for v in [
        "SEPTIMA_APP_ID",
        "SEPTIMA_PROFILE",
        "SEPTIMA_LOCALEDIR",
        "SEPTIMA_PKGDATADIR",
    ] {
        println!("cargo:rerun-if-env-changed={v}");
    }
}

/// cargo-dev only: compile the Blueprint and bundle the gresource into OUT_DIR.
fn compile_resources(data_dir: &Path, out_dir: &Path) {
    run(
        Command::new("blueprint-compiler")
            .arg("compile")
            .arg(data_dir.join("ui/window.blp"))
            .arg("--output")
            .arg(out_dir.join("window.ui")),
        "blueprint-compiler",
    );
    fs::copy(data_dir.join("style.css"), out_dir.join("style.css")).expect("copy style.css");
    let gresource_xml = out_dir.join("resources.gresource.xml");
    fs::copy(data_dir.join("resources.gresource.xml"), &gresource_xml)
        .expect("copy resources.gresource.xml");
    run(
        Command::new("glib-compile-resources")
            .arg("--sourcedir")
            .arg(out_dir)
            .arg("--target")
            .arg(out_dir.join("septima.gresource"))
            .arg(&gresource_xml),
        "glib-compile-resources",
    );
}

fn run(cmd: &mut Command, tool: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {tool}: {e}"));
    assert!(status.success(), "{tool} exited with {status}");
}
