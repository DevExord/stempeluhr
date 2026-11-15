// SPDX-License-Identifier: GPL-3.0-or-later
//! Turns errors into friendly, translated sentences. Raw error text never
//! reaches the UI.

use stempeluhr_core::kimai::{Error, ErrorKind};

use crate::i18n::{tr, tr_f};

pub fn describe(e: &Error) -> String {
    match &e.kind {
        ErrorKind::Offline => tr("Can’t connect to the server. Check your internet connection."),
        ErrorKind::Dns => tr(
            "The server address couldn’t be found. Check the address and your internet connection.",
        ),
        ErrorKind::Timeout => tr("The server took too long to respond."),
        ErrorKind::Tls => tr("The server’s security certificate couldn’t be verified."),
        ErrorKind::Unauthorized => {
            tr("The access token was rejected. It may be invalid or revoked.")
        }
        ErrorKind::NotFound => tr("Nothing was found at this address. Check the server address."),
        ErrorKind::Validation(msgs) if !msgs.is_empty() => tr_f(
            "Kimai rejected the request: {details}",
            &[("details", &msgs.join(" "))],
        ),
        ErrorKind::Validation(_) => tr("Kimai rejected the request."),
        ErrorKind::Redirect => tr(
            "The server redirected the request. Check the server address, preferably use https://.",
        ),
        ErrorKind::Server(code) => tr_f(
            "The server reported an internal error ({code}). Try again later.",
            &[("code", &code.to_string())],
        ),
        ErrorKind::Client(code) => tr_f(
            "The server refused the request ({code}).",
            &[("code", &code.to_string())],
        ),
        ErrorKind::UnexpectedResponse => {
            tr("The server’s answer wasn’t understood. Is this the address of a Kimai server?")
        }
        ErrorKind::Other => tr("Something went wrong while contacting the server."),
    }
}
