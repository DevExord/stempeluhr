// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end scenarios of engine + client against a mock Kimai server.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use jiff::Timestamp;
use serde_json::{json, Value};
use stempeluhr_core::engine::{Engine, EngineOptions, Outcome, RefreshReason};
use stempeluhr_core::kimai::{Client, ClientOptions, ErrorKind, StartRequest, Token};
use stempeluhr_core::state::{elapsed, State};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const SECRET: &str = "s3cr3t-token-value";

fn running(id: u64, begin: &str, project: u64, activity: u64) -> Value {
    json!({"id": id, "begin": begin, "end": null, "project": project, "activity": activity, "description": null})
}

/// Serves `responses` in order, repeating the last one.
struct Seq {
    calls: AtomicUsize,
    responses: Vec<ResponseTemplate>,
}

impl Seq {
    fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            responses,
        }
    }
}

impl Respond for Seq {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let i = self.calls.fetch_add(1, Ordering::SeqCst);
        self.responses[i.min(self.responses.len() - 1)].clone()
    }
}

fn json_ok(v: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(v)
}

fn fast_opts() -> ClientOptions {
    ClientOptions {
        connect_timeout: Duration::from_millis(500),
        request_timeout: Duration::from_millis(400),
        retry_delays: vec![],
    }
}

fn engine_for(server: &MockServer, opts: ClientOptions) -> Engine {
    let engine = Engine::new(EngineOptions {
        focus_throttle: Duration::from_millis(300),
    });
    engine.configure(Some(
        Client::with_options(&server.uri(), Token::new(SECRET), opts).unwrap(),
    ));
    engine
}

async fn mount(server: &MockServer, m: &str, p: &str, r: impl Respond + 'static) {
    Mock::given(method(m))
        .and(path(p))
        .respond_with(r)
        .mount(server)
        .await;
}

async fn count(server: &MockServer, m: &str, p: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == m && r.url.path() == p)
        .count()
}

async fn mount_me(server: &MockServer, tz: &str) {
    mount(
        server,
        "GET",
        "/api/users/me",
        json_ok(json!({"id": 1, "username": "tester", "timezone": tz})),
    )
    .await;
}

fn req() -> StartRequest {
    StartRequest {
        project: 3,
        activity: 4,
        description: None,
    }
}

fn running_id(e: &Engine) -> Option<u64> {
    e.snapshot().state.displayed_running_id()
}

// ---------------------------------------------------------------------

#[tokio::test]
async fn bearer_token_is_sent_and_never_in_debug_output() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/timesheets/active"))
        .and(wiremock::matchers::header(
            "authorization",
            format!("Bearer {SECRET}").as_str(),
        ))
        .respond_with(json_ok(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::with_options(&server.uri(), Token::new(SECRET), fast_opts()).unwrap();
    assert!(!format!("{client:?}").contains(SECRET));
    assert!(!format!("{:?}", Token::new(SECRET)).contains(SECRET));
    client.active().await.unwrap();
}

#[tokio::test]
async fn stopped_elsewhere_is_a_silent_success() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)])),
            json_ok(json!([])), // pre-flight of stop(): already gone
        ]),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/7/stop",
        ResponseTemplate::new(404),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert_eq!(running_id(&e), Some(7));

    assert!(matches!(e.stop().await, Outcome::AlreadyStopped));
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
    assert_eq!(count(&server, "PATCH", "/api/timesheets/7/stop").await, 0);
}

#[tokio::test]
async fn stop_rejected_with_4xx_but_actually_stopped_is_success() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)])), // refresh
            json_ok(json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)])), // pre-flight
            json_ok(json!([])),                                             // verification
        ]),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/7/stop",
        ResponseTemplate::new(400).set_body_json(json!({"code":400,"message":"already stopped"})),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert!(matches!(e.stop().await, Outcome::AlreadyStopped));
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
}

