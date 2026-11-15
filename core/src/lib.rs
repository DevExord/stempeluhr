// SPDX-License-Identifier: GPL-3.0-or-later
//! GTK-free core of the app: the Kimai API client, the tracking state
//! machine and the synchronisation engine. Everything here is testable
//! against a mock server.

pub mod engine;
pub mod kimai;
pub mod state;
