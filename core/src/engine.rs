// SPDX-License-Identifier: GPL-3.0-or-later
//! Synchronisation engine: owns the [`Model`], talks to Kimai through the
//! [`Client`], and publishes [`Snapshot`]s.
//!
//! Rules enforced here (see the project brief):
//! * only server-confirmed facts reach the state;
//! * at most one write action at a time;
//! * `POST`/`PATCH` are never repeated — after an uncertain failure the
//!   server is asked what happened instead;
//! * stopping something that is already stopped is a success;
//! * starting while something runs does not create a second entry;
//! * answers older than an already applied one are dropped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use tokio::sync::watch;

use crate::kimai::{clock_offset, Client, Error, ErrorKind, Reply, StartRequest, Timesheet};
use crate::state::{Action, Model, State, Tracking};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshReason {
    Startup,
    /// Window gained focus — throttled.
    Focus,
    /// Periodic poll while the window is visible.
    Timer,
    /// Network monitor reported a change.
    Network,
    /// Resume from suspend or wall-clock jump.
    Resume,
    /// Explicit user request / retry.
    Manual,
}

/// Result of a start/stop request, for toasts and notifications.
#[derive(Debug, Clone)]
pub enum Outcome {
    Started,
    /// Something was already running (started elsewhere): nothing created.
    AlreadyRunning,
    Stopped,
    /// Already stopped elsewhere: treated as success.
    AlreadyStopped,
    /// Stop requested but nothing is displayed as running.
    NothingToStop,
    /// Toggle wanted to start but has no project/activity to start with.
    NeedsInput,
    /// Another write is in flight, or no connection configured.
    Ignored,
    Failed(Error),
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub state: State,
    /// `server − local` clock offset.
    pub clock_offset: SignedDuration,
    /// Increases with every published change.
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Minimum spacing of focus-triggered refreshes.
    pub focus_throttle: Duration,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            focus_throttle: Duration::from_secs(5),
        }
    }
}

struct Inner {
    client: Mutex<Option<Arc<Client>>>,
    model: Mutex<Model>,
    tx: watch::Sender<Snapshot>,
    rev: AtomicU64,
    last_focus: Mutex<Option<Instant>>,
    opts: EngineOptions,
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(EngineOptions::default())
    }
}

