use adw::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gio, glib, CompositeTemplate, TemplateChild};

use crate::archive_view::SeptimaArchiveView;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/window.ui")]
    pub struct SeptimaWindow {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub archive_view: TemplateChild<SeptimaArchiveView>,
    }

    #[gtk::template_callbacks]
    impl SeptimaWindow {
        #[template_callback]
        fn on_open_clicked(&self) {
            self.obj().open_archive_dialog();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaWindow {
        const NAME: &'static str = "SeptimaWindow";
        type Type = super::SeptimaWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            // The window template references $SeptimaArchiveView; ensure the type
            // is registered before the template is parsed.
            SeptimaArchiveView::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SeptimaWindow {
        fn constructed(&self) {
            self.parent_constructed();
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

    /// Show a file chooser (portal-backed under Flatpak) and open the picked
    /// archive. Cancellation is silent; other failures show a toast.
    fn open_archive_dialog(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettextrs::gettext("Open Archive"))
            .modal(true)
            .build();

        let window = self.clone();
        dialog.open(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(file) => window.open_file(file),
            Err(err) => {
                // Dismissal/cancellation is not an error worth a toast.
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    /// Open and list `file` (from the file chooser, CLI args, or a file manager).
    pub fn open_file(&self, file: gio::File) {
        let Some(path) = file.path() else {
            self.show_toast(&gettextrs::gettext("That location can't be read directly."));
            return;
        };

        let window = self.clone();
        let sevenzip = septima_engine::sevenzip_path();
        glib::spawn_future_local(async move {
            let result =
                gio::spawn_blocking(move || septima_engine::list_archive(&sevenzip, &path)).await;
            match result {
                Ok(Ok(listing)) => {
                    let imp = window.imp();
                    imp.archive_view.load(&listing);
                    imp.stack.set_visible_child_name("archive");
                }
                Ok(Err(err)) => window.show_toast(&err.to_string()),
                Err(_) => window.show_toast(&gettextrs::gettext("The listing task failed.")),
            }
        });
    }

    fn show_toast(&self, message: &str) {
        self.imp()
            .toast_overlay
            .add_toast(adw::Toast::new(message));
    }
}
