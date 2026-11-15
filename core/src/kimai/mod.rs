// SPDX-License-Identifier: GPL-3.0-or-later
//! Kimai REST API client. No GTK, no UI.

mod client;
mod error;
mod time;
mod types;
mod url;

pub use client::{Client, ClientOptions, Reply, Token};
pub use error::{Error, ErrorKind, Result};
pub use time::{clock_offset, format_kimai_begin, parse_http_date, parse_kimai_timestamp};
pub use types::{Activity, EntryUpdate, Project, Ref, StartRequest, Timesheet, User, Version};
pub use url::{normalize_base_url, UrlError};
