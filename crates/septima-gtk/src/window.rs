use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib, CompositeTemplate, TemplateChild};

use septima_engine::{
    CompressionRequest, EngineError, ExtractProgress, ExtractRequest, OverwriteMode,
};

use crate::archive_view::SeptimaArchiveView;
use crate::create_dialog::{CreateSettings, SeptimaCreateDialog};
use crate::progress_row::SeptimaProgressRow;

/// Messages from the extraction worker thread to the UI.
enum Job {
    Progress(ExtractProgress),
    Done(Result<(), EngineError>),
}

fn gettext(s: &str) -> String {
    gettextrs::gettext(s)
}

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
        #[template_child]
        pub extract_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub jobs_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub jobs_box: TemplateChild<gtk::Box>,
        /// The currently open archive, used as the extract source.
        pub archive_path: RefCell<Option<PathBuf>>,
        /// Password the current archive was opened with (reused for extraction).
        pub archive_password: RefCell<Option<String>>,
    }

    #[gtk::template_callbacks]
    impl SeptimaWindow {
        #[template_callback]
        fn on_open_clicked(&self) {
            self.obj().open_archive_dialog();
        }

        #[template_callback]
        fn on_extract_clicked(&self) {
            self.obj().choose_destination_and_extract();
        }

        #[template_callback]
        fn on_new_clicked(&self) {
            self.obj().new_archive_dialog();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaWindow {
        const NAME: &'static str = "SeptimaWindow";
        type Type = super::SeptimaWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
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

            let checksums = gio::ActionEntry::builder("checksums")
                .activate(|window: &super::SeptimaWindow, _, _| window.open_checksums())
                .build();
            self.obj().add_action_entries([checksums]);
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

    // --- Open ---------------------------------------------------------------

    fn open_archive_dialog(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Open Archive"))
            .modal(true)
            .build();

        let window = self.clone();
        dialog.open(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(file) => window.open_file(file),
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    /// Open and list `file` (file chooser, CLI args, or a file manager).
    pub fn open_file(&self, file: gio::File) {
        let Some(path) = file.path() else {
            self.show_toast(&gettext("That location can't be read directly."));
            return;
        };
        self.load_archive(path, None);
    }

    /// List `path` (optionally with `password`); on an encrypted archive, prompt
    /// for a password and retry. The working password is remembered so extraction
    /// doesn't ask again.
    fn load_archive(&self, path: PathBuf, password: Option<String>) {
        let window = self.clone();
        let sevenzip = septima_engine::sevenzip_path();
        let task_path = path.clone();
        let task_password = password.clone();

        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                septima_engine::list_archive(&sevenzip, &task_path, task_password.as_deref())
            })
            .await;

            match result {
                Ok(Ok(listing)) => {
                    let imp = window.imp();
                    let archive_path = listing.path.clone();
                    imp.archive_view.load(&listing);
                    imp.stack.set_visible_child_name("archive");
                    imp.extract_button.set_sensitive(true);
                    imp.archive_path.replace(Some(archive_path.clone()));
                    imp.archive_password.replace(password.clone());
                    // Dev/test hook: extract without the folder portal.
                    if crate::config::PROFILE == "Devel" {
                        if let Some(dir) = std::env::var_os("SEPTIMA_AUTO_EXTRACT") {
                            window.start_extract(archive_path, PathBuf::from(dir), password);
                        }
                    }
                }
                Ok(Err(EngineError::PasswordRequired)) => {
                    let retry = window.clone();
                    window.prompt_password(
                        &gettext("This archive is encrypted. Enter its password to open it."),
                        move |pw| retry.load_archive(path.clone(), Some(pw)),
                    );
                }
                Ok(Err(err)) => window.show_error(&err.to_string()),
                Err(_) => window.show_toast(&gettext("The listing task failed.")),
            }
        });
    }

    // --- Extract ------------------------------------------------------------

    fn choose_destination_and_extract(&self) {
        let Some(archive) = self.imp().archive_path.borrow().clone() else {
            return;
        };
        let password = self.imp().archive_password.borrow().clone();

        let dialog = gtk::FileDialog::builder()
            .title(gettext("Extract To"))
            .modal(true)
            .build();

        let window = self.clone();
        dialog.select_folder(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(folder) => match folder.path() {
                Some(dest) => window.start_extract(archive.clone(), dest, password.clone()),
                None => window.show_toast(&gettext("That folder can't be written to directly.")),
            },
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    fn start_extract(&self, archive: PathBuf, dest: PathBuf, password: Option<String>) {
        let name = file_name(&archive);
        let row = SeptimaProgressRow::new(&format!("{}: {name}", gettext("Extracting")));
        let imp = self.imp();
        imp.jobs_box.append(&row);
        imp.jobs_revealer.set_reveal_child(true);

        let cancel = septima_engine::new_cancel_token();
        let cancel_ui = cancel.clone();
        row.connect_cancel(move || cancel_ui.store(true, Ordering::Relaxed));

        let (sender, receiver) = async_channel::unbounded::<Job>();
        let sevenzip = septima_engine::sevenzip_path();
        let req = ExtractRequest {
            archive: archive.clone(),
            dest_dir: dest.clone(),
            password,
            overwrite: OverwriteMode::default(),
        };

        std::thread::spawn(move || {
            let result = septima_engine::run_extract(&sevenzip, &req, &cancel, |p| {
                let _ = sender.send_blocking(Job::Progress(p.clone()));
            });
            let _ = sender.send_blocking(Job::Done(result));
        });

        let window = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(message) = receiver.recv().await {
                match message {
                    Job::Progress(p) => row.set_progress(p.percent, p.current_file.as_deref()),
                    Job::Done(result) => {
                        window.finish_job(&row);
                        match result {
                            Ok(()) => window.show_toast(&format!(
                                "{} {}",
                                gettext("Extracted to"),
                                dest.display()
                            )),
                            Err(EngineError::Cancelled) => {} // silent
                            Err(EngineError::PasswordRequired) => {
                                let retry = window.clone();
                                let (archive, dest) = (archive.clone(), dest.clone());
                                window.prompt_password(
                                    &gettext("This archive is encrypted. Enter its password to extract."),
                                    move |pw| retry.start_extract(archive.clone(), dest.clone(), Some(pw)),
                                );
                            }
                            Err(err) => window.show_error(&err.to_string()),
                        }
                        break;
                    }
                }
            }
        });
    }

    fn finish_job(&self, row: &SeptimaProgressRow) {
        let imp = self.imp();
        imp.jobs_box.remove(row);
        if imp.jobs_box.first_child().is_none() {
            imp.jobs_revealer.set_reveal_child(false);
        }
    }

    /// Ask for a password; `on_password` runs with the entered text on Unlock.
    fn prompt_password<F: Fn(String) + 'static>(&self, body: &str, on_password: F) {
        let dialog = adw::AlertDialog::new(Some(&gettext("Password Required")), Some(body));
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("unlock", &gettext("Unlock"));
        dialog.set_response_appearance("unlock", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("unlock"));
        dialog.set_close_response("cancel");

        let entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .activates_default(true)
            .build();
        dialog.set_extra_child(Some(&entry));

        dialog.connect_response(None, move |_, response| {
            if response == "unlock" {
                on_password(entry.text().to_string());
            }
        });
        dialog.present(Some(self));
    }

    // --- Create ------------------------------------------------------------

    fn new_archive_dialog(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Add Files to Archive"))
            .modal(true)
            .build();

        let window = self.clone();
        dialog.open_multiple(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(files) => {
                let inputs: Vec<PathBuf> = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                    .filter_map(|f| f.path())
                    .collect();
                if !inputs.is_empty() {
                    window.show_create_dialog(inputs);
                }
            }
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    fn show_create_dialog(&self, inputs: Vec<PathBuf>) {
        let suggested = inputs
            .first()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".to_string());

        let dialog = SeptimaCreateDialog::new(&suggested);
        let window = self.clone();
        dialog.connect_create(move |dlg| {
            let settings = dlg.settings();
            dlg.close();
            window.choose_output_and_compress(inputs.clone(), settings);
        });
        dialog.present(Some(self));
    }

    fn choose_output_and_compress(&self, inputs: Vec<PathBuf>, settings: CreateSettings) {
        let filename = format!("{}.{}", settings.name, archive_extension(&settings));
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Save Archive"))
            .modal(true)
            .initial_name(&filename)
            .build();

        let window = self.clone();
        dialog.save(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(file) => match file.path() {
                Some(output) => window.start_compress(compression_request(&inputs, &settings, output)),
                None => window.show_toast(&gettext("That location can't be written to directly.")),
            },
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    fn start_compress(&self, req: CompressionRequest) {
        let output = req.output.clone();
        let row = SeptimaProgressRow::new(&format!("{}: {}", gettext("Creating"), file_name(&output)));
        let imp = self.imp();
        imp.jobs_box.append(&row);
        imp.jobs_revealer.set_reveal_child(true);

        let cancel = septima_engine::new_cancel_token();
        let cancel_ui = cancel.clone();
        row.connect_cancel(move || cancel_ui.store(true, Ordering::Relaxed));

        let (sender, receiver) = async_channel::unbounded::<Job>();
        let sevenzip = septima_engine::sevenzip_path();

        std::thread::spawn(move || {
            let progress = |p: &ExtractProgress| {
                let _ = sender.send_blocking(Job::Progress(p.clone()));
            };
            // tar + a real compressor produces a .tar.<ext> in two steps.
            let result = if req.format == "tar" && req.codec.as_deref().is_some_and(|c| c != "copy") {
                septima_engine::run_tar_and_compress(&sevenzip, &req, &cancel, progress)
            } else {
                septima_engine::run_add(&sevenzip, &req, &cancel, progress)
            };
            let _ = sender.send_blocking(Job::Done(result));
        });

        let window = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(message) = receiver.recv().await {
                match message {
                    Job::Progress(p) => row.set_progress(p.percent, p.current_file.as_deref()),
                    Job::Done(result) => {
                        window.finish_job(&row);
                        match result {
                            Ok(()) => window.show_toast(&format!(
                                "{} {}",
                                gettext("Created"),
                                output.display()
                            )),
                            Err(EngineError::Cancelled) => {}
                            Err(err) => window.show_error(&err.to_string()),
                        }
                        break;
                    }
                }
            }
        });
    }

    fn open_checksums(&self) {
        crate::hash_dialog::SeptimaHashDialog::new().present(Some(self));
    }

    fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    /// Show a full (possibly long) error in a dialog — toasts truncate.
    fn show_error(&self, message: &str) {
        let dialog =
            adw::AlertDialog::new(Some(&gettext("Something Went Wrong")), Some(message.trim()));
        dialog.add_response("close", &gettext("Close"));
        dialog.set_default_response(Some("close"));
        dialog.present(Some(self));
    }
}

/// Full file extension for the chosen settings, e.g. `7z`, `zip`, `tar.zst`.
fn archive_extension(settings: &CreateSettings) -> String {
    if settings.format.id == "tar" {
        match settings.codec.id {
            "zstd" => "tar.zst",
            "xz" => "tar.xz",
            "gzip" => "tar.gz",
            "bzip2" => "tar.bz2",
            _ => "tar",
        }
        .to_string()
    } else {
        settings.format.extension.to_string()
    }
}

fn compression_request(
    inputs: &[PathBuf],
    settings: &CreateSettings,
    output: PathBuf,
) -> CompressionRequest {
    let mut req = CompressionRequest::new(output, inputs.to_vec(), settings.format.id);
    req.codec = Some(settings.codec.id.to_string());
    req.level = settings.level;
    req.threads = Some(settings.threads);
    req.dictionary = settings.dictionary.clone();
    req.solid = settings.solid;
    req.volume_size = settings.volume_size.clone();
    req.bcj = settings.bcj;
    req.password = settings.password.clone();
    req.encrypt_headers = settings.encrypt_headers;
    req.extra_params = settings.extra_params.clone();
    req
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
