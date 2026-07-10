use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate, TemplateChild};

use septima_engine::capabilities::{formats, Codec, Format};

/// The compression settings collected by the dialog (output path is chosen after).
pub struct CreateSettings {
    pub name: String,
    pub format: &'static Format,
    pub codec: &'static Codec,
    pub level: Option<u8>,
    pub threads: u32,
    pub password: Option<String>,
    pub encrypt_headers: bool,
}

type CreateCallback = Box<dyn Fn(&SeptimaCreateDialog)>;

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/create_dialog.ui")]
    pub struct SeptimaCreateDialog {
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub format_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub codec_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub level_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub threads_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub password_row: TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub encrypt_headers_row: TemplateChild<adw::SwitchRow>,
        pub on_create: RefCell<Option<CreateCallback>>,
    }

    #[gtk::template_callbacks]
    impl SeptimaCreateDialog {
        #[template_callback]
        fn on_cancel(&self) {
            self.obj().close();
        }

        #[template_callback]
        fn on_create(&self) {
            let obj = self.obj();
            if let Some(cb) = self.on_create.borrow().as_ref() {
                cb(&obj);
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaCreateDialog {
        const NAME: &'static str = "SeptimaCreateDialog";
        type Type = super::SeptimaCreateDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SeptimaCreateDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Format list.
            let format_labels: Vec<&str> = formats().iter().map(|f| f.label).collect();
            self.format_row.set_model(Some(&gtk::StringList::new(&format_labels)));

            // Default thread count = available CPUs.
            let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            self.threads_row.adjustment().set_upper(cpus.max(1) as f64);
            self.threads_row.adjustment().set_value(cpus as f64);

            self.format_row.connect_selected_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.imp().on_format_changed()
            ));
            self.codec_row.connect_selected_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.imp().on_codec_changed()
            ));

            // Prime format 0 (rebuilds codecs and level range).
            self.on_format_changed();
        }
    }

    impl WidgetImpl for SeptimaCreateDialog {}
    impl AdwDialogImpl for SeptimaCreateDialog {}

    impl SeptimaCreateDialog {
        pub(super) fn current_format(&self) -> &'static Format {
            &formats()[self.format_row.selected() as usize]
        }

        pub(super) fn current_codec(&self) -> &'static Codec {
            let fmt = self.current_format();
            let idx = (self.codec_row.selected() as usize).min(fmt.codecs.len().saturating_sub(1));
            &fmt.codecs[idx]
        }

        fn on_format_changed(&self) {
            let fmt = self.current_format();
            let labels: Vec<&str> = fmt.codecs.iter().map(|c| c.label).collect();
            self.codec_row.set_model(Some(&gtk::StringList::new(&labels)));
            self.codec_row.set_selected(0); // fires on_codec_changed
            self.encrypt_headers_row.set_sensitive(fmt.supports_header_encryption);
            self.on_codec_changed();
        }

        fn on_codec_changed(&self) {
            let codec = self.current_codec();
            let adj = self.level_row.adjustment();
            if codec.is_store() {
                self.level_row.set_sensitive(false);
            } else {
                self.level_row.set_sensitive(true);
                adj.set_lower(codec.level_min as f64);
                adj.set_upper(codec.level_max as f64);
                adj.set_value(codec.default_level as f64);
            }
        }
    }
}

glib::wrapper! {
    pub struct SeptimaCreateDialog(ObjectSubclass<imp::SeptimaCreateDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SeptimaCreateDialog {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SeptimaCreateDialog {
    pub fn new(suggested_name: &str) -> Self {
        let dialog = Self::default();
        dialog.imp().name_row.set_text(suggested_name);
        dialog
    }

    /// Register the handler run when the user confirms (Create).
    pub fn connect_create<F: Fn(&Self) + 'static>(&self, f: F) {
        self.imp().on_create.replace(Some(Box::new(f)));
    }

    /// Read the current widget state into a [`CreateSettings`].
    pub fn settings(&self) -> CreateSettings {
        let imp = self.imp();
        let format = imp.current_format();
        let codec = imp.current_codec();

        let level = (!codec.is_store()).then(|| imp.level_row.value() as u8);
        let password = {
            let text = imp.password_row.text().to_string();
            (!text.is_empty()).then_some(text)
        };

        CreateSettings {
            name: imp.name_row.text().to_string(),
            format,
            codec,
            level,
            threads: imp.threads_row.value() as u32,
            password,
            encrypt_headers: format.supports_header_encryption && imp.encrypt_headers_row.is_active(),
        }
    }
}
