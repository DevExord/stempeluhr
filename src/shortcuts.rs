// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional global shortcut through the GlobalShortcuts portal. It only
//! works while the app runs; nothing is registered otherwise.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};

use crate::i18n::tr;
use crate::runtime;

pub const SHORTCUT_ID: &str = "toggle";

/// Starts the portal session. Dropping the returned sender stops it. Every
/// activation of the shortcut produces one message on the receiver.
pub fn start() -> (oneshot::Sender<()>, mpsc::UnboundedReceiver<()>) {
    let (stop_tx, stop_rx) = oneshot::channel();
    let (tx, rx) = mpsc::unbounded_channel();
    runtime::detach(async move {
        if let Err(e) = run(tx, stop_rx).await {
            tracing::warn!(error = %e, "global shortcut unavailable");
        }
    });
    (stop_tx, rx)
}

async fn run(
    tx: mpsc::UnboundedSender<()>,
    mut stop: oneshot::Receiver<()>,
) -> Result<(), ashpd::Error> {
    if let Ok(id) = crate::config::APP_ID.try_into() {
        // Needed for portal permissions when not running in a sandbox.
        let _ = ashpd::register_host_app(id).await;
    }
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(Default::default()).await?;
    let shortcut = NewShortcut::new(SHORTCUT_ID, tr("Toggle time tracking"));
    portal
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;
    let mut activated = portal.receive_activated().await?;
    loop {
        tokio::select! {
            item = activated.next() => match item {
                Some(a) if a.shortcut_id() == SHORTCUT_ID => {
                    if tx.send(()).is_err() {
                        break;
                    }
                }
                Some(_) => {}
                None => break,
            },
            _ = &mut stop => break,
        }
    }
    session.close().await
}
