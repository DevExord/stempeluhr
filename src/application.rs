// SPDX-License-Identifier: GPL-3.0-or-later
//! The application object: owns the engine, the settings, the catalog cache
//! and all "outside the window" behaviour (actions, notification, portals).

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use stempeluhr_core::engine::{Engine, Outcome, RefreshReason, Snapshot};
use stempeluhr_core::kimai::{
    Activity, Client, Error, ErrorKind, Project, StartRequest, Timesheet, Token,
};
use stempeluhr_core::state::State;

use crate::config::{APP_ID, RESOURCE_PREFIX, VERSION};
use crate::connect_dialog::ConnectDialog;
use crate::i18n::tr;
use crate::preferences::PreferencesDialog;
use crate::window::StempeluhrWindow;
use crate::{format, messages, runtime, secrets, shortcuts};

const NOTIFICATION_RUNNING: &str = "running";
const NOTIFICATION_ERROR: &str = "error";
const CATALOG_TTL: Duration = Duration::from_secs(300);

/// Human-readable names for a running timesheet.
#[derive(Debug, Clone)]
pub struct Labels {
    pub project: String,
    pub customer: Option<String>,
    pub activity: String,
    /// Project colour as sent by Kimai (unvalidated; see `ring::parse_hex`).
    pub color: Option<String>,
}

