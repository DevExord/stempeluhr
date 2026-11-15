// SPDX-License-Identifier: GPL-3.0-or-later
//! The main window. It renders snapshots of the engine and nothing else:
//! whatever it shows is what the server last confirmed, and when that is no
//! longer certain, it says so.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib, CompositeTemplate};
use jiff::{SignedDuration, Timestamp};
use stempeluhr_core::engine::{RefreshReason, Snapshot};
use stempeluhr_core::kimai::{Activity, Error, Project, StartRequest, Timesheet};
use stempeluhr_core::state::{elapsed, Action, State, Tracking};

use crate::application::{Application, Labels};
use crate::i18n::{ntr_f, tr, tr_f};
use crate::ring::{self, RingState};
use crate::{format, messages};

const POLL_SECONDS: u32 = 45;
const CLOCK_JUMP_SECONDS: i64 = 5;

/// What the window should show for a state. Pure mapping, no widgets.
#[allow(clippy::large_enum_variant)] // short-lived, built once per render
enum View {
    Loading,
    Welcome,
    Unauthorized,
    /// Nothing known and the server can't be reached.
    Unreachable(Error),
    Tracker {
        tracking: Tracking,
        confirmed_at: Timestamp,
        /// `false` while the server is unreachable: shown dimmed and frozen.
        confirmed: bool,
        busy: Option<Action>,
        problem: Option<(Error, Timestamp)>,
    },
}

fn view_for(state: &State) -> View {
    match state {
        State::NotConfigured => View::Welcome,
        State::Loading => View::Loading,
        State::Unauthorized => View::Unauthorized,
        State::Idle { confirmed_at } => View::Tracker {
            tracking: Tracking::Idle,
            confirmed_at: *confirmed_at,
            confirmed: true,
            busy: None,
            problem: None,
        },
        State::Running {
            timesheet,
            others,
            confirmed_at,
        } => View::Tracker {
            tracking: Tracking::Running {
                timesheet: timesheet.clone(),
                others: *others,
            },
            confirmed_at: *confirmed_at,
            confirmed: true,
            busy: None,
            problem: None,
        },
        State::Busy { action, previous } => match previous {
            Some(o) => View::Tracker {
                tracking: o.tracking.clone(),
                confirmed_at: o.at,
                confirmed: true,
                busy: Some(*action),
                problem: None,
            },
            None => View::Loading,
        },
        State::Error {
            last_known,
            error,
            since,
        } => match last_known {
            Some(o) => View::Tracker {
                tracking: o.tracking.clone(),
                confirmed_at: o.at,
                confirmed: false,
                busy: None,
                problem: Some((error.clone(), *since)),
            },
            None => View::Unreachable(error.clone()),
        },
    }
}

