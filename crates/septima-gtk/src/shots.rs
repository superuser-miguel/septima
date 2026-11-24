//! Dev-only screenshots, for looking at the UI without a person at the screen.
//!
//! Built only with `--features shots`, which no release enables. With it, and
//! `SEPTIMA_SHOTS_DIR` set, the app walks through a fixed list of views, renders
//! the main window to a PNG after each, and quits. It renders through GTK
//! itself, so it works on a headless compositor where no screenshot service
//! exists: run it with `build-aux/shots-headless.sh`, which keeps every window
//! off the real display.

use std::path::{Path, PathBuf};
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use crate::window::SeptimaWindow;

/// Render `widget` (normally the whole window, dialogs included) to `path`.
fn save(widget: &gtk::Widget, path: &Path) {
    let (w, h) = (widget.width() as f64, widget.height() as f64);
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, w, h);
    let Some(node) = snapshot.to_node() else {
        eprintln!("[shots] nothing rendered for {}", path.display());
        return;
    };
    let renderer = widget.native().and_then(|n| n.renderer()).expect("window has a renderer");
    let texture = renderer.render_texture(node, None);
    match texture.save_to_png(path) {
        Ok(()) => eprintln!("[shots] {}", path.display()),
        Err(e) => eprintln!("[shots] couldn't write {}: {e}", path.display()),
    }
}

/// One view: a name for the file, and what to do before rendering it.
type Step = (&'static str, fn(&adw::Application, &SeptimaWindow));

fn close_dialog(win: &SeptimaWindow) {
    if let Some(d) = win.visible_dialog() {
        d.force_close();
    }
}

const STEPS: &[Step] = &[
    ("main", |_, _| {}),
    // A batch bigger than the cap: some rows running, the rest waiting.
    ("batch-queue", |_, win| {
        close_dialog(win);
        if let Some(dir) = std::env::var_os("SEPTIMA_SHOTS_BATCH") {
            win.shots_batch_extract(Path::new(&dir));
            // Report again once the jobs have had time to spawn their 7zz.
            let win = win.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || report_queue(&win, "batch-queue"));
        }
    }),
    ("batch-cancel-all", |_, win| {
        win.shots_cancel_all();
        let win = win.clone();
        glib::timeout_add_local_once(Duration::from_millis(600), move || report_queue(&win, "batch-cancel-all"));
    }),
    ("batch-report", |_, win| {
        close_dialog(win);
        win.shots_long_report();
    }),
    ("about", |app, win| {
        close_dialog(win);
        app.activate_action("about", None);
    }),
    ("shortcuts", |app, win| {
        close_dialog(win);
        app.activate_action("shortcuts", None);
    }),
    ("can-do-formats", |app, win| {
        close_dialog(win);
        app.activate_action("capabilities", None);
    }),
    ("can-do-switches", |_, win| show_page(win, "switches")),
    ("can-do-switches-2", |_, win| scroll_dialog(win, 1.0)),
    ("can-do-switches-3", |_, win| scroll_dialog(win, 2.0)),
    ("can-do-switches-4", |_, win| scroll_dialog(win, 3.0)),
    ("can-do-notes", |_, win| show_page(win, "notes")),
    ("extract-clash", |_, win| {
        close_dialog(win);
        if let (Some(a), Some(d)) = (std::env::var_os("SEPTIMA_SHOTS_ARCHIVE"), std::env::var_os("SEPTIMA_SHOTS_DEST")) {
            win.confirm_extract(a.into(), d.into(), None);
        }
    }),
    ("extract-clash-skip", |_, win| click_radio(win, "Skip")),
    // New Archive, part-way through: a few things staged, Zstandard picked.
    ("create-dialog", |_, win| {
        close_dialog(win);
        win.shots_create_dialog(sample_inputs(), "zstd", false);
    }),
    ("create-dialog-2", |_, win| scroll_dialog(win, 0.85)),
    ("create-dialog-3", |_, win| scroll_dialog(win, 50.0)),
    // The same in batch mode, with a password per archive.
    ("create-batch", |_, win| {
        close_dialog(win);
        win.shots_create_dialog(sample_inputs(), "zstd", true);
    }),
    ("create-batch-2", |_, win| scroll_dialog(win, 0.85)),
    ("create-batch-3", |_, win| scroll_dialog(win, 50.0)),
];

/// The folders under `SEPTIMA_SHOTS_INPUTS`, to stage in the create dialog.
fn sample_inputs() -> Vec<PathBuf> {
    let Some(dir) = std::env::var_os("SEPTIMA_SHOTS_INPUTS") else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> =
        std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())).collect();
    v.sort();
    v
}

