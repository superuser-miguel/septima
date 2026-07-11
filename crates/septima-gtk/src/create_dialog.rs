use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate, TemplateChild};

use septima_engine::capabilities::{formats, Codec, Format};
use septima_engine::estimate_add_memory;

/// Dictionary-size presets: (label, `-md` arg, size in bytes). `None` = Auto.
const DICT_PRESETS: &[(&str, Option<&str>, Option<u64>)] = &[
    ("Auto", None, None),
    ("1 MiB", Some("1m"), Some(1 << 20)),
    ("4 MiB", Some("4m"), Some(4 << 20)),
    ("16 MiB", Some("16m"), Some(16 << 20)),
    ("64 MiB", Some("64m"), Some(64 << 20)),
    ("256 MiB", Some("256m"), Some(256 << 20)),
    ("1 GiB", Some("1g"), Some(1 << 30)),
];

/// Volume-split presets: (label, `-v` size). `None` = single file.
const VOLUME_PRESETS: &[(&str, Option<&str>)] = &[
    ("Off", None),
    ("25 MiB", Some("25m")),
    ("100 MiB", Some("100m")),
    ("700 MiB (CD)", Some("700m")),
    ("1 GiB", Some("1g")),
    ("4 GiB", Some("4g")),
];

fn codec_uses_dictionary(codec: &Codec) -> bool {
    matches!(codec.id, "lzma2" | "lzma" | "flzma2")
}

/// The compression settings collected by the dialog (output path is chosen after).
pub struct CreateSettings {
    pub name: String,
    pub format: &'static Format,
    pub codec: &'static Codec,
    pub level: Option<u8>,
    pub threads: u32,
    pub dictionary: Option<String>,
    pub solid: Option<bool>,
    pub volume_size: Option<String>,
    pub bcj: bool,
    pub password: Option<String>,
    pub encrypt_headers: bool,
    pub extra_params: Vec<String>,
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
        pub dictionary_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub solid_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub memory_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub bcj_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub volume_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub params_row: TemplateChild<adw::EntryRow>,
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

            let format_labels: Vec<&str> = formats().iter().map(|f| f.label).collect();
            self.format_row.set_model(Some(&gtk::StringList::new(&format_labels)));

            let dict_labels: Vec<&str> = DICT_PRESETS.iter().map(|(l, _, _)| *l).collect();
            self.dictionary_row.set_model(Some(&gtk::StringList::new(&dict_labels)));

            let vol_labels: Vec<&str> = VOLUME_PRESETS.iter().map(|(l, _)| *l).collect();
            self.volume_row.set_model(Some(&gtk::StringList::new(&vol_labels)));

            let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
            self.threads_row.adjustment().set_upper(cpus.max(1) as f64);
            self.threads_row.adjustment().set_value(cpus as f64);

            // Anything that affects the memory estimate refreshes it live.
            let refresh = glib::clone!(
                #[weak]
                obj,
                move |_: &_| obj.imp().update_memory()
            );
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
            self.dictionary_row.connect_selected_notify(refresh.clone());
            self.level_row.adjustment().connect_value_changed(glib::clone!(
                #[weak]
                obj,
                move |_| obj.imp().update_memory()
            ));
            self.threads_row.adjustment().connect_value_changed(glib::clone!(
                #[weak]
                obj,
                move |_| obj.imp().update_memory()
            ));

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

        pub(super) fn selected_dict(&self) -> (Option<String>, Option<u64>) {
            let (_, arg, bytes) = DICT_PRESETS[self.dictionary_row.selected() as usize];
            (arg.map(str::to_string), bytes)
        }

        fn on_format_changed(&self) {
            let fmt = self.current_format();
            let labels: Vec<&str> = fmt.codecs.iter().map(|c| c.label).collect();
            self.codec_row.set_model(Some(&gtk::StringList::new(&labels)));
            self.codec_row.set_selected(0); // fires on_codec_changed
            self.solid_row.set_sensitive(fmt.supports_solid);
            self.bcj_row.set_sensitive(fmt.id == "7z");
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
            self.dictionary_row.set_sensitive(codec_uses_dictionary(codec));
            self.update_memory();
        }

        pub(super) fn update_memory(&self) {
            let codec = self.current_codec();
            let level = Some(self.level_row.value() as u8);
            let (_, dict_bytes) = self.selected_dict();
            let threads = self.threads_row.value() as u32;

            let text = match estimate_add_memory(codec.id, level, dict_bytes, threads) {
                Some(bytes) => format!("≈ {}", glib::format_size(bytes)),
                None => "—".to_string(),
            };
            self.memory_row.set_subtitle(&text);
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
        let (dictionary, _) = imp.selected_dict();
        let password = {
            let text = imp.password_row.text().to_string();
            (!text.is_empty()).then_some(text)
        };
        let extra_params = imp
            .params_row
            .text()
            .split_whitespace()
            .map(str::to_string)
            .collect();

        CreateSettings {
            name: imp.name_row.text().to_string(),
            format,
            codec,
            level,
            threads: imp.threads_row.value() as u32,
            dictionary: dictionary.filter(|_| codec_uses_dictionary(codec)),
            solid: format.supports_solid.then(|| imp.solid_row.is_active()),
            volume_size: VOLUME_PRESETS[imp.volume_row.selected() as usize]
                .1
                .map(str::to_string),
            bcj: format.id == "7z" && imp.bcj_row.is_active(),
            password,
            encrypt_headers: format.supports_header_encryption && imp.encrypt_headers_row.is_active(),
            extra_params,
        }
    }
}