#[tokio::test]
async fn started_elsewhere_creates_no_second_entry() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([])),
            json_ok(json!([running(11, "2024-05-01T09:00:00+0000", 9, 9)])),
        ]),
    )
    .await;
    mount(
        &server,
        "POST",
        "/api/timesheets",
        json_ok(running(12, "2024-05-01T09:00:00+0000", 3, 4)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert!(matches!(e.snapshot().state, State::Idle { .. }));

    assert!(matches!(e.start(req()).await, Outcome::AlreadyRunning));
    assert_eq!(running_id(&e), Some(11));
    assert_eq!(count(&server, "POST", "/api/timesheets").await, 0);
}

#[tokio::test]
async fn several_active_shows_newest_and_stops_exactly_that_one() {
    let server = MockServer::start().await;
    let list = json!([
        running(1, "2024-05-01T07:00:00+0000", 3, 4),
        running(2, "2024-05-01T09:00:00+0000", 3, 4),
        running(3, "2024-05-01T08:00:00+0000", 3, 4),
    ]);
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(list.clone()),
            json_ok(list),
            json_ok(json!([
                running(1, "2024-05-01T07:00:00+0000", 3, 4),
                running(3, "2024-05-01T08:00:00+0000", 3, 4)
            ])),
        ]),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/2/stop",
        json_ok(running(2, "2024-05-01T09:00:00+0000", 3, 4)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    match e.snapshot().state {
        State::Running {
            timesheet, others, ..
        } => {
            assert_eq!(timesheet.id, 2);
            assert_eq!(others, 2);
        }
        s => panic!("{s:?}"),
    }
    assert!(matches!(e.stop().await, Outcome::Stopped));
    assert_eq!(count(&server, "PATCH", "/api/timesheets/2/stop").await, 1);
    assert_eq!(count(&server, "PATCH", "/api/timesheets/1/stop").await, 0);
    // The remaining ones are shown honestly, not hidden.
    assert_eq!(running_id(&e), Some(3));
}

#[tokio::test]
async fn timeout_on_start_but_it_arrived_is_never_repeated() {
    let server = MockServer::start().await;
    mount_me(&server, "Europe/Berlin").await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([])),                                              // refresh
            json_ok(json!([])),                                              // pre-flight
            json_ok(json!([running(20, "2024-05-01T09:00:00+0000", 3, 4)])), // verification
        ]),
    )
    .await;
    mount(
        &server,
        "POST",
        "/api/timesheets",
        json_ok(running(20, "2024-05-01T09:00:00+0000", 3, 4)).set_delay(Duration::from_secs(2)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert!(matches!(e.start(req()).await, Outcome::Started));
    assert_eq!(running_id(&e), Some(20));
    assert_eq!(
        count(&server, "POST", "/api/timesheets").await,
        1,
        "POST must not be repeated"
    );
}

#[tokio::test]
async fn timeout_on_start_and_it_did_not_arrive_reports_failure_and_idle() {
    let server = MockServer::start().await;
    mount_me(&server, "Europe/Berlin").await;
    mount(&server, "GET", "/api/timesheets/active", json_ok(json!([]))).await;
    mount(
        &server,
        "POST",
        "/api/timesheets",
        json_ok(json!({})).set_delay(Duration::from_secs(2)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    match e.start(req()).await {
        Outcome::Failed(err) => assert_eq!(err.kind, ErrorKind::Timeout),
        o => panic!("{o:?}"),
    }
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
    assert_eq!(count(&server, "POST", "/api/timesheets").await, 1);
}

#[tokio::test]
async fn timeout_on_stop_but_it_is_stopped() {
    let server = MockServer::start().await;
    let r = json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)]);
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![json_ok(r.clone()), json_ok(r), json_ok(json!([]))]),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/7/stop",
        json_ok(running(7, "2024-05-01T08:00:00+0000", 3, 4)).set_delay(Duration::from_secs(2)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert!(matches!(e.stop().await, Outcome::Stopped));
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
    assert_eq!(count(&server, "PATCH", "/api/timesheets/7/stop").await, 1);
}

#[tokio::test]
async fn timeout_on_stop_and_still_running_stays_running_with_error() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        json_ok(json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)])),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/7/stop",
        ResponseTemplate::new(200).set_delay(Duration::from_secs(2)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert!(matches!(e.stop().await, Outcome::Failed(_)));
    assert_eq!(running_id(&e), Some(7));
    assert_eq!(count(&server, "PATCH", "/api/timesheets/7/stop").await, 1);
}