/// Print the queue's own count next to how many `7zz` this process really
/// has running, so the cap can be checked from outside the gate's bookkeeping.
fn report_queue(win: &SeptimaWindow, step: &str) {
    let (limit, running, waiting) = win.shots_queue_state();
    let children = std::process::Command::new("pgrep")
        .args(["-c", "-P", &std::process::id().to_string(), "7zz"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    eprintln!("[shots] {step}: limit {limit}, running {running}, waiting {waiting}, 7zz processes {children}");
}

/// Activate the radio in the open dialog whose label starts with `prefix`.
fn click_radio(win: &SeptimaWindow, prefix: &str) {
    fn walk(w: &gtk::Widget, prefix: &str) -> bool {
        if let Some(check) = w.downcast_ref::<gtk::CheckButton>() {
            let label = check.child().and_downcast::<gtk::Label>().map(|l| l.label().to_string());
            if label.is_some_and(|l| l.starts_with(prefix)) {
                check.set_active(true);
                return true;
            }
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            if walk(&c, prefix) {
                return true;
            }
            child = c.next_sibling();
        }
        false
    }
    if let Some(d) = win.visible_dialog() {
        walk(d.upcast_ref(), prefix);
    }
}

/// Scroll the open dialog's visible page down by `screens` page heights.
fn scroll_dialog(win: &SeptimaWindow, screens: f64) {
    fn find(w: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
        if let Some(sw) = w.downcast_ref::<gtk::ScrolledWindow>() {
            if sw.is_mapped() {
                return Some(sw.clone());
            }
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            if let Some(found) = find(&c) {
                return Some(found);
            }
            child = c.next_sibling();
        }
        None
    }
    if let Some(sw) = win.visible_dialog().and_then(|d| find(d.upcast_ref())) {
        let adj = sw.vadjustment();
        adj.set_value(adj.page_size() * 0.9 * screens);
    }
}

fn show_page(win: &SeptimaWindow, name: &str) {
    if let Some(d) = win.visible_dialog().and_downcast::<adw::PreferencesDialog>() {
        d.set_visible_page_name(name);
    }
}

/// A plain cargo build installs nothing, so the app's icon isn't in any theme
/// and About shows a placeholder. Meson installs it as `<app id>.svg`; do the
/// same into a private theme dir, so the shots look like a real build.
fn install_icon_for_cargo_build(dir: &Path) {
    let src = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/icons/hicolor/scalable/apps/io.github.superuser_miguel.Septima.svg");
    let theme = dir.join(".icons");
    let apps = theme.join("hicolor/scalable/apps");
    if std::fs::create_dir_all(&apps).is_ok()
        && std::fs::copy(src, apps.join(format!("{}.svg", crate::config::APP_ID))).is_ok()
    {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_search_path(&theme);
        }
    }
}

/// Start the walk if `SEPTIMA_SHOTS_DIR` is set. Each view is shown in light
/// and dark, so both themes get looked at.
pub fn maybe_start(app: &adw::Application, win: &SeptimaWindow) {
    let Some(dir) = std::env::var_os("SEPTIMA_SHOTS_DIR").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("create SEPTIMA_SHOTS_DIR");
    install_icon_for_cargo_build(&dir);
    // Room for the taller dialogs, and an archive open so the list shows.
    win.set_default_size(1100, 760);
    if let Some(archive) = std::env::var_os("SEPTIMA_SHOTS_ARCHIVE") {
        win.open_file(gtk::gio::File::for_path(archive));
    }
    let style = adw::StyleManager::default();
    let mut queue: Vec<(String, Option<Step>, adw::ColorScheme)> = Vec::new();
    for scheme in [adw::ColorScheme::ForceLight, adw::ColorScheme::ForceDark] {
        let tag = if scheme == adw::ColorScheme::ForceDark { "dark" } else { "light" };
        for step in STEPS {
            queue.push((format!("{}-{tag}.png", step.0), Some(*step), scheme));
        }
    }
    queue.reverse();
    let (app, win) = (app.clone(), win.clone());
    fn next(
        app: adw::Application,
        win: SeptimaWindow,
        dir: PathBuf,
        style: adw::StyleManager,
        mut queue: Vec<(String, Option<Step>, adw::ColorScheme)>,
    ) {
        let Some((file, step, scheme)) = queue.pop() else {
            app.quit();
            return;
        };
        style.set_color_scheme(scheme);
        if let Some((name, act)) = step {
            if name == "main" {
                close_dialog(&win);
            }
            act(&app, &win);
        }
        // Long enough for the dialog's open animation to finish.
        glib::timeout_add_local_once(Duration::from_millis(900), move || {
            save(win.upcast_ref(), &dir.join(&file));
            next(app, win, dir, style, queue);
        });
    }
    glib::timeout_add_local_once(Duration::from_millis(900), move || next(app, win, dir, style, queue));
}
