// SPDX-License-Identifier: GPL-3.0-or-later
use gtk::glib;
use jiff::{SignedDuration, Timestamp};

use crate::i18n::tr_f;

/// `H:MM:SS`
pub fn hms(d: SignedDuration) -> String {
    let total = d.as_secs().max(0);
    format!(
        "{}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

fn local(ts: Timestamp) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(ts.as_second()).ok()
}

/// Local clock time, e.g. `09:41`.
pub fn clock(ts: Timestamp) -> String {
    local(ts)
        .and_then(|d| d.format("%H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// "Since 09:41", or with the date when it started on another day.
pub fn since(ts: Timestamp) -> String {
    let Some(d) = local(ts) else {
        return String::new();
    };
    let same_day = glib::DateTime::now_local()
        .map(|n| n.ymd() == d.ymd())
        .unwrap_or(true);
    let when = if same_day {
        d.format("%H:%M")
    } else {
        d.format("%x, %H:%M")
    }
    .map(|s| s.to_string())
    .unwrap_or_default();
    tr_f("Since {time}", &[("time", &when)])
}
