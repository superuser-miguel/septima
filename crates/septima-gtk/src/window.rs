use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib, CompositeTemplate, TemplateChild};

use septima_engine::{
    CompressionRequest, EngineError, ExtractProgress, ExtractRequest, Manifest, ManifestEntry,
    OverwriteMode,
};

use crate::archive_view::SeptimaArchiveView;
use crate::create_dialog::{CreateSettings, SeptimaCreateDialog};
use crate::progress_row::SeptimaProgressRow;

/// Messages from the extraction worker thread to the UI.
enum Job {
    Progress(ExtractProgress),
    Done(Result<(), EngineError>),
}

/// Shared state of a batch run's passwords file. Entries are updated on the
/// main thread as jobs finish; every write is a fresh, complete snapshot
/// serialised through one writer task, so the file on disk is always whole.
struct BatchManifest {
    manifest: RefCell<Manifest>,
    dest: PathBuf,
    /// `Some` = GPG-protect the file; captured before the batch started.
    passphrase: Option<String>,
}

impl BatchManifest {
    /// Snapshot → (optionally encrypt) → atomic write, off the main thread.
    /// In protected mode the plaintext exists only in memory, never on disk.
    async fn write(&self) -> Result<(), String> {
        let json = self.manifest.borrow().to_json();
        let dest = self.dest.clone();
        let passphrase = self.passphrase.clone();
        let outcome = gio::spawn_blocking(move || {
            let bytes = match &passphrase {
                Some(p) => {
                    septima_engine::encrypt_symmetric(json.as_bytes(), p).map_err(|e| e.to_string())?
                }
                None => json.into_bytes(),
            };
            septima_engine::write_atomic(&dest, &bytes).map_err(|e| e.to_string())
        })
        .await;
        outcome.map_err(|_| gettext("the write task failed"))?
    }
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
        pub window_title: TemplateChild<adw::WindowTitle>,
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
            let close = gio::ActionEntry::builder("close-archive")
                .activate(|window: &super::SeptimaWindow, _, _| window.close_archive())
                .build();
            let test_archive = gio::ActionEntry::builder("test-archive")
                .activate(|window: &super::SeptimaWindow, _, _| window.test_archive())
                .build();
            let obj = self.obj();
            obj.add_action_entries([checksums, close, test_archive]);
            obj.set_archive_actions_enabled(false);

