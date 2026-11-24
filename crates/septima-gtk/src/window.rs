use std::cell::{Cell, RefCell};
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
use crate::batch_run::{BatchKind, BatchRun, Outcome};
use crate::create_dialog::{CreateSettings, SeptimaCreateDialog};
use crate::job_queue::{self, JobGate};
use crate::progress_row::SeptimaProgressRow;

/// Messages from the extraction worker thread to the UI.
enum Job {
    Progress(ExtractProgress),
    Done(Result<(), EngineError>),
}

/// A job's place in the queue (see `job_queue`): what `finish_job` needs to
/// give its slots back, and whether it ever started.
struct JobTicket {
    id: u64,
    slots: usize,
    started: Rc<Cell<bool>>,
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
        #[template_child]
        pub jobs_header: TemplateChild<gtk::Box>,
        #[template_child]
        pub jobs_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub jobs_scroller: TemplateChild<gtk::ScrolledWindow>,
        /// Holds extract and create jobs back so only a few `7zz` run at once.
        pub job_gate: RefCell<JobGate<Box<dyn FnOnce()>>>,
        /// The currently open archive, used as the extract source.
        pub archive_path: RefCell<Option<PathBuf>>,
        /// Password the current archive was opened with (reused for extraction).
        pub archive_password: RefCell<Option<String>>,
        /// The open archive's files (not folders), in-archive paths: what an
        /// extract would write, so clashes can be counted before it starts.
        pub archive_files: RefCell<Vec<String>>,
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

        #[template_callback]
        fn on_batch_extract_clicked(&self) {
            self.obj().batch_extract_dialog();
        }

        #[template_callback]
        fn on_cancel_all_clicked(&self) {
            self.obj().cancel_all_jobs();
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
            // Keyboard twins of the Open and New buttons (Ctrl+O, Ctrl+N).
            let open = gio::ActionEntry::builder("open")
                .activate(|window: &super::SeptimaWindow, _, _| window.open_archive_dialog())
                .build();
            // Batch extract's own way in (Ctrl+Shift+E); it used to be reachable
            // only by picking several files in Open or dropping them.
            let batch_extract = gio::ActionEntry::builder("batch-extract")
                .activate(|window: &super::SeptimaWindow, _, _| window.batch_extract_dialog())
                .build();
            let new_archive = gio::ActionEntry::builder("new-archive")
                .activate(|window: &super::SeptimaWindow, _, _| window.new_archive_dialog())
                .build();
            // Keyboard twins of the Extract button and the list's Rename and
            // Delete buttons (Ctrl+E, F2, Delete; see about::SHORTCUTS).
            let extract = gio::ActionEntry::builder("extract")
                .activate(|window: &super::SeptimaWindow, _, _| window.choose_destination_and_extract())
                .build();
            let rename = gio::ActionEntry::builder("rename-entry")
                .activate(|window: &super::SeptimaWindow, _, _| window.imp().archive_view.request_rename())
                .build();
            let delete = gio::ActionEntry::builder("delete-entries")
                .activate(|window: &super::SeptimaWindow, _, _| window.imp().archive_view.request_delete())
                .build();
            let obj = self.obj();
            obj.add_action_entries([checksums, close, test_archive, open, batch_extract, new_archive, extract, rename, delete]);
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
            self.archive_view.connect_drag_extract(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                None,
                move |entries| obj.extract_for_drag(entries)
            ));

            // Whatever a previous session staged for drag-out is disposable
            // by definition — the drops it fed either happened or never will.
            let _ = std::fs::remove_dir_all(drag_staging_base());
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

