use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib, CompositeTemplate, TemplateChild};

use septima_engine::{hash_algorithms, Digest};

fn gettext(s: &str) -> String {
    gettextrs::gettext(s)
}

/// Display label for an algorithm switch name (falls back to the name itself).
fn algo_label(switch: &str) -> &str {
    hash_algorithms()
        .iter()
        .find(|a| a.switch == switch)
        .map(|a| a.label)
        .unwrap_or(switch)
}

/// Pull every hex token (≥ 8 chars) out of a checksum file — covers the common
/// `<hex>  name`, `<hex> *name`, and BSD `ALGO (name) = <hex>` layouts.
fn parse_checksum_hexes(text: &str) -> HashSet<String> {
    let mut hexes = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for token in line.split(|c: char| c.is_whitespace() || matches!(c, '=' | '(' | ')')) {
            let token = token.trim().trim_start_matches('*');
            if token.len() >= 8 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
                hexes.insert(token.to_ascii_lowercase());
            }
        }
    }
    hexes
}

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/hash_dialog.ui")]
    pub struct SeptimaHashDialog {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub clear_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub verify_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub verify_status: TemplateChild<gtk::Label>,
        #[template_child]
        pub files_box: TemplateChild<gtk::Box>,
        /// (path, digests), cached so re-highlighting doesn't re-hash.
        pub results: RefCell<Vec<(PathBuf, Vec<Digest>)>>,
        /// Hex digests loaded from a checksum file.
        pub verify_hexes: RefCell<HashSet<String>>,
    }

    #[gtk::template_callbacks]
    impl SeptimaHashDialog {
        #[template_callback]
        fn on_add_clicked(&self) {
            self.obj().add_files();
        }

        #[template_callback]
        fn on_clear_clicked(&self) {
            self.obj().clear();
        }

        #[template_callback]
        fn on_verify_changed(&self) {
            self.obj().rebuild();
        }

        #[template_callback]
        fn on_load_file_clicked(&self) {
            self.obj().load_checksum_file();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaHashDialog {
        const NAME: &'static str = "SeptimaHashDialog";
        type Type = super::SeptimaHashDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SeptimaHashDialog {}
    impl WidgetImpl for SeptimaHashDialog {}
    impl AdwDialogImpl for SeptimaHashDialog {}
}

glib::wrapper! {
    pub struct SeptimaHashDialog(ObjectSubclass<imp::SeptimaHashDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SeptimaHashDialog {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SeptimaHashDialog {
    pub fn new() -> Self {
        Self::default()
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.root().and_downcast::<gtk::Window>()
    }

    fn add_files(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Add Files"))
            .modal(true)
            .build();
        let window = self.clone();
        dialog.open_multiple(self.parent_window().as_ref(), gio::Cancellable::NONE, move |result| {
            if let Ok(files) = result {
                let paths: Vec<PathBuf> = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                    .filter_map(|f| f.path())
                    .collect();
                window.add_paths(paths);
            }
        });
    }

    /// Hash the given files and show their digests (portal, drag-drop, or CLI).
    pub fn add_paths(&self, paths: Vec<PathBuf>) {
        let algos: Vec<&'static str> = hash_algorithms().iter().map(|a| a.switch).collect();
        let window = self.clone();
        glib::spawn_future_local(async move {
            for path in paths {
                let sevenzip = septima_engine::sevenzip_path();
                let p = path.clone();
                let algos = algos.clone();
                let result =
                    gio::spawn_blocking(move || septima_engine::hash_file(&sevenzip, &p, &algos))
                        .await;
                match result {
                    Ok(Ok(digests)) => {
                        window.imp().results.borrow_mut().push((path, digests));
                        window.rebuild();
                    }
                    Ok(Err(err)) => eprintln!("septima: hash failed: {err}"),
                    Err(_) => {}
                }
            }
        });
    }

    fn clear(&self) {
        let imp = self.imp();
        imp.results.borrow_mut().clear();
        imp.verify_hexes.borrow_mut().clear();
        imp.verify_status.set_visible(false);
        imp.verify_entry.set_text("");
        self.rebuild();
    }

    fn remove_file(&self, path: &Path) {
        self.imp().results.borrow_mut().retain(|(p, _)| p != path);
        self.rebuild();
    }

    fn load_checksum_file(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Verify Against a Checksum File"))
            .modal(true)
            .build();
        let window = self.clone();
        dialog.open(self.parent_window().as_ref(), gio::Cancellable::NONE, move |result| {
            let Ok(file) = result else { return };
            let Some(path) = file.path() else { return };
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let hexes = parse_checksum_hexes(&text);
                    let count = hexes.len();
                    window.imp().verify_hexes.replace(hexes);
                    let status = &window.imp().verify_status;
                    status.set_text(&format!(
                        "{} — {}",
                        file_name(&path),
                        n_checksums(count)
                    ));
                    status.set_visible(count > 0);
                    window.rebuild();
                }
                Err(err) => eprintln!("septima: cannot read checksum file: {err}"),
            }
        });
    }

    /// Rebuild the per-file sections from cached results, applying the verify
    /// highlight (pasted hash + any loaded checksum-file hashes).
    fn rebuild(&self) {
        let imp = self.imp();
        while let Some(child) = imp.files_box.first_child() {
            imp.files_box.remove(&child);
        }

        let results = imp.results.borrow();
        if results.is_empty() {
            imp.stack.set_visible_child_name("empty");
            imp.clear_button.set_sensitive(false);
            return;
        }
        imp.stack.set_visible_child_name("results");
        imp.clear_button.set_sensitive(true);

        let mut targets = imp.verify_hexes.borrow().clone();
        let pasted = imp.verify_entry.text().trim().to_ascii_lowercase();
        if !pasted.is_empty() {
            targets.insert(pasted);
        }

        for (path, digests) in results.iter() {
            imp.files_box.append(&self.file_section(path, digests, &targets));
        }
    }

    fn file_section(&self, path: &Path, digests: &[Digest], targets: &HashSet<String>) -> gtk::Box {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 6);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::builder()
            .label(file_name(path))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        title.add_css_class("heading");
        header.append(&title);

        let remove = gtk::Button::from_icon_name("window-close-symbolic");
        remove.add_css_class("flat");
        remove.set_valign(gtk::Align::Center);
        remove.set_tooltip_text(Some(&gettext("Remove")));
        let owned = path.to_path_buf();
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.remove_file(&owned)
        ));
        header.append(&remove);
        section.append(&header);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);

        for digest in digests {
            let row = adw::ActionRow::builder()
                .title(algo_label(&digest.algo))
                .subtitle(&digest.hex)
                .subtitle_selectable(true)
                .build();
            row.add_css_class("property");

            if targets.contains(&digest.hex) {
                row.add_css_class("success");
                let check = gtk::Image::from_icon_name("object-select-symbolic");
                check.add_css_class("success");
                row.add_prefix(&check);
            }

            let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
            copy.add_css_class("flat");
            copy.set_valign(gtk::Align::Center);
            copy.set_tooltip_text(Some(&gettext("Copy")));
            let hex = digest.hex.clone();
            copy.connect_clicked(move |button| button.clipboard().set_text(&hex));
            row.add_suffix(&copy);

            list.append(&row);
        }

        section.append(&list);
        section
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn n_checksums(n: usize) -> String {
    gettextrs::ngettext("{} checksum", "{} checksums", n as u32).replacen("{}", &n.to_string(), 1)
}

#[cfg(test)]
mod tests {
    use super::parse_checksum_hexes;

    #[test]
    fn parses_common_checksum_formats() {
        let text = "\
# a comment
49f5819f475bf2c8e2ed80998789dba47a4a25ed19f97b6c8c6a4902eea0c1a1  ubuntu.iso
6dd738acab109c85 *other.bin
SHA256 (thing.tar) = 62590f1b3d1a534d8df8ea2f3b5542a2b3fc46b3ac0b3d5e03bae13a12dc97e5
";
        let hexes = parse_checksum_hexes(text);
        assert!(hexes.contains("49f5819f475bf2c8e2ed80998789dba47a4a25ed19f97b6c8c6a4902eea0c1a1"));
        assert!(hexes.contains("6dd738acab109c85")); // strips leading '*'
        assert!(hexes.contains("62590f1b3d1a534d8df8ea2f3b5542a2b3fc46b3ac0b3d5e03bae13a12dc97e5"));
    }
}
