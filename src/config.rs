// SPDX-License-Identifier: GPL-3.0-or-later
//! Build-time configuration. Meson passes the real values as environment
//! variables to Cargo; the defaults allow plain `cargo check`.

pub const APP_ID: &str = match option_env!("STEMPELUHR_APP_ID") {
    Some(v) => v,
    None => "digital.fischers.stempeluhr",
};
pub const VERSION: &str = match option_env!("STEMPELUHR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
pub const GETTEXT_PACKAGE: &str = match option_env!("STEMPELUHR_GETTEXT_PACKAGE") {
    Some(v) => v,
    None => "stempeluhr",
};
pub const LOCALEDIR: &str = match option_env!("STEMPELUHR_LOCALEDIR") {
    Some(v) => v,
    None => "/usr/share/locale",
};
pub const RESOURCES_FILE: &str = match option_env!("STEMPELUHR_RESOURCES_FILE") {
    Some(v) => v,
    None => "/usr/share/stempeluhr/stempeluhr.gresource",
};
pub const RESOURCE_PREFIX: &str = "/digital/fischers/stempeluhr";
