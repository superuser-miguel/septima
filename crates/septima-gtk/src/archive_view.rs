use adw::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gio, glib, pango, CompositeTemplate, TemplateChild};

use septima_engine::{ArchiveEntry, ArchiveListing};

use crate::entry_object::EntryObject;

mod imp {
    use super::*;
    use std::cell::OnceCell;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/archive_view.ui")]
    pub struct SeptimaArchiveView {
        #[template_child]
        pub summary_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub column_view: TemplateChild<gtk::ColumnView>,
        pub model: OnceCell<gio::ListStore>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaArchiveView {
        const NAME: &'static str = "SeptimaArchiveView";
        type Type = super::SeptimaArchiveView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SeptimaArchiveView {
        fn constructed(&self) {
            self.parent_constructed();

            let model = gio::ListStore::new::<EntryObject>();
            self.column_view
                .set_model(Some(&gtk::NoSelection::new(Some(model.clone()))));
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
        }
    }

    impl WidgetImpl for SeptimaArchiveView {}
    impl BinImpl for SeptimaArchiveView {}
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