#[tokio::test]
async fn stale_answer_arriving_after_a_newer_one_is_ignored() {
    let server = MockServer::start().await;
    // 1st request: slow and says "running". 2nd: fast and says "idle".
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)]))
                .set_delay(Duration::from_millis(250)),
            json_ok(json!([])),
        ]),
    )
    .await;
    let opts = ClientOptions {
        request_timeout: Duration::from_secs(5),
        ..fast_opts()
    };
    let e = engine_for(&server, opts);
    let slow = tokio::spawn({
        let e = e.clone();
        async move { e.refresh(RefreshReason::Timer).await }
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    e.refresh(RefreshReason::Manual).await;
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
    slow.await.unwrap();
    assert!(
        matches!(e.snapshot().state, State::Idle { .. }),
        "stale 'running' must not win"
    );
}

#[tokio::test]
async fn unauthorized_in_the_middle_of_use() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)])),
            ResponseTemplate::new(401).set_body_json(json!({"code":401,"message":"Invalid token"})),
        ]),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert_eq!(running_id(&e), Some(7));
    e.refresh(RefreshReason::Timer).await;
    assert!(matches!(e.snapshot().state, State::Unauthorized));
    assert_eq!(
        running_id(&e),
        None,
        "no timer may be shown after the token died"
    );
    assert!(matches!(e.start(req()).await, Outcome::Ignored));
}

#[tokio::test]
async fn clock_skew_of_minutes_is_compensated() {
    let server = MockServer::start().await;
    // The server clock runs 7 minutes ahead of this machine.
    let server_now = Timestamp::now()
        .checked_add(jiff::SignedDuration::from_secs(420))
        .unwrap();
    let begin = server_now
        .checked_sub(jiff::SignedDuration::from_secs(3600))
        .unwrap();
    let date = server_now.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string();
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        json_ok(json!([running(7, &begin.to_string(), 3, 4)])).insert_header("Date", date.as_str()),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    let snap = e.snapshot();
    assert!(
        (snap.clock_offset.as_secs() - 420).abs() <= 2,
        "offset {:?}",
        snap.clock_offset
    );
    let State::Running { timesheet, .. } = snap.state else {
        panic!()
    };
    let shown = elapsed(timesheet.begin, Timestamp::now(), snap.clock_offset).as_secs();
    assert!((shown - 3600).abs() <= 2, "shows {shown}s instead of 1h");
}

#[tokio::test]
async fn standby_and_network_change_recover_from_unconfirmed_state() {
    let server = MockServer::start().await;
    let running_json = json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)]);
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(running_json),
            ResponseTemplate::new(503), // network is down after wake-up
            json_ok(json!([])),         // network back; stopped meanwhile elsewhere
        ]),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    e.refresh(RefreshReason::Resume).await;
    match e.snapshot().state {
        State::Error {
            last_known: Some(o),
            ..
        } => assert_eq!(o.tracking.running_id(), Some(7)),
        s => panic!("{s:?}"),
    }
    assert!(!e.snapshot().state.is_confirmed());
    e.refresh(RefreshReason::Network).await;
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
}

#[tokio::test]
async fn get_is_retried_with_backoff_but_post_is_not() {
    let server = MockServer::start().await;
    mount_me(&server, "Europe/Berlin").await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            ResponseTemplate::new(502),
            ResponseTemplate::new(502),
            json_ok(json!([])),
        ]),
    )
    .await;
    let opts = ClientOptions {
        retry_delays: vec![Duration::from_millis(10); 2],
        ..fast_opts()
    };
    let e = engine_for(&server, opts);
    e.refresh(RefreshReason::Startup).await;
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
    assert_eq!(count(&server, "GET", "/api/timesheets/active").await, 3);

    mount(
        &server,
        "POST",
        "/api/timesheets",
        ResponseTemplate::new(500),
    )
    .await;
    let _ = e.start(req()).await;
    assert_eq!(count(&server, "POST", "/api/timesheets").await, 1);
}

