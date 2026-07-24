use std::cell::RefCell;
use std::path::{Path, PathBuf};
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
        self.load_archive(path, None);
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
            window.choose_output_and_compress(settings);
        });
        dialog.present(Some(self));
    }

    fn choose_output_and_compress(&self, settings: CreateSettings) {
        let filename = format!("{}.{}", settings.name, archive_extension(&settings));
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
                    window.start_compress(compression_request(&settings, output), write_checksum)
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

    fn start_compress(&self, req: CompressionRequest, write_checksum: bool) {
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

fn compression_request(settings: &CreateSettings, output: PathBuf) -> CompressionRequest {
    let mut req = CompressionRequest::new(output, settings.inputs.clone(), settings.format.id);
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
