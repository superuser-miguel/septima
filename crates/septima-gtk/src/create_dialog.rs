use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib, CompositeTemplate, TemplateChild};

use septima_engine::capabilities::{formats, Codec, Format};
use septima_engine::{estimate_add_memory, measure_selection, new_cancel_token, CancelToken, Selection};

use crate::preset::{Preset, PresetStore};

fn gettext(s: &str) -> String {
    gettextrs::gettext(s)
}

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

/// Past either of these, warn that the selection is big. Tuned from the real
/// failure it exists to prevent: a whole project dir dragged in, where `target/`
/// and `builddir/` dwarfed the source the user actually meant to archive.
const LARGE_SELECTION_BYTES: u64 = 1024 * 1024 * 1024;
const LARGE_SELECTION_FILES: u64 = 10_000;

fn codec_uses_dictionary(codec: &Codec) -> bool {
    matches!(codec.id, "lzma2" | "lzma" | "flzma2")
}

/// The compression settings collected by the dialog (output path is chosen after).
pub struct CreateSettings {
    pub inputs: Vec<PathBuf>,
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
        pub create_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub files_group: TemplateChild<adw::PreferencesGroup>,
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
        #[template_child]
        pub presets_button: TemplateChild<gtk::MenuButton>,
        pub on_create: RefCell<Option<CreateCallback>>,
        pub inputs: RefCell<Vec<PathBuf>>,
        pub input_rows: RefCell<Vec<adw::ActionRow>>,
        /// Cancels the background walk when the selection changes under it.
        pub measure_cancel: RefCell<Option<CancelToken>>,
        /// Bumped per measurement so a superseded walk's answer is discarded.
        pub measure_gen: Cell<u64>,
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

        #[template_callback]
        fn on_add_files(&self) {
            self.obj().pick_files();
        }

        #[template_callback]
        fn on_add_folder(&self) {
            self.obj().pick_folders();
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
            obj.rebuild_presets_popover();

            // Drag-and-drop files/folders onto the dialog.
            let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
            drop.connect_drop(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                false,
                move |_, value, _, _| {
                    if let Ok(list) = value.get::<gdk::FileList>() {
                        // A dropped file may carry a real path the sandbox has no
                        // permission for (unlike Add Files/Folder, which grant
                        // access via the file-chooser portal). Stage only the
                        // readable ones and flag any that were skipped.
                        let (ok, skipped): (Vec<PathBuf>, Vec<PathBuf>) = list
                            .files()
                            .iter()
                            .filter_map(|f| f.path())
                            .partition(|p| path_is_accessible(p));
                        obj.add_inputs(ok);
                        if !skipped.is_empty() {
                            obj.warn_dropped_inaccessible();
                        }
                        true
                    } else {
                        false
                    }
                }
            ));
            obj.add_controller(drop);
            obj.rebuild_files();
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
    pub fn new() -> Self {
        Self::default()
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.root().and_downcast::<gtk::Window>()
    }

    fn pick_files(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Add Files"))
            .modal(true)
            .build();
        let obj = self.clone();
        dialog.open_multiple(self.parent_window().as_ref(), gio::Cancellable::NONE, move |res| {
            if let Ok(files) = res {
                obj.add_inputs(model_paths(&files));
            }
        });
    }

    fn pick_folders(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Add Folders"))
            .modal(true)
            .build();
        let obj = self.clone();
        dialog.select_multiple_folders(self.parent_window().as_ref(), gio::Cancellable::NONE, move |res| {
            if let Ok(files) = res {
                obj.add_inputs(model_paths(&files));
            }
        });
    }

    /// Append inputs (files or folders), de-duplicated, and refresh the list.
    pub fn add_inputs(&self, paths: Vec<PathBuf>) {
        let imp = self.imp();
        {
            let mut inputs = imp.inputs.borrow_mut();
            for path in paths {
                if !inputs.contains(&path) {
                    inputs.push(path);
                }
            }
        }
        if imp.name_row.text().is_empty() {
            if let Some(name) = imp
                .inputs
                .borrow()
                .first()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
            {
                imp.name_row.set_text(&name);
            }
        }
        self.rebuild_files();
    }

    fn remove_input(&self, path: &Path) {
        self.imp().inputs.borrow_mut().retain(|p| p != path);
        self.rebuild_files();
    }

