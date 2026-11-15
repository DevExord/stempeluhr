// SPDX-License-Identifier: GPL-3.0-or-later
//! The ring around the running timer: a faint track and an arc that shows the
//! seconds of the current minute. Drawn with Cairo in colours taken from the
//! theme (foreground, accent) or the project colour.

use std::cell::Cell;
use std::f64::consts::{FRAC_PI_2, TAU};
use std::rc::Rc;

use gtk::prelude::*;

pub type Rgb = (f64, f64, f64);

#[derive(Default)]
pub struct RingState {
    /// 0.0‥1.0: fraction of the current minute.
    pub progress: Cell<f64>,
    /// 0.0‥1.0: fade/scale-in when tracking starts.
    pub reveal: Cell<f64>,
    /// Unconfirmed: drawn faint.
    pub dim: Cell<bool>,
    /// Project colour (already contrast-adjusted); `None` = accent colour.
    pub color: Cell<Option<Rgb>>,
    /// 0.0 = none; 0.0‥1.0 while the "stamp" ripple expands after stopping.
    pub ripple: Cell<f64>,
}

pub fn install(area: &gtk::DrawingArea) -> Rc<RingState> {
    let state = Rc::new(RingState::default());
    state.reveal.set(1.0);
    let st = state.clone();
    area.set_draw_func(move |area, cr, w, h| {
        let (w, h) = (f64::from(w), f64::from(h));
        let reveal = st.reveal.get().clamp(0.0, 1.0);
        let fade = reveal * if st.dim.get() { 0.35 } else { 1.0 };
        let radius = (w.min(h) / 2.0 - 7.0) * (0.9 + 0.1 * reveal);
        let (cx, cy) = (w / 2.0, h / 2.0);

        cr.set_line_width(6.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);

        let fg = area.color();
        cr.set_source_rgba(
            f64::from(fg.red()),
            f64::from(fg.green()),
            f64::from(fg.blue()),
            0.10 * reveal,
        );
        cr.arc(cx, cy, radius, 0.0, TAU);
        let _ = cr.stroke();

        let (r, g, b) = st.color.get().unwrap_or_else(|| {
            let a = adw::StyleManager::default().accent_color_rgba();
            (
                f64::from(a.red()),
                f64::from(a.green()),
                f64::from(a.blue()),
            )
        });
        cr.set_source_rgba(r, g, b, fade);
        // Never fully empty: a dot at twelve o'clock right after the minute turns.
        let sweep = (TAU * st.progress.get().clamp(0.0, 1.0)).max(0.001);
        cr.arc(cx, cy, radius, -FRAC_PI_2, -FRAC_PI_2 + sweep);
        let _ = cr.stroke();

        // The stamp: one soft ripple leaving the ring.
        let t = st.ripple.get();
        if t > 0.0 && t < 1.0 {
            cr.set_line_width(2.5);
            cr.set_source_rgba(r, g, b, 0.45 * (1.0 - t));
            cr.arc(cx, cy, radius + 2.0 + 4.0 * t, 0.0, TAU);
            let _ = cr.stroke();
        }
    });
    state
}

/// Accepts only `#rgb` / `#rrggbb`. Anything else (CSS injection included)
/// is rejected.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#')?;
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize, len: usize| -> Option<f64> {
        let part = &h[i..i + len];
        let n = u8::from_str_radix(part, 16).ok()?;
        Some(
            if len == 1 {
                f64::from(n * 17)
            } else {
                f64::from(n)
            } / 255.0,
        )
    };
    match h.len() {
        3 => Some((v(0, 1)?, v(1, 1)?, v(2, 1)?)),
        6 => Some((v(0, 2)?, v(2, 2)?, v(4, 2)?)),
        _ => None,
    }
}

/// Keeps a project colour usable: very light colours get darker on a light
/// theme, very dark ones lighter on a dark theme.
pub fn tune_for_theme((r, g, b): Rgb, dark: bool) -> Rgb {
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let mix = |c: f64, t: f64, k: f64| c * (1.0 - k) + t * k;
    if !dark && lum > 0.6 {
        (mix(r, 0.0, 0.4), mix(g, 0.0, 0.4), mix(b, 0.0, 0.4))
    } else if dark && lum < 0.25 {
        (mix(r, 1.0, 0.45), mix(g, 1.0, 0.45), mix(b, 1.0, 0.45))
    } else {
        (r, g, b)
    }
}

pub fn to_css((r, g, b): Rgb) -> String {
    let c = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_strict() {
        assert!(parse_hex("#3584e4").is_some());
        assert!(parse_hex("#fff").is_some());
        for bad in [
            "3584e4",
            "#35 84e4",
            "#3584e4; } * { x",
            "red",
            "#12",
            "#gggggg",
            "#3584e4ff",
            "",
        ] {
            assert!(parse_hex(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn light_colours_are_darkened_on_light_theme() {
        let yellow = parse_hex("#ffff00").unwrap_or_default();
        let tuned = tune_for_theme(yellow, false);
        assert!(tuned.0 < yellow.0);
        assert_eq!(tune_for_theme(yellow, true), yellow);
    }
}