#[tokio::test]
async fn double_click_runs_one_write_only() {
    let server = MockServer::start().await;
    mount_me(&server, "Europe/Berlin").await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([])),
            json_ok(json!([])),
            json_ok(json!([running(20, "2024-05-01T09:00:00+0000", 3, 4)])),
        ]),
    )
    .await;
    mount(
        &server,
        "POST",
        "/api/timesheets",
        json_ok(running(20, "2024-05-01T09:00:00+0000", 3, 4))
            .set_delay(Duration::from_millis(150)),
    )
    .await;
    let opts = ClientOptions {
        request_timeout: Duration::from_secs(5),
        ..fast_opts()
    };
    let e = engine_for(&server, opts);
    e.refresh(RefreshReason::Startup).await;
    let (a, b) = tokio::join!(e.start(req()), e.start(req()));
    let ignored = [&a, &b]
        .iter()
        .filter(|o| matches!(o, Outcome::Ignored))
        .count();
    assert_eq!(ignored, 1, "{a:?} {b:?}");
    assert_eq!(count(&server, "POST", "/api/timesheets").await, 1);
}

#[tokio::test]
async fn focus_refreshes_are_throttled_other_reasons_are_not() {
    let server = MockServer::start().await;
    mount(&server, "GET", "/api/timesheets/active", json_ok(json!([]))).await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Focus).await;
    e.refresh(RefreshReason::Focus).await;
    assert_eq!(count(&server, "GET", "/api/timesheets/active").await, 1);
    e.refresh(RefreshReason::Resume).await;
    assert_eq!(count(&server, "GET", "/api/timesheets/active").await, 2);
    tokio::time::sleep(Duration::from_millis(350)).await;
    e.refresh(RefreshReason::Focus).await;
    assert_eq!(count(&server, "GET", "/api/timesheets/active").await, 3);
}

#[tokio::test]
async fn reconfigure_discards_answers_of_the_old_connection() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        json_ok(json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)]))
            .set_delay(Duration::from_millis(200)),
    )
    .await;
    let opts = ClientOptions {
        request_timeout: Duration::from_secs(5),
        ..fast_opts()
    };
    let e = engine_for(&server, opts);
    let h = tokio::spawn({
        let e = e.clone();
        async move { e.refresh(RefreshReason::Startup).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    e.configure(None);
    h.await.unwrap();
    assert!(matches!(e.snapshot().state, State::NotConfigured));
}

#[tokio::test]
async fn toggle_stops_when_running_and_asks_for_input_when_idle_without_defaults() {
    let server = MockServer::start().await;
    let r = json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)]);
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(r.clone()),
            json_ok(r),
            json_ok(json!([])),
            json_ok(json!([])),
        ]),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/7/stop",
        json_ok(running(7, "2024-05-01T08:00:00+0000", 3, 4)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    assert!(matches!(e.toggle(None).await, Outcome::Stopped));
    assert!(matches!(e.toggle(None).await, Outcome::NeedsInput));
}

#[tokio::test]
async fn stop_in_a_fresh_process_asks_the_server_first() {
    let server = MockServer::start().await;
    let r = json!([running(7, "2024-05-01T08:00:00+0000", 3, 4)]);
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![json_ok(r.clone()), json_ok(r), json_ok(json!([]))]),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/7/stop",
        json_ok(running(7, "2024-05-01T08:00:00+0000", 3, 4)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    assert!(matches!(e.stop().await, Outcome::Stopped));
    assert_eq!(count(&server, "PATCH", "/api/timesheets/7/stop").await, 1);
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
}

#[tokio::test]
async fn stale_notification_stop_is_idempotent_and_spares_a_newer_entry() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        json_ok(json!([running(8, "2024-05-01T10:00:00+0000", 3, 4)])),
    )
    .await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/8/stop",
        json_ok(running(8, "2024-05-01T10:00:00+0000", 3, 4)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    assert!(matches!(e.stop_entry(7).await, Outcome::AlreadyStopped));
    assert_eq!(
        count(&server, "PATCH", "/api/timesheets/8/stop").await,
        0,
        "must not stop the newer entry"
    );
    assert_eq!(running_id(&e), Some(8), "and shows it honestly");
}

// ---- error mapping ---------------------------------------------------

