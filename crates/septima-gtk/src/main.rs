//! Septima — GTK4 / libadwaita frontend for 7-Zip ZS (`7zz`).
//!
//! Placeholder entry point (Task A1). Task A2 replaces this with an
//! `adw::Application` whose main window is an `adw::ApplicationWindow` subclass
//! bound via `#[derive(gtk::CompositeTemplate)]` to the `window.blp` resource.

fn main() {
    // Proves the engine crate is wired in; real app skeleton is Task A2.
    println!(
        "Septima {} — drives `{}`. GTK application skeleton lands in Task A2.",
        env!("CARGO_PKG_VERSION"),
        septima_engine::SEVENZIP_BIN,
    );
}
