// SPDX-License-Identifier: GPL-3.0-or-later
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use jiff::{tz::TimeZone, Timestamp};
use reqwest::{header, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::error::{Error, ErrorKind, Result};
use super::time::{format_kimai_begin, parse_http_date};
use super::types::{Activity, EntryUpdate, Project, StartRequest, Timesheet, User, Version};

/// The API token. `Debug`/`Display` never reveal it.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(s: impl Into<String>) -> Self {
        Token(s.into().trim().to_owned())
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// Delays between retries of idempotent GET requests. POST/PATCH are
    /// never retried.
    pub retry_delays: Vec<Duration>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(20),
            retry_delays: vec![Duration::from_millis(400), Duration::from_millis(1500)],
        }
    }
}

/// A parsed answer plus what we learned about the server clock.
#[derive(Debug, Clone)]
pub struct Reply<T> {
    pub value: T,
    /// Value of the HTTP `Date` header, if present and parseable.
    pub server_date: Option<Timestamp>,
    /// Local wall clock when the answer arrived.
    pub received: Timestamp,
}

struct Raw {
    body: Vec<u8>,
    server_date: Option<Timestamp>,
    received: Timestamp,
    total_pages: Option<u32>,
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Token,
    opts: ClientOptions,
    user_tz: Mutex<Option<TimeZone>>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// `base_url` must already be normalised (see `normalize_base_url`).
    pub fn new(base_url: &str, token: Token) -> Result<Self> {
        Self::with_options(base_url, token, ClientOptions::default())
    }

    pub fn with_options(base_url: &str, token: Token, opts: ClientOptions) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(opts.connect_timeout)
            .timeout(opts.request_timeout)
            // A redirected POST would silently become a GET.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("stempeluhr/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_owned(),
            token,
            opts,
            user_tz: Mutex::new(None),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    // ---- endpoints -------------------------------------------------------

    pub async fn version(&self) -> Result<Reply<Version>> {
        self.get_json("/api/version", &[]).await
    }

    pub async fn me(&self) -> Result<Reply<User>> {
        let r: Reply<User> = self.get_json("/api/users/me", &[]).await?;
        if let Some(tz) = r
            .value
            .timezone
            .as_deref()
            .and_then(|n| TimeZone::get(n).ok())
        {
            if let Ok(mut g) = self.user_tz.lock() {
                *g = Some(tz);
            }
        }
        Ok(r)
    }

    /// Connection test used by onboarding: reachable, authenticated, and a
    /// real Kimai answer.
    pub async fn test_connection(&self) -> Result<(Version, User)> {
        let v = self.version().await?.value;
        let u = self.me().await?.value;
        Ok((v, u))
    }

    /// The Kimai user's time zone (cached after the first lookup). Kimai
    /// reads and writes naive date-times in this zone, so it must come from
    /// the server, never from the local machine.
    pub async fn user_timezone(&self) -> Result<TimeZone> {
        if let Some(tz) = self.user_tz.lock().ok().and_then(|g| g.clone()) {
            return Ok(tz);
        }
        let user = self.me().await?.value;
        Ok(user
            .timezone
            .as_deref()
            .and_then(|n| TimeZone::get(n).ok())
            .unwrap_or_else(TimeZone::system))
    }

    pub async fn active(&self) -> Result<Reply<Vec<Timesheet>>> {
        self.get_json("/api/timesheets/active", &[]).await
    }

    pub async fn recent(&self) -> Result<Reply<Vec<Timesheet>>> {
        self.get_json("/api/timesheets/recent", &[("size", "10".into())])
            .await
    }

    pub async fn projects(&self) -> Result<Vec<Project>> {
        let mut all: Vec<Project> = self
            .get_paged(
                "/api/projects",
                &[
                    ("visible", "1".into()),
                    ("order", "ASC".into()),
                    ("orderBy", "name".into()),
                ],
            )
            .await?;
        all.retain(|p| p.visible);
        Ok(all)
    }

    /// Activities usable with the given project (project-specific and global).
    pub async fn activities(&self, project: u64) -> Result<Vec<Activity>> {
        let mut all: Vec<Activity> = self
            .get_paged(
                "/api/activities",
                &[
                    ("project", project.to_string()),
                    ("visible", "1".into()),
                    ("order", "ASC".into()),
                    ("orderBy", "name".into()),
                ],
            )
            .await?;
        all.retain(|a| a.visible);
        Ok(all)
    }

