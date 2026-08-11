use std::cell::RefCell;

use adw::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango, CompositeTemplate, TemplateChild};

use septima_engine::{ArchiveEntry, ArchiveListing};

use crate::entry_object::EntryObject;

type DeleteCallback = Box<dyn Fn(Vec<String>)>;
type RenameCallback = Box<dyn Fn(String)>;
/// Given the selected in-archive paths, stage them as real files (extract to a
/// scratch dir) and return those paths — or `None` to abort the drag.
type DragExtractCallback = Box<dyn Fn(Vec<String>) -> Option<Vec<std::path::PathBuf>>>;

/// Don't offer a drag that would stall the UI staging gigabytes — past this,
/// the Extract button is the right tool. Directory entries list a size of 0,
/// so a huge selected *folder* can slip past; the cap is a guard rail, not an
/// accounting system.
const DRAG_EXTRACT_CAP_BYTES: u64 = 512 * 1024 * 1024;

mod imp {
    use super::*;
    use std::cell::OnceCell;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/archive_view.ui")]
    pub struct SeptimaArchiveView {
        #[template_child]
        pub summary_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub column_view: TemplateChild<gtk::ColumnView>,
        #[template_child]
        pub rename_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub delete_button: TemplateChild<gtk::Button>,
        pub model: OnceCell<gio::ListStore>,
        pub on_delete: RefCell<Option<DeleteCallback>>,
        pub on_rename: RefCell<Option<RenameCallback>>,
        pub on_drag_extract: RefCell<Option<DragExtractCallback>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaArchiveView {
        const NAME: &'static str = "SeptimaArchiveView";
        type Type = super::SeptimaArchiveView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SeptimaArchiveView {
        fn constructed(&self) {
            self.parent_constructed();

            let model = gio::ListStore::new::<EntryObject>();
            let selection = gtk::MultiSelection::new(Some(model.clone()));
            selection.connect_selection_changed(glib::clone!(
                #[weak(rename_to = imp)]
                self,
                move |selection, _, _| imp.update_selection_sensitivity(selection)
            ));
            self.column_view.set_model(Some(&selection));
            self.model.set(model).unwrap();

            let view = &*self.column_view;
            view.append_column(&text_column(&gettext("Name"), true, |e| e.path.clone()));
            view.append_column(&text_column(&gettext("Size"), false, |e| glib::format_size(e.size).to_string()));
            view.append_column(&text_column(&gettext("Packed"), false, |e| {
                e.packed_size.map(|s| glib::format_size(s).to_string()).unwrap_or_default()
            }));
            view.append_column(&text_column(&gettext("Method"), false, |e| e.method.clone().unwrap_or_default()));
            view.append_column(&text_column(&gettext("Modified"), false, short_time));
            view.append_column(&text_column(&gettext("CRC"), false, |e| e.crc.clone().unwrap_or_default()));

            // Drag entries out of the archive: the window-side handler stages
            // the selection as real files (an on-the-spot extract) and the
            // drop hands them to the receiver — GNOME Files, a desktop, any
            // app that takes files. Returning no provider quietly aborts.
            let drag = gtk::DragSource::new();
            drag.set_actions(gdk::DragAction::COPY);
            drag.connect_prepare(glib::clone!(
                #[weak(rename_to = imp)]
                self,
                #[upgrade_or]
                None,
                move |_, _, _| {
                    let (paths, total) = imp.obj().selected_entries();
                    if paths.is_empty() || total > DRAG_EXTRACT_CAP_BYTES {
                        return None;
                    }
                    let staged = imp.on_drag_extract.borrow().as_ref().and_then(|cb| cb(paths))?;
                    let files: Vec<gio::File> =
                        staged.iter().map(gio::File::for_path).collect();
                    Some(gdk::ContentProvider::for_value(
                        &gdk::FileList::from_array(&files).to_value(),
                    ))
                }
            ));
            self.column_view.add_controller(drag);
        }
    }

    impl WidgetImpl for SeptimaArchiveView {}
    impl BinImpl for SeptimaArchiveView {}

    #[gtk::template_callbacks]
    impl SeptimaArchiveView {
        fn update_selection_sensitivity(&self, selection: &gtk::MultiSelection) {
            let n = selection.selection().size();
            self.delete_button.set_sensitive(n >= 1);
            self.rename_button.set_sensitive(n == 1);
        }

        #[template_callback]
        fn on_delete_clicked(&self) {
            let paths = self.obj().selected_paths();
            if let Some(cb) = self.on_delete.borrow().as_ref() {
                cb(paths);
            }
        }

        #[template_callback]
        fn on_rename_clicked(&self) {
            if let Some(path) = self.obj().selected_paths().into_iter().next() {
                if let Some(cb) = self.on_rename.borrow().as_ref() {
                    cb(path);
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct SeptimaArchiveView(ObjectSubclass<imp::SeptimaArchiveView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SeptimaArchiveView {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SeptimaArchiveView {
    /// Replace the displayed contents with `listing`.
    pub fn load(&self, listing: &ArchiveListing) {
        let imp = self.imp();
        let name = listing
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fmt = listing.format.as_deref().unwrap_or("archive");
        // e.g. "sample.7z — 3 files · 4.0 kB (7z)"
        imp.summary_label.set_label(&format!(
            "{name} — {} · {} ({fmt})",
            ngettext_files(listing.file_count()),
            glib::format_size(listing.total_size()),
        ));

        let model = imp.model.get().unwrap();
        model.remove_all();
        for entry in &listing.entries {
            model.append(&EntryObject::new(entry.clone()));
        }
    }

    /// In-archive paths of the currently selected entries.
    fn selected_paths(&self) -> Vec<String> {
        self.selected_entries().0
    }

    /// Selected in-archive paths plus their total listed size (directory
    /// entries list 0 — their children count only when selected themselves).
    fn selected_entries(&self) -> (Vec<String>, u64) {
        let Some(selection) = self.imp().column_view.model().and_downcast::<gtk::MultiSelection>() else {
            return (Vec::new(), 0);
        };
        let bitset = selection.selection();
        let mut total = 0u64;
        let paths = (0..bitset.size())
            .filter_map(|i| selection.item(bitset.nth(i as u32)))
            .filter_map(|obj| obj.downcast::<EntryObject>().ok())
            .map(|obj| {
                let entry = obj.entry();
                total += entry.size;
                entry.path.clone()
            })
            .collect();
        (paths, total)
    }

    /// Register the handler run when the user asks to delete the current
    /// selection (one or more entries).
    pub fn connect_delete_requested<F: Fn(Vec<String>) + 'static>(&self, f: F) {
        self.imp().on_delete.replace(Some(Box::new(f)));
    }

    /// Register the handler run when the user asks to rename the current
    /// (single-entry) selection. Called with the entry's in-archive path.
    pub fn connect_rename_requested<F: Fn(String) + 'static>(&self, f: F) {
        self.imp().on_rename.replace(Some(Box::new(f)));
    }

    /// Register the drag-out handler: given the selected in-archive paths, it
    /// stages them as real files and returns those paths (`None` aborts the
    /// drag). Runs synchronously inside the drag gesture's prepare.
    pub fn connect_drag_extract<F>(&self, f: F)
    where
        F: Fn(Vec<String>) -> Option<Vec<std::path::PathBuf>> + 'static,
    {
        self.imp().on_drag_extract.replace(Some(Box::new(f)));
    }
}

/// Build a text column whose cell text comes from `getter(&ArchiveEntry)`.
fn text_column(
    title: &str,
    expand: bool,
    getter: impl Fn(&ArchiveEntry) -> String + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let obj = item.item().and_downcast::<EntryObject>().unwrap();
        let label = item.child().and_downcast::<gtk::Label>().unwrap();
        label.set_label(&getter(&obj.entry()));
    });

    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_expand(expand);
    column.set_resizable(true);
    column
}

/// Trim `7zz`'s fractional seconds: `2026-07-10 15:09:36.959` -> `2026-07-10 15:09:36`.
fn short_time(entry: &ArchiveEntry) -> String {
    match &entry.modified {
        Some(m) => m.split('.').next().unwrap_or(m).to_string(),
        None => String::new(),
    }
}

fn gettext(s: &str) -> String {
    gettextrs::gettext(s)
}

fn ngettext_files(n: usize) -> String {
    let template = gettextrs::ngettext("{} file", "{} files", n as u32);
    template.replacen("{}", &n.to_string(), 1)
}
