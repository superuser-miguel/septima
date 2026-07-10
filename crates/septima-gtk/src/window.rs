use adw::subclass::prelude::*;
use gtk::prelude::WidgetExt;
use gtk::{gio, glib, CompositeTemplate, TemplateChild};

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/window.ui")]
    pub struct SeptimaWindow {
        // Bound to the `status_page` object in window.blp. Unused for now; the
        // "Open an archive" placeholder is replaced by the ColumnView in B2/P2.
        #[template_child]
        pub status_page: TemplateChild<adw::StatusPage>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaWindow {
        // Must match `template $SeptimaWindow` in window.blp.
        const NAME: &'static str = "SeptimaWindow";
        type Type = super::SeptimaWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SeptimaWindow {
        fn constructed(&self) {
            self.parent_constructed();
            // Striped "devel" header for non-release builds (libadwaita convention).
            if crate::config::PROFILE == "Devel" {
                self.obj().add_css_class("devel");
            }
        }
    }
    impl WidgetImpl for SeptimaWindow {}
    impl WindowImpl for SeptimaWindow {}
    impl ApplicationWindowImpl for SeptimaWindow {}
    impl AdwApplicationWindowImpl for SeptimaWindow {}
}

glib::wrapper! {
    pub struct SeptimaWindow(ObjectSubclass<imp::SeptimaWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl SeptimaWindow {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }
}
