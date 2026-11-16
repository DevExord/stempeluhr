// SPDX-License-Identifier: GPL-3.0-or-later
//! Edit or delete a finished entry. Changes are sent once (never retried);
//! whatever the outcome, the caller reloads the list from the server.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate};
use jiff::{tz::TimeZone, Timestamp};
use stempeluhr_core::kimai::{Activity, EntryUpdate, Project, Timesheet};

use crate::application::Application;
use crate::i18n::tr;
use crate::{messages, runtime};

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/digital/fischers/stempeluhr/ui/edit-dialog.ui")]
    pub struct EditDialog {
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub project_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub activity_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub description_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub time_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub start_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub end_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub status_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub delete_button: TemplateChild<gtk::Button>,

        pub entry: RefCell<Option<Timesheet>>,
        pub projects: RefCell<Vec<Project>>,
        pub activities: RefCell<Vec<Activity>>,
        pub start_text: RefCell<String>,
        pub end_text: RefCell<String>,
        pub updating: Cell<bool>,
        pub generation: Cell<u64>,
        pub on_changed: RefCell<Option<Rc<dyn Fn()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditDialog {
        const NAME: &'static str = "StempeluhrEditDialog";
        type Type = super::EditDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EditDialog {}
    impl WidgetImpl for EditDialog {}
    impl AdwDialogImpl for EditDialog {}
}

glib::wrapper! {
    pub struct EditDialog(ObjectSubclass<imp::EditDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

fn hm(ts: Timestamp) -> String {
    glib::DateTime::from_unix_local(ts.as_second())
        .ok()
        .and_then(|d| d.format("%H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Parses `H:MM`, `HH:MM`, `HH.MM` or `HH:MM:SS`.
fn parse_time(s: &str) -> Option<(i8, i8, i8)> {
    let mut it = s.trim().split([':', '.']);
    let h: i8 = it.next()?.trim().parse().ok()?;
    let m: i8 = it.next()?.trim().parse().ok()?;
    let sec: i8 = match it.next() {
        Some(x) => x.trim().parse().ok()?,
        None => 0,
    };
    if it.next().is_some()
        || !(0..24).contains(&h)
        || !(0..60).contains(&m)
        || !(0..60).contains(&sec)
    {
        return None;
    }
    Some((h, m, sec))
}

/// `original` with its local clock time replaced by `text`.
fn with_time(original: Timestamp, text: &str) -> Option<Timestamp> {
    let (h, m, s) = parse_time(text)?;
    let tz = TimeZone::system();
    let date = original.to_zoned(tz.clone()).date();
    date.at(h, m, s, 0).to_zoned(tz).ok().map(|z| z.timestamp())
}

impl EditDialog {
    pub fn new(app: &Application, ts: &Timesheet, on_changed: Rc<dyn Fn()>) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.entry.replace(Some(ts.clone()));
        imp.on_changed.replace(Some(on_changed));

        let end = ts.end.unwrap_or(ts.begin);
        imp.start_text.replace(hm(ts.begin));
        imp.end_text.replace(hm(end));
        imp.start_row.set_text(&hm(ts.begin));
        imp.end_row.set_text(&hm(end));
        imp.description_row
            .set_text(ts.description.as_deref().unwrap_or_default());
        dialog.set_date_title(ts, end);

        let d = dialog.downgrade();
        let a = app.clone();
        imp.project_row.connect_selected_notify(move |_| {
            if let Some(d) = d.upgrade() {
                d.load_activities(&a);
            }
        });
        let (d, a) = (dialog.downgrade(), app.clone());
        imp.save_button.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.save(&a);
            }
        });
        let (d, a) = (dialog.downgrade(), app.clone());
        imp.delete_button.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.confirm_delete(&a);
            }
        });

        dialog.load_projects(app);

        // Debug builds: STEMPELUHR_DEBUG_EDIT_AUTOSAVE=1 changes the
        // description and saves, to exercise the whole path without clicking.
        #[cfg(debug_assertions)]
        if std::env::var("STEMPELUHR_DEBUG_EDIT_AUTOSAVE").is_ok() {
            let d = dialog.downgrade();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                if let Some(d) = d.upgrade() {
                    d.imp().description_row.set_text("edited by debug hook");
                    d.imp().start_row.set_text("07:30");
                    d.imp().save_button.emit_clicked();
                }
            });
        }
        dialog
    }

    fn set_date_title(&self, ts: &Timesheet, end: Timestamp) {
        let day = |t: Timestamp| {
            glib::DateTime::from_unix_local(t.as_second())
                .ok()
                .and_then(|d| d.format("%A, %e %B").ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        let (a, b) = (day(ts.begin), day(end));
        self.imp()
            .time_group
            .set_title(&if a == b { a } else { format!("{a} → {b}") });
    }

    fn show_status(&self, msg: &str) {
        let l = &self.imp().status_label;
        l.set_label(msg);
        l.set_visible(!msg.is_empty());
    }

    fn set_busy(&self, busy: bool) {
        let imp = self.imp();
        for w in [
            imp.project_row.upcast_ref::<gtk::Widget>(),
            imp.activity_row.upcast_ref(),
            imp.description_row.upcast_ref(),
            imp.start_row.upcast_ref(),
            imp.end_row.upcast_ref(),
            imp.save_button.upcast_ref(),
            imp.delete_button.upcast_ref(),
        ] {
            w.set_sensitive(!busy);
        }
    }

    fn changed(&self) {
        let cb = self.imp().on_changed.borrow().clone();
        if let Some(cb) = cb {
            cb();
        }
    }

    // ---- catalog -----------------------------------------------------------------

    fn load_projects(&self, app: &Application) {
        let (d, a) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            let Some(entry) = d.imp().entry.borrow().clone() else {
                return;
            };
            match a.load_projects(false).await {
                Ok(projects) => {
                    let imp = d.imp();
                    let names: Vec<String> = projects.iter().map(|p| p.name.clone()).collect();
                    let idx = projects
                        .iter()
                        .position(|p| p.id == entry.project.id)
                        .unwrap_or(0);
                    imp.updating.set(true);
                    *imp.projects.borrow_mut() = projects;
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    imp.project_row
                        .set_model(Some(&gtk::StringList::new(&refs)));
                    imp.project_row.set_selected(idx as u32);
                    imp.project_row.set_sensitive(true);
                    imp.updating.set(false);
                    d.load_activities(&a);
                }
                Err(e) => d.show_status(&messages::describe(&e)),
            }
        });
    }

    fn load_activities(&self, app: &Application) {
        let imp = self.imp();
        if imp.updating.get() {
            return;
        }
        let Some(project) = imp
            .projects
            .borrow()
            .get(imp.project_row.selected() as usize)
            .map(|p| p.id)
        else {
            return;
        };
        let generation = imp.generation.get() + 1;
        imp.generation.set(generation);
        imp.activity_row.set_sensitive(false);
        imp.save_button.set_sensitive(false);
        let (d, a) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            let result = a.load_activities(project).await;
            let imp = d.imp();
            if imp.generation.get() != generation {
                return;
            }
            match result {
                Ok(list) => {
                    let wanted = imp.entry.borrow().as_ref().map(|e| e.activity.id);
                    let idx = wanted
                        .and_then(|id| list.iter().position(|x| x.id == id))
                        .unwrap_or(0);
                    let names: Vec<&str> = list.iter().map(|x| x.name.as_str()).collect();
                    imp.updating.set(true);
                    imp.activity_row
                        .set_model(Some(&gtk::StringList::new(&names)));
                    imp.activity_row.set_selected(idx as u32);
                    imp.activity_row.set_sensitive(!names.is_empty());
                    imp.save_button.set_sensitive(!names.is_empty());
                    *imp.activities.borrow_mut() = list;
                    imp.updating.set(false);
                }
                Err(e) => d.show_status(&messages::describe(&e)),
            }
        });
    }

    // ---- save / delete ----------------------------------------------------------------

    fn build_update(&self) -> Result<(u64, EntryUpdate), String> {
        let imp = self.imp();
        let entry = imp
            .entry
            .borrow()
            .clone()
            .ok_or_else(|| tr("Nothing to save."))?;
        let end_orig = entry.end.unwrap_or(entry.begin);
        let project = imp
            .projects
            .borrow()
            .get(imp.project_row.selected() as usize)
            .map(|p| p.id)
            .ok_or_else(|| tr("Please choose a project."))?;
        let activity = imp
            .activities
            .borrow()
            .get(imp.activity_row.selected() as usize)
            .map(|a| a.id)
            .ok_or_else(|| tr("Please choose an activity."))?;

        // Untouched times keep their exact original values (seconds included).
        let begin = if imp.start_row.text() == imp.start_text.borrow().as_str() {
            entry.begin
        } else {
            with_time(entry.begin, &imp.start_row.text())
                .ok_or_else(|| tr("The start time isn’t valid. Use hh:mm."))?
        };
        let end = if imp.end_row.text() == imp.end_text.borrow().as_str() {
            end_orig
        } else {
            with_time(end_orig, &imp.end_row.text())
                .ok_or_else(|| tr("The end time isn’t valid. Use hh:mm."))?
        };
        if end <= begin {
            return Err(tr("The end must be after the start."));
        }
        Ok((
            entry.id,
            EntryUpdate {
                project,
                activity,
                begin,
                end,
                description: imp.description_row.text().trim().to_string(),
            },
        ))
    }

    fn save(&self, app: &Application) {
        let (id, update) = match self.build_update() {
            Ok(v) => v,
            Err(msg) => return self.show_status(&msg),
        };
        let Some(client) = app.engine().client() else {
            return self.show_status(&tr("Not connected."));
        };
        self.show_status("");
        self.set_busy(true);
        let (d, a) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            let result =
                runtime::spawn_result(
                    async move { client.update_entry(id, &update).await.map(|_| ()) },
                )
                .await;
            d.finish(&a, result, &tr("Saved."));
        });
    }

    fn confirm_delete(&self, app: &Application) {
        let (d, a) = (self.clone(), app.clone());
        glib::spawn_future_local(async move {
            let confirm = adw::AlertDialog::new(
                Some(&tr("Delete this entry?")),
                Some(&tr("It is permanently removed from Kimai.")),
            );
            confirm.add_responses(&[("cancel", &tr("_Cancel")), ("delete", &tr("_Delete"))]);
            confirm.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            confirm.set_default_response(Some("cancel"));
            confirm.set_close_response("cancel");
            if confirm.choose_future(Some(&d)).await != "delete" {
                return;
            }
            let Some(entry) = d.imp().entry.borrow().clone() else {
                return;
            };
            let Some(client) = a.engine().client() else {
                return d.show_status(&tr("Not connected."));
            };
            d.show_status("");
            d.set_busy(true);
            let id = entry.id;
            let result = runtime::spawn_result(async move { client.delete_entry(id).await }).await;
            d.finish(&a, result, &tr("Entry deleted."));
        });
    }

    /// One outcome path for save and delete. On success the dialog closes.
    /// On failure the message stays visible; if the request may have
    /// arrived anyway the list is reloaded so it shows what the server has.
    fn finish(
        &self,
        app: &Application,
        result: Result<(), stempeluhr_core::kimai::Error>,
        ok_msg: &str,
    ) {
        match result {
            Ok(()) => {
                self.changed();
                self.close();
                if let Some(win) = app.main_window() {
                    win.toast(ok_msg);
                }
            }
            Err(e) => {
                app.engine().note_error(&e);
                self.set_busy(false);
                self.show_status(&messages::describe(&e));
                if e.is_ambiguous() {
                    self.changed();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_parsing_is_lenient_but_strict_on_ranges() {
        assert_eq!(parse_time("9:05"), Some((9, 5, 0)));
        assert_eq!(parse_time(" 09.05 "), Some((9, 5, 0)));
        assert_eq!(parse_time("23:59:58"), Some((23, 59, 58)));
        for bad in ["", "24:00", "12:60", "12", "ab:cd", "1:2:3:4", "-1:00"] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }
}
