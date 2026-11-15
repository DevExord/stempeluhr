// SPDX-License-Identifier: GPL-3.0-or-later
use std::error::Error as StdError;
use std::fmt;

/// What went wrong, in terms the UI can translate into a friendly message.
/// The raw error text is only kept in `Error::detail` for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// No network / connection refused / reset.
    Offline,
    Dns,
    Timeout,
    Tls,
    /// 401 or 403: token invalid, revoked or without permission.
    Unauthorized,
    NotFound,
    /// 400/422 with validation messages supplied by the server.
    Validation(Vec<String>),
    /// 3xx — we never follow redirects (a redirected POST would silently
    /// turn into a GET), so the configured URL is wrong.
    Redirect,
    /// 5xx.
    Server(u16),
    /// Other 4xx.
    Client(u16),
    /// The answer was not the JSON we expected (e.g. a proxy login page).
    UnexpectedResponse,
    /// Anything else.
    Other,
}

#[derive(Debug, Clone)]
pub struct Error {
    pub kind: ErrorKind,
    /// Technical detail for logs only. Never contains the token.
    pub detail: String,
}

impl Error {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn is_unauthorized(&self) -> bool {
        self.kind == ErrorKind::Unauthorized
    }

    /// The request may or may not have reached the server (or may have been
    /// processed without us seeing the answer). After such a failure a write
    /// must never be repeated; the state has to be re-read instead.
    pub fn is_ambiguous(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Offline
                | ErrorKind::Dns
                | ErrorKind::Timeout
                | ErrorKind::Server(_)
                | ErrorKind::UnexpectedResponse
                | ErrorKind::Other
        )
    }

    /// Worth retrying for idempotent GET requests.
    pub fn is_transient(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Offline | ErrorKind::Dns | ErrorKind::Timeout | ErrorKind::Server(_)
        )
    }

    /// Whether this looks like "no connection" as opposed to a server-side
    /// answer. Used for the "offline" wording in the UI.
    pub fn is_connectivity(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Offline | ErrorKind::Dns | ErrorKind::Timeout
        )
    }

    pub(crate) fn from_reqwest(err: reqwest::Error) -> Self {
        let mut chain = String::new();
        let mut src: Option<&(dyn StdError + 'static)> = Some(&err);
        while let Some(e) = src {
            chain.push_str(&e.to_string());
            chain.push_str(" | ");
            src = e.source();
        }
        let lower = chain.to_lowercase();
        let kind = if err.is_timeout() {
            ErrorKind::Timeout
        } else if lower.contains("dns")
            || lower.contains("failed to lookup")
            || lower.contains("name or service not known")
            || lower.contains("no address associated")
            || lower.contains("temporary failure in name resolution")
        {
            ErrorKind::Dns
        } else if lower.contains("certificate")
            || lower.contains("handshake")
            || lower.contains("tls")
            || lower.contains("ssl")
        {
            ErrorKind::Tls
        } else if err.is_connect() || err.is_request() || lower.contains("connection") {
            ErrorKind::Offline
        } else {
            ErrorKind::Other
        };
        // reqwest's Display may include the URL; the URL holds no secret
        // (the token is only ever sent as a header).
        Self::new(kind, chain)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

impl StdError for Error {}

pub type Result<T> = std::result::Result<T, Error>;