/// What the ticking label needs.
#[derive(Clone, Copy)]
pub struct RunningClock {
    begin: Timestamp,
    confirmed_at: Timestamp,
    confirmed: bool,
    offset: SignedDuration,
}

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/de/example/Stempeluhr/ui/window.ui")]
    pub struct StempeluhrWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub main_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub error_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub refresh_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub history_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub nav_view: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub selection_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub done_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub tracker_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub project_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub activity_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub description_entry: TemplateChild<gtk::Entry>,
        #[template_child]
        pub recent_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub recent_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub start_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub ring: TemplateChild<gtk::DrawingArea>,
        #[template_child]
        pub timer_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub since_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub org_dot: TemplateChild<gtk::Box>,
        #[template_child]
        pub project_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub activity_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub description_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub others_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub stop_button: TemplateChild<gtk::Button>,

        pub projects: RefCell<Vec<Project>>,
        pub activities: RefCell<Vec<Activity>>,
        pub recents: RefCell<Vec<crate::application::Recent>>,
        pub pending_activity: Cell<Option<u64>>,
        pub updating: Cell<bool>,
        pub catalog_loading: Cell<bool>,
        pub catalog_generation: Cell<u64>,

        pub visible: Cell<bool>,
        pub tick: RefCell<Option<glib::SourceId>>,
        pub poll: RefCell<Option<glib::SourceId>>,
        pub net_debounce: RefCell<Option<glib::SourceId>>,
        pub last_clock: Cell<(i64, i64)>,
        pub first_show_done: Cell<bool>,

        pub running: Cell<Option<RunningClock>>,
        pub snapshot: RefCell<Option<Snapshot>>,
        pub resolving: Cell<Option<u64>>,
        pub ring_state: OnceCell<Rc<RingState>>,
        /// A stop is in flight and the window shows the running view; an
        /// Idle snapshot waits until the outcome says whether to celebrate.
        pub holding: Cell<bool>,
        pub celebrating: Cell<bool>,
        pub pending: RefCell<Option<Snapshot>>,
        pub recent_available: Cell<bool>,
        pub stamp_animation: RefCell<Option<adw::Animation>>,
        pub ring_tick: RefCell<Option<gtk::TickCallbackId>>,
        pub reveal_animation: RefCell<Option<adw::TimedAnimation>>,
        pub org_provider: gtk::CssProvider,
        pub org_color: RefCell<Option<String>>,
        pub was_running: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StempeluhrWindow {
        const NAME: &'static str = "StempeluhrWindow";
        type Type = super::StempeluhrWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for StempeluhrWindow {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for StempeluhrWindow {}
    impl WindowImpl for StempeluhrWindow {
        fn close_request(&self) -> glib::Propagation {
            self.obj().stop_timers();
            self.parent_close_request()
        }
    }
    impl ApplicationWindowImpl for StempeluhrWindow {}
    impl AdwApplicationWindowImpl for StempeluhrWindow {}
}

glib::wrapper! {
    pub struct StempeluhrWindow(ObjectSubclass<imp::StempeluhrWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl StempeluhrWindow {
    pub fn new(app: &Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn app(&self) -> Option<Application> {
        self.application()
            .and_then(|a| a.downcast::<Application>().ok())
    }

    /// Opens the history page (a no-op if it is already on top).
    pub fn show_history(&self) {
        let nav = &*self.imp().nav_view;
        if nav.visible_page_tag().as_deref() == Some("history") {
            return;
        }
        if let Some(app) = self.app() {
            nav.push(&crate::history::HistoryPage::new(&app));
        }
    }

    pub fn toast(&self, msg: &str) {
        let toast = adw::Toast::new(msg);
        toast.set_timeout(4);
        self.imp().toast_overlay.add_toast(toast);
    }

    // ---- setup -------------------------------------------------------------

    fn setup(&self) {
        let imp = self.imp();

        // Search needs an expression for plain string models.
        let expr = gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<&gtk::Expression>,
            "string",
        );
        for dd in [&*imp.project_dropdown, &*imp.activity_dropdown] {
            dd.set_expression(Some(&expr));
            // Long names must ellipsize instead of widening the window.
            dd.set_factory(Some(&ellipsizing_factory()));
            dd.set_sensitive(false);
        }

        imp.project_dropdown.connect_selected_notify(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_| win.on_project_selected()
        ));
        imp.activity_dropdown.connect_selected_notify(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_| win.update_start_sensitivity()
        ));
        imp.description_entry.connect_activate(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_| {
                if let Some(app) = win.app() {
                    app.activate_action("start", None);
                }
            }
        ));
        // The description field must be leavable: Esc, or a click anywhere
        // else, takes the focus away again.
        let esc = gtk::EventControllerKey::new();
        esc.connect_key_pressed(glib::clone!(
            #[weak(rename_to = win)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    GtkWindowExt::set_focus(&win, None::<&gtk::Widget>);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        imp.description_entry.add_controller(esc);

        let outside = gtk::GestureClick::new();
        outside.set_propagation_phase(gtk::PropagationPhase::Capture);
        outside.connect_pressed(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_, _, x, y| {
                let entry = win.imp().description_entry.get();
                let editing = GtkWindowExt::focus(&win).is_some_and(|f| {
                    f == *entry.upcast_ref::<gtk::Widget>() || f.is_ancestor(&entry)
                });
                if !editing {
                    return;
                }
                let hit = win.pick(x, y, gtk::PickFlags::DEFAULT);
                let inside = hit.is_some_and(|w| {
                    w == *entry.upcast_ref::<gtk::Widget>() || w.is_ancestor(&entry)
                });
                if !inside {
                    GtkWindowExt::set_focus(&win, None::<&gtk::Widget>);
                }
            }
        ));
        self.add_controller(outside);

        imp.recent_list.connect_row_activated(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_, row| {
                let idx = usize::try_from(row.index()).ok();
                let pick = idx.and_then(|i| win.imp().recents.borrow().get(i).cloned());
                if let Some(r) = pick {
                    win.imp().recent_button.popdown();
                    win.select_combination(r.project_id, r.activity_id);
                }
            }
        ));

        // Rows (hover, focus ring) must follow the card's rounded corners.
        // `overflow` is a widget property in GTK, not a CSS property.
        imp.selection_card.set_overflow(gtk::Overflow::Hidden);

        // The ring and the project colour.
        let _ = imp.ring_state.set(ring::install(&imp.ring));
        #[allow(deprecated)]
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &imp.org_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }
        let style = adw::StyleManager::default();
        style.connect_dark_notify(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_| win.apply_org_color()
        ));
        style.connect_accent_color_rgba_notify(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_| win.imp().ring.queue_draw()
        ));

        // Visibility: mapped, not minimised, not occluded.
        self.connect_map(|w| w.update_visibility());
        self.connect_unmap(|w| w.update_visibility());
        self.connect_realize(|w| {
            if let Some(toplevel) = w.surface().and_then(|s| s.downcast::<gdk::Toplevel>().ok()) {
                toplevel.connect_state_notify(glib::clone!(
                    #[weak]
                    w,
                    move |_| w.update_visibility()
                ));
            }
        });
        self.connect_is_active_notify(|w| {
            if w.is_active() {
                w.refresh(RefreshReason::Focus);
            }
        });

        // Network changes (debounced, only while visible).
        let monitor = gio::NetworkMonitor::default();
        monitor.connect_network_changed(glib::clone!(
            #[weak(rename_to = win)]
            self,
            move |_, _| win.on_network_changed()
        ));

        #[cfg(debug_assertions)]
        crate::debug_shot::install(self);

        if let Some(app) = self.app() {
            watch_for_sleep(&app);
            self.render(&app.engine().snapshot());
        }
    }

    // ---- visibility, timers ------------------------------------------------------

    fn update_visibility(&self) {
        let imp = self.imp();
        let hidden_state = self
            .surface()
            .and_then(|s| s.downcast::<gdk::Toplevel>().ok())
            .map(|t| {
                t.state()
                    .intersects(gdk::ToplevelState::MINIMIZED | gdk::ToplevelState::SUSPENDED)
            })
            .unwrap_or(false);
        let visible = self.is_mapped() && !hidden_state;
        if visible == imp.visible.get() {
            return;
        }
        imp.visible.set(visible);
        if visible {
            self.start_timers();
            let reason = if imp.first_show_done.replace(true) {
                RefreshReason::Focus
            } else {
                RefreshReason::Startup
            };
            self.refresh(reason);
            self.update_timer();
            if let Some(app) = self.app() {
                if let Some(msg) = app.take_keyring_error() {
                    self.toast(&tr_f(
                        "Couldn’t read the access token from the keyring: {error}",
                        &[("error", &msg)],
                    ));
                }
            }
        } else {
            self.stop_timers();
        }
    }

    fn start_timers(&self) {
        let imp = self.imp();
        self.stop_timers();
        imp.last_clock
            .set((glib::real_time(), glib::monotonic_time()));
        let win = self.downgrade();
        imp.tick
            .replace(Some(glib::timeout_add_seconds_local(1, move || {
                let Some(win) = win.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                win.on_tick();
                glib::ControlFlow::Continue
            })));
        let win = self.downgrade();
        imp.poll.replace(Some(glib::timeout_add_seconds_local(
            POLL_SECONDS,
            move || {
                let Some(win) = win.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                win.refresh(RefreshReason::Timer);
                glib::ControlFlow::Continue
            },
        )));
        self.update_ring_animation();
    }

    fn stop_timers(&self) {
        let imp = self.imp();
        if let Some(id) = imp.tick.take() {
            id.remove();
        }
        if let Some(id) = imp.poll.take() {
            id.remove();
        }
        if let Some(id) = imp.ring_tick.take() {
            id.remove();
        }
    }

    fn on_tick(&self) {
        // Wall clock vs. monotonic clock: the monotonic one stops during
        // suspend, so a gap means we just woke up (or the clock was set).
        let imp = self.imp();
        let (real, mono) = (glib::real_time(), glib::monotonic_time());
        let (last_real, last_mono) = imp.last_clock.replace((real, mono));
        let drift = ((real - last_real) - (mono - last_mono)) / 1_000_000;
        if drift.abs() > CLOCK_JUMP_SECONDS {
            tracing::info!(drift, "wall clock jump, refreshing");
            self.refresh(RefreshReason::Resume);
        }
        self.update_timer();
    }

    fn on_network_changed(&self) {
        let imp = self.imp();
        if !imp.visible.get() || imp.net_debounce.borrow().is_some() {
            return;
        }
        let win = self.downgrade();
        imp.net_debounce.replace(Some(glib::timeout_add_local_once(
            Duration::from_millis(1500),
            move || {
                if let Some(win) = win.upgrade() {
                    win.imp().net_debounce.replace(None);
                    win.refresh(RefreshReason::Network);
                }
            },
        )));
    }

    pub fn refresh(&self, reason: RefreshReason) {
        if let Some(app) = self.app() {
            app.refresh(reason);
        }
    }

    // ---- rendering ---------------------------------------------------------------

    pub fn render(&self, snap: &Snapshot) {
        let imp = self.imp();
        // Celebrating a stop, or waiting to learn whether to: hold back the
        // switch to the start screen and apply it afterwards.
        if imp.celebrating.get() || (imp.holding.get() && matches!(snap.state, State::Idle { .. }))
        {
            imp.pending.replace(Some(snap.clone()));
            return;
        }
        if imp.holding.get() && !matches!(snap.state, State::Busy { .. } | State::Running { .. }) {
            imp.holding.set(false);
        }
        imp.snapshot.replace(Some(snap.clone()));
        imp.running.set(None);
        let banner = &*imp.banner;
        banner.set_revealed(false);

        match view_for(&snap.state) {
            View::Loading => imp.main_stack.set_visible_child_name("loading"),
            View::Welcome => imp.main_stack.set_visible_child_name("welcome"),
            View::Unauthorized => imp.main_stack.set_visible_child_name("unauthorized"),
            View::Unreachable(e) => {
                imp.error_label.set_label(&messages::describe(&e));
                imp.main_stack.set_visible_child_name("error");
            }
            View::Tracker {
                tracking,
                confirmed_at,
                confirmed,
                busy,
                problem,
            } => {
                imp.main_stack.set_visible_child_name("tracker");
                if let Some((e, _since)) = problem {
                    banner.set_title(&tr_f(
                        "{problem} Showing the state from {time}.",
                        &[
                            ("problem", &messages::describe(&e)),
                            ("time", &format::clock(confirmed_at)),
                        ],
                    ));
                    banner.set_revealed(true);
                }
                match tracking {
                    Tracking::Idle => self.render_idle(busy),
                    Tracking::Running { timesheet, others } => self.render_running(
                        &timesheet,
                        others,
                        confirmed,
                        confirmed_at,
                        snap.clock_offset,
                        busy,
                    ),
                }
            }
        }
        self.update_header(&snap.state);
        self.update_ring_animation();
    }

    /// Header buttons only where they make sense: refresh once connected,
    /// recent combinations only on the start screen.
    fn update_header(&self, state: &State) {
        let imp = self.imp();
        let connected_view = matches!(view_for(state), View::Tracker { .. } | View::Unreachable(_));
        imp.refresh_button.set_visible(connected_view);
        imp.history_button.set_visible(connected_view);
        let idle = imp.main_stack.visible_child_name().as_deref() == Some("tracker")
            && imp.tracker_stack.visible_child_name().as_deref() == Some("idle");
        imp.recent_button
            .set_visible(idle && imp.recent_available.get());
    }

    fn render_idle(&self, busy: Option<Action>) {
        let imp = self.imp();
        imp.tracker_stack.set_visible_child_name("idle");
        imp.running.set(None);
        imp.was_running.set(false);
        if busy == Some(Action::Start) {
            let spinner = adw::Spinner::new();
            imp.start_button.set_child(Some(&spinner));
        } else {
            imp.start_button
                .set_icon_name("media-playback-start-symbolic");
        }
        self.update_start_sensitivity();
        // Tint the halo with the selected project's colour.
        let color = imp
            .projects
            .borrow()
            .get(imp.project_dropdown.selected() as usize)
            .and_then(|p| p.color.clone());
        self.set_org_color(color);
        self.ensure_catalog();
    }

    fn render_running(
        &self,
        ts: &Timesheet,
        others: usize,
        confirmed: bool,
        confirmed_at: Timestamp,
        offset: SignedDuration,
        busy: Option<Action>,
    ) {
        let imp = self.imp();
        imp.tracker_stack.set_visible_child_name("running");
        imp.running.set(Some(RunningClock {
            begin: ts.begin,
            confirmed_at,
            confirmed,
            offset,
        }));

        match self.app().and_then(|a| a.labels_cached(ts)) {
            Some(Labels {
                project,
                customer,
                activity,
                color,
            }) => {
                imp.project_label.set_label(&project);
                imp.activity_label.set_label(&match customer.as_deref() {
                    Some(c) if !c.is_empty() => format!("{activity} · {c}"),
                    _ => activity,
                });
                imp.activity_label.set_visible(true);
                self.set_org_color(color);
            }
            None => {
                imp.project_label.set_label("…");
                imp.activity_label.set_visible(false);
                self.resolve_names(ts);
            }
        }
        let description = ts.description.as_deref().map(str::trim).unwrap_or_default();
        imp.description_label.set_label(description);
        imp.description_label.set_visible(!description.is_empty());
        imp.since_label.set_label(&if confirmed {
            format::since(ts.begin)
        } else {
            tr("Not confirmed")
        });

        imp.others_label.set_visible(others > 0);
        if others > 0 {
            imp.others_label.set_label(&ntr_f(
                "{n} more entry is running in Kimai. Stopping affects only the one shown.",
                "{n} more entries are running in Kimai. Stopping affects only the one shown.",
                others as u32,
                &[],
            ));
        }

        for w in [
            imp.timer_label.upcast_ref::<gtk::Widget>(),
            imp.org_dot.upcast_ref(),
        ] {
            if confirmed {
                w.remove_css_class("unconfirmed");
            } else {
                w.add_css_class("unconfirmed");
            }
        }
        if let Some(state) = imp.ring_state.get() {
            state.dim.set(!confirmed);
        }
        imp.stop_button.set_label(&if busy == Some(Action::Stop) {
            tr("Stopping…")
        } else {
            tr("Stop")
        });
        imp.stop_button.set_sensitive(busy.is_none());

        // Tracking just began (or the window just learned about it): draw
        // the ring in.
        if !imp.was_running.replace(true) {
            self.play_reveal();
        }
        self.update_timer();
    }

    fn resolve_names(&self, ts: &Timesheet) {
        let imp = self.imp();
        if imp.resolving.get() == Some(ts.id) {
            return;
        }
        imp.resolving.set(Some(ts.id));
        let Some(app) = self.app() else { return };
        let win = self.clone();
        let ts = ts.clone();
        glib::spawn_future_local(async move {
            app.resolve_labels(&ts).await;
            win.imp().resolving.set(None);
            let snap = win.imp().snapshot.borrow().clone();
            if let Some(snap) = snap {
                // Cached now (or unresolvable: show the fallback once).
                if app.labels_cached(&ts).is_some() {
                    win.render(&snap);
                } else {
                    let l = app.resolve_labels(&ts).await;
                    win.imp().project_label.set_label(&l.project);
                    win.imp().activity_label.set_label(&l.activity);
                    win.imp().activity_label.set_visible(true);
                }
            }
        });
    }

    fn update_timer(&self) {
        let imp = self.imp();
        let Some(r) = imp.running.get() else { return };
        if imp.celebrating.get() {
            return;
        }
        imp.timer_label
            .set_label(&format::hms(self.elapsed_now(&r)));
        self.update_ring_progress();
    }

    /// Confirmed: derived from the server's begin and clock. Not confirmed:
    /// frozen at the last moment the server vouched for.
    fn elapsed_now(&self, r: &RunningClock) -> SignedDuration {
        let at = if r.confirmed {
            Timestamp::now()
        } else {
            r.confirmed_at
        };
        elapsed(r.begin, at, r.offset)
    }

    // ---- stop feedback: the "stamp" ---------------------------------------------------

    /// A stop was requested. While it is in flight the window keeps showing
    /// the running view, so the result can be celebrated.
    pub fn begin_stop(&self) {
        let imp = self.imp();
        if imp.celebrating.get() || !imp.running.get().is_some_and(|r| r.confirmed) {
            return;
        }
        imp.holding.set(true);
        // Safety net: never hold the screen for long.
        let win = self.downgrade();
        glib::timeout_add_seconds_local_once(15, move || {
            if let Some(win) = win.upgrade() {
                win.stop_finished(false);
            }
        });
    }

    /// The stop request has an outcome. `recorded`: the server stopped the
    /// entry because of us (not "was already stopped elsewhere").
    pub fn stop_finished(&self, recorded: bool) {
        let imp = self.imp();
        if !imp.holding.replace(false) {
            return;
        }
        match (recorded, imp.running.get()) {
            (true, Some(r)) => self.celebrate(self.elapsed_now(&r)),
            _ => self.apply_pending(),
        }
    }

    /// Shows what the engine knows *now* (not a stale snapshot).
    fn apply_pending(&self) {
        self.imp().pending.take();
        if let Some(app) = self.app() {
            self.render(&app.engine().snapshot());
        }
    }

    /// The ring closes, one ripple leaves it, a check mark appears and the
    /// final time stays for a moment — like stamping a time card.
    fn celebrate(&self, total: SignedDuration) {
        let imp = self.imp();
        let Some(state) = imp.ring_state.get().cloned() else {
            return self.apply_pending();
        };
        imp.celebrating.set(true);
        if let Some(id) = imp.ring_tick.take() {
            id.remove();
        }
        imp.timer_label.set_label(&format::hms(total));
        imp.since_label.set_label(&tr("Recorded"));
        imp.done_icon.set_visible(true);
        imp.stop_button.set_sensitive(false);
        imp.stop_button.set_opacity(0.0);
        state.dim.set(false);
        self.announce(
            &tr_f("Recorded {time}", &[("time", &format::hms(total))]),
            gtk::AccessibleAnnouncementPriority::Medium,
        );

        let hold_ms = if Self::animations_enabled() {
            2400
        } else {
            1800
        };
        if Self::animations_enabled() {
            let from = state.progress.get();
            let ring = imp.ring.clone();
            let st = state.clone();
            let target = adw::CallbackAnimationTarget::new(move |v| {
                st.progress.set(from + (1.0 - from) * v.min(0.5) / 0.5);
                st.ripple.set(((v - 0.45) / 0.55).clamp(0.0, 1.0));
                ring.queue_draw();
            });
            let anim = adw::TimedAnimation::builder()
                .widget(&*imp.ring)
                .value_from(0.0)
                .value_to(1.0)
                .duration(1100)
                .easing(adw::Easing::EaseOutCubic)
                .target(&target)
                .build();
            let st = state.clone();
            let ring = imp.ring.clone();
            anim.connect_done(move |_| {
                st.ripple.set(0.0);
                ring.queue_draw();
            });
            anim.play();
            imp.stamp_animation.replace(Some(anim.upcast()));
        } else {
            state.progress.set(1.0);
            imp.ring.queue_draw();
        }

        let win = self.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(hold_ms), move || {
            if let Some(win) = win.upgrade() {
                win.end_celebration();
            }
        });
    }

    /// After the hold: the ring contracts and fades, then the start screen
    /// pops in from the middle.
    fn end_celebration(&self) {
        let imp = self.imp();
        let Some(state) = imp.ring_state.get().cloned() else {
            return self.finish_celebration();
        };
        if !Self::animations_enabled() {
            return self.finish_celebration();
        }
        let ring = imp.ring.clone();
        let stack = imp.tracker_stack.clone();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            state.reveal.set(1.0 - v);
            ring.queue_draw();
            stack.set_opacity(1.0 - v);
        });
        let anim = adw::TimedAnimation::builder()
            .widget(&*imp.ring)
            .value_from(0.0)
            .value_to(1.0)
            .duration(380)
            .easing(adw::Easing::EaseInCubic)
            .target(&target)
            .build();
        let win = self.downgrade();
        anim.connect_done(move |_| {
            if let Some(win) = win.upgrade() {
                win.finish_celebration();
            }
        });
        anim.play();
        imp.stamp_animation.replace(Some(anim.upcast()));
    }

    fn finish_celebration(&self) {
        let imp = self.imp();
        imp.celebrating.set(false);
        imp.done_icon.set_visible(false);
        imp.stop_button.set_opacity(1.0);
        imp.stop_button.set_label(&tr("Stop"));
        imp.stamp_animation.replace(None);
        if let Some(state) = imp.ring_state.get() {
            state.ripple.set(0.0);
            state.reveal.set(1.0);
        }
        // Switch instantly (the running view is already faded out) and let
        // the start screen settle in with a CSS transition.
        let pop: [&gtk::Widget; 2] = [
            imp.start_button.upcast_ref(),
            imp.selection_card.upcast_ref(),
        ];
        for w in pop {
            w.add_css_class("pop-in");
        }
        imp.tracker_stack
            .set_transition_type(gtk::StackTransitionType::None);
        self.apply_pending();
        imp.tracker_stack.set_opacity(1.0);
        imp.tracker_stack
            .set_transition_type(gtk::StackTransitionType::Crossfade);
        let win = self.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(60), move || {
            if let Some(win) = win.upgrade() {
                win.imp().start_button.remove_css_class("pop-in");
                win.imp().selection_card.remove_css_class("pop-in");
            }
        });
    }

    // ---- ring & colour -------------------------------------------------------------------

    fn animations_enabled() -> bool {
        gtk::Settings::default().is_some_and(|s| s.is_gtk_enable_animations())
    }

    fn update_ring_progress(&self) {
        let imp = self.imp();
        let (Some(r), Some(state)) = (imp.running.get(), imp.ring_state.get()) else {
            return;
        };
        if imp.celebrating.get() {
            return;
        }
        let ms = self.elapsed_now(&r).as_millis();
        state.progress.set((ms % 60_000) as f64 / 60_000.0);
        imp.ring.queue_draw();
    }

    /// Smooth sweep: redraw every frame while tracking is confirmed, the
    /// window is visible and animations are on. Otherwise the arc advances
    /// once per second with the timer label.
    fn update_ring_animation(&self) {
        let imp = self.imp();
        let wants = imp.visible.get()
            && imp.running.get().is_some_and(|r| r.confirmed)
            && Self::animations_enabled();
        let has = imp.ring_tick.borrow().is_some();
        if wants && !has {
            let id = imp.ring.add_tick_callback(glib::clone!(
                #[weak(rename_to = win)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move |_, _| {
                    win.update_ring_progress();
                    glib::ControlFlow::Continue
                }
            ));
            imp.ring_tick.replace(Some(id));
        } else if !wants && has {
            if let Some(id) = imp.ring_tick.take() {
                id.remove();
            }
        }
    }

    fn play_reveal(&self) {
        let imp = self.imp();
        let Some(state) = imp.ring_state.get().cloned() else {
            return;
        };
        if !Self::animations_enabled() {
            state.reveal.set(1.0);
            imp.ring.queue_draw();
            return;
        }
        state.reveal.set(0.0);
        let ring = imp.ring.clone();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            state.reveal.set(v);
            ring.queue_draw();
        });
        let anim = adw::TimedAnimation::builder()
            .widget(&*imp.ring)
            .value_from(0.0)
            .value_to(1.0)
            .duration(600)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        anim.play();
        imp.reveal_animation.replace(Some(anim));
    }

    /// Sets (or clears) the project colour used by the start halo, the ring
    /// and the dot. Only well-formed `#rgb`/`#rrggbb` values are accepted and
    /// the colour is adjusted for contrast with the current theme.
    fn set_org_color(&self, hex: Option<String>) {
        let hex = hex.filter(|h| ring::parse_hex(h).is_some());
        if *self.imp().org_color.borrow() == hex {
            return;
        }
        *self.imp().org_color.borrow_mut() = hex;
        self.apply_org_color();
    }

    fn apply_org_color(&self) {
        let imp = self.imp();
        let tuned = imp
            .org_color
            .borrow()
            .as_deref()
            .and_then(ring::parse_hex)
            .map(|c| ring::tune_for_theme(c, adw::StyleManager::default().is_dark()));
        let css = match tuned {
            Some(c) => format!(".stempel-root {{ --org-color: {}; }}", ring::to_css(c)),
            None => String::new(),
        };
        imp.org_provider.load_from_string(&css);
        imp.org_dot.set_visible(tuned.is_some());
        if let Some(state) = imp.ring_state.get() {
            state.color.set(tuned);
        }
        imp.ring.queue_draw();
    }

    // ---- selection & catalog ---------------------------------------------------------

    /// The project/activity/description currently chosen, if complete.
    pub fn start_request(&self) -> Option<StartRequest> {
        let imp = self.imp();
        let project = imp
            .projects
            .borrow()
            .get(imp.project_dropdown.selected() as usize)?
            .id;
        let activity = imp
            .activities
            .borrow()
            .get(imp.activity_dropdown.selected() as usize)?
            .id;
        let description = imp.description_entry.text().trim().to_string();
        Some(StartRequest {
            project,
            activity,
            description: (!description.is_empty()).then_some(description),
        })
    }

    fn update_start_sensitivity(&self) {
        let imp = self.imp();
        let busy = matches!(
            imp.snapshot.borrow().as_ref().map(|s| &s.state),
            Some(State::Busy { .. })
        );
        imp.start_button
            .set_sensitive(!busy && self.start_request().is_some());
    }

    fn ensure_catalog(&self) {
        let imp = self.imp();
        if imp.projects.borrow().is_empty() && !imp.catalog_loading.get() {
            self.load_catalog(false);
        }
    }

    pub fn reload_catalog(&self) {
        if !self.imp().catalog_loading.get() {
            self.load_catalog(true);
        }
    }

    fn load_catalog(&self, force: bool) {
        let imp = self.imp();
        let Some(app) = self.app() else { return };
        if app.engine().client().is_none() {
            return;
        }
        imp.catalog_loading.set(true);
        let win = self.clone();
        glib::spawn_future_local(async move {
            let projects = app.load_projects(force).await;
            win.imp().catalog_loading.set(false);
            match projects {
                Ok(list) => {
                    let remembered = app.settings().int64("last-project");
                    win.set_projects(list, (remembered > 0).then_some(remembered as u64));
                    if let Ok(recent) = app.load_recent().await {
                        win.set_recent(recent);
                    }
                }
                Err(e) => {
                    // Auth and connectivity problems are already explained by the
                    // page or banner; only report the rest.
                    if !e.is_unauthorized() && !e.is_connectivity() {
                        win.toast(&tr_f(
                            "Couldn’t load projects. {reason}",
                            &[("reason", &messages::describe(&e))],
                        ));
                    }
                }
            }
        });
    }

    fn set_projects(&self, projects: Vec<Project>, prefer: Option<u64>) {
        let imp = self.imp();
        let current = imp
            .projects
            .borrow()
            .get(imp.project_dropdown.selected() as usize)
            .map(|p| p.id);
        let names: Vec<String> = projects
            .iter()
            .map(|p| match &p.customer {
                Some(c) if !c.is_empty() => format!("{} ({})", p.name, c),
                _ => p.name.clone(),
            })
            .collect();
        let index = prefer
            .or(current)
            .and_then(|id| projects.iter().position(|p| p.id == id))
            .unwrap_or(0);
        imp.updating.set(true);
        *imp.projects.borrow_mut() = projects;
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        imp.project_dropdown
            .set_model(Some(&gtk::StringList::new(&refs)));
        imp.project_dropdown.set_selected(index as u32);
        imp.project_dropdown.set_sensitive(!names.is_empty());
        imp.updating.set(false);
        self.on_project_selected();
    }

    fn on_project_selected(&self) {
        let imp = self.imp();
        if imp.updating.get() {
            return;
        }
        let Some((project, color)) = imp
            .projects
            .borrow()
            .get(imp.project_dropdown.selected() as usize)
            .map(|p| (p.id, p.color.clone()))
        else {
            return;
        };
        if imp.tracker_stack.visible_child_name().as_deref() == Some("idle") {
            self.set_org_color(color);
        }
        imp.activity_dropdown.set_sensitive(false);
        imp.activities.borrow_mut().clear();
        self.update_start_sensitivity();
        let generation = imp.catalog_generation.get() + 1;
        imp.catalog_generation.set(generation);
        let Some(app) = self.app() else { return };
        let win = self.clone();
        glib::spawn_future_local(async move {
            let result = app.load_activities(project).await;
            let imp = win.imp();
            if imp.catalog_generation.get() != generation {
                return; // user picked another project meanwhile
            }
            match result {
                Ok(list) => {
                    let wanted = imp.pending_activity.take().or_else(|| {
                        let last = app.settings().int64("last-activity");
                        (last > 0).then_some(last as u64)
                    });
                    let index = wanted
                        .and_then(|id| list.iter().position(|a| a.id == id))
                        .unwrap_or(0);
                    let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
                    imp.updating.set(true);
                    imp.activity_dropdown
                        .set_model(Some(&gtk::StringList::new(&names)));
                    imp.activity_dropdown.set_selected(index as u32);
                    imp.activity_dropdown.set_sensitive(!names.is_empty());
                    *imp.activities.borrow_mut() = list;
                    imp.updating.set(false);
                    win.update_start_sensitivity();
                }
                Err(e) => {
                    // Auth and connectivity problems are already explained by the
                    // page or banner; only report the rest.
                    if !e.is_unauthorized() && !e.is_connectivity() {
                        win.toast(&tr_f(
                            "Couldn’t load activities. {reason}",
                            &[("reason", &messages::describe(&e))],
                        ));
                    }
                }
            }
        });
    }

    fn set_recent(&self, recent: Vec<crate::application::Recent>) {
        let imp = self.imp();
        imp.recent_list.remove_all();
        imp.recent_available.set(!recent.is_empty());
        let on_start_screen = imp.main_stack.visible_child_name().as_deref() == Some("tracker")
            && imp.tracker_stack.visible_child_name().as_deref() == Some("idle");
        imp.recent_button
            .set_visible(on_start_screen && !recent.is_empty());
        for r in &recent {
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_top(4)
                .margin_bottom(4)
                .build();
            let title = gtk::Label::builder()
                .label(&r.labels.activity)
                .xalign(0.0)
                .max_width_chars(40)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let sub = gtk::Label::builder()
                .label(match &r.labels.customer {
                    Some(c) if !c.is_empty() => format!("{} ({})", r.labels.project, c),
                    _ => r.labels.project.clone(),
                })
                .xalign(0.0)
                .max_width_chars(48)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["caption", "dim-label"])
                .build();
            content.append(&title);
            content.append(&sub);
            imp.recent_list.append(&content);
        }
        imp.recents.replace(recent);
    }

    fn select_combination(&self, project: u64, activity: u64) {
        let imp = self.imp();
        let Some(p_idx) = imp.projects.borrow().iter().position(|p| p.id == project) else {
            return;
        };
        imp.pending_activity.set(Some(activity));
        if imp.project_dropdown.selected() as usize == p_idx {
            // Same project: only the activity changes.
            if let Some(a_idx) = imp
                .activities
                .borrow()
                .iter()
                .position(|a| a.id == activity)
            {
                imp.activity_dropdown.set_selected(a_idx as u32);
                imp.pending_activity.set(None);
            }
        } else {
            imp.project_dropdown.set_selected(p_idx as u32);
        }
    }
}

