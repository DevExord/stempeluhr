// SPDX-License-Identifier: GPL-3.0-or-later
use jiff::Timestamp;
use serde::{de, Deserialize, Deserializer};
use serde_json::Value;

use super::time::parse_kimai_timestamp;

/// A reference to a project/activity. Depending on the endpoint Kimai sends
/// either a bare id or an expanded object; both are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ref {
    pub id: u64,
    pub name: Option<String>,
    /// Customer name for projects, project name for activities (if known).
    pub parent: Option<String>,
    /// Project colour (`#rrggbb`) if Kimai sent one. Unvalidated.
    pub color: Option<String>,
}

impl<'de> Deserialize<'de> for Ref {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Number(n) => Ok(Ref {
                id: n.as_u64().ok_or_else(|| de::Error::custom("bad id"))?,
                ..Default::default()
            }),
            Value::Object(o) => {
                let id = o
                    .get("id")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| de::Error::custom("missing id"))?;
                let name = o.get("name").and_then(Value::as_str).map(str::to_owned);
                let parent = o
                    .get("parentTitle")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        o.get("customer")
                            .and_then(|c| c.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .or_else(|| {
                        o.get("project")
                            .and_then(|c| c.get("name"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                let color = o.get("color").and_then(Value::as_str).map(str::to_owned);
                Ok(Ref {
                    id,
                    name,
                    parent,
                    color,
                })
            }
            _ => Err(de::Error::custom("expected id or object")),
        }
    }
}

fn de_ts<'de, D: Deserializer<'de>>(d: D) -> Result<Timestamp, D::Error> {
    let s = String::deserialize(d)?;
    parse_kimai_timestamp(&s).map_err(de::Error::custom)
}

fn de_opt_ts<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Timestamp>, D::Error> {
    match Option::<String>::deserialize(d)? {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => parse_kimai_timestamp(&s)
            .map(Some)
            .map_err(de::Error::custom),
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Timesheet {
    pub id: u64,
    #[serde(deserialize_with = "de_ts")]
    pub begin: Timestamp,
    #[serde(default, deserialize_with = "de_opt_ts")]
    pub end: Option<Timestamp>,
    pub project: Ref,
    pub activity: Ref,
    #[serde(default)]
    pub description: Option<String>,
}

impl Timesheet {
    pub fn is_running(&self) -> bool {
        self.end.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Project {
    pub id: u64,
    pub name: String,
    #[serde(default, rename = "parentTitle")]
    pub customer: Option<String>,
    #[serde(default = "yes")]
    pub visible: bool,
    /// Colour (`#rrggbb`) if the project has one. Unvalidated.
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Activity {
    pub id: u64,
    pub name: String,
    #[serde(default = "yes")]
    pub visible: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct User {
    pub id: u64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Version {
    pub version: String,
}

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    pub project: u64,
    pub activity: u64,
    pub description: Option<String>,
}

/// New values for a finished timesheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryUpdate {
    pub project: u64,
    pub activity: u64,
    pub begin: Timestamp,
    pub end: Timestamp,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timesheet_with_ids_or_objects() {
        let a: Timesheet = serde_json::from_str(
            r#"{"id":5,"begin":"2024-05-01T10:00:00+0200","end":null,"project":3,"activity":4}"#,
        )
        .unwrap();
        assert_eq!(a.project.id, 3);
        assert!(a.is_running());
        let b: Timesheet = serde_json::from_str(
            r#"{"id":5,"begin":"2024-05-01T10:00:00+0200","end":null,"description":null,
                "project":{"id":3,"name":"Web","customer":{"id":1,"name":"ACME"}},
                "activity":{"id":4,"name":"Dev","parentTitle":"Web"},"tags":[],"extra":1}"#,
        )
        .unwrap();
        assert_eq!(b.project.name.as_deref(), Some("Web"));
        assert_eq!(b.project.parent.as_deref(), Some("ACME"));
        assert_eq!(b.activity.parent.as_deref(), Some("Web"));
        let c: Project =
            serde_json::from_str(r##"{"id":1,"name":"P","color":"#3584e4"}"##).unwrap();
        assert_eq!(c.color.as_deref(), Some("#3584e4"));
        let d: Project = serde_json::from_str(r#"{"id":1,"name":"P","color":null}"#).unwrap();
        assert_eq!(d.color, None);
    }

    #[test]
    fn broken_timesheet_is_an_error() {
        assert!(serde_json::from_str::<Timesheet>(
            r#"{"id":1,"begin":"nope","project":1,"activity":1}"#
        )
        .is_err());
    }
}
