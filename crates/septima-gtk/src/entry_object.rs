use std::cell::RefCell;

use gtk::glib;
use gtk::subclass::prelude::*;

use septima_engine::ArchiveEntry;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct EntryObject {
        pub entry: RefCell<ArchiveEntry>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EntryObject {
        const NAME: &'static str = "SeptimaEntryObject";
        type Type = super::EntryObject;
    }

    impl ObjectImpl for EntryObject {}
}

glib::wrapper! {
    /// A GObject wrapper around an [`ArchiveEntry`] so it can live in a
    /// `gio::ListStore` and back a `Gtk.ColumnView`.
    pub struct EntryObject(ObjectSubclass<imp::EntryObject>);
}

impl EntryObject {
    pub fn new(entry: ArchiveEntry) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().entry.replace(entry);
        obj
    }

    pub fn entry(&self) -> std::cell::Ref<'_, ArchiveEntry> {
        self.imp().entry.borrow()
    }
}
