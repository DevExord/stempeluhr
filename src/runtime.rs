// SPDX-License-Identifier: GPL-3.0-or-later
//! One shared Tokio runtime for all network and D-Bus work. Results travel
//! back to the GLib main loop by awaiting the join handle from
//! `glib::spawn_future_local`, so the UI thread never blocks.

use std::future::Future;
use std::sync::OnceLock;

use stempeluhr_core::kimai::{Error, ErrorKind};
use tokio::runtime::{Builder, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn init() -> std::io::Result<()> {
    if RUNTIME.get().is_none() {
        let rt = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("stempeluhr-net")
            .build()?;
        let _ = RUNTIME.set(rt);
    }
    Ok(())
}

/// Runs `fut` on the shared runtime and waits for it on the calling
/// (main-loop) task. A panic or cancellation inside the task is reported as
/// an error instead of taking the app down.
pub async fn spawn<T, F>(fut: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    let Some(rt) = RUNTIME.get() else {
        return Err(Error::new(ErrorKind::Other, "runtime not initialised"));
    };
    rt.spawn(fut)
        .await
        .map_err(|e| Error::new(ErrorKind::Other, format!("background task failed: {e}")))
}

/// Like [`spawn`] for futures that already return a kimai `Result`.
pub async fn spawn_result<T, F>(fut: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: Future<Output = Result<T, Error>> + Send + 'static,
{
    spawn(fut).await?
}

/// Fire-and-forget on the runtime (used by the global shortcut task).
pub fn detach<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    if let Some(rt) = RUNTIME.get() {
        rt.spawn(fut);
    }
}