/// A recently used project/activity combination, with display names.
#[derive(Debug, Clone)]
pub struct Recent {
    pub project_id: u64,
    pub activity_id: u64,
    pub labels: Labels,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Start,
    Stop,
    /// Stop one specific entry (from the notification button).
    StopEntry(u64),
    Toggle,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Application {
        pub engine: Engine,
        pub settings: OnceCell<gio::Settings>,
        pub config_loaded: tokio::sync::OnceCell<()>,
        pub keyring_error: RefCell<Option<String>>,
        pub projects: RefCell<Option<(Vec<Project>, Instant)>>,
        pub activities: RefCell<HashMap<u64, Vec<Activity>>>,
        pub labels: RefCell<HashMap<(u64, u64), Labels>>,
        pub last_notification: RefCell<Option<String>>,
        pub resolving_notification: std::cell::Cell<bool>,
        pub shortcut_stop: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "StempeluhrApplication";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn startup(&self) {
            self.parent_startup();
            self.obj().setup();
        }

        fn activate(&self) {
            self.parent_activate();
            self.obj().present_window();
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Application {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("flags", gio::ApplicationFlags::FLAGS_NONE)
            .property("resource-base-path", RESOURCE_PREFIX)
            .build()
    }

    pub fn engine(&self) -> &Engine {
        &self.imp().engine
    }

    pub fn settings(&self) -> &gio::Settings {
        self.imp()
            .settings
            .get_or_init(|| gio::Settings::new(APP_ID))
    }

    pub fn main_window(&self) -> Option<StempeluhrWindow> {
        self.windows()
            .into_iter()
            .find_map(|w| w.downcast::<StempeluhrWindow>().ok())
    }

    pub fn present_window(&self) -> StempeluhrWindow {
        let win = self
            .main_window()
            .unwrap_or_else(|| StempeluhrWindow::new(self));
        win.present();
        win
    }

    // ---- setup -----------------------------------------------------------

    fn setup(&self) {
        let entries = [
            gio::ActionEntry::builder("start")
                .activate(|app: &Self, _, _| app.run_action(Kind::Start))
                .build(),
            gio::ActionEntry::builder("stop")
                .activate(|app: &Self, _, _| app.run_action(Kind::Stop))
                .build(),
            gio::ActionEntry::builder("stop-entry")
                .parameter_type(Some(glib::VariantTy::INT64))
                .activate(|app: &Self, _, param| {
                    match param
                        .and_then(|p| p.get::<i64>())
                        .and_then(|id| u64::try_from(id).ok())
                    {
                        Some(id) => app.run_action(Kind::StopEntry(id)),
                        None => app.run_action(Kind::Stop),
                    }
                })
                .build(),
            gio::ActionEntry::builder("toggle")
                .activate(|app: &Self, _, _| app.run_action(Kind::Toggle))
                .build(),
            gio::ActionEntry::builder("refresh")
                .activate(|app: &Self, _, _| app.manual_refresh())
                .build(),
            gio::ActionEntry::builder("show")
                .activate(|app: &Self, _, _| {
                    app.present_window();
                })
                .build(),
            gio::ActionEntry::builder("history")
                .activate(|app: &Self, _, _| {
                    if app.engine().client().is_some() {
                        app.present_window().show_history();
                    }
                })
                .build(),
            #[cfg(debug_assertions)]
            gio::ActionEntry::builder("debug-focus")
                .parameter_type(Some(glib::VariantTy::STRING))
                .activate(|app: &Self, _, param| {
                    if let Some(w) = app.main_window() {
                        w.set_focus_visible(true);
                        match param.and_then(|p| p.get::<String>()).as_deref() {
                            Some("entry") => {
                                w.imp().description_entry.grab_focus();
                            }
                            _ => {
                                w.imp().project_dropdown.grab_focus();
                            }
                        }
                    }
                })
                .build(),
            #[cfg(debug_assertions)]
            gio::ActionEntry::builder("debug-recent")
                .activate(|app: &Self, _, _| {
                    if let Some(w) = app.main_window() {
                        w.imp().recent_button.popup();
                    }
                })
                .build(),
            #[cfg(debug_assertions)]
            gio::ActionEntry::builder("debug-edit-first")
                .activate(|app: &Self, _, _| {
                    let page = app
                        .main_window()
                        .and_then(|w| w.imp().nav_view.visible_page())
                        .and_then(|p| p.downcast::<crate::history::HistoryPage>().ok());
                    if let Some(page) = page {
                        page.debug_activate_first_row();
                    }
                })
                .build(),
            gio::ActionEntry::builder("connect")
                .activate(|app: &Self, _, _| app.show_connect_dialog())
                .build(),
            gio::ActionEntry::builder("preferences")
                .activate(|app: &Self, _, _| app.show_preferences())
                .build(),
            gio::ActionEntry::builder("shortcuts")
                .activate(|app: &Self, _, _| app.show_shortcuts())
                .build(),
            gio::ActionEntry::builder("about")
                .activate(|app: &Self, _, _| app.show_about())
                .build(),
            gio::ActionEntry::builder("quit")
                .activate(|app: &Self, _, _| app.quit())
                .build(),
        ];
        self.add_action_entries(entries);
        self.set_accels_for_action("app.toggle", &["<Control>Return"]);
        self.set_accels_for_action("app.refresh", &["<Control>r", "F5"]);
        self.set_accels_for_action("app.history", &["<Control>h"]);
        self.set_accels_for_action("app.preferences", &["<Control>comma"]);
        self.set_accels_for_action("app.shortcuts", &["<Control>question"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
        self.set_accels_for_action("app.quit", &["<Control>q"]);

        // Single place that reacts to confirmed state changes.
        let app = self.clone();
        let mut rx = self.engine().subscribe();
        glib::spawn_future_local(async move {
            while rx.changed().await.is_ok() {
                let snap = rx.borrow_and_update().clone();
                app.on_snapshot(&snap);
            }
        });

        // Whatever the settings say, an enabled global shortcut is re-armed
        // on every start (it only lives as long as the process).
        if self.settings().boolean("global-shortcut-enabled") {
            self.set_global_shortcut(true);
        }
    }

    fn on_snapshot(&self, snap: &Snapshot) {
        self.sync_notification(snap);
        if let Some(win) = self.main_window() {
            win.render(snap);
        }
    }

    // ---- configuration ----------------------------------------------------

    /// Loads URL + token once and configures the engine. Safe to call from
    /// anywhere, any number of times.
    pub async fn ensure_configured(&self) {
        let app = self.clone();
        self.imp()
            .config_loaded
            .get_or_init(|| async move { app.load_config().await })
            .await;
    }

    async fn load_config(&self) {
        let url = self.settings().string("server-url").to_string();
        if url.is_empty() {
            self.engine().configure(None);
            return;
        }
        match secrets::load().await {
            Ok(Some(token)) => match Client::new(&url, Token::new(token)) {
                Ok(client) => self.engine().configure(Some(client)),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot build client");
                    self.engine().configure(None);
                }
            },
            Ok(None) => self.engine().configure(None),
            Err(e) => {
                tracing::warn!(error = %e, "keyring unavailable");
                *self.imp().keyring_error.borrow_mut() = Some(e.to_string());
                self.engine().configure(None);
            }
        }
    }

    /// A one-shot message about a keyring problem found at startup.
    pub fn take_keyring_error(&self) -> Option<String> {
        self.imp().keyring_error.borrow_mut().take()
    }

    fn reset_catalog(&self) {
        let imp = self.imp();
        *imp.projects.borrow_mut() = None;
        imp.activities.borrow_mut().clear();
        imp.labels.borrow_mut().clear();
    }

    /// Called by the connect dialog after a successful, saved connection.
    pub fn set_connection(&self, url: &str, client: Client) {
        let _ = self.imp().config_loaded.set(());
        if let Err(e) = self.settings().set_string("server-url", url) {
            tracing::warn!("cannot save server url: {e}");
        }
        self.reset_catalog();
        self.engine().configure(Some(client));
        self.refresh(RefreshReason::Startup);
    }

    pub async fn disconnect(&self) {
        if let Err(e) = secrets::clear().await {
            tracing::warn!(error = %e, "cannot remove token from keyring");
        }
        let _ = self.settings().set_string("server-url", "");
        let _ = self.settings().set_int64("last-project", 0);
        let _ = self.settings().set_int64("last-activity", 0);
        self.reset_catalog();
        self.engine().configure(None);
        self.withdraw_notification(NOTIFICATION_RUNNING);
        *self.imp().last_notification.borrow_mut() = None;
    }

    // ---- refreshing -------------------------------------------------------

    pub fn refresh(&self, reason: RefreshReason) {
        let app = self.clone();
        glib::spawn_future_local(async move {
            app.ensure_configured().await;
            let engine = app.engine().clone();
            let _ = runtime::spawn(async move { engine.refresh(reason).await }).await;
        });
    }

    fn manual_refresh(&self) {
        self.refresh(RefreshReason::Manual);
        if let Some(win) = self.main_window() {
            win.reload_catalog();
        }
    }

    // ---- catalog ----------------------------------------------------------

    pub async fn load_projects(&self, force: bool) -> Result<Vec<Project>, Error> {
        if !force {
            if let Some((p, at)) = &*self.imp().projects.borrow() {
                if at.elapsed() < CATALOG_TTL {
                    return Ok(p.clone());
                }
            }
        }
        let client = self.client()?;
        match runtime::spawn_result(async move { client.projects().await }).await {
            Ok(p) => {
                *self.imp().projects.borrow_mut() = Some((p.clone(), Instant::now()));
                Ok(p)
            }
            Err(e) => {
                self.engine().note_error(&e);
                Err(e)
            }
        }
    }

    pub async fn load_activities(&self, project: u64) -> Result<Vec<Activity>, Error> {
        if let Some(a) = self.imp().activities.borrow().get(&project) {
            return Ok(a.clone());
        }
        let client = self.client()?;
        match runtime::spawn_result(async move { client.activities(project).await }).await {
            Ok(a) => {
                self.imp()
                    .activities
                    .borrow_mut()
                    .insert(project, a.clone());
                Ok(a)
            }
            Err(e) => {
                self.engine().note_error(&e);
                Err(e)
            }
        }
    }

    fn client(&self) -> Result<std::sync::Arc<Client>, Error> {
        self.engine()
            .client()
            .ok_or_else(|| Error::new(ErrorKind::Other, "not connected"))
    }

    /// Distinct recent project/activity combinations, newest first.
    pub async fn load_recent(&self) -> Result<Vec<Recent>, Error> {
        let client = self.client()?;
        let list = match runtime::spawn_result(
            async move { client.recent().await.map(|r| r.value) },
        )
        .await
        {
            Ok(l) => l,
            Err(e) => {
                self.engine().note_error(&e);
                return Err(e);
            }
        };
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for ts in list {
            let key = (ts.project.id, ts.activity.id);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let labels = self.resolve_labels(&ts).await;
            out.push(Recent {
                project_id: key.0,
                activity_id: key.1,
                labels,
            });
            if out.len() >= 3 {
                break;
            }
        }
        Ok(out)
    }

    /// Finished entries of the last 30 days, newest first.
    pub async fn load_history(&self) -> Result<Vec<Timesheet>, Error> {
        let client = self.client()?;
        let since = jiff::Timestamp::now()
            .checked_sub(jiff::SignedDuration::from_secs(30 * 24 * 3600))
            .unwrap_or_else(|_| jiff::Timestamp::now());
        match runtime::spawn_result(async move { client.history(since).await }).await {
            Ok(list) => Ok(list),
            Err(e) => {
                self.engine().note_error(&e);
                Err(e)
            }
        }
    }

    pub fn labels_cached(&self, ts: &Timesheet) -> Option<Labels> {
        // Fast path only when the server sent everything, colour included;
        // otherwise the catalog may know more.
        if let (Some(project), Some(activity), Some(_)) =
            (&ts.project.name, &ts.activity.name, &ts.project.color)
        {
            return Some(Labels {
                project: project.clone(),
                customer: ts.project.parent.clone(),
                activity: activity.clone(),
                color: ts.project.color.clone(),
            });
        }
        self.imp()
            .labels
            .borrow()
            .get(&(ts.project.id, ts.activity.id))
            .cloned()
    }

    /// Names for a timesheet, looking them up in the catalog if the server
    /// only sent ids. Always returns something displayable.
    pub async fn resolve_labels(&self, ts: &Timesheet) -> Labels {
        if let Some(l) = self.labels_cached(ts) {
            return l;
        }
        let projects = self.load_projects(false).await.ok();
        let activities = self.load_activities(ts.project.id).await.ok();
        let project = ts.project.name.clone().or_else(|| {
            projects
                .as_ref()
                .and_then(|p| p.iter().find(|p| p.id == ts.project.id))
                .map(|p| p.name.clone())
        });
        let customer = ts.project.parent.clone().or_else(|| {
            projects
                .as_ref()
                .and_then(|p| p.iter().find(|p| p.id == ts.project.id))
                .and_then(|p| p.customer.clone())
        });
        let activity = ts.activity.name.clone().or_else(|| {
            activities
                .as_ref()
                .and_then(|a| a.iter().find(|a| a.id == ts.activity.id))
                .map(|a| a.name.clone())
        });
        let complete = project.is_some() && activity.is_some();
        let labels = Labels {
            project: project.unwrap_or_else(|| format!("#{}", ts.project.id)),
            customer,
            activity: activity.unwrap_or_else(|| format!("#{}", ts.activity.id)),
            color: ts.project.color.clone().or_else(|| {
                projects
                    .as_ref()
                    .and_then(|p| p.iter().find(|p| p.id == ts.project.id))
                    .and_then(|p| p.color.clone())
            }),
        };
        if complete {
            self.imp()
                .labels
                .borrow_mut()
                .insert((ts.project.id, ts.activity.id), labels.clone());
        }
        labels
    }

    // ---- start / stop / toggle -------------------------------------------

    /// What a start should use: the window's selection if a window is open,
    /// otherwise (depending on the setting) the last used combination.
    fn start_request(&self) -> Option<StartRequest> {
        if let Some(req) = self.main_window().and_then(|w| w.start_request()) {
            return Some(req);
        }
        if self.settings().string("start-behavior") == "window" {
            return None;
        }
        let project = self.settings().int64("last-project");
        let activity = self.settings().int64("last-activity");
        (project > 0 && activity > 0).then_some(StartRequest {
            project: project as u64,
            activity: activity as u64,
            description: None,
        })
    }

    fn run_action(&self, kind: Kind) {
        let guard = self.hold();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let _guard = guard; // released when the work is done
            app.perform(kind).await;
            app.sync_notification(&app.engine().snapshot());
        });
    }

    async fn perform(&self, kind: Kind) {
        if matches!(kind, Kind::Stop | Kind::StopEntry(_) | Kind::Toggle) {
            if let Some(win) = self.main_window() {
                win.begin_stop();
            }
        }
        self.ensure_configured().await;
        let engine = self.engine().clone();
        let request = self.start_request();
        let outcome = match kind {
            Kind::Start => match request.clone() {
                Some(req) => runtime::spawn(async move { engine.start(req).await }).await,
                None => Ok(Outcome::NeedsInput),
            },
            Kind::Stop => runtime::spawn(async move { engine.stop().await }).await,
            Kind::StopEntry(id) => runtime::spawn(async move { engine.stop_entry(id).await }).await,
            Kind::Toggle => {
                let req = request.clone();
                runtime::spawn(async move { engine.toggle(req).await }).await
            }
        }
        .unwrap_or_else(Outcome::Failed);

        if matches!(outcome, Outcome::Started) {
            if let Some(req) = &request {
                let _ = self
                    .settings()
                    .set_int64("last-project", req.project as i64);
                let _ = self
                    .settings()
                    .set_int64("last-activity", req.activity as i64);
            }
        }
        self.handle_outcome(outcome);
    }

    fn handle_outcome(&self, outcome: Outcome) {
        if let Some(win) = self.main_window() {
            win.stop_finished(matches!(outcome, Outcome::Stopped));
        }
        match outcome {
            Outcome::Started | Outcome::Stopped => {
                self.withdraw_notification(NOTIFICATION_ERROR);
            }
            Outcome::AlreadyRunning => self.say(&tr("Already running — it was started elsewhere.")),
            Outcome::AlreadyStopped => self.say(&tr("Already stopped elsewhere.")),
            Outcome::NothingToStop => self.say(&tr("Nothing is running.")),
            Outcome::NeedsInput => {
                let win = self.present_window();
                win.toast(&tr("Choose a project and activity, then press Start."));
            }
            Outcome::Ignored => {
                if matches!(
                    self.engine().snapshot().state,
                    State::NotConfigured | State::Unauthorized
                ) {
                    self.present_window();
                }
            }
            Outcome::Failed(e) => {
                if e.is_unauthorized() {
                    self.present_window();
                } else {
                    self.report_failure(&messages::describe(&e));
                }
            }
        }
    }

    /// Short feedback: a toast if the window is open, nothing otherwise
    /// (the notification already shows the result).
    fn say(&self, msg: &str) {
        if let Some(win) = self.main_window() {
            win.toast(msg);
        }
    }

    fn report_failure(&self, msg: &str) {
        if let Some(win) = self.main_window() {
            win.toast(msg);
        } else {
            let n = gio::Notification::new(&tr("Time tracking"));
            n.set_body(Some(msg));
            n.set_priority(gio::NotificationPriority::High);
            n.set_default_action("app.show");
            self.send_notification(Some(NOTIFICATION_ERROR), &n);
        }
    }

    // ---- notification -------------------------------------------------------

    fn sync_notification(&self, snap: &Snapshot) {
        match &snap.state {
            State::Running { timesheet, .. } => self.show_running_notification(timesheet),
            // Unknown yet / in flight / unconfirmed: leave whatever is shown alone.
            State::Loading | State::Busy { .. } | State::Error { .. } => {}
            // Server confirmed that nothing runs (or we can no longer tell).
            State::Idle { .. } | State::NotConfigured | State::Unauthorized => {
                self.withdraw_notification(NOTIFICATION_RUNNING);
                *self.imp().last_notification.borrow_mut() = None;
            }
        }
    }

    fn show_running_notification(&self, ts: &Timesheet) {
        let Some(labels) = self.labels_cached(ts) else {
            // Names still unknown: resolve, then try again.
            if self.imp().resolving_notification.replace(true) {
                return;
            }
            let app = self.clone();
            let ts = ts.clone();
            let guard = self.hold();
            glib::spawn_future_local(async move {
                let _guard = guard;
                app.resolve_labels(&ts).await;
                app.imp().resolving_notification.set(false);
                if matches!(app.engine().snapshot().state, State::Running { .. }) {
                    app.sync_notification(&app.engine().snapshot());
                }
            });
            return;
        };
        let what = match &labels.customer {
            Some(c) => format!("{} · {} · {}", c, labels.project, labels.activity),
            None => format!("{} · {}", labels.project, labels.activity),
        };
        let body = format!("{}\n{}", what, format::since(ts.begin));
        let key = format!("{}|{}", ts.id, body);
        if self.imp().last_notification.borrow().as_deref() == Some(key.as_str()) {
            return;
        }
        let n = gio::Notification::new(&tr("Time tracking is running"));
        n.set_body(Some(&body));
        n.set_priority(gio::NotificationPriority::Low);
        n.set_default_action("app.show");
        // The button remembers which entry it is about, so a stale one can
        // never stop a different, newer entry.
        n.add_button_with_target_value(
            &tr("Stop"),
            "app.stop-entry",
            Some(&(ts.id as i64).to_variant()),
        );
        self.send_notification(Some(NOTIFICATION_RUNNING), &n);
        *self.imp().last_notification.borrow_mut() = Some(key);
    }

    // ---- dialogs ----------------------------------------------------------

    fn show_connect_dialog(&self) {
        let win = self.present_window();
        ConnectDialog::new(self).present(Some(&win));
    }

    fn show_preferences(&self) {
        let win = self.present_window();
        PreferencesDialog::new(self).present(Some(&win));
    }

    fn show_shortcuts(&self) {
        let win = self.present_window();
        let builder =
            gtk::Builder::from_resource(&format!("{RESOURCE_PREFIX}/ui/shortcuts-dialog.ui"));
        if let Some(dialog) = builder.object::<adw::ShortcutsDialog>("shortcuts_dialog") {
            dialog.present(Some(&win));
        }
    }

    fn show_about(&self) {
        let win = self.present_window();
        let about = adw::AboutDialog::builder()
            .application_name("Stempeluhr")
            .application_icon(APP_ID)
            .developer_name(tr("The Stempeluhr authors"))
            .version(VERSION)
            .comments(tr("Start and stop Kimai time tracking."))
            .license_type(gtk::License::Gpl30)
            .copyright("© The Stempeluhr authors")
            .build();
        about.present(Some(&win));
    }

    // ---- global shortcut ------------------------------------------------------

    /// Arms or disarms the optional GlobalShortcuts portal binding.
    pub fn set_global_shortcut(&self, enabled: bool) {
        // Dropping the sender ends the portal task and closes its session.
        self.imp().shortcut_stop.borrow_mut().take();
        if !enabled {
            return;
        }
        let (stop, mut rx) = shortcuts::start();
        *self.imp().shortcut_stop.borrow_mut() = Some(stop);
        let app = self.clone();
        glib::spawn_future_local(async move {
            while rx.recv().await.is_some() {
                app.activate_action("toggle", None);
            }
        });
    }
}
