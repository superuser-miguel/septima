use std::cell::RefCell;
use std::path::PathBuf;

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

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/hash_dialog.ui")]
    pub struct SeptimaHashDialog {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub verify_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub files_box: TemplateChild<gtk::Box>,
        /// Computed results: (path, digests), cached so re-highlighting on verify
        /// doesn't re-hash.
        pub results: RefCell<Vec<(PathBuf, Vec<Digest>)>>,
    }

    #[gtk::template_callbacks]
    impl SeptimaHashDialog {
        #[template_callback]
        fn on_add_clicked(&self) {
            self.obj().add_files();
        }

        #[template_callback]
        fn on_verify_changed(&self) {
            self.obj().rebuild();
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

    fn add_files(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Add Files"))
            .modal(true)
            .build();

        let parent = self.root().and_downcast::<gtk::Window>();
        let window = self.clone();
        dialog.open_multiple(parent.as_ref(), gio::Cancellable::NONE, move |result| {
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
                let result = gio::spawn_blocking(move || {
                    septima_engine::hash_file(&sevenzip, &p, &algos)
                })
                .await;

                match result {
                    Ok(Ok(digests)) => {
                        window.imp().results.borrow_mut().push((path, digests));
                        window.imp().stack.set_visible_child_name("results");
                        window.rebuild();
                    }
                    Ok(Err(err)) => eprintln!("septima: hash failed: {err}"),
                    Err(_) => {}
                }
            }
        });
    }

    /// Rebuild the per-file sections from cached results, applying the verify
    /// highlight. Re-render only — no re-hashing.
    fn rebuild(&self) {
        let imp = self.imp();
        while let Some(child) = imp.files_box.first_child() {
            imp.files_box.remove(&child);
        }

        let needle = imp.verify_entry.text().trim().to_ascii_lowercase();
        for (path, digests) in imp.results.borrow().iter() {
            imp.files_box.append(&file_section(path, digests, &needle));
        }
    }
}

fn file_section(path: &std::path::Path, digests: &[Digest], needle: &str) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = gtk::Label::builder()
        .label(&name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    title.add_css_class("heading");
    section.append(&title);

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

        if !needle.is_empty() && digest.hex == needle {
            row.add_css_class("success");
            let check = gtk::Image::from_icon_name("emblem-ok-symbolic");
            row.add_prefix(&check);
        }

        let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
        copy.add_css_class("flat");
        copy.set_valign(gtk::Align::Center);
        copy.set_tooltip_text(Some(&gettext("Copy")));
        let hex = digest.hex.clone();
        copy.connect_clicked(move |button| {
            button.clipboard().set_text(&hex);
        });
        row.add_suffix(&copy);

        list.append(&row);
    }

    section.append(&list);
    section
}