    /// Tell the user a dropped item couldn't be read and point them at the
    /// buttons, which go through the file-chooser portal and always work.
    fn warn_dropped_inaccessible(&self) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext("Some items couldn't be added")),
            Some(&gettext(
                "They were dropped in a way the sandbox can't read. Add them with the Add Files or Add Folder button instead.",
            )),
        );
        dialog.add_response("close", &gettext("Close"));
        dialog.set_default_response(Some("close"));
        dialog.present(Some(self));
    }

    fn rebuild_files(&self) {
        let imp = self.imp();
        for row in imp.input_rows.borrow_mut().drain(..) {
            imp.files_group.remove(&row);
        }
        let inputs = imp.inputs.borrow();
        for path in inputs.iter() {
            let row = adw::ActionRow::builder()
                .title(file_name(path))
                .subtitle(path.to_string_lossy())
                .build();
            let icon = if path.is_dir() {
                "folder-symbolic"
            } else {
                "text-x-generic-symbolic"
            };
            row.add_prefix(&gtk::Image::from_icon_name(icon));

            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.add_css_class("flat");
            remove.set_valign(gtk::Align::Center);
            remove.set_tooltip_text(Some(&gettext("Remove")));
            let owned = path.clone();
            remove.connect_clicked(glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| obj.remove_input(&owned)
            ));
            row.add_suffix(&remove);

            imp.files_group.add(&row);
            imp.input_rows.borrow_mut().push(row);
        }
        imp.create_button.set_sensitive(!inputs.is_empty());
        drop(inputs);
        self.start_measure();
    }

    /// Total up the staged selection on a background thread and report it under
    /// the Files heading. Walking a deep tree over portal FUSE mounts costs a
    /// `stat` per entry, so this must never run on the UI thread — measuring the
    /// selection is not worth freezing the dialog that stages it.
    fn start_measure(&self) {
        let imp = self.imp();

        // A walk for the previous selection is now answering the wrong question.
        if let Some(old) = imp.measure_cancel.take() {
            old.store(true, Ordering::Relaxed);
        }

        let inputs = imp.inputs.borrow().clone();
        if inputs.is_empty() {
            imp.files_group
                .set_description(Some(&gettext("Drop files or folders here, or add them.")));
            return;
        }

        let generation = imp.measure_gen.get().wrapping_add(1);
        imp.measure_gen.set(generation);
        imp.files_group.set_description(Some(&gettext("Measuring…")));

        let cancel = new_cancel_token();
        imp.measure_cancel.replace(Some(cancel.clone()));

        let (sender, receiver) = async_channel::bounded::<Selection>(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(measure_selection(&inputs, &cancel));
        });

        let dialog = self.clone();
        glib::spawn_future_local(async move {
            if let Ok(total) = receiver.recv().await {
                // Only the newest walk gets to speak; an older one is stale.
                if dialog.imp().measure_gen.get() == generation {
                    dialog.show_selection_total(total);
                }
            }
        });
    }

    fn show_selection_total(&self, total: Selection) {
        let files = if total.files == 1 {
            gettext("1 file")
        } else {
            format!("{} {}", total.files, gettext("files"))
        };
        // A truncated walk undercounts, so say "over" rather than claim a total.
        let mut text = if total.truncated {
            format!(
                "{} {} · {} {}",
                gettext("Over"),
                files,
                gettext("over"),
                glib::format_size(total.bytes)
            )
        } else {
            format!("{} · {}", files, glib::format_size(total.bytes))
        };

        if total.truncated
            || total.bytes >= LARGE_SELECTION_BYTES
            || total.files >= LARGE_SELECTION_FILES
        {
            text.push_str("  —  ");
            text.push_str(&gettext(
                "that's a big selection and will take a while. Check you meant to include everything (build and cache folders add up fast).",
            ));
        }
        self.imp().files_group.set_description(Some(&text));
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
            inputs: imp.inputs.borrow().clone(),
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

    // --- Presets ------------------------------------------------------------

    /// Current settings captured as a named preset (no password).
    fn current_preset(&self, name: String) -> Preset {
        let s = self.settings();
        Preset {
            name,
            format: s.format.id.to_string(),
            codec: s.codec.id.to_string(),
            level: s.level,
            threads: s.threads,
            dictionary: s.dictionary,
            solid: s.solid,
            volume_size: s.volume_size,
            bcj: s.bcj,
            encrypt_headers: s.encrypt_headers,
            extra_params: s.extra_params,
        }
    }

    /// Apply a preset to the dialog. Order matters: format first (rebuilds the
    /// codec list), then codec (sets the level range), then the explicit values.
    fn apply_preset(&self, p: &Preset) {
        let imp = self.imp();
        imp.format_row.set_selected(format_index(&p.format));
        imp.codec_row
            .set_selected(codec_index(imp.current_format(), &p.codec));
        if let Some(level) = p.level {
            imp.level_row.set_value(level as f64);
        }
        imp.threads_row.set_value(p.threads as f64);
        imp.dictionary_row.set_selected(dict_index(p.dictionary.as_deref()));
        if let Some(solid) = p.solid {
            imp.solid_row.set_active(solid);
        }
        imp.volume_row.set_selected(vol_index(p.volume_size.as_deref()));
        imp.bcj_row.set_active(p.bcj);
        imp.encrypt_headers_row.set_active(p.encrypt_headers);
        imp.params_row.set_text(&p.extra_params.join(" "));
        imp.update_memory();
    }

    fn rebuild_presets_popover(&self) {
        let store = PresetStore::new();
        let popover = gtk::Popover::new();
        let vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .width_request(220)
            .build();

        if !store.is_available() {
            vbox.append(&dim_label(&gettext("Presets are saved in the installed app.")));
            popover.set_child(Some(&vbox));
            self.imp().presets_button.set_popover(Some(&popover));
            return;
        }

        let presets = store.list();
        if presets.is_empty() {
            vbox.append(&dim_label(&gettext("No presets yet.")));
        }
        for preset in presets {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let apply = gtk::Button::builder().label(&preset.name).hexpand(true).build();
            apply.add_css_class("flat");
            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.add_css_class("flat");
            delete.set_tooltip_text(Some(&gettext("Delete")));

            apply.connect_clicked(glib::clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                popover,
                #[strong]
                preset,
                move |_| {
                    obj.apply_preset(&preset);
                    popover.popdown();
                }
            ));
            delete.connect_clicked(glib::clone!(
                #[weak(rename_to = obj)]
                self,
                #[strong(rename_to = name)]
                preset.name,
                move |_| {
                    PresetStore::new().delete(&name);
                    obj.rebuild_presets_popover();
                }
            ));
            row.append(&apply);
            row.append(&delete);
            vbox.append(&row);
        }

        vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let save = gtk::Button::with_label(&gettext("Save current settings…"));
        save.add_css_class("flat");
        save.connect_clicked(glib::clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                obj.prompt_save_preset();
            }
        ));
        vbox.append(&save);

        popover.set_child(Some(&vbox));
        self.imp().presets_button.set_popover(Some(&popover));
    }

    fn prompt_save_preset(&self) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext("Save Preset")),
            Some(&gettext("Name this set of compression settings.")),
        );
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("save", &gettext("Save"));
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let entry = gtk::Entry::builder().activates_default(true).build();
        dialog.set_extra_child(Some(&entry));

        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, response| {
                    if response == "save" {
                        let name = entry.text().trim().to_string();
                        if !name.is_empty() {
                            PresetStore::new().save(obj.current_preset(name));
                            obj.rebuild_presets_popover();
                        }
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }
}

fn model_paths(model: &gio::ListModel) -> Vec<PathBuf> {
    (0..model.n_items())
        .filter_map(|i| model.item(i).and_downcast::<gio::File>())
        .filter_map(|f| f.path())
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether the sandbox can actually reach `path`. A dropped file may carry a
/// real path the sandbox has no permission for; opening it (works for files and
/// directories on Unix) probes that without reading contents.
fn path_is_accessible(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}

fn dim_label(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).wrap(true).xalign(0.0).build();
    label.add_css_class("dim-label");
    label
}

fn format_index(id: &str) -> u32 {
    formats().iter().position(|f| f.id == id).unwrap_or(0) as u32
}

fn codec_index(fmt: &Format, id: &str) -> u32 {
    fmt.codecs.iter().position(|c| c.id == id).unwrap_or(0) as u32
}

fn dict_index(arg: Option<&str>) -> u32 {
    DICT_PRESETS.iter().position(|(_, a, _)| *a == arg).unwrap_or(0) as u32
}

fn vol_index(arg: Option<&str>) -> u32 {
    VOLUME_PRESETS.iter().position(|(_, a)| *a == arg).unwrap_or(0) as u32
}
