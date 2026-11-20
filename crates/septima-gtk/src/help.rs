//! "What Septima Can Do": what this release can create, every `7zz` command and
//! switch it sends, and the behaviour worth knowing about.
//!
//! Built from data, not prose about the data: the formats page reads the same
//! tables as the New Archive dialog (`septima_engine::capabilities`), and the
//! switches page reads `septima_engine::switches`, whose test fails the build
//! if it drifts from the commands the engine really builds. Only the "Good to
//! know" notes are hand-written, and each one describes code that exists.

use adw::prelude::*;

use septima_engine::capabilities::{encryption_methods, filters, formats};
use septima_engine::switches::{Group, ADVANCED_EXAMPLES, COMMANDS, SWITCHES};

use crate::config;

fn gettext(s: &str) -> String {
    gettextrs::gettext(s)
}

pub fn present(parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::PreferencesDialog::builder()
        .title(gettext("What Septima Can Do"))
        .search_enabled(true)
        .build();
    dialog.add(&formats_page());
    dialog.add(&switches_page());
    dialog.add(&good_to_know_page());
    dialog.present(Some(parent));
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(glib_escape(title))
        .subtitle(glib_escape(subtitle))
        .subtitle_lines(0)
        .build()
}

/// Row titles and subtitles are Pango markup; the data has `<` and `&` in it.
fn glib_escape(s: &str) -> String {
    gtk::glib::markup_escape_text(s).to_string()
}

fn mono_suffix(row: &adw::ActionRow, text: &str) {
    row.add_suffix(
        &gtk::Label::builder()
            .label(text)
            .css_classes(["monospace", "dim-label"])
            .valign(gtk::Align::Center)
            .wrap(true)
            .max_width_chars(18)
            .xalign(1.0)
            .build(),
    );
}

fn formats_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("formats")
        .title(gettext("Formats"))
        .icon_name("package-x-generic-symbolic")
        .description(gettext("Everything Septima can create, with the levels each method accepts."))
        .build();

    for format in formats() {
        let title = match format.id {
            "tar" => gettext("Tar, plain or compressed"),
            "stream" => gettext("Single file (raw stream)"),
            _ => format.label.to_string(),
        };
        let mut notes = Vec::new();
        if format.supports_solid {
            notes.push(gettext("solid mode"));
        }
        if format.supports_encryption {
            notes.push(gettext("passwords"));
        }
        if format.supports_header_encryption {
            notes.push(gettext("encrypted file names"));
        }
        let group = adw::PreferencesGroup::builder().title(glib_escape(&title)).build();
        if !notes.is_empty() {
            group.set_description(Some(&glib_escape(&format!("{} {}", gettext("Also:"), notes.join(", ")))));
        }
        for codec in format.codecs {
            let label = if format.id == "tar" && codec.is_store() {
                gettext("None (plain .tar)")
            } else {
                codec.label.to_string()
            };
            let detail = if codec.is_store() {
                gettext("No compression")
            } else if codec.id == "lizard" {
                format!(
                    "{} {}–{} {} · {} {}",
                    gettext("Levels"),
                    codec.level_min,
                    codec.level_max,
                    gettext("in each of four families"),
                    gettext("default"),
                    codec.default_level
                )
            } else {
                format!(
                    "{} {}–{} · {} {}",
                    gettext("Levels"),
                    codec.level_min,
                    codec.level_max,
                    gettext("default"),
                    codec.default_level
                )
            };
            let r = row(&label, &detail);
            if format.id == "stream" {
                mono_suffix(&r, &format!(".{}", septima_engine::stream_extension(codec.id)));
            }
            group.add(&r);
        }
        page.add(&group);
    }

    let filter_group = adw::PreferencesGroup::builder()
        .title(gettext("Filters (7z)"))
        .description(gettext("Run before the method, to make some kinds of data compress better."))
        .build();
    for f in filters().iter().filter(|f| !f.id.is_empty()) {
        let r = row(f.label, "");
        mono_suffix(&r, f.id);
        filter_group.add(&r);
    }
    page.add(&filter_group);

    let enc_group = adw::PreferencesGroup::builder()
        .title(gettext("Encryption"))
        .description(gettext("What this copy of 7zz offers, checked when Septima starts."))
        .build();
    for format in formats().iter().filter(|f| f.supports_encryption) {
        let names: Vec<&str> = encryption_methods(format.id).iter().map(|m| m.label).collect();
        enc_group.add(&row(format.label, &names.join(" · ")));
    }
    page.add(&enc_group);
    page
}

fn group_title(g: Group) -> String {
    match g {
        Group::Create => gettext("Creating archives"),
        Group::Extract => gettext("Opening and extracting"),
        Group::Edit => gettext("Checksums"),
        Group::Plumbing => gettext("How Septima talks to 7zz"),
    }
}