    /// `POST /api/timesheets`. Never retried. `now` should be the
    /// server-adjusted current time.
    pub async fn start(&self, req: &StartRequest, now: Timestamp) -> Result<Reply<Timesheet>> {
        let tz = self.user_timezone().await?;
        let mut body = json!({
            "project": req.project,
            "activity": req.activity,
            "begin": format_kimai_begin(now, &tz),
        });
        if let Some(d) = req
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            body["description"] = Value::String(d.to_owned());
        }
        let raw = self
            .send(Method::POST, "/api/timesheets", &[], Some(body))
            .await?;
        parse(raw)
    }

    /// Finished timesheets since `since`, newest first.
    pub async fn history(&self, since: Timestamp) -> Result<Vec<Timesheet>> {
        let tz = self.user_timezone().await?;
        let mut list: Vec<Timesheet> = self
            .get_paged(
                "/api/timesheets",
                &[
                    ("begin", format_kimai_begin(since, &tz)),
                    ("active", "0".into()),
                    ("full", "1".into()),
                    ("order", "DESC".into()),
                    ("orderBy", "begin".into()),
                ],
            )
            .await?;
        list.retain(|t| t.end.is_some());
        Ok(list)
    }

    /// `PATCH /api/timesheets/{id}`. Never retried.
    pub async fn update_entry(&self, id: u64, u: &EntryUpdate) -> Result<Reply<Timesheet>> {
        let tz = self.user_timezone().await?;
        let body = json!({
            "project": u.project,
            "activity": u.activity,
            "begin": format_kimai_begin(u.begin, &tz),
            "end": format_kimai_begin(u.end, &tz),
            "description": u.description,
        });
        let raw = self
            .send(
                Method::PATCH,
                &format!("/api/timesheets/{id}"),
                &[],
                Some(body),
            )
            .await?;
        parse(raw)
    }

    /// `DELETE /api/timesheets/{id}`. Never retried.
    pub async fn delete_entry(&self, id: u64) -> Result<()> {
        self.send(Method::DELETE, &format!("/api/timesheets/{id}"), &[], None)
            .await
            .map(|_| ())
    }

    /// `PATCH /api/timesheets/{id}/stop`. Never retried.
    pub async fn stop(&self, id: u64) -> Result<Reply<Timesheet>> {
        let raw = self
            .send(
                Method::PATCH,
                &format!("/api/timesheets/{id}/stop"),
                &[],
                None,
            )
            .await?;
        parse(raw)
    }

    // ---- plumbing --------------------------------------------------------

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Reply<T>> {
        let mut attempt = 0;
        loop {
            let res = self
                .send(Method::GET, path, query, None)
                .await
                .and_then(parse::<T>);
            match res {
                Err(e) if e.is_transient() && attempt < self.opts.retry_delays.len() => {
                    tracing::debug!(%path, attempt, error = ?e.kind, "GET failed, retrying");
                    tokio::time::sleep(self.opts.retry_delays[attempt]).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    async fn get_paged<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<T>> {
        const SIZE: usize = 100;
        let mut out = Vec::new();
        for page in 1..=50u32 {
            let mut q: Vec<(&str, String)> = query.to_vec();
            q.push(("page", page.to_string()));
            q.push(("size", SIZE.to_string()));
            // Retry applies per page.
            let mut attempt = 0;
            let raw = loop {
                match self.send(Method::GET, path, &q, None).await {
                    Err(e) if e.is_transient() && attempt < self.opts.retry_delays.len() => {
                        tokio::time::sleep(self.opts.retry_delays[attempt]).await;
                        attempt += 1;
                    }
                    other => break other?,
                }
            };
            let total = raw.total_pages;
            let chunk: Vec<T> = parse::<Vec<T>>(raw)?.value;
            let n = chunk.len();
            out.extend(chunk);
            let done = match total {
                Some(t) => page >= t,
                None => n < SIZE,
            };
            if done || n == 0 {
                break;
            }
        }
        Ok(out)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Raw> {
        let url = format!("{}{}", self.base, path);
        let mut rb = self
            .http
            .request(method.clone(), &url)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token.0))
            .header(header::ACCEPT, "application/json");
        if !query.is_empty() {
            rb = rb.query(query);
        }
        if let Some(b) = &body {
            rb = rb.json(b);
        } else if method != Method::GET {
            rb = rb.header(header::CONTENT_LENGTH, "0");
        }
        tracing::debug!(%method, %path, "request");
        let resp = rb.send().await.map_err(Error::from_reqwest)?;
        let received = Timestamp::now();
        let status = resp.status();
        let server_date = resp
            .headers()
            .get(header::DATE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_http_date);
        let total_pages = resp
            .headers()
            .get("x-total-pages")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let bytes = resp.bytes().await.map_err(Error::from_reqwest)?;
        tracing::debug!(%method, %path, status = status.as_u16(), "response");
        if status.is_success() {
            Ok(Raw {
                body: bytes.to_vec(),
                server_date,
                received,
                total_pages,
            })
        } else {
            Err(error_for_status(status, &bytes))
        }
    }
}

fn parse<T: DeserializeOwned>(raw: Raw) -> Result<Reply<T>> {
    let value = serde_json::from_slice::<T>(&raw.body)
        .map_err(|e| Error::new(ErrorKind::UnexpectedResponse, format!("JSON: {e}")))?;
    Ok(Reply {
        value,
        server_date: raw.server_date,
        received: raw.received,
    })
}

fn error_for_status(status: StatusCode, body: &[u8]) -> Error {
    let code = status.as_u16();
    let kind = match code {
        401 | 403 => ErrorKind::Unauthorized,
        404 => ErrorKind::NotFound,
        400 | 422 => ErrorKind::Validation(validation_messages(body)),
        300..=399 => ErrorKind::Redirect,
        500..=599 => ErrorKind::Server(code),
        _ => ErrorKind::Client(code),
    };
    Error::new(kind, format!("HTTP {code}"))
}

/// Extracts human-readable messages from Symfony-style validation answers:
/// `{"message":"Validation Failed","errors":{"errors":[..],"children":{"f":{"errors":[..]}}}}`.
fn validation_messages(body: &[u8]) -> Vec<String> {
    fn collect(v: &Value, out: &mut Vec<String>) {
        if let Some(arr) = v.get("errors").and_then(Value::as_array) {
            out.extend(arr.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        if let Some(ch) = v.get("children").and_then(Value::as_object) {
            for c in ch.values() {
                collect(c, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        if let Some(e) = v.get("errors") {
            collect(e, &mut out);
        }
        if out.is_empty() {
            if let Some(m) = v.get("message").and_then(Value::as_str) {
                out.push(m.to_owned());
            }
        }
    }
    out
}