    /// Pick any number of archives and extract them all, each into its own
    /// folder. Unlike Open, one file is extracted too, not browsed.
    fn batch_extract_dialog(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Choose Archives to Extract"))
            .accept_label(gettext("_Extract…"))
            .modal(true)
            .build();

        let window = self.clone();
        dialog.open_multiple(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(files) => {
                let files: Vec<gio::File> = files.iter::<gio::File>().filter_map(Result::ok).collect();
                if !files.is_empty() {
                    window.confirm_batch_extract(files);
                }
            }
            Err(err) => {
                if !err.matches(gtk::DialogError::Dismissed) {
                    window.show_toast(err.message());
                }
            }
        });
    }

    /// Files handed over from outside (command line, Open With).
    pub fn open_files(&self, files: Vec<gio::File>) {
        self.open_or_batch_extract(files);
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
    /// hand-edited "../evil" can't walk out of the folder). A manifest opened
    /// through the file portal sits *alone* in its document directory, so
    /// "next to it" can legitimately hold nothing — in that case the user
    /// points Septima at the folder that really holds the archives.
    fn confirm_manifest_extract(&self, manifest_path: PathBuf, manifest: Manifest) {
        let dir = manifest_path.parent().map(Path::to_path_buf).unwrap_or_default();
        let rows = manifest_rows(&dir, &manifest);
        if rows.iter().any(|r| r.job.is_some()) {
            self.present_manifest_extract(rows);
            return;
        }
        if manifest.entries.is_empty() {
            self.show_error(&gettext("This passwords file lists no archives."));
            return;
        }

        // Opened through the file chooser, the passwords file is the only
        // thing in its folder the sandbox lets Septima see — the archives can
        // be right beside it and still look missing. Say that, not "not
        // found", and open the folder picker where the file really lives so
        // granting access is one click.
        let host_folder = portal_host_path(&manifest_path).and_then(|p| p.parent().map(Path::to_path_buf));
        let dialog = if under_doc_portal(&manifest_path) {
            adw::AlertDialog::new(
                Some(&gettext("Allow Access to the Folder")),
                Some(&gettext(
                    "Septima can read the passwords file, but not the other files in its folder \
                     until you choose that folder. Choose it to continue.",
                )),
            )
        } else {
            adw::AlertDialog::new(
                Some(&gettext("Locate the Archives")),
                Some(&gettext(
                    "None of the archives in this passwords file were found next to it. \
                     Point Septima at the folder that holds them?",
                )),
            )
        };
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("locate", &gettext("Choose Folder…"));
        dialog.set_response_appearance("locate", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("locate"));

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "locate" {
                return;
            }
            let window2 = window.clone();
            let manifest = manifest.clone();
            let picker = gtk::FileDialog::builder()
                .title(gettext("Folder With the Archives"))
                .modal(true)
                .build();
            if let Some(folder) = &host_folder {
                picker.set_initial_folder(Some(&gio::File::for_path(folder)));
            }
            picker.select_folder(Some(&window), gio::Cancellable::NONE, move |result| {
                if let Ok(folder) = result {
                    if let Some(dir) = folder.path() {
                        let rows = manifest_rows(&dir, &manifest);
                        if rows.iter().any(|r| r.job.is_some()) {
                            window2.present_manifest_extract(rows);
                        } else {
                            window2.show_error(&gettext(
                                "None of the listed archives are in that folder either.",
                            ));
                        }
                    }
                }
            });
        });
        dialog.present(Some(self));
    }

    /// The picker for a resolved manifest batch: one row per listed archive —
    /// found ones pre-checked, missing or password-less ones greyed with the
    /// reason — then the checked extractions. Seeing every entry's state here
    /// is deliberate: a bare count once made two successful extractions read
    /// as one.
    fn present_manifest_extract(&self, rows: Vec<ManifestRow>) {
        let n_rows = rows.len();
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("boxed-list");
        let mut checks: Vec<(gtk::CheckButton, PathBuf, String)> = Vec::new();
        for row in rows {
            let action = adw::ActionRow::builder()
                .title(&row.title)
                .subtitle(&row.subtitle)
                .title_lines(1)
                .subtitle_lines(1)
                .build();
            let check = gtk::CheckButton::builder().valign(gtk::Align::Center).build();
            action.add_prefix(&check);
            match row.job {
                Some((archive, password)) => {
                    check.set_active(true);
                    action.set_activatable_widget(Some(&check));
                    checks.push((check, archive, password));
                }
                None => {
                    check.set_sensitive(false);
                    action.set_sensitive(false);
                }
            }
            list.append(&action);
        }

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(320)
            .min_content_width(340)
            .child(&list)
            .build();

        // Long lists need selection ergonomics, not just a scrollbar: a live
        // "N of M" count, Select All / Clear (so 1-of-20 is Clear + one
        // check), and — only once the list is long enough to lose things in —
        // a filter box.
        let selected_label = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        selected_label.add_css_class("dim-label");
        let select_all = gtk::Button::with_label(&gettext("Select All"));
        select_all.add_css_class("flat");
        let clear = gtk::Button::with_label(&gettext("Clear"));
        clear.add_css_class("flat");
        let strip = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        strip.append(&selected_label);
        strip.append(&select_all);
        strip.append(&clear);

        let delete_after = gtk::CheckButton::builder()
            .label(gettext("Delete the archives after extracting"))
            .build();
        let extra = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        extra.append(&strip);
        if n_rows >= 10 {
            let search = gtk::SearchEntry::builder()
                .placeholder_text(gettext("Filter archives…"))
                .build();
            let query: Rc<RefCell<String>> = Rc::default();
            {
                let query = query.clone();
                list.set_filter_func(move |row| {
                    let q = query.borrow();
                    q.is_empty()
                        || row
                            .downcast_ref::<adw::ActionRow>()
                            .map(|r| r.title().to_lowercase().contains(q.as_str()))
                            .unwrap_or(true)
                });
            }
            let list_for_filter = list.clone();
            search.connect_search_changed(move |s| {
                *query.borrow_mut() = s.text().to_lowercase();
                list_for_filter.invalidate_filter();
            });
            extra.append(&search);
        }
        extra.append(&scroller);
        // Same rule as a batch extract: ask only when a folder they'd land in
        // already exists. Counted over every extractable row, before picking.
        let existing = sibling_extract_dirs(checks.iter().map(|(_, archive, _)| archive.as_path()))
            .iter()
            .filter(|d| folder_has_entries(d))
            .count();
        let choice = (existing > 0).then(|| ClashChoice::new(&delete_after));
        if let Some(choice) = &choice {
            extra.append(&choice.widget);
        }
        extra.append(&delete_after);

        let mut body = gettext("Choose which archives to extract — each uses its recorded password.");
        if existing > 0 {
            body = format!("{body}\n\n{}", n_existing_folders(existing));
        }
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Extract From Passwords File"))
            .body(body)
            .extra_child(&extra)
            .build();
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("extract", &gettext("Extract"));
        dialog.set_response_appearance("extract", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("extract"));

        // The Extract button and the count label follow what's checked; the
        // button disables at zero.
        let checks = Rc::new(checks);
        let update = {
            let dialog = dialog.clone();
            let checks = checks.clone();
            let selected_label = selected_label.clone();
            move || {
                let n = checks.iter().filter(|(c, _, _)| c.is_active()).count();
                dialog.set_response_label("extract", &extract_label(n));
                dialog.set_response_enabled("extract", n > 0);
                selected_label.set_text(
                    &gettext("{} of {} selected")
                        .replacen("{}", &n.to_string(), 1)
                        .replacen("{}", &checks.len().to_string(), 1),
                );
            }
        };
        update();
        for (check, _, _) in checks.iter() {
            let update = update.clone();
            check.connect_toggled(move |_| update());
        }
        {
            let checks = checks.clone();
            select_all.connect_clicked(move |_| {
                for (check, _, _) in checks.iter() {
                    check.set_active(true);
                }
            });
        }
        {
            let checks = checks.clone();
            clear.connect_clicked(move |_| {
                for (check, _, _) in checks.iter() {
                    check.set_active(false);
                }
            });
        }

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "extract" {
                return;
            }
            let picked: Vec<_> = checks.iter().filter(|(check, _, _)| check.is_active()).collect();
            let dests = sibling_extract_dirs(picked.iter().map(|(_, archive, _)| archive.as_path()));
            let overwrite = ClashChoice::mode_of(choice.as_ref());
            for ((_, archive, password), dest) in picked.into_iter().zip(dests) {
                window.start_extract(
                    archive.clone(),
                    dest,
                    Some(password.clone()),
                    overwrite,
                    delete_after.is_active(),
                    None,
                );
            }
        });
        dialog.present(Some(self));
    }

    /// Confirm, then extract every archive in `files` into a new sibling
    /// folder next to itself (e.g. `photos.zip` -> `photos/`) — one Extract
    /// job per archive, run independently. With two or more they share a
    /// [`BatchRun`]: no per-archive toasts, dialogs or password prompts, just
    /// one password prompt for any encrypted leftovers and one summary at the
    /// end (see `finish_batch`).
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

        // Encrypted batches usually share one password, so offer a single
        // field applied to every archive — but only behind a check, since an
        // always-visible field read as demanding a password. Leaving it off
        // loses nothing: archives that turn out encrypted (or don't open with
        // this password) are gathered into one prompt at the end of the round.
        let protected = gtk::CheckButton::builder()
            .label(gettext("These archives are password-protected"))
            .build();
        let password_entry = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .activates_default(true)
            .placeholder_text(gettext("Password"))
            .margin_top(6)
            .build();
        let password_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&password_entry)
            .build();
        {
            let password_entry = password_entry.clone();
            let password_revealer = password_revealer.clone();
            protected.connect_toggled(move |check| {
                password_revealer.set_reveal_child(check.is_active());
                if check.is_active() {
                    password_entry.grab_focus();
                }
            });
        }
        let delete_after = gtk::CheckButton::builder()
            .label(gettext("Delete the archives after extracting"))
            .build();
        let extra = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        // Its own unspaced box, so the hidden revealer adds no gap.
        let password_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        password_box.append(&protected);
        password_box.append(&password_revealer);
        extra.append(&password_box);
        // Each archive gets a new folder — but one that already exists is
        // extracted into, so ask only then.
        let dests = sibling_extract_dirs(archives.iter().map(PathBuf::as_path));
        let existing = dests.iter().filter(|d| folder_has_entries(d)).count();
        let choice = (existing > 0).then(|| ClashChoice::new(&delete_after));
        if let Some(choice) = &choice {
            extra.append(&choice.widget);
        }
        extra.append(&delete_after);

        let mut body = n_archives_body(archives.len());
        if skipped > 0 {
            body = format!("{body} {}", n_skipped(skipped));
        }
        if existing > 0 {
            body = format!("{body}\n\n{}", n_existing_folders(existing));
        }
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
            let password = (protected.is_active() && !pw.is_empty()).then_some(pw);
            let overwrite = ClashChoice::mode_of(choice.as_ref());
            let run = (archives.len() >= 2).then(|| {
                Rc::new(RefCell::new(
                    BatchRun::new(BatchKind::Extract, archives.len())
                        .with_delete_after(delete_after.is_active())
                        .with_overwrite(overwrite),
                ))
            });
            for (archive, dest) in archives.iter().zip(dests.iter().cloned()) {
                window.start_extract(
                    archive.clone(),
                    dest,
                    password.clone(),
                    overwrite,
                    delete_after.is_active(),
                    run.clone(),
                );
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
                    imp.archive_files.replace(
                        listing.entries.iter().filter(|e| !e.is_dir).map(|e| e.path.clone()).collect(),
                    );
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
                            window.start_extract(archive_path, PathBuf::from(dir), password, OverwriteMode::Overwrite, false, None);
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

    /// Offer "delete the archive afterwards" before starting the extract job,
    /// and — only when files of the same name are already in `dest` — ask what
    /// should happen to them.
    pub(crate) fn confirm_extract(&self, archive: PathBuf, dest: PathBuf, password: Option<String>) {
        let delete_after = gtk::CheckButton::builder()
            .label(gettext("Delete the archive after extracting"))
            .build();
        let clashes = count_clashes(&dest, &self.imp().archive_files.borrow());
        let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        let choice = (clashes > 0).then(|| ClashChoice::new(&delete_after));
        if let Some(choice) = &choice {
            extra.append(&choice.widget);
        }
        extra.append(&delete_after);
        let body = if clashes > 0 {
            n_clashing_files(clashes, &file_name(&dest))
        } else {
            gettext("Extract the contents of this archive to the chosen folder?")
        };

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Extract Archive"))
            .body(body)
            .extra_child(&extra)
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
                    ClashChoice::mode_of(choice.as_ref()),
                    delete_after.is_active(),
                    None,
                );
            }
        });
        dialog.present(Some(self));
    }

    /// Extract `archive` into `dest`. With `batch`, this job reports its
    /// outcome to the run instead of speaking for itself: no toast, no error
    /// dialog, and an encrypted archive is held for the end-of-round prompt.
    fn start_extract(
        &self,
        archive: PathBuf,
        dest: PathBuf,
        password: Option<String>,
        overwrite: OverwriteMode,
        delete_after: bool,
        batch: Option<Rc<RefCell<BatchRun>>>,
    ) {
        let name = file_name(&archive);
        let row = SeptimaProgressRow::new(&format!("{}: {name}", gettext("Extracting")));
        let imp = self.imp();
        imp.jobs_box.append(&row);
        imp.jobs_revealer.set_reveal_child(true);

        let cancel = septima_engine::new_cancel_token();
        let cancel_ui = cancel.clone();

        let (sender, receiver) = async_channel::unbounded::<Job>();
        let waiting_sender = sender.downgrade();
        let sevenzip = septima_engine::sevenzip_path();
        let req = ExtractRequest {
            archive: archive.clone(),
            dest_dir: dest.clone(),
            password,
            overwrite,
            entries: Vec::new(),
        };

        let ticket = self.submit_job(&row, 1, move || {
            std::thread::spawn(move || {
                let result = septima_engine::run_extract(&sevenzip, &req, &cancel, |p| {
                    let _ = sender.send_blocking(Job::Progress(p.clone()));
                });
                let _ = sender.send_blocking(Job::Done(result));
            });
        });
        self.connect_job_cancel(&row, cancel_ui, &ticket, waiting_sender);

        let window = self.clone();
        glib::spawn_future_local(async move {
            let mut settled = false;
            while let Ok(message) = receiver.recv().await {
                match message {
                    Job::Progress(p) => row.set_progress(p.percent, p.current_file.as_deref()),
                    Job::Done(result) => {
                        window.finish_job(&row, Some(&ticket));
                        match (result, &batch) {
                            (Ok(()), _) => {
                                if batch.is_none() {
                                    window.show_folder_toast(
                                        &format!("{} {}", gettext("Extracted to"), dest.display()),
                                        Some(&dest),
                                    );
                                }
                                let done = Outcome::Done { output: dest.clone() };
                                if delete_after {
                                    // A batch job is recorded only once the
                                    // delete has settled, so its warning makes
                                    // the report.
                                    let archive = archive.clone();
                                    let window = window.clone();
                                    let batch = batch.clone();
                                    glib::spawn_future_local(async move {
                                        let delete_path = archive.clone();
                                        let outcome = gio::spawn_blocking(move || {
                                            septima_engine::delete_archive(&delete_path)
                                        })
                                        .await;
                                        let deleted = matches!(outcome, Ok(Ok(())));
                                        match &batch {
                                            Some(run) => {
                                                if !deleted {
                                                    run.borrow_mut().warn(
                                                        &archive,
                                                        &gettext("extracted, but the archive couldn't be deleted"),
                                                    );
                                                }
                                                window.record_batch(run, &archive, done);
                                            }
                                            None if !deleted => window.show_toast(&gettext(
                                                "Extracted, but the archive couldn't be deleted.",
                                            )),
                                            None => {}
                                        }
                                    });
                                } else if let Some(run) = &batch {
                                    window.record_batch(run, &archive, done);
                                }
                            }
                            (Err(EngineError::Cancelled), None) => {} // silent
                            (Err(EngineError::Cancelled), Some(run)) => {
                                window.record_batch(run, &archive, Outcome::Cancelled);
                            }
                            // Held for the one end-of-round prompt.
                            (Err(EngineError::PasswordRequired), Some(run)) => {
                                let outcome = Outcome::NeedsPassword { dest: dest.clone() };
                                window.record_batch(run, &archive, outcome);
                            }
                            (Err(err), Some(run)) => {
                                let outcome = Outcome::Failed { message: err.to_string() };
                                window.record_batch(run, &archive, outcome);
                            }
                            (Err(EngineError::PasswordRequired), None) => {
                                let retry = window.clone();
                                let (archive, dest) = (archive.clone(), dest.clone());
                                let body = gettext("“{}” is encrypted. Enter its password to extract.")
                                    .replacen("{}", &file_name(&archive), 1);
                                window.prompt_password(&body, move |pw| {
                                    retry.start_extract(
                                        archive.clone(),
                                        dest.clone(),
                                        Some(pw),
                                        overwrite,
                                        delete_after,
                                        None,
                                    )
                                });
                            }
                            (Err(err), None) => window.show_error(&err.to_string()),
                        }
                        settled = true;
                        break;
                    }
                }
            }
            if !settled {
                window.worker_died(&row, &ticket, &archive, batch.as_ref());
            }
        });
    }

    /// Drag-out backing: synchronously extract `entries` from the open
    /// archive into a fresh staging dir and return the staged paths, which
    /// become the drag's file list. Synchronous on purpose — the content
    /// provider must exist when the gesture starts, and the view caps the
    /// selection size so the stall stays unnoticeable. The staging dir lives
    /// in the app cache, whose path is identical inside and outside the
    /// sandbox, so receivers can read the files with or without the
    /// FileTransfer portal.
    fn extract_for_drag(&self, entries: Vec<String>) -> Option<Vec<PathBuf>> {
        let archive = self.imp().archive_path.borrow().clone()?;
        let password = self.imp().archive_password.borrow().clone();
        let dest = drag_staging_base().join(format!("drag-{}", glib::monotonic_time()));
        std::fs::create_dir_all(&dest).ok()?;
        let mut req = ExtractRequest::new(archive, dest.clone());
        req.password = password;
        req.entries = entries.clone();
        let outcome = septima_engine::run_extract(
            &septima_engine::sevenzip_path(),
            &req,
            &septima_engine::new_cancel_token(),
            |_| {},
        );
        match outcome {
            Ok(()) => {
                let staged: Vec<PathBuf> =
                    entries.iter().map(|e| dest.join(e)).filter(|p| p.exists()).collect();
                (!staged.is_empty()).then_some(staged)
            }
            Err(err) => {
                let _ = std::fs::remove_dir_all(&dest);
                self.show_toast(&format!(
                    "{} {err}",
                    gettext("Couldn't stage the dragged entries:")
                ));
                None
            }
        }
    }

    /// Dev-only (screenshots): batch-extract every file in `dir` with the
    /// dialog's defaults, as its Extract button would.
    #[cfg(feature = "shots")]
    pub(crate) fn shots_batch_extract(&self, dir: &Path) {
        let mut archives: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();
        archives.sort();
        let dests = sibling_extract_dirs(archives.iter().map(PathBuf::as_path));
        let run = Rc::new(RefCell::new(BatchRun::new(BatchKind::Extract, archives.len())));
        for (archive, dest) in archives.into_iter().zip(dests) {
            self.start_extract(archive, dest, None, OverwriteMode::default(), false, Some(run.clone()));
        }
    }

    /// Dev-only (screenshots): the New Archive dialog with `inputs` staged and
    /// the method at `codec`, as someone part-way through would have it.
    #[cfg(feature = "shots")]
    pub(crate) fn shots_create_dialog(&self, inputs: Vec<PathBuf>, codec: &str, batch: bool) {
        let dialog = SeptimaCreateDialog::new();
        dialog.add_inputs(inputs);
        dialog.shots_set(codec, batch);
        dialog.present(Some(self));
    }

    /// Dev-only (screenshots): (limit, running, waiting) of the job queue.
    #[cfg(feature = "shots")]
    pub(crate) fn shots_queue_state(&self) -> (usize, usize, usize) {
        let gate = self.imp().job_gate.borrow();
        (gate.limit(), gate.running(), gate.waiting())
    }

    /// Dev-only (screenshots): press Cancel All.
    #[cfg(feature = "shots")]
    pub(crate) fn shots_cancel_all(&self) {
        self.cancel_all_jobs();
    }

    /// Dev-only (screenshots): a failure report long enough to scroll.
    #[cfg(feature = "shots")]
    pub(crate) fn shots_long_report(&self) {
        let lines: Vec<String> = (1..=40)
            .map(|n| format!("holiday-photos-{n:02}.7z: 7zz exited with code 2: Data Error"))
            .collect();
        self.show_alert(&gettext("Some Archives Failed"), &lines.join("\n"));
    }

    /// Put a job behind the cap on jobs at once. `start` runs now if there's
    /// room, otherwise when its turn comes; until then `row` reads "Waiting…".
    fn submit_job(&self, row: &SeptimaProgressRow, slots: usize, start: impl FnOnce() + 'static) -> JobTicket {
        let started = Rc::new(Cell::new(false));
        let job: Box<dyn FnOnce()> = {
            let started = started.clone();
            let row = row.clone();
            Box::new(move || {
                started.set(true);
                row.set_running();
                start();
            })
        };
        // The gate hands the job back rather than running it, so the borrow
        // is over before any job code runs.
        let (id, now) = self.imp().job_gate.borrow_mut().submit(slots, job);
        match now {
            Some(job) => job(),
            None => row.set_waiting(),
        }
        self.update_jobs_header();
        JobTicket { id, slots, started }
    }

    /// Wire a row's cancel button. A running job is stopped through its
    /// cancel token. One still waiting is taken out of the queue and reported
    /// as cancelled through its own channel, so it ends the same way a
    /// running one does (row removed, batch told).
    fn connect_job_cancel(
        &self,
        row: &SeptimaProgressRow,
        cancel: septima_engine::CancelToken,
        ticket: &JobTicket,
        waiting_sender: async_channel::WeakSender<Job>,
    ) {
        let id = ticket.id;
        let window = self.downgrade();
        row.connect_cancel(move || {
            cancel.store(true, Ordering::Relaxed);
            let Some(window) = window.upgrade() else {
                return;
            };
            let removed = window.imp().job_gate.borrow_mut().cancel_waiting(id);
            if let Some(job) = removed {
                // Hold a sender before dropping the job: the job owns the
                // only other one, and the channel must stay open for this.
                let sender = waiting_sender.upgrade();
                drop(job);
                if let Some(sender) = sender {
                    let _ = sender.try_send(Job::Done(Err(EngineError::Cancelled)));
                }
                window.start_waiting_jobs();
            }
        });
    }

    /// Start whatever the gate now has room for.
    fn start_waiting_jobs(&self) {
        let next = self.imp().job_gate.borrow_mut().startable();
        for job in next {
            job();
        }
        self.update_jobs_header();
    }

    /// Remove a finished job's row. A queued job (`ticket`) that had started
    /// gives its slots back, which lets the next ones in.
    fn finish_job(&self, row: &SeptimaProgressRow, ticket: Option<&JobTicket>) {
        let imp = self.imp();
        imp.jobs_box.remove(row);
        if imp.jobs_box.first_child().is_none() {
            imp.jobs_revealer.set_reveal_child(false);
        }
        if let Some(ticket) = ticket.filter(|t| t.started.get()) {
            imp.job_gate.borrow_mut().release(ticket.slots);
        }
        self.start_waiting_jobs();
    }

    /// The line above the job list: how many are running and waiting, with
    /// Cancel All. Shown once there is more than one job.
    fn update_jobs_header(&self) {
        let imp = self.imp();
        let mut rows = 0usize;
        let mut row_height = 0;
        let mut child = imp.jobs_box.first_child();
        while let Some(widget) = child {
            rows += 1;
            row_height = row_height.max(widget.measure(gtk::Orientation::Vertical, -1).1);
            child = widget.next_sibling();
        }
        // Tall enough for up to JOB_ROWS_SHOWN rows; more than that scroll.
        let shown = rows.min(JOB_ROWS_SHOWN) as i32;
        // GTK insists min <= max at every step, so clear min before moving max.
        let height = shown * row_height;
        imp.jobs_scroller.set_min_content_height(-1);
        imp.jobs_scroller.set_max_content_height(height.max(1));
        imp.jobs_scroller.set_min_content_height(height);
        let waiting = imp.job_gate.borrow().waiting();
        imp.jobs_header.set_visible(rows >= 2);
        let text = if waiting > 0 {
            gettext("{running} running, {waiting} waiting")
                .replace("{running}", &rows.saturating_sub(waiting).to_string())
                .replace("{waiting}", &waiting.to_string())
        } else {
            gettext("{running} running").replace("{running}", &rows.to_string())
        };
        imp.jobs_label.set_label(&text);
    }

    /// Cancel every job in the list, running or waiting.
    fn cancel_all_jobs(&self) {
        let mut rows = Vec::new();
        let mut child = self.imp().jobs_box.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            if let Ok(row) = widget.downcast::<SeptimaProgressRow>() {
                rows.push(row);
            }
        }
        for row in rows {
            row.cancel();
        }
    }

    /// A job's worker thread ended without reporting (a panic in the
    /// engine). Clear its row, and in a batch count it as failed — otherwise
    /// the round never ends and the rest of the batch never reports.
    fn worker_died(
        &self,
        row: &SeptimaProgressRow,
        ticket: &JobTicket,
        archive: &Path,
        batch: Option<&Rc<RefCell<BatchRun>>>,
    ) {
        self.finish_job(row, Some(ticket));
        let message = gettext("the job stopped unexpectedly");
        match batch {
            Some(run) => self.record_batch(run, archive, Outcome::Failed { message }),
            None => self.show_error(&format!("{}: {message}", file_name(archive))),
        }
    }

    /// Record one batch job's outcome; the round's last one ends it.
    fn record_batch(&self, run: &Rc<RefCell<BatchRun>>, archive: &Path, outcome: Outcome) {
        let last = run.borrow_mut().record(archive, outcome);
        if last {
            self.finish_batch(run.clone());
        }
    }

    /// End of a batch round. Encrypted leftovers get one password prompt —
    /// Unlock retries them all with it as a new round of the same run, Cancel
    /// skips them — and only once none are pending does the run speak: one
    /// summary toast, plus one dialog if anything failed or warned.
    fn finish_batch(&self, run: Rc<RefCell<BatchRun>>) {
        debug_assert!(run.borrow().is_finished());
        let pending = run.borrow().pending_passwords();
        if !pending.is_empty() {
            let names: Vec<String> = pending.iter().map(|p| file_name(&p.archive)).collect();
            let body = format!("{}\n\n{}", n_encrypted_body(names.len()), name_list(&names, 3));
            let window = self.clone();
            let unlock_run = run.clone();
            let cancel_window = self.clone();
            self.prompt_password_or_cancel(
                &body,
                move |pw| {
                    let retry = unlock_run.borrow_mut().begin_password_retry();
                    let delete_after = unlock_run.borrow().delete_after();
                    let overwrite = unlock_run.borrow().overwrite();
                    if retry.is_empty() {
                        window.finish_batch(unlock_run.clone());
                        return;
                    }
                    for job in retry {
                        window.start_extract(
                            job.archive,
                            job.dest,
                            Some(pw.clone()),
                            overwrite,
                            delete_after,
                            Some(unlock_run.clone()),
                        );
                    }
                },
                move || {
                    run.borrow_mut().skip_pending();
                    cancel_window.finish_batch(run.clone());
                },
            );
            return;
        }

        let (summary, folder, report, failed) = {
            let run = run.borrow();
            (run.summary(), run.common_folder(), run.report(), run.has_failures())
        };
        if let Some(summary) = summary {
            self.show_folder_toast(&summary, folder.as_deref());
        }
        if let Some(report) = report {
            // Warnings alone (a checksum file, a delete) aren't failures.
            let heading = if failed {
                gettext("Some Archives Failed")
            } else {
                gettext("Some Archives Had Problems")
            };
            self.show_alert(&heading, &report);
        }
    }

    /// Ask for a password; `on_password` runs with the entered text on Unlock.
    fn prompt_password<F: Fn(String) + 'static>(&self, body: &str, on_password: F) {
        self.prompt_password_or_cancel(body, on_password, || {});
    }

    /// `prompt_password` with a cancel path: `on_cancel` runs on Cancel, and
    /// on Escape or closing the dialog, which report as Cancel too.
    fn prompt_password_or_cancel<F, C>(&self, body: &str, on_password: F, on_cancel: C)
    where
        F: Fn(String) + 'static,
        C: Fn() + 'static,
    {
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
            } else {
                on_cancel();
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
                    window.start_compress(compression_request(&settings, inputs, output), write_checksum, None, None)
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

    /// Confirm, then compress each staged item into its own archive — saved
    /// next to it when we can really write there, or into one user-picked
    /// destination folder when we can't (or when a passwords file is coming).
    ///
    /// "Next to it" is a lie for portal-picked items: their paths live under
    /// the document portal, where a sibling write lands in the portal's
    /// private store, invisible in the real folder. And a passwords file must
    /// sit with its archives for the read side to find them. Both cases go
    /// through the destination picker.
    fn confirm_batch_compress(&self, settings: CreateSettings) {
        let items = settings.inputs.clone();
        let ext = archive_extension(&settings);
        let pick_destination =
            settings.generate_passwords || items.iter().any(|p| under_doc_portal(p));

        let body = if pick_destination {
            n_archives_create_pick_body(items.len(), settings.generate_passwords)
        } else {
            n_archives_create_body(items.len())
        };
        let dialog = adw::AlertDialog::new(Some(&gettext("Create Archives")), Some(&body));
        dialog.add_response("cancel", &gettext("Cancel"));
        dialog.add_response("create", &gettext("Create"));
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("create"));

        let window = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "create" {
                return;
            }
            if pick_destination {
                window.choose_batch_destination(settings.clone(), items.clone(), ext.clone());
                return;
            }
            // Numbered like the picked-folder path: `a.txt` + `a.pdf` would
            // otherwise both write `a.7z`, two 7zz processes on one file.
            let mut taken = std::collections::HashSet::new();
            let reqs: Vec<CompressionRequest> = items
                .iter()
                .filter_map(|item| {
                    let stem = item.file_stem()?;
                    let dir = item.parent()?;
                    let output = unique_output(dir, &stem.to_string_lossy(), &ext, &mut taken);
                    Some(compression_request(&settings, vec![item.clone()], output))
                })
                .collect();
            let run = compress_run(reqs.len());
            for req in reqs {
                window.start_compress(req, settings.write_checksum, None, run.clone());
            }
        });
        dialog.present(Some(self));
    }

    /// Pick the one folder the whole batch lands in (archives, and the
    /// passwords file when generating). Cancelling cancels the batch —
    /// nothing is created anywhere.
    fn choose_batch_destination(&self, settings: CreateSettings, items: Vec<PathBuf>, ext: String) {
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Save Archives To"))
            .modal(true)
            .build();

        let window = self.clone();
        dialog.select_folder(Some(self), gio::Cancellable::NONE, move |result| match result {
            Ok(folder) => match folder.path() {
                Some(dir) => {
                    if settings.generate_passwords {
                        window.start_batch_with_manifest(settings, items, ext, dir);
                    } else {
                        let mut taken = std::collections::HashSet::new();
                        let reqs: Vec<CompressionRequest> = items
                            .iter()
                            .filter_map(|item| {
                                let stem = item.file_stem()?;
                                let output =
                                    unique_output(&dir, &stem.to_string_lossy(), &ext, &mut taken);
                                Some(compression_request(&settings, vec![item.clone()], output))
                            })
                            .collect();
                        let run = compress_run(reqs.len());
                        for req in reqs {
                            window.start_compress(req, settings.write_checksum, None, run.clone());
                        }
                    }
                }
                None => window.show_toast(&gettext("That folder can't be written to directly.")),
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
        dir: PathBuf,
    ) {
        let now = glib::DateTime::now_utc()
            .ok()
            .and_then(|d| d.format_iso8601().ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let stamp = glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.format("%Y-%m-%d_%H%M").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let suffix = if settings.manifest_passphrase.is_some() { "json.gpg" } else { "json" };
        let dest = dir.join(format!("septima-passwords-{stamp}.{suffix}"));

        let mut manifest = Manifest::new();
        manifest.septima = crate::config::VERSION.to_string();
        manifest.created = now.clone();
        let mut jobs: Vec<(PathBuf, PathBuf, String)> = Vec::new();
        let mut taken = std::collections::HashSet::new();
        for item in &items {
            let Some(stem) = item.file_stem() else {
                continue;
            };
            let output = unique_output(&dir, &stem.to_string_lossy(), &ext, &mut taken);
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

            let run = compress_run(jobs.len());
            for (item, output, password) in jobs {
                let mut req = compression_request(&settings, vec![item], output);
                req.password = Some(password);
                window.start_compress(
                    req,
                    settings.write_checksum,
                    Some((state.clone(), nudge.clone())),
                    run.clone(),
                );
            }
        });
    }

    fn start_compress(
        &self,
        req: CompressionRequest,
        write_checksum: bool,
        manifest: Option<(Rc<BatchManifest>, async_channel::Sender<()>)>,
        batch: Option<Rc<RefCell<BatchRun>>>,
    ) {
        let output = req.output.clone();
        let row = SeptimaProgressRow::new(&format!("{}: {}", gettext("Creating"), file_name(&output)));
        let imp = self.imp();
        imp.jobs_box.append(&row);
        imp.jobs_revealer.set_reveal_child(true);

        let cancel = septima_engine::new_cancel_token();
        let cancel_ui = cancel.clone();

        let (sender, receiver) = async_channel::unbounded::<Job>();
        let waiting_sender = sender.downgrade();
        let sevenzip = septima_engine::sevenzip_path();
        let sevenzip_for_checksum = sevenzip.clone();

        let slots = compress_job_slots(imp.job_gate.borrow().limit(), &req);
        let ticket = self.submit_job(&row, slots, move || {
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
        });
        self.connect_job_cancel(&row, cancel_ui, &ticket, waiting_sender);

        let window = self.clone();
        glib::spawn_future_local(async move {
            let mut settled = false;
            while let Ok(message) = receiver.recv().await {
                match message {
                    Job::Progress(p) => row.set_progress(p.percent, p.current_file.as_deref()),
                    Job::Done(result) => {
                        window.finish_job(&row, Some(&ticket));
                        match result {
                            Ok(()) => {
                                if batch.is_none() {
                                    window.show_toast(&format!(
                                        "{} {}",
                                        gettext("Created"),
                                        output.display()
                                    ));
                                }
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
                                let done = Outcome::Done { output: output.clone() };
                                if write_checksum {
                                    // A batch job is recorded only once the
                                    // checksum file has settled: success is
                                    // silent, failure a warning in the report.
                                    let output = output.clone();
                                    let window = window.clone();
                                    let sevenzip = sevenzip_for_checksum.clone();
                                    let batch = batch.clone();
                                    glib::spawn_future_local(async move {
                                        let checksum_output = output.clone();
                                        let outcome = gio::spawn_blocking(move || {
                                            septima_engine::write_checksum_file(&sevenzip, &checksum_output)
                                        })
                                        .await;
                                        match (outcome, &batch) {
                                            (Ok(Ok(_)), Some(run)) => {
                                                window.record_batch(run, &output, done);
                                            }
                                            (_, Some(run)) => {
                                                run.borrow_mut().warn(
                                                    &output,
                                                    &gettext("created, but the checksum file couldn't be written"),
                                                );
                                                window.record_batch(run, &output, done);
                                            }
                                            (Ok(Ok(checksum_path)), None) => window.show_toast(&format!(
                                                "{} {}",
                                                gettext("Wrote"),
                                                file_name(&checksum_path)
                                            )),
                                            (_, None) => window.show_toast(&gettext(
                                                "Created, but the checksum file couldn't be written.",
                                            )),
                                        }
                                    });
                                } else if let Some(run) = &batch {
                                    window.record_batch(run, &output, done);
                                }
                            }
                            Err(EngineError::Cancelled) => {
                                if let Some(run) = &batch {
                                    window.record_batch(run, &output, Outcome::Cancelled);
                                }
                            }
                            Err(err) => match &batch {
                                Some(run) => {
                                    let outcome = Outcome::Failed { message: err.to_string() };
                                    window.record_batch(run, &output, outcome);
                                }
                                None => window.show_error(&err.to_string()),
                            },
                        }
                        settled = true;
                        break;
                    }
                }
            }
            if !settled {
                window.worker_died(&row, &ticket, &output, batch.as_ref());
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
                        window.finish_job(&row, None);
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
        imp.archive_files.replace(Vec::new());
        imp.window_title.set_title("Septima");
        imp.window_title.set_subtitle("");
        self.set_archive_actions_enabled(false);
    }

    fn set_archive_actions_enabled(&self, enabled: bool) {
        for name in ["close-archive", "test-archive", "extract", "rename-entry", "delete-entries"] {
            if let Some(action) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                action.set_enabled(enabled);
            }
        }
    }

    fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    /// A toast with `title`, plus — when there's a `folder` — a "Show in
    /// Files" button that opens it in the file manager via the OpenURI
    /// portal. Used after an extract and for a batch's summary.
    fn show_folder_toast(&self, title: &str, folder: Option<&Path>) {
        let Some(dest) = folder else {
            self.show_toast(title);
            return;
        };
        let toast = adw::Toast::builder()
            .title(title)
            .button_label(gettext("Show in Files"))
            .build();

        let window = self.clone();
        let dest = dest.to_path_buf();
        toast.connect_button_clicked(move |_| {
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&dest)));
            let window_for_err = window.clone();
            launcher.launch(Some(&window), gio::Cancellable::NONE, move |result| {
                if let Err(err) = result {
                    window_for_err.show_toast(err.message());
                }
            });
        });

        self.imp().toast_overlay.add_toast(toast);
    }

    /// Show a full (possibly long) error in a dialog — toasts truncate.
    fn show_error(&self, message: &str) {
        self.show_alert(&gettext("Something Went Wrong"), message);
    }

    /// A one-button dialog with `heading` and a (possibly long) `message`.
    /// A long one — a big batch's list of failures — scrolls inside the
    /// dialog instead of growing it past the screen.
    fn show_alert(&self, heading: &str, message: &str) {
        let message = message.trim();
        let dialog = if message.lines().count() > REPORT_LINES_BEFORE_SCROLL {
            let label = gtk::Label::builder()
                .label(message)
                .xalign(0.0)
                .yalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build();
            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .max_content_height(320)
                .child(&label)
                .build();
            let dialog = adw::AlertDialog::new(Some(heading), None);
            dialog.set_extra_child(Some(&scroller));
            dialog
        } else {
            adw::AlertDialog::new(Some(heading), Some(message))
        };
        dialog.add_response("close", &gettext("Close"));
        dialog.set_default_response(Some("close"));
        dialog.present(Some(self));
    }
}

/// The job list shows this many rows before it scrolls.
const JOB_ROWS_SHOWN: usize = 5;

/// Longer reports than this scroll inside their dialog.
const REPORT_LINES_BEFORE_SCROLL: usize = 8;

/// How many of the gate's slots a create job takes: more when one `7zz`
/// already fills the CPU or a large share of free memory.
fn compress_job_slots(limit: usize, req: &CompressionRequest) -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, usize::from);
    let estimate = req.codec.as_deref().and_then(|codec| {
        septima_engine::estimate_add_memory(
            codec,
            req.level,
            req.dictionary.as_deref().and_then(job_queue::size_arg_bytes),
            req.threads.unwrap_or(cpus as u32),
        )
    });
    job_queue::compress_slots(limit, cpus, req.threads, estimate, job_queue::memory_available())
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
    // The passwords file is read by people, possibly years on: it keeps the
    // plain cipher name, without the menu's "(standard)" aside.
    let cipher = match (settings.format.id, settings.encryption_method.as_deref()) {
        ("7z", None) => "AES-256",
        (format, method) => match septima_engine::capabilities::encryption_label(format, method) {
            Some(label) => label,
            None => return String::new(),
        },
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

/// Body of the end-of-round prompt for a batch's encrypted archives.
fn n_encrypted_body(n: usize) -> String {
    gettextrs::ngettext(
        "{} archive is encrypted. Enter its password to extract it.",
        "{} archives are encrypted. Enter their password to extract them.",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

/// Up to `shown` names, comma-separated, then "and N more" for the rest —
/// enough to recognise the batch without the dialog growing a scrollbar.
fn name_list(names: &[String], shown: usize) -> String {
    let head = names.iter().take(shown).cloned().collect::<Vec<_>>().join(", ");
    let rest = names.len().saturating_sub(shown);
    if rest == 0 {
        return head;
    }
    // Named placeholders, count first: a file name may itself contain "{…}",
    // and translators may need the two in the other order.
    gettextrs::ngettext("{names} and {count} more", "{names} and {count} more", rest as u32)
        .replacen("{count}", &rest.to_string(), 1)
        .replacen("{names}", &head, 1)
}

/// The shared tally for a batch compress of `jobs` archives — `None` for a
/// single one, which keeps its own toast and error dialog.
fn compress_run(jobs: usize) -> Option<Rc<RefCell<BatchRun>>> {
    (jobs >= 2).then(|| Rc::new(RefCell::new(BatchRun::new(BatchKind::Compress, jobs))))
}

/// Where dragged-out entries are staged before the receiver copies them.
/// Inside the Flatpak, `XDG_CACHE_HOME` is `~/.var/app/<id>/cache` — the same
/// path the host sees — so even a plain-URI receiver can read the files.
fn drag_staging_base() -> PathBuf {
    glib::user_cache_dir().join("septima-dnd")
}

/// Whether `path` sits under the document portal's FUSE mount
/// (`$XDG_RUNTIME_DIR/doc/…`). Writes to *siblings* of a granted item there
/// land in the portal's private store, not in the real folder — so "save next
/// to it" must not be attempted for such paths.
fn under_doc_portal(path: &Path) -> bool {
    path.starts_with(glib::user_runtime_dir().join("doc"))
}

/// Where a file opened through the document portal really lives. The portal
/// shows it under `/run/user/<uid>/doc/<id>/` and records the real path in an
/// extended attribute. `None` for ordinary paths or older portals.
fn portal_host_path(path: &Path) -> Option<PathBuf> {
    const ATTR: &str = "xattr::document-portal.host-path";
    if !under_doc_portal(path) {
        return None;
    }
    let info = gio::File::for_path(path)
        .query_info(ATTR, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        .ok()?;
    info.attribute_string(ATTR).map(|s| PathBuf::from(s.as_str()))
}

/// A non-colliding output path in `dir`: `photos.7z`, then `photos_2.7z`, …
/// Collisions happen when same-named items are staged from different folders,
/// when same-stem items share one (`a.txt` + `a.pdf`), or when the destination
/// already holds an archive by that name (which `7zz a` would silently
/// *update* rather than replace). `taken` holds full paths, so one set can
/// serve a batch whose outputs land in several folders.
fn unique_output(
    dir: &Path,
    stem: &str,
    ext: &str,
    taken: &mut std::collections::HashSet<PathBuf>,
) -> PathBuf {
    let mut n = 1usize;
    loop {
        let name = if n == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}_{n}.{ext}")
        };
        let candidate = dir.join(&name);
        if !taken.contains(&candidate) && !candidate.exists() {
            taken.insert(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

/// One manifest entry as the extract picker shows it: the archive's name, a
/// one-line context subtitle, and — when it's present with a password — the
/// (path, password) job a checked row runs.
struct ManifestRow {
    title: String,
    subtitle: String,
    job: Option<(PathBuf, String)>,
}

/// Resolve a manifest's entries against `dir` into picker rows. Only the
/// basename of each recorded archive is trusted (a hand-edited "../evil"
/// can't walk out of the folder), and an empty password greys the row rather
/// than guessing — never trimmed, never defaulted.
fn manifest_rows(dir: &Path, manifest: &Manifest) -> Vec<ManifestRow> {
    manifest
        .entries
        .iter()
        .map(|entry| {
            let title = entry.archive.clone();
            if entry.password.is_empty() {
                return ManifestRow {
                    title,
                    subtitle: gettext("No password recorded — check the passwords file"),
                    job: None,
                };
            }
            let archive = Path::new(&entry.archive).file_name().map(|n| dir.join(n));
            match archive {
                Some(archive) if archive.is_file() => {
                    let mut subtitle = String::new();
                    if !entry.source.is_empty() {
                        subtitle = gettext("from {}").replacen("{}", &entry.source, 1);
                    }
                    if !entry.encryption.is_empty() {
                        if !subtitle.is_empty() {
                            subtitle.push_str(" · ");
                        }
                        subtitle.push_str(&entry.encryption);
                    }
                    ManifestRow {
                        title,
                        subtitle,
                        job: Some((archive, entry.password.clone())),
                    }
                }
                _ => ManifestRow {
                    title,
                    subtitle: gettext("Not found next to the passwords file"),
                    job: None,
                },
            }
        })
        .collect()
}

/// The picker's confirm-button label: "Extract 2" while counting, plain
/// "Extract" only at zero (where the button is disabled anyway).
fn extract_label(n: usize) -> String {
    if n == 0 {
        gettext("Extract")
    } else {
        format!("{} {n}", gettext("Extract"))
    }
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

fn n_archives_create_body(n: usize) -> String {
    gettextrs::ngettext(
        "Create {} archive? It will be saved next to the item it's made from.",
        "Create {} archives? Each will be saved next to the item it's made from.",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

fn n_archives_create_pick_body(n: usize, with_manifest: bool) -> String {
    let text = if with_manifest {
        gettextrs::ngettext(
            "Create {} archive? You'll pick the folder where it and the passwords file are saved.",
            "Create {} archives? You'll pick one folder where they and the passwords file are saved.",
            n as u32,
        )
    } else {
        gettextrs::ngettext(
            "Create {} archive? You'll pick the folder it's saved into.",
            "Create {} archives? You'll pick the folder they're saved into.",
            n as u32,
        )
    };
    text.replacen("{}", &n.to_string(), 1)
}

/// What to do with files that already exist where an archive is extracted.
/// Shown only when something would clash; Keep both is the default because
/// it's the one choice that can't lose a file.
struct ClashChoice {
    widget: gtk::Box,
    skip: gtk::CheckButton,
    replace: gtk::CheckButton,
}

impl ClashChoice {
    fn new(delete_after: &gtk::CheckButton) -> Self {
        let radio = |text: &str| {
            let label = gtk::Label::builder().label(text).wrap(true).xalign(0.0).build();
            gtk::CheckButton::builder().child(&label).build()
        };
        let keep = radio(&gettext("Keep both — the new copies get a number, like report_1.txt"));
        let skip = radio(&gettext("Skip — leave the existing files, and don't extract those"));
        let replace = radio(&gettext("Replace the existing files"));
        skip.set_group(Some(&keep));
        replace.set_group(Some(&keep));
        keep.set_active(true);

        // Skip + delete-after would throw away the skipped files with the
        // archive, so the two can't be picked together.
        let delete_after = delete_after.clone();
        skip.connect_toggled(move |skip| {
            if skip.is_active() {
                delete_after.set_active(false);
            }
            delete_after.set_sensitive(!skip.is_active());
            delete_after.set_tooltip_text(
                skip.is_active()
                    .then(|| gettext("The skipped files would be lost with the archive"))
                    .as_deref(),
            );
        });

        let widget = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        widget.append(&keep);
        widget.append(&skip);
        widget.append(&replace);
        Self { widget, skip, replace }
    }

    fn mode(&self) -> OverwriteMode {
        if self.replace.is_active() {
            OverwriteMode::Overwrite
        } else if self.skip.is_active() {
            OverwriteMode::Skip
        } else {
            OverwriteMode::RenameExtracted
        }
    }

    /// The mode to extract with. With no choice shown, nothing clashed when
    /// the dialog opened; Keep both still guards anything that appears since.
    fn mode_of(choice: Option<&Self>) -> OverwriteMode {
        choice.map_or(OverwriteMode::RenameExtracted, Self::mode)
    }
}

/// How many of `files` (in-archive paths) already exist under `dest`.
fn count_clashes(dest: &Path, files: &[String]) -> usize {
    files
        .iter()
        .filter(|f| dest.join(f.trim_start_matches('/')).symlink_metadata().is_ok())
        .count()
}

/// Whether `dir` exists and has anything in it.
fn folder_has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut it| it.next().is_some()).unwrap_or(false)
}

fn n_clashing_files(n: usize, folder: &str) -> String {
    gettextrs::ngettext(
        "{} file in “{}” has the same name as one in this archive. What should happen to it?",
        "{} files in “{}” have the same names as files in this archive. What should happen to them?",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
    .replacen("{}", folder, 1)
}

fn n_existing_folders(n: usize) -> String {
    gettextrs::ngettext(
        "{} of their folders already exists, so files in it could have the same names.",
        "{} of their folders already exist, so files in them could have the same names.",
        n as u32,
    )
    .replacen("{}", &n.to_string(), 1)
}

/// Sibling extract folders for a run of archives, numbered within the run
/// (`photos/`, `photos_2/`, …) so `photos.zip` and `photos.7z` don't land in
/// one folder and overwrite each other. Only in-run clashes are numbered: an
/// existing `photos/` is still extracted into, as a single archive would be.
fn sibling_extract_dirs<'a>(archives: impl IntoIterator<Item = &'a Path>) -> Vec<PathBuf> {
    let mut taken = std::collections::HashSet::new();
    archives
        .into_iter()
        .map(|archive| {
            let mut n = 1;
            loop {
                let dest = sibling_extract_dir_numbered(archive, n);
                if taken.insert(dest.clone()) {
                    return dest;
                }
                n += 1;
            }
        })
        .collect()
}

/// Where a batch-extracted archive's contents land: a new folder next to it,
/// named after the archive (`photos.zip` -> `photos/`, `data.tar.gz` -> `data/`),
/// with `_n` appended for the `n`th clash (`n` = 1 is the plain name).
fn sibling_extract_dir_numbered(archive: &Path, n: usize) -> PathBuf {
    let stem = archive
        .file_stem()
        .map(PathBuf::from)
        .unwrap_or_else(|| archive.to_path_buf());
    let stem = match stem.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("tar") => stem.file_stem().map(PathBuf::from).unwrap_or(stem),
        _ => stem,
    };
    if n == 1 {
        archive.with_file_name(stem)
    } else {
        archive.with_file_name(format!("{}_{n}", stem.to_string_lossy()))
    }
}

#[cfg(test)]
mod tests {
    use super::{archive_filename, count_clashes, folder_has_entries, name_list, sibling_extract_dirs, unique_output};
    use std::path::{Path, PathBuf};

    #[test]
    fn unique_output_numbers_collisions() {
        let dir = std::env::temp_dir().join(format!("septima-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("busy.7z"), b"existing archive").unwrap();
        let mut taken = std::collections::HashSet::new();
        // In-batch duplicate names number up.
        assert_eq!(unique_output(&dir, "photos", "7z", &mut taken), dir.join("photos.7z"));
        assert_eq!(unique_output(&dir, "photos", "7z", &mut taken), dir.join("photos_2.7z"));
        // A file already on disk is a collision too — 7zz would update it.
        assert_eq!(unique_output(&dir, "busy", "7z", &mut taken), dir.join("busy_2.7z"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unique_output_only_numbers_clashes_in_the_same_folder() {
        let mut taken = std::collections::HashSet::new();
        let (a, b) = (Path::new("/nonexistent-a"), Path::new("/nonexistent-b"));
        assert_eq!(unique_output(a, "notes", "7z", &mut taken), a.join("notes.7z"));
        assert_eq!(unique_output(b, "notes", "7z", &mut taken), b.join("notes.7z"));
        assert_eq!(unique_output(a, "notes", "7z", &mut taken), a.join("notes_2.7z"));
    }

    #[test]
    fn clashes_count_only_files_that_already_exist() {
        let dir = std::env::temp_dir().join(format!("septima-clash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        std::fs::write(dir.join("photos/beach.jpg"), b"old").unwrap();
        std::fs::write(dir.join("notes.txt"), b"old").unwrap();
        let files: Vec<String> =
            ["photos/beach.jpg", "photos/sunset.jpg", "notes.txt", "/notes.txt", "new/readme.txt"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        // A leading slash is stripped, as 7zz does, so it still counts.
        assert_eq!(count_clashes(&dir, &files), 3);
        assert_eq!(count_clashes(&dir.join("elsewhere"), &files), 0);

        assert!(folder_has_entries(&dir));
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        assert!(!folder_has_entries(&dir.join("empty")), "an empty folder can't clash");
        assert!(!folder_has_entries(&dir.join("missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sibling_extract_dirs_number_same_stem_archives() {
        let archives = ["/d/photos.zip", "/d/photos.7z", "/d/data.tar.gz", "/d/data.zip", "/e/photos.zip"]
            .map(PathBuf::from);
        let dests = sibling_extract_dirs(archives.iter().map(PathBuf::as_path));
        assert_eq!(dests, ["/d/photos", "/d/photos_2", "/d/data", "/d/data_2", "/e/photos"].map(PathBuf::from));
    }

    #[test]
    fn name_list_shows_a_few_then_counts_the_rest() {
        let names: Vec<String> = ["a.zip", "b.7z", "c.zip", "d.zip", "e.zip"].map(String::from).into();
        assert_eq!(name_list(&names[..1], 3), "a.zip");
        assert_eq!(name_list(&names[..3], 3), "a.zip, b.7z, c.zip");
        assert_eq!(name_list(&names, 3), "a.zip, b.7z, c.zip and 2 more");
        assert_eq!(name_list(&[], 3), "");
    }

    #[test]
    fn name_list_leaves_braces_in_file_names_alone() {
        let names: Vec<String> = ["a{}.zip", "b.zip", "c.zip", "d.zip"].map(String::from).into();
        assert_eq!(name_list(&names, 3), "a{}.zip, b.zip, c.zip and 1 more");
    }

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