#[tokio::test]
async fn validation_errors_carry_the_server_messages() {
    let server = MockServer::start().await;
    mount_me(&server, "Europe/Berlin").await;
    mount(
        &server,
        "POST",
        "/api/timesheets",
        ResponseTemplate::new(400).set_body_json(json!({
            "code": 400, "message": "Validation Failed",
            "errors": {"errors": [], "children": {"project": {"errors": ["Project is closed."]}}}
        })),
    )
    .await;
    let c = Client::with_options(&server.uri(), Token::new(SECRET), fast_opts()).unwrap();
    let err = c.start(&req(), Timestamp::now()).await.unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::Validation(vec!["Project is closed.".into()])
    );
    assert!(!err.is_ambiguous());
}

#[tokio::test]
async fn html_instead_of_json_and_redirects_are_reported_not_followed() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/version",
        ResponseTemplate::new(200).set_body_string("<html>captive portal</html>"),
    )
    .await;
    mount(
        &server,
        "GET",
        "/api/users/me",
        ResponseTemplate::new(301).insert_header("Location", "https://elsewhere.invalid/"),
    )
    .await;
    let c = Client::with_options(&server.uri(), Token::new(SECRET), fast_opts()).unwrap();
    assert_eq!(
        c.version().await.unwrap_err().kind,
        ErrorKind::UnexpectedResponse
    );
    assert_eq!(c.me().await.unwrap_err().kind, ErrorKind::Redirect);
}

#[tokio::test]
async fn status_codes_map_to_kinds() {
    let server = MockServer::start().await;
    for (p, code) in [
        ("/api/timesheets/active", 403u16),
        ("/api/projects", 404),
        ("/api/version", 500),
    ] {
        mount(&server, "GET", p, ResponseTemplate::new(code)).await;
    }
    let c = Client::with_options(&server.uri(), Token::new(SECRET), fast_opts()).unwrap();
    assert_eq!(c.active().await.unwrap_err().kind, ErrorKind::Unauthorized);
    assert_eq!(c.projects().await.unwrap_err().kind, ErrorKind::NotFound);
    assert_eq!(c.version().await.unwrap_err().kind, ErrorKind::Server(500));
}

#[tokio::test]
async fn unreachable_server_is_offline_not_a_panic() {
    let c = Client::with_options("http://127.0.0.1:1", Token::new(SECRET), fast_opts()).unwrap();
    let err = c.active().await.unwrap_err();
    assert!(err.is_connectivity(), "{err:?}");
}

#[tokio::test]
async fn catalog_pagination_and_visibility() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        "/api/projects",
        Seq::new(vec![
            json_ok(json!([{"id":1,"name":"A","parentTitle":"ACME","visible":true},{"id":2,"name":"Hidden","visible":false}])).insert_header("X-Total-Pages", "2"),
            json_ok(json!([{"id":3,"name":"C"}])).insert_header("X-Total-Pages", "2"),
        ]),
    )
    .await;
    let c = Client::with_options(&server.uri(), Token::new(SECRET), fast_opts()).unwrap();
    let ps = c.projects().await.unwrap();
    assert_eq!(ps.iter().map(|p| p.id).collect::<Vec<_>>(), vec![1, 3]);
    assert_eq!(ps[0].customer.as_deref(), Some("ACME"));
}

// ---- time zone, history, edit, delete ---------------------------------

#[tokio::test]
async fn start_sends_begin_in_the_kimai_users_time_zone_not_the_local_one() {
    let server = MockServer::start().await;
    mount_me(&server, "Pacific/Kiritimati").await; // UTC+14, far from any test machine
    mount(
        &server,
        "GET",
        "/api/timesheets/active",
        Seq::new(vec![
            json_ok(json!([])),
            json_ok(json!([])),
            json_ok(json!([running(9, "2024-05-01T09:00:00+0000", 3, 4)])),
        ]),
    )
    .await;
    mount(
        &server,
        "POST",
        "/api/timesheets",
        json_ok(running(9, "2024-05-01T09:00:00+0000", 3, 4)),
    )
    .await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    let before = Timestamp::now();
    assert!(matches!(e.start(req()).await, Outcome::Started));
    let posts: Vec<_> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST")
        .collect();
    assert_eq!(posts.len(), 1);
    let body: Value = serde_json::from_slice(&posts[0].body).unwrap_or_default();
    let sent = body["begin"].as_str().unwrap_or_default().to_string();
    // Naive local time in UTC+14 == now + 14h, formatted without offset.
    let expected = before
        .checked_add(jiff::SignedDuration::from_secs(14 * 3600))
        .unwrap();
    let expected = expected.strftime("%Y-%m-%dT%H:%M").to_string();
    assert!(
        sent.starts_with(&expected[..13]),
        "sent {sent}, expected ~{expected}"
    );
    assert_eq!(body["project"], 3);
}

