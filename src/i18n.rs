// SPDX-License-Identifier: GPL-3.0-or-later
//! Thin gettext helpers. Source strings are English.

use gettextrs::{gettext, ngettext};

/// Translates `msgid`.
pub fn tr(msgid: &str) -> String {
    gettext(msgid)
}

/// Translates `msgid` and substitutes `{name}` placeholders.
pub fn tr_f(msgid: &str, args: &[(&str, &str)]) -> String {
    substitute(gettext(msgid), args)
}

/// Plural-aware variant of [`tr_f`]; `{n}` is always available.
pub fn ntr_f(singular: &str, plural: &str, n: u32, args: &[(&str, &str)]) -> String {
    let n_text = n.to_string();
    let mut all: Vec<(&str, &str)> = vec![("n", n_text.as_str())];
    all.extend_from_slice(args);
    substitute(ngettext(singular, plural, n), &all)
}

fn substitute(mut s: String, args: &[(&str, &str)]) -> String {
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}
