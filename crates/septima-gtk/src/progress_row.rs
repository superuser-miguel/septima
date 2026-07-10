use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, CompositeTemplate, TemplateChild};

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
        row
    }

    /// Update from a percentage (0–100) and the current file name.
    pub fn set_progress(&self, percent: Option<u8>, current_file: Option<&str>) {
        let bar = &self.imp().progress_bar;
        match percent {
            Some(p) => bar.set_fraction(p as f64 / 100.0),
            None => bar.pulse(),
        }
        bar.set_text(current_file);
    }

    pub fn connect_cancel<F: Fn() + 'static>(&self, f: F) {
        self.imp().cancel_button.connect_clicked(move |_| f());
    }
}
