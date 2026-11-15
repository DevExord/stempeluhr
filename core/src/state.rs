// SPDX-License-Identifier: GPL-3.0-or-later
//! The tracking state machine. Pure data and transitions: no I/O, no clock
//! access (callers pass `now`), no GTK. The UI only ever renders what this
//! model says, and this model only ever contains what the server confirmed.

use jiff::{SignedDuration, Timestamp};

use crate::kimai::{Error, Timesheet};

/// What the server told us is going on.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // one value at a time; boxing would only add noise
pub enum Tracking {
    Idle,
    Running {
        /// The newest active timesheet — the one the app displays and stops.
        timesheet: Timesheet,
        /// How many *additional* timesheets are running.
        others: usize,
    },
}

impl Tracking {
    /// Reduces the answer of `GET /timesheets/active` to what we display.
    /// Several active entries are possible; show the newest.
    pub fn from_active(list: Vec<Timesheet>) -> Tracking {
        let mut running: Vec<Timesheet> = list.into_iter().filter(Timesheet::is_running).collect();
        running.sort_by_key(|t| std::cmp::Reverse((t.begin, t.id)));
        let others = running.len().saturating_sub(1);
        match running.into_iter().next() {
            Some(timesheet) => Tracking::Running { timesheet, others },
            None => Tracking::Idle,
        }
    }

    pub fn running_id(&self) -> Option<u64> {
        match self {
            Tracking::Running { timesheet, .. } => Some(timesheet.id),
            Tracking::Idle => None,
        }
    }
}

/// A server-confirmed fact and when we learned it (local wall clock).
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    pub tracking: Tracking,
    pub at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
}

#[derive(Debug, Clone)]
pub enum State {
    /// No credentials stored.
    NotConfigured,
    /// First fetch in flight; nothing is shown yet.
    Loading,
    /// Server confirmed: nothing is running.
    Idle { confirmed_at: Timestamp },
    /// Server confirmed: this one runs.
    Running {
        timesheet: Timesheet,
        others: usize,
        confirmed_at: Timestamp,
    },
    /// A start/stop request is in flight.
    Busy {
        action: Action,
        previous: Option<Observed>,
    },
    /// Server unreachable or misbehaving. `last_known` must be displayed as
    /// *unconfirmed*; its clock must not tick.
    Error {
        last_known: Option<Observed>,
        since: Timestamp,
        error: Error,
    },
    /// Token invalid or revoked.
    Unauthorized,
}

impl State {
    /// The last server-confirmed observation contained in this state, if any
    /// (also when the state itself is Busy or Error).
    pub fn observed(&self) -> Option<Observed> {
        match self {
            State::Idle { confirmed_at } => Some(Observed {
                tracking: Tracking::Idle,
                at: *confirmed_at,
            }),
            State::Running {
                timesheet,
                others,
                confirmed_at,
            } => Some(Observed {
                tracking: Tracking::Running {
                    timesheet: timesheet.clone(),
                    others: *others,
                },
                at: *confirmed_at,
            }),
            State::Busy { previous, .. } => previous.clone(),
            State::Error { last_known, .. } => last_known.clone(),
            _ => None,
        }
    }

    fn from_observed(o: Observed) -> State {
        match o.tracking {
            Tracking::Idle => State::Idle { confirmed_at: o.at },
            Tracking::Running { timesheet, others } => State::Running {
                timesheet,
                others,
                confirmed_at: o.at,
            },
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(self, State::Busy { .. })
    }

    /// Whether a displayed running timesheet is currently *confirmed*.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, State::Idle { .. } | State::Running { .. })
    }

    /// The id the "stop" action would target, if any is displayed.
    pub fn displayed_running_id(&self) -> Option<u64> {
        self.observed().and_then(|o| o.tracking.running_id())
    }
}

/// The state plus the bookkeeping that keeps stale answers out.
#[derive(Debug)]
pub struct Model {
    state: State,
    /// Highest request number whose result was applied (or fenced off).
    last_applied: u64,
    next_seq: u64,
    busy_token: Option<u64>,
    clock_offset: SignedDuration,
}