/// logind tells us when the machine wakes up. Best effort: inside a sandbox
/// without access to the system bus this simply never fires; the wall-clock
/// jump detection and the network monitor cover that case.
fn watch_for_sleep(app: &Application) {
    let app = app.downgrade();
    gio::bus_get(gio::BusType::System, gio::Cancellable::NONE, move |res| {
        let Ok(conn) = res else { return };
        let subscription = conn.subscribe_to_signal(
            Some("org.freedesktop.login1"),
            Some("org.freedesktop.login1.Manager"),
            Some("PrepareForSleep"),
            Some("/org/freedesktop/login1"),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                let going_to_sleep = signal
                    .parameters
                    .child_value(0)
                    .get::<bool>()
                    .unwrap_or(true);
                if going_to_sleep {
                    return;
                }
                if let Some(app) = app.upgrade() {
                    if app.main_window().is_some_and(|w| w.imp().visible.get()) {
                        app.refresh(RefreshReason::Resume);
                    }
                }
            },
        );
        // Dropping the subscription would unsubscribe; keep it for the
        // lifetime of the process.
        std::mem::forget(subscription);
    });
}

/// A label factory whose labels ellipsize, so long project names can't make
/// the window wider.
fn ellipsizing_factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = item.child().and_downcast::<gtk::Label>();
        let text = item
            .item()
            .and_downcast::<gtk::StringObject>()
            .map(|s| s.string());
        if let (Some(label), Some(text)) = (label, text) {
            label.set_label(&text);
        }
    });
    factory
}
