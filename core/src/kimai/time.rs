// SPDX-License-Identifier: GPL-3.0-or-later
use jiff::{tz::TimeZone, SignedDuration, Timestamp};

/// Parses Kimai's timestamps. PHP's `O` format yields offsets without a
/// colon (`2024-05-01T10:00:00+0200`), which strict ISO parsers reject.
pub fn parse_kimai_timestamp(s: &str) -> Result<Timestamp, String> {
    let s = s.trim();
    let fixed = fix_offset(s);
    fixed
        .parse::<Timestamp>()
        .map_err(|e| format!("invalid timestamp {s:?}: {e}"))
}

fn fix_offset(s: &str) -> String {
    // Find a trailing +HHMM / -HHMM after the time part.
    if let Some(t) = s.find('T') {
        let tail = &s[t..];
        if let Some(sign_pos) = tail.rfind(['+', '-']) {
            let off = &tail[sign_pos + 1..];
            if off.len() == 4 && off.bytes().all(|b| b.is_ascii_digit()) {
                let abs = t + sign_pos;
                return format!("{}{}:{}", &s[..=abs], &off[..2], &off[2..]);
            }
        }
    }
    s.to_string()
}

/// Formats an instant as the naive local time Kimai expects for `begin`
/// (interpreted in the user's configured time zone).
pub fn format_kimai_begin(ts: Timestamp, tz: &TimeZone) -> String {
    ts.to_zoned(tz.clone())
        .strftime("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

/// Server clock offset derived from an HTTP `Date` header.
/// `Date` has one-second resolution, so differences below two seconds are
/// treated as "in sync" to avoid a flickering display.
pub fn clock_offset(server_date: Timestamp, local_received: Timestamp) -> SignedDuration {
    // The true server time lies within [date, date + 1s); use the middle.
    let est = server_date
        .checked_add(SignedDuration::from_millis(500))
        .unwrap_or(server_date);
    let off = est.duration_since(local_received);
    if off.as_secs().abs() < 2 {
        SignedDuration::ZERO
    } else {
        SignedDuration::from_secs(off.as_secs())
    }
}

/// Parses an HTTP-date (RFC 9110 IMF-fixdate, a subset of RFC 2822).
pub fn parse_http_date(s: &str) -> Option<Timestamp> {
    jiff::fmt::rfc2822::parse(s).ok().map(|z| z.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_php_offsets() {
        let a = parse_kimai_timestamp("2024-05-01T10:00:00+0200").unwrap();
        let b = parse_kimai_timestamp("2024-05-01T08:00:00Z").unwrap();
        let c = parse_kimai_timestamp("2024-05-01T10:00:00+02:00").unwrap();
        let d = parse_kimai_timestamp("2024-05-01T03:30:00-0430").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
    }

    #[test]
    fn rejects_naive_and_garbage() {
        assert!(parse_kimai_timestamp("2024-05-01T10:00:00").is_err());
        assert!(parse_kimai_timestamp("yesterday").is_err());
    }

    #[test]
    fn offset_deadzone() {
        let now: Timestamp = "2024-05-01T08:00:00Z".parse().unwrap();
        let near: Timestamp = "2024-05-01T08:00:01Z".parse().unwrap();
        assert_eq!(clock_offset(near, now), SignedDuration::ZERO);
        let far: Timestamp = "2024-05-01T08:07:00Z".parse().unwrap();
        assert_eq!(clock_offset(far, now).as_secs(), 420);
    }

    #[test]
    fn http_date() {
        let t = parse_http_date("Wed, 01 May 2024 08:00:00 GMT").unwrap();
        assert_eq!(t, "2024-05-01T08:00:00Z".parse::<Timestamp>().unwrap());
    }

    #[test]
    fn begin_format() {
        let ts: Timestamp = "2024-05-01T08:00:00Z".parse().unwrap();
        let tz = TimeZone::get("Europe/Berlin").unwrap();
        assert_eq!(format_kimai_begin(ts, &tz), "2024-05-01T10:00:00");
    }
}