fn switches_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("switches")
        .title(gettext("7zz Switches"))
        .icon_name("utilities-terminal-symbolic")
        .description(gettext(
            "Septima does its work by running 7zz, the 7-Zip ZS command-line program. \
             These are all the commands and switches it sends, and what sets each one.",
        ))
        .build();

    let commands = adw::PreferencesGroup::builder().title(gettext("Commands")).build();
    for c in COMMANDS {
        let r = row(c.title, c.used_by);
        mono_suffix(&r, if c.name.is_empty() { "7zz" } else { c.name });
        commands.add(&r);
    }
    page.add(&commands);

    for g in Group::ORDER {
        let group = adw::PreferencesGroup::builder().title(glib_escape(&group_title(g))).build();
        for sw in SWITCHES.iter().filter(|s| s.group == g) {
            let subtitle = if sw.offered {
                format!("{}\n{} {}", sw.description, gettext("Set by:"), sw.control)
            } else {
                format!("{} {}", gettext("Not offered yet."), sw.description)
            };
            let r = row(sw.title, &subtitle);
            let shown = match sw.matching {
                septima_engine::switches::Match::Prefix => format!("{}…", sw.switch),
                septima_engine::switches::Match::Exact => sw.switch.to_string(),
            };
            mono_suffix(&r, &shown);
            if !sw.offered {
                r.add_css_class("dim-label");
            }
            group.add(&r);
        }
        page.add(&group);
    }

    let advanced = adw::PreferencesGroup::builder()
        .title(gettext("Anything else"))
        .description(gettext(
            "New Archive → Advanced passes any other 7-Zip switch straight to 7zz. A few that people reach for:",
        ))
        .build();
    for (switch, what) in ADVANCED_EXAMPLES {
        let r = row(what, "");
        mono_suffix(&r, switch);
        advanced.add(&r);
    }
    page.add(&advanced);

    let version = adw::PreferencesGroup::builder().title(gettext("This copy")).build();
    version.add(&row("Septima", config::VERSION));
    let sevenzip = septima_engine::sevenzip_path();
    let banner = septima_engine::sevenzip_banner(&sevenzip).unwrap_or_else(|| gettext("couldn't be run"));
    version.add(&row(&format!("7zz ({})", sevenzip.display()), &banner));
    page.add(&version);
    page
}

/// Hand-written, so each note must describe code that exists — keep them in
/// step with the functions they name.
fn good_to_know_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("notes")
        .title(gettext("Good to Know"))
        .icon_name("help-about-symbolic")
        .build();
    let group = adw::PreferencesGroup::new();
    let notes: [(String, String); 5] = [
        (
            // window.rs: ClashChoice, count_clashes, folder_has_entries.
            gettext("Extracting never replaces a file without asking"),
            gettext(
                "Before extracting, Septima checks whether files of the same name are already in the \
                 folder. If so, it asks: keep both (the new copies get a number), skip them, or replace \
                 them. Batch extract gives each archive a new folder and asks the same when one already \
                 exists. Skip can't be combined with deleting the archive afterwards, since the skipped \
                 files would go with it.",
            ),
        ),
        (
            // compress.rs: run_tar_and_compress; extract.rs: decompress_tar_command + untar_command.
            gettext("Compressed tarballs take two steps"),
            gettext(
                "Creating a .tar.zst, .tar.xz and the like makes a plain tar in a private temporary \
                 folder first, compresses it, then removes the temporary file. Opening or extracting one \
                 runs the two steps the other way through a pipe, so the plain tar never lands on disk.",
            ),
        ),
        (
            // command.rs: brotli_retry_flag / should_retry_brotli.
            gettext("Brotli files get a second try"),
            gettext(
                "Brotli has two kinds of stream that can't be told apart before reading. When a .br file \
                 or a .7z with brotli inside fails to open, Septima tries once more the other way. It \
                 never retries for a wrong password or a cancel.",
            ),
        ),
        (
            gettext("Without encrypted file names, names stay visible"),
            gettext(
                "A password encrypts the files' contents. In a 7z, turn on Encrypt file names to hide the \
                 list of files, their sizes and dates too. Zip has no way to hide them.",
            ),
        ),
        (
            gettext("Keep the passwords file"),
            gettext(
                "With unique passwords, each archive's password exists only in the passwords file. Septima \
                 writes every password to it before creating the first archive, so a run that stops halfway \
                 still leaves you the passwords for what it made. Lose the file, and those archives can't \
                 be opened.",
            ),
        ),
    ];
    for (title, body) in notes {
        group.add(&row(&title, &body));
    }
    page.add(&group);
    page
}
