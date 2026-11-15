// SPDX-License-Identifier: GPL-3.0-or-later
//! Development aid, compiled into debug builds only: with
//! `STEMPELUHR_SCREENSHOT=/some/prefix` every open window is rendered to
//! `/some/prefix-N[-wK].png` every two seconds (`STEMPELUHR_SCREENSHOT_MS`
//! changes the interval).

use adw::subclass::prelude::ObjectSubclassIsExt;
use gtk::prelude::*;
use gtk::{gdk, glib};

fn shoot(win: &gtk::Window, path: &str) {
    let (w, h) = (win.width(), win.height());
    if w <= 0 || h <= 0 {
        return;
    }
    let paintable = gtk::WidgetPaintable::new(Some(win));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
    let renderer = win.native().and_then(|n| n.renderer());
    if let (Some(node), Some(renderer)) = (snapshot.to_node(), renderer) {
        let texture: gdk::Texture = renderer.render_texture(&node, None);
        let _ = texture.save_to_png(path);
    }
}

pub fn install(win: &crate::window::StempeluhrWindow) {
    let Ok(prefix) = std::env::var("STEMPELUHR_SCREENSHOT") else {
        return;
    };
    let win = win.downgrade();
    let mut n = 0u32;
    let ms = std::env::var("STEMPELUHR_SCREENSHOT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000u64);
    glib::timeout_add_local(std::time::Duration::from_millis(ms), move || {
        let Some(win) = win.upgrade() else {
            return glib::ControlFlow::Break;
        };
        n += 1;
        shoot(win.upcast_ref(), &format!("{prefix}-{n}.png"));
        if let Some(pop) = win.imp().recent_button.popover() {
            if pop.is_visible() && pop.width() > 0 {
                let paintable = gtk::WidgetPaintable::new(Some(&pop));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, f64::from(pop.width()), f64::from(pop.height()));
                let renderer = pop.native().and_then(|n| n.renderer());
                if let (Some(node), Some(renderer)) = (snapshot.to_node(), renderer) {
                    let texture: gdk::Texture = renderer.render_texture(&node, None);
                    let _ = texture.save_to_png(format!("{prefix}-{n}-popover.png"));
                }
            }
        }
        for (k, other) in gtk::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Window>().ok())
            .filter(|w| w.is_visible() && w != win.upcast_ref::<gtk::Window>())
            .enumerate()
        {
            shoot(&other, &format!("{prefix}-{n}-w{k}.png"));
        }
        glib::ControlFlow::Continue
    });
}