            // Drop an archive onto the window to open it; drop several to
            // batch-extract them instead (see open_or_batch_extract).
            let drop = gtk::DropTarget::new(
                gtk::gdk::FileList::static_type(),
                gtk::gdk::DragAction::COPY,
            );
            drop.connect_drop(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                false,
                move |_, value, _, _| {
                    if let Ok(list) = value.get::<gtk::gdk::FileList>() {
                        let files = list.files();
                        if !files.is_empty() {
                            obj.open_or_batch_extract(files);
                            return true;
                        }
                    }
                    false
                }
            ));
            obj.add_controller(drop);

            self.archive_view.connect_delete_requested(glib::clone!(
                #[weak]
                obj,
                move |paths| obj.delete_entries(paths)
            ));
            self.archive_view.connect_rename_requested(glib::clone!(
                #[weak]
                obj,
                move |path| obj.rename_entry(path)
            ));
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

    /// Pick one archive to open and browse, or several to batch-extract.
    fn open_archive_dialog(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Open Archive"))
            .modal(true)
            .build();

        let window = self.clone();
        dialog.open_multiple(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(files) => window.open_or_batch_extract(files.iter::<gio::File>().filter_map(Result::ok).collect()),
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    /// A single file opens for browsing (unchanged); two or more trigger a
    /// batch-extract flow instead — one at a time isn't what someone picking
    /// several archives at once wants.
    fn open_or_batch_extract(&self, mut files: Vec<gio::File>) {
        match files.len() {
            0 => {}
            1 => self.open_file(files.remove(0)),
            _ => self.confirm_batch_extract(files),
        }
    }

    /// Open and list `file` (file chooser, CLI args, or a file manager).
    pub fn open_file(&self, file: gio::File) {
        let Some(path) = file.path() else {
            self.show_toast(&gettext("That location can't be read directly."));
            return;
        };
        // A dropped or command-line file may carry a real path the sandbox has
        // no permission to read — unlike the file chooser, which grants access
        // via the document portal. Catch that here so it surfaces as guidance
        // rather than a cryptic "cannot open the file as archive" from 7zz.
        // (Opening works for files and directories on Unix; contents unread.)
        if std::fs::File::open(&path).is_err() {
            self.show_error(&gettext(
                "Septima couldn't read that file. If you dragged it in, open it \
                 with the Open Archive button instead — some apps hand over \
                 files in a way the sandbox can't access.",
            ));
            return;
        }
        // A passwords file opens the batch-decrypt flow, not the archive view.
        if is_manifest_name(&path) {
            self.open_manifest(path);
            return;
        }
        self.load_archive(path, None);
    }

    // --- Passwords-file (manifest) decrypt ----------------------------------

    /// Open a passwords file: decrypt it if GPG-protected (prompting for its
    /// passphrase), parse it, and offer to extract the archives it lists.
    fn open_manifest(&self, path: PathBuf) {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(err) => {
                self.show_error(&format!("{} {err}", gettext("The passwords file couldn't be read:")));
                return;
            }
        };
        if septima_engine::looks_gpg_encrypted(&bytes) {
            let window = self.clone();
            let body = gettext("“{}” is protected. Enter the passwords file's password to open it.")
                .replacen("{}", &file_name(&path), 1);
            self.prompt_password(&body, move |passphrase| {
                window.decrypt_and_parse_manifest(path.clone(), bytes.clone(), passphrase);
            });
        } else {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            self.parse_manifest(path, &text);
        }
    }

    fn decrypt_and_parse_manifest(&self, path: PathBuf, bytes: Vec<u8>, passphrase: String) {
        let window = self.clone();
        glib::spawn_future_local(async move {
            let outcome =
                gio::spawn_blocking(move || septima_engine::decrypt_symmetric(&bytes, &passphrase))
                    .await;
            match outcome {
                Ok(Ok(plain)) => {
                    window.parse_manifest(path, &String::from_utf8_lossy(&plain));
                }
                Ok(Err(septima_engine::GpgError::WrongPassphrase)) => {
                    window.show_toast(&gettext("Wrong password for this file."));
                    window.open_manifest(path); // re-read + re-prompt
                }
                Ok(Err(err)) => window.show_error(&err.to_string()),
                Err(_) => window.show_toast(&gettext("The decrypt task failed.")),
            }
        });
    }

    fn parse_manifest(&self, path: PathBuf, text: &str) {
        match Manifest::parse(text) {
            Ok(manifest) => self.confirm_manifest_extract(path, manifest),
            Err(err) => self.show_error(&format!(
                "{}\n\n{err}",
                gettext("This doesn't look like a Septima passwords file.")
            )),
        }
    }

    /// Offer to batch-extract the archives a passwords file lists, each with
    /// its own recorded password. Archives resolve next to the manifest (the
    /// manifest stores basenames only — and only the basename is trusted, so a
    /// hand-edited "../evil" can't walk out of the folder).
    fn confirm_manifest_extract(&self, manifest_path: PathBuf, manifest: Manifest) {
        let dir = manifest_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut jobs: Vec<(PathBuf, String)> = Vec::new();
        let mut missing = 0usize;
        let mut passwordless = 0usize;
        for entry in &manifest.entries {
            // Never trim, never guess: an empty password row is skipped loudly.
            if entry.password.is_empty() {
                passwordless += 1;
                continue;
            }
            let Some(name) = Path::new(&entry.archive).file_name() else {
                missing += 1;
                continue;
            };
            let archive = dir.join(name);
            if archive.is_file() {
                jobs.push((archive, entry.password.clone()));
            } else {
                missing += 1;
            }
        }

        if jobs.is_empty() {
            self.show_error(&gettext(
                "None of the archives in this passwords file were found next to it. \
                 Move the file into the folder that holds the archives, then open it again.",
            ));
            return;
        }

        let mut body = n_manifest_body(jobs.len());
        if missing > 0 {
            body.push(' ');
            body.push_str(&n_manifest_missing(missing));
        }
        if passwordless > 0 {
            body.push(' ');
            body.push_str(&n_manifest_passwordless(passwordless));
        }

        let delete_after = gtk::CheckButton::builder()
            .label(gettext("Delete the archives after extracting"))
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Extract From Passwords File"))
            .body(body)
            .extra_child(&delete_after)
            .build();
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("extract", &gettext("Extract"));
        dialog.set_response_appearance("extract", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("extract"));

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "extract" {
                return;
            }
            for (archive, password) in &jobs {
                let dest = sibling_extract_dir(archive);
                window.start_extract(
                    archive.clone(),
                    dest,
                    Some(password.clone()),
                    delete_after.is_active(),
                );
            }
        });
        dialog.present(Some(self));
    }

    /// Confirm, then extract every archive in `files` into a new sibling
    /// folder next to itself (e.g. `photos.zip` -> `photos/`) — one Extract
    /// job per archive, run independently, no per-archive prompts.
    fn confirm_batch_extract(&self, files: Vec<gio::File>) {
        let skipped = files.len();
        let archives: Vec<PathBuf> = files
            .into_iter()
            .filter_map(|f| f.path())
            .filter(|p| std::fs::File::open(p).is_ok())
            .collect();
        let skipped = skipped - archives.len();
        if archives.is_empty() {
            self.show_error(&gettext(
                "Septima couldn't read those files. If you dragged them in, use the \
                 Open Archive button instead — some apps hand over files in a way \
                 the sandbox can't access.",
            ));
            return;
        }

        // These batches usually share one password, so offer a single field
        // applied to every archive — no need to retype it per archive. An
        // archive with a different (or no) password falls back to the named
        // per-archive prompt in start_extract.
        let password_entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .placeholder_text(gettext("Password (only if encrypted)"))
            .build();
        let delete_after = gtk::CheckButton::builder()
            .label(gettext("Delete the archives after extracting"))
            .build();
        let extra = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        extra.append(&password_entry);
        extra.append(&delete_after);

        let body = if skipped > 0 {
            format!(
                "{} {}",
                n_archives_body(archives.len()),
                n_skipped(skipped)
            )
        } else {
            n_archives_body(archives.len())
        };
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Extract Archives"))
            .body(body)
            .extra_child(&extra)
            .build();
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("extract", &gettext("Extract"));
        dialog.set_response_appearance("extract", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("extract"));

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "extract" {
                return;
            }
            let pw = password_entry.text().to_string();
            let password = (!pw.is_empty()).then_some(pw);
            for archive in &archives {
                let dest = sibling_extract_dir(archive);
                window.start_extract(archive.clone(), dest, password.clone(), delete_after.is_active());
            }
        });
        dialog.present(Some(self));
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
                    imp.window_title.set_title(&file_name(&archive_path));
                    imp.window_title.set_subtitle(&format!(
                        "{} · {}",
                        n_files(listing.file_count()),
                        glib::format_size(listing.total_size())
                    ));
                    window.set_archive_actions_enabled(true);
                    // Dev/test hook: extract without the folder portal.
                    if crate::config::PROFILE == "Devel" {
                        if let Some(dir) = std::env::var_os("SEPTIMA_AUTO_EXTRACT") {
                            window.start_extract(archive_path, PathBuf::from(dir), password, false);
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
                Some(dest) => window.confirm_extract(archive.clone(), dest, password.clone()),
                None => window.show_toast(&gettext("That folder can't be written to directly.")),
            },
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    /// Offer "delete the archive afterwards" before starting the extract job.
    fn confirm_extract(&self, archive: PathBuf, dest: PathBuf, password: Option<String>) {
        let delete_after = gtk::CheckButton::builder()
            .label(gettext("Delete the archive after extracting"))
            .build();

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Extract Archive"))
            .body(gettext("Extract the contents of this archive to the chosen folder?"))
            .extra_child(&delete_after)
            .build();
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("extract", &gettext("Extract"));
        dialog.set_response_appearance("extract", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("extract"));

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "extract" {
                window.start_extract(
                    archive.clone(),
                    dest.clone(),
                    password.clone(),
                    delete_after.is_active(),
                );
            }
        });
        dialog.present(Some(self));
    }

    fn start_extract(&self, archive: PathBuf, dest: PathBuf, password: Option<String>, delete_after: bool) {
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
                            Ok(()) => {
                                window.show_extracted_toast(&dest);
                                if delete_after {
                                    let archive = archive.clone();
                                    let window = window.clone();
                                    glib::spawn_future_local(async move {
                                        let outcome =
                                            gio::spawn_blocking(move || septima_engine::delete_archive(&archive))
                                                .await;
                                        if !matches!(outcome, Ok(Ok(()))) {
                                            window.show_toast(&gettext(
                                                "Extracted, but the archive couldn't be deleted.",
                                            ));
                                        }
                                    });
                                }
                            }
                            Err(EngineError::Cancelled) => {} // silent
                            Err(EngineError::PasswordRequired) => {
                                let retry = window.clone();
                                let (archive, dest) = (archive.clone(), dest.clone());
                                let body = gettext("“{}” is encrypted. Enter its password to extract.")
                                    .replacen("{}", &file_name(&archive), 1);
                                window.prompt_password(&body, move |pw| {
                                    retry.start_extract(
                                        archive.clone(),
                                        dest.clone(),
                                        Some(pw),
                                        delete_after,
                                    )
                                });
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
        let dialog = SeptimaCreateDialog::new();
        let window = self.clone();
        dialog.connect_create(move |dlg| {
            let settings = dlg.settings();
            if settings.inputs.is_empty() {
                return;
            }
            dlg.close();
            // Generated per-archive passwords only make sense through the batch
            // flow (the password lives in the manifest, not in the user's head),
            // so it takes that path even for a single staged item.
            if settings.batch_mode && (settings.inputs.len() >= 2 || settings.generate_passwords) {
                window.confirm_batch_compress(settings);
            } else {
                window.choose_output_and_compress(settings);
            }
        });
        dialog.present(Some(self));
    }

    fn choose_output_and_compress(&self, settings: CreateSettings) {
        let filename = archive_filename(&settings.name, &archive_extension(&settings));
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Save Archive"))
            .modal(true)
            .initial_name(&filename)
            .build();

        let window = self.clone();
        dialog.save(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(file) => match file.path() {
                Some(output) => {
                    let write_checksum = settings.write_checksum;
                    let inputs = settings.inputs.clone();
                    window.start_compress(compression_request(&settings, inputs, output), write_checksum, None)
                }
                None => window.show_toast(&gettext("That location can't be written to directly.")),
            },
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    /// Confirm, then compress each staged item into its own archive saved
    /// next to it (e.g. `dir1/` -> `dir1.7z`) — no per-item Save dialog.
    fn confirm_batch_compress(&self, settings: CreateSettings) {
        let items = settings.inputs.clone();
        let ext = archive_extension(&settings);

        let dialog = adw::AlertDialog::new(
            Some(&gettext("Create Archives")),
            Some(&n_archives_create_body(items.len())),
        );
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("create", &gettext("Create"));
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("create"));

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "create" {
                return;
            }
            if settings.generate_passwords {
                window.choose_manifest_and_batch(settings.clone(), items.clone(), ext.clone());
                return;
            }
            for item in &items {
                let Some(stem) = item.file_stem() else {
                    continue;
                };
                let output = item.with_file_name(format!("{}.{ext}", stem.to_string_lossy()));
                let req = compression_request(&settings, vec![item.clone()], output);
                window.start_compress(req, settings.write_checksum, None);
            }
        });
        dialog.present(Some(self));
    }

    /// Pick where the batch's passwords file lands, then run the batch.
    /// Cancelling here cancels the whole batch — no manifest, no archives.
    fn choose_manifest_and_batch(&self, settings: CreateSettings, items: Vec<PathBuf>, ext: String) {
        let stamp = glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.format("%Y-%m-%d_%H%M").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let suffix = if settings.manifest_passphrase.is_some() { "json.gpg" } else { "json" };
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Save Passwords File"))
            .modal(true)
            .initial_name(&format!("septima-passwords-{stamp}.{suffix}"))
            .build();

        let window = self.clone();
        dialog.save(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(file) => match file.path() {
                Some(dest) => window.start_batch_with_manifest(settings, items, ext, dest),
                None => window.show_toast(&gettext("That location can't be written to directly.")),
            },
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    /// The generated-passwords batch. Order is deliberate: every password is
    /// persisted to the passwords file **before** any archive exists, so a
    /// crash mid-batch can never leave an archive on disk whose password is
    /// lost. As each job finishes, its sha256 lands and the file is rewritten
    /// (atomically, one writer, freshest snapshot).
    fn start_batch_with_manifest(
        &self,
        settings: CreateSettings,
        items: Vec<PathBuf>,
        ext: String,
        dest: PathBuf,
    ) {
        let now = glib::DateTime::now_utc()
            .ok()
            .and_then(|d| d.format_iso8601().ok())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let mut manifest = Manifest::new();
        manifest.septima = crate::config::VERSION.to_string();
        manifest.created = now.clone();
        let mut jobs: Vec<(PathBuf, PathBuf, String)> = Vec::new();
        for item in &items {
            let Some(stem) = item.file_stem() else {
                continue;
            };
            let output = item.with_file_name(format!("{}.{ext}", stem.to_string_lossy()));
            let password = match septima_engine::generate_password(64, septima_engine::Charset::Alphanumeric)
            {
                Ok(pw) => pw,
                Err(err) => {
                    // No password, no batch — never fall back to something weaker.
                    self.show_error(&format!(
                        "{} {err}",
                        gettext("Passwords couldn't be generated, so no archives were created.")
                    ));
                    return;
                }
            };
            manifest.push(ManifestEntry {
                archive: file_name(&output),
                source: file_name(item),
                password: password.clone(),
                sha256: String::new(),
                created: now.clone(),
                encryption: encryption_description(&settings),
            });
            jobs.push((item.clone(), output, password));
        }
        if jobs.is_empty() {
            return;
        }

        let state = Rc::new(BatchManifest {
            manifest: RefCell::new(manifest),
            dest,
            passphrase: settings.manifest_passphrase.clone(),
        });

        let window = self.clone();
        glib::spawn_future_local(async move {
            // The single point of failure, handled first: if the passwords
            // can't be persisted, nothing gets encrypted with them.
            if let Err(err) = state.write().await {
                window.show_error(&format!(
                    "{} {err}",
                    gettext("The passwords file couldn't be written, so no archives were created.")
                ));
                return;
            }
            window.show_toast(&format!(
                "{} {}",
                gettext("Passwords file written to"),
                state.dest.display()
            ));

            // One writer task serialises rewrites; job completions just queue a
            // nudge. It ends when the last job's sender drops.
            let (nudge, rewrites) = async_channel::unbounded::<()>();
            let writer_state = state.clone();
            let writer_window = window.clone();
            glib::spawn_future_local(async move {
                while rewrites.recv().await.is_ok() {
                    if let Err(err) = writer_state.write().await {
                        writer_window.show_toast(&format!(
                            "{} {err}",
                            gettext("The passwords file couldn't be updated:")
                        ));
                    }
                }
            });

            for (item, output, password) in jobs {
                let mut req = compression_request(&settings, vec![item], output);
                req.password = Some(password);
                window.start_compress(req, settings.write_checksum, Some((state.clone(), nudge.clone())));
            }
        });
    }

    fn start_compress(
        &self,
        req: CompressionRequest,
        write_checksum: bool,
        manifest: Option<(Rc<BatchManifest>, async_channel::Sender<()>)>,
    ) {
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
        let sevenzip_for_checksum = sevenzip.clone();

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
                            Ok(()) => {
                                window.show_toast(&format!(
                                    "{} {}",
                                    gettext("Created"),
                                    output.display()
                                ));
                                // Batch-with-manifest: record this archive's
                                // sha256 and rewrite the passwords file. The
                                // password itself was persisted before the job
                                // started, so nothing is at risk meanwhile.
                                if let Some((state, nudge)) = &manifest {
                                    let state = state.clone();
                                    let nudge = nudge.clone();
                                    let sevenzip = sevenzip_for_checksum.clone();
                                    let archive = output.clone();
                                    glib::spawn_future_local(async move {
                                        let hash_path = archive.clone();
                                        let digest = gio::spawn_blocking(move || {
                                            septima_engine::hash_file(&sevenzip, &hash_path, &["SHA256"])
                                        })
                                        .await;
                                        let hex = match digest {
                                            Ok(Ok(digests)) => digests
                                                .into_iter()
                                                .next()
                                                .map(|d| d.hex)
                                                .unwrap_or_default(),
                                            _ => String::new(), // sha256 is optional; the password is what matters
                                        };
                                        let name = file_name(&archive);
                                        let done = glib::DateTime::now_utc()
                                            .ok()
                                            .and_then(|d| d.format_iso8601().ok())
                                            .map(|s| s.to_string())
                                            .unwrap_or_default();
                                        {
                                            let mut m = state.manifest.borrow_mut();
                                            if let Some(entry) =
                                                m.entries.iter_mut().find(|e| e.archive == name)
                                            {
                                                entry.sha256 = hex;
                                                entry.created = done;
                                            }
                                        }
                                        let _ = nudge.try_send(());
                                    });
                                }
                                if write_checksum {
                                    let output = output.clone();
                                    let window = window.clone();
                                    let sevenzip = sevenzip_for_checksum.clone();
                                    glib::spawn_future_local(async move {
                                        let outcome = gio::spawn_blocking(move || {
                                            septima_engine::write_checksum_file(&sevenzip, &output)
                                        })
                                        .await;
                                        match outcome {
                                            Ok(Ok(checksum_path)) => window.show_toast(&format!(
                                                "{} {}",
                                                gettext("Wrote"),
                                                file_name(&checksum_path)
                                            )),
                                            _ => window.show_toast(&gettext(
                                                "Created, but the checksum file couldn't be written.",
                                            )),
                                        }
                                    });
                                }
                            }
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

    /// Verify the open archive's integrity via `7zz t`.
    fn test_archive(&self) {
        let Some(archive) = self.imp().archive_path.borrow().clone() else {
            return;
        };
        let password = self.imp().archive_password.borrow().clone();
        self.run_test_job(archive, password);
    }

    fn run_test_job(&self, archive: PathBuf, password: Option<String>) {
        let row = SeptimaProgressRow::new(&format!("{}: {}", gettext("Testing"), file_name(&archive)));
        let imp = self.imp();
        imp.jobs_box.append(&row);
        imp.jobs_revealer.set_reveal_child(true);

        let cancel = septima_engine::new_cancel_token();
        let cancel_ui = cancel.clone();
        row.connect_cancel(move || cancel_ui.store(true, Ordering::Relaxed));

        let (sender, receiver) = async_channel::unbounded::<Job>();
        let sevenzip = septima_engine::sevenzip_path();
        let archive_for_retry = archive.clone();

        std::thread::spawn(move || {
            let result = septima_engine::run_test(&sevenzip, &archive, password.as_deref(), &cancel, |p| {
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
                            Ok(()) => window.show_toast(&gettext(
                                "No errors found — the archive is intact.",
                            )),
                            Err(EngineError::Cancelled) => {}
                            Err(EngineError::PasswordRequired) => {
                                let retry = window.clone();
                                let archive = archive_for_retry.clone();
                                window.prompt_password(
                                    &gettext("This archive is encrypted. Enter its password to test it."),
                                    move |pw| {
                                        retry.imp().archive_password.replace(Some(pw.clone()));
                                        retry.run_test_job(archive.clone(), Some(pw));
                                    },
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

    /// Ask to confirm, then delete `paths` from the open archive.
    fn delete_entries(&self, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        let Some(archive) = self.imp().archive_path.borrow().clone() else {
            return;
        };
        let password = self.imp().archive_password.borrow().clone();

        let dialog = adw::AlertDialog::new(
            Some(&gettext("Delete Entries")),
            Some(&n_entries_body(paths.len())),
        );
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("delete", &gettext("Delete"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "delete" {
                window.run_delete_job(archive.clone(), password.clone(), paths.clone());
            }
        });
        dialog.present(Some(self));
    }

    fn run_delete_job(&self, archive: PathBuf, password: Option<String>, paths: Vec<String>) {
        let window = self.clone();
        let archive_for_retry = archive.clone();
        let paths_for_retry = paths.clone();
        glib::spawn_future_local(async move {
            let sevenzip = septima_engine::sevenzip_path();
            let archive_job = archive.clone();
            let password_job = password.clone();
            let paths_job = paths.clone();
            let outcome = gio::spawn_blocking(move || {
                septima_engine::run_delete(&sevenzip, &archive_job, &paths_job, password_job.as_deref())
            })
            .await;
            match outcome {
                Ok(Ok(())) => {
                    window.show_toast(&n_deleted(paths.len()));
                    window.load_archive(archive, password);
                }
                Ok(Err(EngineError::PasswordRequired)) => {
                    let retry = window.clone();
                    let archive = archive_for_retry.clone();
                    let paths = paths_for_retry.clone();
                    window.prompt_password(
                        &gettext("This archive is encrypted. Enter its password to make changes."),
                        move |pw| {
                            retry.imp().archive_password.replace(Some(pw.clone()));
                            retry.run_delete_job(archive.clone(), Some(pw), paths.clone());
                        },
                    );
                }
                Ok(Err(err)) => window.show_error(&err.to_string()),
                Err(_) => window.show_error(&gettext("The delete task failed.")),
            }
        });
    }

    /// Ask for a new path, then rename `old_path` within the open archive.
    fn rename_entry(&self, old_path: String) {
        let Some(archive) = self.imp().archive_path.borrow().clone() else {
            return;
        };
        let password = self.imp().archive_password.borrow().clone();

        let dialog = adw::AlertDialog::new(Some(&gettext("Rename Entry")), None);
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("rename", &gettext("Rename"));
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");

        let entry = adw::EntryRow::builder()
            .title(gettext("New path"))
            .text(old_path.as_str())
            .build();
        dialog.set_extra_child(Some(&entry));

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            let new_path = entry.text().to_string();
            if response == "rename" && !new_path.is_empty() && new_path != old_path {
                window.run_rename_job(archive.clone(), password.clone(), old_path.clone(), new_path);
            }
        });
        dialog.present(Some(self));
    }

    fn run_rename_job(&self, archive: PathBuf, password: Option<String>, old_path: String, new_path: String) {
        let window = self.clone();
        let archive_for_retry = archive.clone();
        let old_for_retry = old_path.clone();
        let new_for_retry = new_path.clone();
        glib::spawn_future_local(async move {
            let sevenzip = septima_engine::sevenzip_path();
            let archive_job = archive.clone();
            let password_job = password.clone();
            let renames = vec![(old_path.clone(), new_path.clone())];
            let outcome = gio::spawn_blocking(move || {
                septima_engine::run_rename(&sevenzip, &archive_job, &renames, password_job.as_deref())
            })
            .await;
            match outcome {
                Ok(Ok(())) => {
                    window.show_toast(&gettext("Entry renamed."));
                    window.load_archive(archive, password);
                }
                Ok(Err(EngineError::PasswordRequired)) => {
                    let retry = window.clone();
                    let archive = archive_for_retry.clone();
                    let (old_path, new_path) = (old_for_retry.clone(), new_for_retry.clone());
                    window.prompt_password(
                        &gettext("This archive is encrypted. Enter its password to make changes."),
                        move |pw| {
                            retry.imp().archive_password.replace(Some(pw.clone()));
                            retry.run_rename_job(archive.clone(), Some(pw), old_path.clone(), new_path.clone());
                        },
                    );
                }
                Ok(Err(err)) => window.show_error(&err.to_string()),
                Err(_) => window.show_error(&gettext("The rename task failed.")),
            }
        });
    }

    /// Clear the open archive and return to the welcome screen.
    fn close_archive(&self) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("empty");
        imp.extract_button.set_sensitive(false);
        imp.archive_path.replace(None);
        imp.archive_password.replace(None);
        imp.window_title.set_title("Septima");
        imp.window_title.set_subtitle("");
        self.set_archive_actions_enabled(false);
    }

    fn set_archive_actions_enabled(&self, enabled: bool) {
        for name in ["close-archive", "test-archive"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    }

    fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    /// The post-extract toast: destination path, plus a "Show in Files" button
    /// that opens `dest` in the file manager via the OpenURI portal.
    fn show_extracted_toast(&self, dest: &std::path::Path) {
        let toast = adw::Toast::builder()
            .title(format!("{} {}", gettext("Extracted to"), dest.display()))
            .button_label(gettext("Show in Files"))
            .build();

        let window = self.clone();
        let dest = dest.to_path_buf();
        toast.connect_button_clicked(move |_| {
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&dest)));
            let window_for_err = window.clone();
            launcher.launch(Some(&window), gio::Cancellable::NONE, move |result| {
                if let Err(err) = result {
                    window_for_err.show_toast(&err.message());
                }
            });
        });

        self.imp().toast_overlay.add_toast(toast);
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
/// Join a user-typed archive name with the format's extension without repeating
/// what the name already ends in.
///
/// People type the extension themselves — the Save dialog shows it, so it looks
/// like part of the name. Blindly appending turned `notes.7z` into
/// `notes.7z.7z`. The multi-part case matters too: for a `tar.zst`, a name
/// ending in `.tar` only needs the `.zst` half.
///
/// Only for names the user typed. Batch mode derives each name from its input
/// (`notes.txt` -> `notes.txt.7z`), where keeping the original suffix is right.
fn archive_filename(name: &str, ext: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(&format!(".{}", ext.to_ascii_lowercase())) {
        return name.to_string();
    }
    if let Some((head, tail)) = ext.split_once('.') {
        if lower.ends_with(&format!(".{}", head.to_ascii_lowercase())) {
            return format!("{name}.{tail}");
        }
    }
    format!("{name}.{ext}")
}

/// The human-readable cipher note stored per manifest entry — the context
/// nobody remembers six months later ("7z, AES-256, encrypted headers").
fn encryption_description(settings: &CreateSettings) -> String {
    let cipher = match (settings.format.id, settings.encryption_method.as_deref()) {
        ("7z", Some("AES256GCM")) => "AES-256-GCM + Argon2id",
        ("7z", _) => "AES-256",
        ("zip", Some("AES256")) => "AES-256",
        ("zip", _) => "ZipCrypto (legacy)",
        _ => return String::new(),
    };
    let mut out = format!("{}, {cipher}", settings.format.id);
    if settings.encrypt_headers {
        out.push_str(", encrypted headers");
    }
    out
}

fn archive_extension(settings: &CreateSettings) -> String {
    if settings.format.id == "stream" {
        // Raw single-file stream: the extension is the codec's (zst/xz/gz/…).
        return septima_engine::stream_extension(settings.codec.id).to_string();
    }
    if settings.format.id == "tar" {
        match settings.codec.id {
            "zstd" => "tar.zst",
            "xz" => "tar.xz",
            "gzip" => "tar.gz",
            "bzip2" => "tar.bz2",
            "brotli" => "tar.br",
            "lz4" => "tar.lz4",
            "lz5" => "tar.lz5",
            "lizard" => "tar.liz",
            _ => "tar",
        }
        .to_string()
    } else {
        settings.format.extension.to_string()
    }
}

fn compression_request(settings: &CreateSettings, inputs: Vec<PathBuf>, output: PathBuf) -> CompressionRequest {
    // A raw stream is `-t<codec>` on one file with no method chain: the codec is
    // the format, so build the request around it directly.
    if settings.format.id == "stream" {
        let mut req = CompressionRequest::new(output, inputs, settings.codec.id);
        req.level = settings.level;
        req.threads = Some(settings.threads);
        req.extra_params = settings.extra_params.clone();
        return req;
    }
    let mut req = CompressionRequest::new(output, inputs, settings.format.id);
    req.codec = Some(settings.codec.id.to_string());
    req.level = settings.level;
    req.threads = Some(settings.threads);
    req.dictionary = settings.dictionary.clone();
    req.solid = settings.solid;
    req.volume_size = settings.volume_size.clone();
    req.filter = settings.filter.clone();
    req.password = settings.password.clone();
    req.encrypt_headers = settings.encrypt_headers;
    req.encryption_method = settings.encryption_method.clone();
    req.extra_params = settings.extra_params.clone();
    req
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn n_files(n: usize) -> String {
    gettextrs::ngettext("{} file", "{} files", n as u32).replacen("{}", &n.to_string(), 1)
}

fn n_entries_body(n: usize) -> String {
    gettextrs::ngettext("Delete {} entry from this archive?", "Delete {} entries from this archive?", n as u32)
        .replacen("{}", &n.to_string(), 1)
}

fn n_deleted(n: usize) -> String {
    gettextrs::ngettext("{} entry deleted.", "{} entries deleted.", n as u32).replacen("{}", &n.to_string(), 1)
}

fn n_archives_body(n: usize) -> String {
    gettextrs::ngettext(
        "Extract {} archive? Each will be extracted into a new folder next to itself.",
        "Extract {} archives? Each will be extracted into a new folder next to itself.",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

fn n_skipped(n: usize) -> String {
    gettextrs::ngettext(
        "({} file couldn't be read and will be skipped.)",
        "({} files couldn't be read and will be skipped.)",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

/// Whether `path` is named like a passwords file rather than an archive:
/// `.json`, `.json.gpg`, or the CSV export re-imported. Content is verified
/// after (parse / OpenPGP sniff); the name just routes it away from `7zz`.
fn is_manifest_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.ends_with(".json") || name.ends_with(".json.gpg") || name.ends_with(".csv")
}

fn n_manifest_body(n: usize) -> String {
    gettextrs::ngettext(
        "Extract {} archive listed in this passwords file, using its recorded password? It will be extracted into a new folder next to itself.",
        "Extract {} archives listed in this passwords file, each with its recorded password? Each will be extracted into a new folder next to itself.",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

fn n_manifest_missing(n: usize) -> String {
    gettextrs::ngettext(
        "({} listed archive wasn't found next to the passwords file and will be skipped.)",
        "({} listed archives weren't found next to the passwords file and will be skipped.)",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

fn n_manifest_passwordless(n: usize) -> String {
    gettextrs::ngettext(
        "({} entry has no password recorded and will be skipped.)",
        "({} entries have no password recorded and will be skipped.)",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

fn n_archives_create_body(n: usize) -> String {
    gettextrs::ngettext(
        "Create {} archive? It will be saved next to the item it's made from.",
        "Create {} archives? Each will be saved next to the item it's made from.",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

/// Where a batch-extracted archive's contents land: a new folder next to it,
/// named after the archive (`photos.zip` -> `photos/`, `data.tar.gz` -> `data/`).
fn sibling_extract_dir(archive: &Path) -> PathBuf {
    let stem = archive
        .file_stem()
        .map(PathBuf::from)
        .unwrap_or_else(|| archive.to_path_buf());
    let stem = match stem.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("tar") => stem.file_stem().map(PathBuf::from).unwrap_or(stem),
        _ => stem,
    };
    archive.with_file_name(stem)
}

#[cfg(test)]
mod tests {
    use super::archive_filename;

    #[test]
    fn does_not_repeat_an_extension_the_user_already_typed() {
        assert_eq!(archive_filename("notes", "7z"), "notes.7z");
        assert_eq!(archive_filename("notes.7z", "7z"), "notes.7z");
        assert_eq!(archive_filename("NOTES.7Z", "7z"), "NOTES.7Z"); // case is theirs to keep
    }

    #[test]
    fn completes_a_partly_typed_multi_part_extension() {
        // "bundle.tar" for a tar.zst needs only the .zst half.
        assert_eq!(archive_filename("bundle.tar", "tar.zst"), "bundle.tar.zst");
        assert_eq!(archive_filename("bundle", "tar.zst"), "bundle.tar.zst");
        assert_eq!(archive_filename("bundle.tar.zst", "tar.zst"), "bundle.tar.zst");
    }

    #[test]
    fn leaves_an_unrelated_suffix_alone() {
        // A name that merely contains a dot is not an extension we should eat.
        assert_eq!(archive_filename("v1.2-final", "7z"), "v1.2-final.7z");
        assert_eq!(archive_filename("report.txt", "7z"), "report.txt.7z");
        // A different archive extension is not ours to swallow either.
        assert_eq!(archive_filename("old.zip", "7z"), "old.zip.7z");
    }
}