#[tokio::test]
async fn a_failing_time_zone_lookup_prevents_the_post() {
    let server = MockServer::start().await;
    mount(&server, "GET", "/api/users/me", ResponseTemplate::new(500)).await;
    mount(&server, "GET", "/api/timesheets/active", json_ok(json!([]))).await;
    mount(&server, "POST", "/api/timesheets", json_ok(json!({}))).await;
    let e = engine_for(&server, fast_opts());
    e.refresh(RefreshReason::Startup).await;
    assert!(matches!(e.start(req()).await, Outcome::Failed(_)));
    assert_eq!(count(&server, "POST", "/api/timesheets").await, 0);
    assert!(matches!(e.snapshot().state, State::Idle { .. }));
}

#[tokio::test]
async fn history_is_requested_in_user_time_and_parsed() {
    let server = MockServer::start().await;
    mount_me(&server, "Europe/Berlin").await;
    Mock::given(method("GET"))
        .and(path("/api/timesheets"))
        .and(wiremock::matchers::query_param("active", "0"))
        .and(wiremock::matchers::query_param("full", "1"))
        .and(wiremock::matchers::query_param("begin", "2024-05-01T10:00:00"))
        .respond_with(json_ok(json!([
            {"id": 5, "begin": "2024-05-01T10:00:00+0200", "end": "2024-05-01T11:30:00+0200",
             "project": {"id": 3, "name": "Web"}, "activity": {"id": 4, "name": "Dev"}, "description": "x"},
            {"id": 6, "begin": "2024-05-01T12:00:00+0200", "end": null, "project": 3, "activity": 4}
        ])))
        .mount(&server)
        .await;
    let c = Client::with_options(&server.uri(), Token::new(SECRET), fast_opts()).unwrap();
    let since: Timestamp = "2024-05-01T08:00:00Z".parse().unwrap();
    let list = c.history(since).await.unwrap();
    assert_eq!(list.len(), 1, "running entries are not history");
    assert_eq!(list[0].project.name.as_deref(), Some("Web"));
}

#[tokio::test]
async fn update_and_delete_are_single_shot_and_use_the_right_shape() {
    use stempeluhr_core::kimai::EntryUpdate;
    let server = MockServer::start().await;
    mount_me(&server, "Europe/Berlin").await;
    mount(
        &server,
        "PATCH",
        "/api/timesheets/5",
        json_ok(running(5, "2024-05-01T10:00:00+0200", 3, 4)),
    )
    .await;
    mount(
        &server,
        "DELETE",
        "/api/timesheets/6",
        ResponseTemplate::new(204),
    )
    .await;
    mount(
        &server,
        "DELETE",
        "/api/timesheets/7",
        ResponseTemplate::new(500),
    )
    .await;
    let c = Client::with_options(&server.uri(), Token::new(SECRET), fast_opts()).unwrap();
    let u = EntryUpdate {
        project: 3,
        activity: 4,
        begin: "2024-05-01T08:00:00Z".parse().unwrap(),
        end: "2024-05-01T09:30:00Z".parse().unwrap(),
        description: "note".into(),
    };
    c.update_entry(5, &u).await.unwrap();
    let req = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.method.as_str() == "PATCH")
        .unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
    assert_eq!(body["begin"], "2024-05-01T10:00:00");
    assert_eq!(body["end"], "2024-05-01T11:30:00");
    assert_eq!(body["description"], "note");

    c.delete_entry(6).await.unwrap();
    assert_eq!(
        c.delete_entry(7).await.unwrap_err().kind,
        ErrorKind::Server(500)
    );
    assert_eq!(
        count(&server, "DELETE", "/api/timesheets/7").await,
        1,
        "no automatic retry"
    );
}
