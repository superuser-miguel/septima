//! The About window and the Keyboard Shortcuts dialog.
//!
//! About is built from the app's own AppStream metainfo (bundled into the
//! gresource as `metainfo.xml`), so its name, links, licence and "What's New"
//! page come from the same file the software centres read — release notes
//! are written once, in the metainfo, and never drift from what About shows.

use adw::prelude::*;

use crate::config;

fn gettext(s: &str) -> String {
    gettextrs::gettext(s)
}

pub fn show_about(parent: &impl IsA<gtk::Widget>) {
    let about = adw::AboutDialog::from_appdata(
        &format!("{}/metainfo.xml", config::GRESOURCE_PREFIX),
        Some(config::VERSION),
    );
    // The metainfo's version list tops out at the last release; a dev build
    // says what it really is.
    about.set_version(config::VERSION);
    about.set_developers(&["Miguel Almanzar https://github.com/superuser-miguel"]);
    about.add_credit_section(
        Some(&gettext("Built on")),
        &[
            "7-Zip ZS by Tino Reichardt https://github.com/mcmilk/7-Zip-zstd",
            "7-Zip by Igor Pavlov https://www.7-zip.org/",
        ],
    );
    about.add_legal_section(
        "7-Zip ZS",
        Some("© 1999 Igor Pavlov, © 2016 Tino Reichardt"),
        gtk::License::Custom,
        Some(&gettext(
            "Septima bundles the unmodified 7-Zip ZS command-line program (7zz) as a \
             separate binary and runs it for every archive operation. 7-Zip ZS is \
             licensed under the GNU LGPL 2.1 or later, with BSD-licensed parts for \
             the added codecs. Its RAR decoder carries the unRAR restriction: it may \
             not be used to recreate the RAR compression algorithm.",
        )),
    );

    // Troubleshooting details: which 7zz actually answered matters (a host
    // install may be a different build from the bundled one).
    let sevenzip = septima_engine::sevenzip_path();
    let banner = septima_engine::sevenzip_banner(&sevenzip)
        .unwrap_or_else(|| gettext("(couldn't be run)"));
    about.set_debug_info(&format!(
        "Septima {}\nApp ID: {}\nProfile: {}\n7zz: {}\n{}\nlibadwaita {}.{}.{} · GTK {}.{}.{}",
        config::VERSION,
        config::APP_ID,
        config::PROFILE,
        sevenzip.display(),
        banner,
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
    ));
    about.present(Some(parent));
}

/// Where a shortcut is bound, which decides when it fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Application accelerator (main.rs): fires anywhere in the window. Only
    /// for chorded keys and function keys — an app accelerator is seen before
    /// a focused text field, so any other bare key would be stolen from every
    /// entry.
    App,
    /// Bound on the archive's file list (archive_view.rs): fires only while
    /// the list has focus, so Delete and F2 keep working in text fields.
    ArchiveList,
    /// Provided by GTK or libadwaita itself; listed so it can be found.
    Builtin,
}

/// Every shortcut, in the order the Keyboard Shortcuts dialog shows them.
/// This one table drives the bindings and the dialog, so a key can't be bound
/// without being listed, or listed without being bound.
pub const SHORTCUTS: &[(&str, &str, Scope)] = &[
    ("win.open", "<Ctrl>o", Scope::App),
    ("win.new-archive", "<Ctrl>n", Scope::App),
    ("win.checksums", "<Ctrl>h", Scope::App),
    ("app.capabilities", "F1", Scope::App),
    ("app.shortcuts", "<Ctrl>question", Scope::App),
    ("menu", "F10", Scope::Builtin),
    ("app.quit", "<Ctrl>q", Scope::App),
    ("win.extract", "<Ctrl>e", Scope::App),
    ("win.test-archive", "<Ctrl>t", Scope::App),
    ("win.close-archive", "<Ctrl>w", Scope::App),
    ("select-all", "<Ctrl>a", Scope::Builtin),
    ("win.rename-entry", "F2", Scope::ArchiveList),
    ("win.delete-entries", "Delete", Scope::ArchiveList),
];

/// The actions that only make sense with an archive open; they get their own
/// section in the dialog.
fn in_archive_section(action: &str) -> bool {
    matches!(
        action,
        "win.extract" | "win.test-archive" | "win.close-archive" | "select-all" | "win.rename-entry" | "win.delete-entries"
    )
}

fn shortcut_title(action: &str) -> String {
    match action {
        "win.open" => gettext("Open Archive"),
        "win.new-archive" => gettext("New Archive"),
        "win.checksums" => gettext("Calculate Checksums"),
        "app.capabilities" => gettext("What Septima Can Do"),
        "app.shortcuts" => gettext("Keyboard Shortcuts"),
        "menu" => gettext("Main Menu"),
        "app.quit" => gettext("Quit"),
        "win.extract" => gettext("Extract"),
        "win.test-archive" => gettext("Test Archive"),
        "win.close-archive" => gettext("Close Archive"),
        "select-all" => gettext("Select All Entries"),
        "win.rename-entry" => gettext("Rename Entry"),
        "win.delete-entries" => gettext("Delete Entries"),
        _ => action.to_owned(),
    }
}

pub fn show_shortcuts(parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::ShortcutsDialog::new();
    let general = adw::ShortcutsSection::new(Some(&gettext("General")));
    let archive = adw::ShortcutsSection::new(Some(&gettext("Open Archive")));
    for (action, accel, _) in SHORTCUTS {
        let section = if in_archive_section(action) { &archive } else { &general };
        section.add(adw::ShortcutsItem::new(&shortcut_title(action), accel));
    }
    dialog.add(general);
    dialog.add(archive);
    dialog.present(Some(parent));
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn every_shortcut_has_a_title_and_a_unique_key() {
        let mut keys = std::collections::HashSet::new();
        for (action, accel, _) in SHORTCUTS {
            assert_ne!(shortcut_title(action), *action, "{action} has no title");
            assert!(keys.insert(*accel), "{accel} is bound twice");
        }
    }

    #[test]
    fn bare_keys_are_never_application_wide() {
        // An app accelerator fires before a focused text field sees the key,
        // so a bare key there would be stolen from every entry. Function keys
        // type nothing, so they're allowed.
        let function_key = |a: &str| a.strip_prefix('F').is_some_and(|n| n.parse::<u8>().is_ok());
        for (action, accel, scope) in SHORTCUTS {
            if *scope == Scope::App {
                assert!(
                    accel.starts_with('<') || function_key(accel),
                    "{action}: bare key {accel} must not be Scope::App"
                );
            }
        }
    }
}
