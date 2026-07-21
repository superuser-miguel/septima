use std::cell::RefCell;
use std::time::Duration;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, CompositeTemplate, TemplateChild};

fn gettext(s: &str) -> String {
    gettextrs::gettext(s)
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/io/github/superuser_miguel/Septima/progress_row.ui")]
    pub struct SeptimaProgressRow {
        #[template_child]
        pub title_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub progress_bar: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        /// Drives the indeterminate "Scanning…" pulse until the first real percent.
        pub pulse_source: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeptimaProgressRow {
        const NAME: &'static str = "SeptimaProgressRow";
        type Type = super::SeptimaProgressRow;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SeptimaProgressRow {}
    impl WidgetImpl for SeptimaProgressRow {}
    impl BoxImpl for SeptimaProgressRow {}
}

glib::wrapper! {
    /// One running job: a title, a progress bar, and a cancel button.
    pub struct SeptimaProgressRow(ObjectSubclass<imp::SeptimaProgressRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for SeptimaProgressRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SeptimaProgressRow {
    pub fn new(title: &str) -> Self {
        let row = Self::default();
        row.imp().title_label.set_label(title);
        row.start_scanning();
        row
    }

    /// Enter the indeterminate "Scanning…" state. 7zz emits no percentage while
    /// it enumerates the input tree — slow and memory-heavy on a large selection
    /// over portal FUSE mounts — so pulse the bar until real progress arrives.
    /// Otherwise a big job just sits at 0% and looks frozen.
    fn start_scanning(&self) {
        let bar = self.imp().progress_bar.get();
        bar.set_text(Some(&gettext("Scanning…")));
        bar.pulse();
        let weak = bar.downgrade();
        let id = glib::timeout_add_local(Duration::from_millis(120), move || match weak.upgrade() {
            Some(bar) => {
                bar.pulse();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break, // row gone — stop the timer
        });
        self.imp().pulse_source.replace(Some(id));
    }

    fn stop_scanning(&self) {
        if let Some(id) = self.imp().pulse_source.take() {
            id.remove();
        }
    }

    /// Update from a percentage (0–100) and the current file name. The first
    /// real percentage ends the "Scanning…" pulse and switches to a determinate
    /// bar; a `None` percentage keeps pulsing (still no measurable progress).
    pub fn set_progress(&self, percent: Option<u8>, current_file: Option<&str>) {
        let bar = &self.imp().progress_bar;
        match percent {
            Some(p) => {
                self.stop_scanning();
                bar.set_fraction(p as f64 / 100.0);
                bar.set_text(current_file);
            }
            None => {
                bar.pulse();
                if current_file.is_some() {
                    bar.set_text(current_file);
                }
            }
        }
    }

    pub fn connect_cancel<F: Fn() + 'static>(&self, f: F) {
        self.imp().cancel_button.connect_clicked(move |_| f());
    }
}
