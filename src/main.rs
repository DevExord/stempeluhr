// SPDX-License-Identifier: GPL-3.0-or-later
#![recursion_limit = "256"]

mod application;
mod config;
mod connect_dialog;
#[cfg(debug_assertions)]
mod debug_shot;
mod edit_dialog;
mod format;
mod history;
mod i18n;
mod messages;
mod preferences;
mod ring;
mod runtime;
mod secrets;
mod shortcuts;
mod window;

use gettextrs::{bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory};
use gtk::prelude::*;
use gtk::{gio, glib};
use tracing_subscriber::EnvFilter;

use crate::application::Application;

fn init_logging() {
    // Structured logging, enabled with e.g. RUST_LOG=stempeluhr=debug.
    // Requests are logged by method and path only — never headers, so the
    // token cannot leak.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
    std::panic::set_hook(Box::new(|info| {
        tracing::error!(panic = %info, "unexpected panic");
    }));
}

fn init_i18n() {
    setlocale(LocaleCategory::LcAll, "");
    let localedir =
        std::env::var("STEMPELUHR_LOCALEDIR").unwrap_or_else(|_| config::LOCALEDIR.to_owned());
    if let Err(e) = bindtextdomain(config::GETTEXT_PACKAGE, localedir) {
        tracing::warn!("bindtextdomain failed: {e}");
    }
    let _ = bind_textdomain_codeset(config::GETTEXT_PACKAGE, "UTF-8");
    let _ = textdomain(config::GETTEXT_PACKAGE);
}

fn load_resources() -> Result<(), glib::Error> {
    let file = std::env::var("STEMPELUHR_RESOURCES_FILE")
        .unwrap_or_else(|_| config::RESOURCES_FILE.to_owned());
    let res = gio::Resource::load(&file)?;
    gio::resources_register(&res);
    Ok(())
}

fn fatal(msg: &str) -> glib::ExitCode {
    eprintln!("stempeluhr: {msg}");
    glib::ExitCode::FAILURE
}

fn main() -> glib::ExitCode {
    init_logging();
    init_i18n();

    if let Err(e) = runtime::init() {
        return fatal(&format!("cannot start the network runtime: {e}"));
    }
    if let Err(e) = load_resources() {
        return fatal(&format!("cannot load resources: {e}"));
    }
    // `gio::Settings::new` aborts the process on a missing schema; check first.
    let schema_ok = gio::SettingsSchemaSource::default()
        .and_then(|s| s.lookup(config::APP_ID, true))
        .is_some();
    if !schema_ok {
        return fatal("the GSettings schema is not installed (run `glib-compile-schemas`, or use `meson devenv`)");
    }

    glib::set_application_name("Stempeluhr");
    glib::set_prgname(Some("stempeluhr"));
    Application::new().run()
}
