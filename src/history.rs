// SPDX-License-Identifier: GPL-3.0-or-later
//! The history page: finished entries of the last 30 days, grouped by day.
//! It shows what the server returned — nothing is changed locally. After an
//! edit or delete the list is simply loaded again.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate};
use jiff::{SignedDuration, Timestamp};
use stempeluhr_core::kimai::Timesheet;

use crate::application::Application;
use crate::edit_dialog::EditDialog;
use crate::i18n::tr;
use crate::{format, messages};

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/de/example/Stempeluhr/ui/history-page.ui")]
    pub struct HistoryPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub list_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub error_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        pub app: RefCell<Option<Application>>,
        pub generation: std::cell::Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HistoryPage {
        const NAME: &'static str = "StempeluhrHistoryPage";
        type Type = super::HistoryPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for HistoryPage {}
    impl WidgetImpl for HistoryPage {}
    impl NavigationPageImpl for HistoryPage {}
}

glib::wrapper! {
    pub struct HistoryPage(ObjectSubclass<imp::HistoryPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

fn local(ts: Timestamp) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(ts.as_second()).ok()
}

fn day_title(day: &glib::DateTime) -> String {
    let today = glib::DateTime::now_local().ok();
    if let Some(now) = &today {
        if now.ymd() == day.ymd() {
            return tr("Today");
        }
        if now
            .add_days(-1)
            .map(|y| y.ymd() == day.ymd())
            .unwrap_or(false)
        {
            return tr("Yesterday");
        }
    }
    day.format("%A, %e %B")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// `H:MM` for durations in a list.
fn short(d: SignedDuration) -> String {
    let m = d.as_secs().max(0) / 60;
    format!("{}:{:02}", m / 60, m % 60)
}

impl HistoryPage {
    pub fn new(app: &Application) -> Self {
        let page: Self = glib::Object::new();
        page.imp().app.replace(Some(app.clone()));
        let this = page.downgrade();
        page.imp().retry_button.connect_clicked(move |_| {
            if let Some(p) = this.upgrade() {
                p.reload();
            }
        });
        page.reload();
        page
    }

    pub fn reload(&self) {
        let imp = self.imp();
        let Some(app) = imp.app.borrow().clone() else {
            return;
        };
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);
        imp.stack.set_visible_child_name("loading");
        let page = self.clone();
        glib::spawn_future_local(async move {
            let result = app.load_history().await;
            let imp = page.imp();
            if imp.generation.get() != generation {
                return;
            }
            match result {
                Ok(list) if list.is_empty() => imp.stack.set_visible_child_name("empty"),
                Ok(list) => {
                    // Names are normally included; fill gaps from the catalog.
                    for ts in &list {
                        if app.labels_cached(ts).is_none() {
                            app.resolve_labels(ts).await;
                        }
                    }
                    if imp.generation.get() != generation {
                        return;
                    }
                    page.fill(&app, list);
                    imp.stack.set_visible_child_name("list");
                }
                Err(e) => {
                    imp.error_page
                        .set_description(Some(&messages::describe(&e)));
                    imp.stack.set_visible_child_name("error");
                }
            }
        });
    }

    /// Debug builds only: opens the edit dialog of the first row.
    #[cfg(debug_assertions)]
    pub fn debug_activate_first_row(&self) {
        let group = self.imp().list_box.first_child();
        let list = group.and_then(|g| g.last_child());
        if let Some(row) = list
            .and_then(|l| l.downcast::<gtk::ListBox>().ok())
            .and_then(|l| l.row_at_index(0))
        {
            row.activate();
        }
    }

    fn fill(&self, app: &Application, list: Vec<Timesheet>) {
        let imp = self.imp();
        while let Some(child) = imp.list_box.first_child() {
            imp.list_box.remove(&child);
        }
        let reload: Rc<dyn Fn()> = {
            let page = self.downgrade();
            Rc::new(move || {
                if let Some(p) = page.upgrade() {
                    p.reload();
                }
            })
        };

        // Entries arrive newest first; group consecutive ones by local day.
        let mut groups: Vec<(glib::DateTime, Vec<Timesheet>)> = Vec::new();
        for ts in list {
            let Some(day) = local(ts.begin) else { continue };
            match groups.last_mut() {
                Some((d, items)) if d.ymd() == day.ymd() => items.push(ts),
                _ => groups.push((day, vec![ts])),
            }
        }

        for (day, items) in groups {
            let total: SignedDuration = items
                .iter()
                .filter_map(|t| t.end.map(|e| e.duration_since(t.begin)))
                .fold(SignedDuration::ZERO, |a, b| a + b);

            let heading = gtk::Box::builder()
                .spacing(8)
                .margin_start(4)
                .margin_end(4)
                .build();
            heading.append(
                &gtk::Label::builder()
                    .label(day_title(&day))
                    .xalign(0.0)
                    .hexpand(true)
                    .css_classes(["heading"])
                    .build(),
            );
            heading.append(
                &gtk::Label::builder()
                    .label(short(total))
                    .css_classes(["numeric", "dim-label"])
                    .build(),
            );

            let list_widget = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            for ts in items {
                list_widget.append(&self.row(app, &ts, &reload));
            }

            let group = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(8)
                .build();
            group.append(&heading);
            group.append(&list_widget);
            imp.list_box.append(&group);
        }
    }

    fn row(&self, app: &Application, ts: &Timesheet, reload: &Rc<dyn Fn()>) -> adw::ActionRow {
        let labels = app.labels_cached(ts);
        let (project, activity) = labels
            .as_ref()
            .map(|l| (l.project.clone(), l.activity.clone()))
            .unwrap_or_else(|| {
                (
                    format!("#{}", ts.project.id),
                    format!("#{}", ts.activity.id),
                )
            });
        let end = ts.end.unwrap_or(ts.begin);
        let range = format!("{}–{}", format::clock(ts.begin), format::clock(end));
        let mut subtitle = format!("{project} · {range}");
        if let Some(d) = ts
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            subtitle.push('\n');
            subtitle.push_str(d);
        }
        let row = adw::ActionRow::builder()
            .title(&activity)
            .subtitle(&subtitle)
            .use_markup(false)
            .activatable(true)
            .build();
        row.add_suffix(
            &gtk::Label::builder()
                .label(short(end.duration_since(ts.begin)))
                .css_classes(["numeric"])
                .valign(gtk::Align::Center)
                .build(),
        );
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (ts, app, reload) = (ts.clone(), app.clone(), reload.clone());
        let page = self.downgrade();
        row.connect_activated(move |_| {
            if let Some(page) = page.upgrade() {
                EditDialog::new(&app, &ts, reload.clone()).present(Some(&page));
            }
        });
        row
    }
}