impl Engine {
    pub fn new(opts: EngineOptions) -> Self {
        let model = Model::new(false);
        let (tx, _) = watch::channel(Snapshot {
            state: model.state().clone(),
            clock_offset: SignedDuration::ZERO,
            revision: 0,
        });
        Self {
            inner: Arc::new(Inner {
                client: Mutex::new(None),
                model: Mutex::new(model),
                tx,
                rev: AtomicU64::new(0),
                last_focus: Mutex::new(None),
                opts,
            }),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.inner.tx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.inner.tx.subscribe()
    }

    /// The current client, for catalog requests (projects, activities, …).
    pub fn client(&self) -> Option<Arc<Client>> {
        lock(&self.inner.client).clone()
    }

    /// Installs (or removes) the connection. Discards everything in flight.
    pub fn configure(&self, client: Option<Client>) {
        let configured = client.is_some();
        *lock(&self.inner.client) = client.map(Arc::new);
        *lock(&self.inner.last_focus) = None;
        let mut m = lock(&self.inner.model);
        m.reset(configured);
        self.publish(&m);
    }

    /// Lets catalog code report failures; a 401 anywhere flips the state.
    pub fn note_error(&self, e: &Error) {
        if e.is_unauthorized() {
            let mut m = lock(&self.inner.model);
            let seq = m.issue();
            m.apply_refresh(seq, Err(e.clone()), Timestamp::now());
            self.publish(&m);
        }
    }

    fn publish(&self, m: &Model) {
        let rev = self.inner.rev.fetch_add(1, Ordering::Relaxed) + 1;
        self.inner.tx.send_replace(Snapshot {
            state: m.state().clone(),
            clock_offset: m.clock_offset(),
            revision: rev,
        });
    }

    fn learn_clock<T>(&self, r: &Reply<T>) {
        if let Some(d) = r.server_date {
            let mut m = lock(&self.inner.model);
            m.set_clock_offset(clock_offset(d, r.received));
        }
    }

    fn server_now(&self) -> Timestamp {
        let off = lock(&self.inner.model).clock_offset();
        let now = Timestamp::now();
        now.checked_add(off).unwrap_or(now)
    }

    // ---- reading ---------------------------------------------------------

    /// Re-reads the active timesheets and applies the answer if it is still
    /// current.
    pub async fn refresh(&self, reason: RefreshReason) {
        let Some(client) = self.client() else { return };
        {
            // Focus events come in bursts (map, activate, …): skip them when
            // any refresh started recently. Other reasons always go through.
            let mut last = lock(&self.inner.last_focus);
            if reason == RefreshReason::Focus
                && last.is_some_and(|t| t.elapsed() < self.inner.opts.focus_throttle)
            {
                tracing::debug!("focus refresh throttled");
                return;
            }
            *last = Some(Instant::now());
        }
        let seq = {
            let mut m = lock(&self.inner.model);
            if m.state().is_busy() {
                return; // the running action will end with a fresh read
            }
            m.issue()
        };
        tracing::debug!(?reason, seq, "refresh");
        let res = client.active().await;
        if let Ok(r) = &res {
            self.learn_clock(r);
        }
        let now = Timestamp::now();
        let mut m = lock(&self.inner.model);
        let applied = m.apply_refresh(seq, res.map(|r| Tracking::from_active(r.value)), now);
        if applied {
            self.publish(&m);
        } else {
            tracing::debug!(seq, "stale refresh dropped");
        }
    }

    async fn fetch_active(&self, client: &Client) -> Result<Vec<Timesheet>, Error> {
        let r = client.active().await?;
        self.learn_clock(&r);
        Ok(r.value)
    }

    // ---- writing ---------------------------------------------------------

    fn begin(&self, action: Action) -> Option<u64> {
        let mut m = lock(&self.inner.model);
        let t = m.begin_action(action)?;
        self.publish(&m);
        Some(t)
    }

    fn finish(&self, token: u64, result: Result<Tracking, Error>) {
        let mut m = lock(&self.inner.model);
        if m.finish_action(token, result, Timestamp::now()) {
            self.publish(&m);
        }
    }

    /// Starts a timesheet. Checks the server first; never creates a second
    /// entry; never retries the `POST`.
    pub async fn start(&self, req: StartRequest) -> Outcome {
        let Some(client) = self.client() else {
            return Outcome::Ignored;
        };
        let Some(token) = self.begin(Action::Start) else {
            return Outcome::Ignored;
        };

        let pre = match self.fetch_active(&client).await {
            Ok(list) => Tracking::from_active(list),
            Err(e) => {
                self.finish(token, Err(e.clone()));
                return Outcome::Failed(e);
            }
        };
        if matches!(pre, Tracking::Running { .. }) {
            tracing::info!("start requested but something already runs; not creating another");
            self.finish(token, Ok(pre));
            return Outcome::AlreadyRunning;
        }

        // Kimai interprets `begin` in the user's time zone: learn it first.
        // Nothing has been written yet, so a failure here is definite.
        if let Err(e) = client.user_timezone().await {
            self.finish(
                token,
                if e.is_unauthorized() {
                    Err(e.clone())
                } else {
                    Ok(pre)
                },
            );
            return Outcome::Failed(e);
        }

        match client.start(&req, self.server_now()).await {
            Ok(reply) => {
                self.learn_clock(&reply);
                let created = reply.value;
                let tracking = match self.fetch_active(&client).await {
                    Ok(list) => Tracking::from_active(list),
                    // The POST answer itself is server confirmation.
                    Err(_) => Tracking::Running {
                        timesheet: created,
                        others: 0,
                    },
                };
                self.finish(token, Ok(tracking));
                Outcome::Started
            }
            Err(e) if e.is_unauthorized() => {
                self.finish(token, Err(e.clone()));
                Outcome::Failed(e)
            }
            Err(e) if !e.is_ambiguous() => {
                // Definite rejection (validation, …): nothing was created.
                self.finish(token, Ok(pre));
                Outcome::Failed(e)
            }
            Err(e) => {
                // Unknown whether it arrived. Ask, never repeat.
                tracing::warn!(error = ?e.kind, "start failed ambiguously, verifying");
                match self.fetch_active(&client).await {
                    Ok(list) => {
                        let t = Tracking::from_active(list);
                        let outcome = match &t {
                            Tracking::Running { timesheet, .. }
                                if timesheet.project.id == req.project
                                    && timesheet.activity.id == req.activity =>
                            {
                                Outcome::Started
                            }
                            Tracking::Running { .. } => Outcome::AlreadyRunning,
                            Tracking::Idle => Outcome::Failed(e),
                        };
                        self.finish(token, Ok(t));
                        outcome
                    }
                    Err(e2) => {
                        self.finish(token, Err(e2));
                        Outcome::Failed(e)
                    }
                }
            }
        }
    }

    /// Stops exactly the timesheet that is currently displayed. If nothing
    /// is displayed yet (e.g. a fresh process activated by a notification
    /// button), the server is asked first and whatever runs is stopped.
    pub async fn stop(&self) -> Outcome {
        self.stop_inner(None).await
    }

    /// Stops the timesheet `id` (from a notification that remembers which
    /// entry it was about). If that entry is no longer running this is a
    /// success and nothing else is touched.
    pub async fn stop_entry(&self, id: u64) -> Outcome {
        self.stop_inner(Some(id)).await
    }

    fn displayed_target(&self) -> Result<Option<u64>, Outcome> {
        let m = lock(&self.inner.model);
        if m.state().is_busy() {
            return Err(Outcome::Ignored);
        }
        Ok(m.state().displayed_running_id())
    }

    async fn stop_inner(&self, explicit: Option<u64>) -> Outcome {
        let Some(client) = self.client() else {
            return Outcome::Ignored;
        };
        let target = match explicit {
            Some(id) => match self.displayed_target() {
                Err(o) => return o,
                Ok(_) => Some(id),
            },
            None => match self.displayed_target() {
                Err(o) => return o,
                Ok(Some(id)) => Some(id),
                Ok(None) => {
                    self.refresh(RefreshReason::Manual).await;
                    match self.displayed_target() {
                        Err(o) => return o,
                        Ok(t) => t,
                    }
                }
            },
        };
        let Some(target) = target else {
            return Outcome::NothingToStop;
        };
        let Some(token) = self.begin(Action::Stop) else {
            return Outcome::Ignored;
        };

        let pre = match self.fetch_active(&client).await {
            Ok(list) => list,
            Err(e) => {
                self.finish(token, Err(e.clone()));
                return Outcome::Failed(e);
            }
        };
        if !pre.iter().any(|t| t.id == target) {
            tracing::info!("stop requested but it was already stopped elsewhere");
            self.finish(token, Ok(Tracking::from_active(pre)));
            return Outcome::AlreadyStopped;
        }

        match client.stop(target).await {
            Ok(reply) => {
                self.learn_clock(&reply);
                let list = match self.fetch_active(&client).await {
                    Ok(l) => l,
                    Err(_) => pre.into_iter().filter(|t| t.id != target).collect(),
                };
                self.finish(token, Ok(Tracking::from_active(list)));
                Outcome::Stopped
            }
            Err(e) if e.is_unauthorized() => {
                self.finish(token, Err(e.clone()));
                Outcome::Failed(e)
            }
            Err(e) => {
                tracing::warn!(error = ?e.kind, "stop failed, verifying");
                match self.fetch_active(&client).await {
                    Ok(list) => {
                        let gone = !list.iter().any(|t| t.id == target);
                        self.finish(token, Ok(Tracking::from_active(list)));
                        if gone {
                            if e.is_ambiguous() {
                                Outcome::Stopped
                            } else {
                                Outcome::AlreadyStopped
                            }
                        } else {
                            Outcome::Failed(e)
                        }
                    }
                    Err(e2) => {
                        self.finish(token, Err(e2));
                        Outcome::Failed(e)
                    }
                }
            }
        }
    }

    /// Reads the server state, then stops if something runs or starts with
    /// `request` otherwise.
    pub async fn toggle(&self, request: Option<StartRequest>) -> Outcome {
        self.refresh(RefreshReason::Manual).await;
        let state = self.snapshot().state;
        match state {
            State::Running { .. } => self.stop().await,
            State::Idle { .. } => match request {
                Some(r) => self.start(r).await,
                None => Outcome::NeedsInput,
            },
            State::Error { error, .. } => Outcome::Failed(error),
            State::Unauthorized => {
                Outcome::Failed(Error::new(ErrorKind::Unauthorized, "unauthorized"))
            }
            _ => Outcome::Ignored,
        }
    }
}