impl Model {
    pub fn new(configured: bool) -> Self {
        Self {
            state: if configured {
                State::Loading
            } else {
                State::NotConfigured
            },
            last_applied: 0,
            next_seq: 1,
            busy_token: None,
            clock_offset: SignedDuration::ZERO,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn clock_offset(&self) -> SignedDuration {
        self.clock_offset
    }

    pub fn set_clock_offset(&mut self, off: SignedDuration) {
        self.clock_offset = off;
    }

    /// Numbers a new request. Whatever is issued earlier but answers later
    /// than a newer applied answer is discarded.
    pub fn issue(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq += 1;
        s
    }

    /// Credentials changed: everything in flight becomes stale.
    pub fn reset(&mut self, configured: bool) {
        self.last_applied = self.issue();
        self.busy_token = None;
        self.clock_offset = SignedDuration::ZERO;
        self.state = if configured {
            State::Loading
        } else {
            State::NotConfigured
        };
    }

    /// Applies the result of a read-only refresh. Returns `false` if the
    /// answer was stale (or must not override a running action) and was
    /// dropped.
    pub fn apply_refresh(
        &mut self,
        seq: u64,
        result: Result<Tracking, Error>,
        now: Timestamp,
    ) -> bool {
        if seq <= self.last_applied
            || self.state.is_busy()
            || matches!(self.state, State::NotConfigured)
        {
            return false;
        }
        self.last_applied = seq;
        self.set_from_result(result, now);
        true
    }

    /// Starts a write action. Returns a token, or `None` when another action
    /// is already in flight / there is nothing to talk to.
    pub fn begin_action(&mut self, action: Action) -> Option<u64> {
        if self.state.is_busy() || matches!(self.state, State::NotConfigured | State::Unauthorized)
        {
            return None;
        }
        let token = self.issue();
        // Anything requested before this moment is now stale.
        self.last_applied = token;
        self.busy_token = Some(token);
        let previous = self.state.observed();
        self.state = State::Busy { action, previous };
        Some(token)
    }

    /// Ends a write action with the final, server-confirmed result (or the
    /// error that left us unsure). Returns `false` if the action was
    /// superseded (e.g. credentials changed meanwhile).
    pub fn finish_action(
        &mut self,
        token: u64,
        result: Result<Tracking, Error>,
        now: Timestamp,
    ) -> bool {
        if self.busy_token != Some(token) {
            return false;
        }
        self.busy_token = None;
        self.last_applied = self.issue();
        self.set_from_result(result, now);
        true
    }

    fn set_from_result(&mut self, result: Result<Tracking, Error>, now: Timestamp) {
        match result {
            Ok(tracking) => {
                self.state = State::from_observed(Observed { tracking, at: now });
            }
            Err(e) if e.is_unauthorized() => {
                self.state = State::Unauthorized;
            }
            Err(e) => {
                let last_known = self.state.observed();
                let since = match &self.state {
                    State::Error { since, .. } => *since,
                    _ => now,
                };
                self.state = State::Error {
                    last_known,
                    since,
                    error: e,
                };
            }
        }
    }
}

/// Elapsed time of a running timesheet, always derived from the server's
/// `begin` and never counted up locally. `offset` is `server − local`.
pub fn elapsed(begin: Timestamp, now: Timestamp, offset: SignedDuration) -> SignedDuration {
    let server_now = now.checked_add(offset).unwrap_or(now);
    let d = server_now.duration_since(begin);
    if d.is_negative() {
        SignedDuration::ZERO
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kimai::{ErrorKind, Ref};

    fn ts(id: u64, begin: &str) -> Timesheet {
        Timesheet {
            id,
            begin: begin.parse().unwrap(),
            end: None,
            project: Ref {
                id: 1,
                ..Default::default()
            },
            activity: Ref {
                id: 1,
                ..Default::default()
            },
            description: None,
        }
    }

    fn now(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn net_err() -> Error {
        Error::new(ErrorKind::Offline, "x")
    }

    #[test]
    fn newest_of_several_is_shown() {
        let t = Tracking::from_active(vec![
            ts(1, "2024-05-01T08:00:00Z"),
            ts(2, "2024-05-01T09:00:00Z"),
            ts(3, "2024-05-01T07:00:00Z"),
        ]);
        assert_eq!(t.running_id(), Some(2));
        assert!(matches!(t, Tracking::Running { others: 2, .. }));
        assert_eq!(Tracking::from_active(vec![]), Tracking::Idle);
    }

    #[test]
    fn stale_refresh_is_dropped() {
        let mut m = Model::new(true);
        let old = m.issue();
        let new = m.issue();
        assert!(m.apply_refresh(new, Ok(Tracking::Idle), now("2024-05-01T08:00:00Z")));
        let running = Tracking::from_active(vec![ts(1, "2024-05-01T07:00:00Z")]);
        assert!(!m.apply_refresh(old, Ok(running), now("2024-05-01T08:00:01Z")));
        assert!(matches!(m.state(), State::Idle { .. }));
    }

    #[test]
    fn refresh_cannot_break_busy_and_older_ones_die_after_action() {
        let mut m = Model::new(true);
        let early = m.issue();
        let s = m.issue();
        m.apply_refresh(s, Ok(Tracking::Idle), now("2024-05-01T08:00:00Z"));
        let tok = m.begin_action(Action::Start).unwrap();
        assert!(m.state().is_busy());
        let during = m.issue();
        assert!(!m.apply_refresh(during, Ok(Tracking::Idle), now("2024-05-01T08:00:01Z")));
        assert!(
            m.begin_action(Action::Stop).is_none(),
            "one write at a time"
        );
        let run = Tracking::from_active(vec![ts(9, "2024-05-01T08:00:02Z")]);
        assert!(m.finish_action(tok, Ok(run), now("2024-05-01T08:00:03Z")));
        assert!(!m.apply_refresh(early, Ok(Tracking::Idle), now("2024-05-01T08:00:04Z")));
        assert!(matches!(m.state(), State::Running { .. }));
    }

    #[test]
    fn error_keeps_last_known_as_unconfirmed_and_since_is_stable() {
        let mut m = Model::new(true);
        let s = m.issue();
        m.apply_refresh(
            s,
            Ok(Tracking::from_active(vec![ts(1, "2024-05-01T07:00:00Z")])),
            now("2024-05-01T08:00:00Z"),
        );
        let s = m.issue();
        m.apply_refresh(s, Err(net_err()), now("2024-05-01T08:01:00Z"));
        let s = m.issue();
        m.apply_refresh(s, Err(net_err()), now("2024-05-01T08:02:00Z"));
        match m.state() {
            State::Error {
                last_known: Some(o),
                since,
                ..
            } => {
                assert_eq!(o.at, now("2024-05-01T08:00:00Z"));
                assert_eq!(*since, now("2024-05-01T08:01:00Z"));
                assert_eq!(o.tracking.running_id(), Some(1));
            }
            other => panic!("unexpected {other:?}"),
        }
        let s = m.issue();
        m.apply_refresh(s, Ok(Tracking::Idle), now("2024-05-01T08:03:00Z"));
        assert!(matches!(m.state(), State::Idle { .. }));
    }

    #[test]
    fn unauthorized_wins() {
        let mut m = Model::new(true);
        let s = m.issue();
        m.apply_refresh(
            s,
            Err(Error::new(ErrorKind::Unauthorized, "401")),
            now("2024-05-01T08:00:00Z"),
        );
        assert!(matches!(m.state(), State::Unauthorized));
        assert!(m.begin_action(Action::Start).is_none());
    }

    #[test]
    fn reset_invalidates_in_flight_work() {
        let mut m = Model::new(true);
        let s = m.issue();
        m.apply_refresh(s, Ok(Tracking::Idle), now("2024-05-01T08:00:00Z"));
        let tok = m.begin_action(Action::Start).unwrap();
        let in_flight = m.issue();
        m.reset(true);
        assert!(!m.finish_action(tok, Ok(Tracking::Idle), now("2024-05-01T08:00:01Z")));
        assert!(!m.apply_refresh(in_flight, Ok(Tracking::Idle), now("2024-05-01T08:00:01Z")));
        assert!(matches!(m.state(), State::Loading));
    }

    #[test]
    fn elapsed_uses_server_clock() {
        let begin = now("2024-05-01T08:00:00Z");
        // Local clock is 5 minutes behind the server.
        let local = now("2024-05-01T08:55:00Z");
        let off = SignedDuration::from_secs(300);
        assert_eq!(elapsed(begin, local, off).as_secs(), 3600);
        // Never negative.
        assert_eq!(
            elapsed(begin, now("2024-05-01T07:00:00Z"), SignedDuration::ZERO),
            SignedDuration::ZERO
        );
    }
}
