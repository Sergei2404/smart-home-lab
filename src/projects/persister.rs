use super::store::ProjectStore;
use crate::lab::Persisted;
use std::sync::Arc;
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// Saves the latest state whenever it changes (bursts collapse into one write), to the file of the
/// project each payload names. Cancelling `stop` makes it write once more and finish, so awaiting the
/// handle means the last state is on disk and no write is in flight.
pub fn spawn_persister(
    store: Arc<ProjectStore>,
    mut snapshots: watch::Receiver<Persisted>,
    stop: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let stopping = tokio::select! {
                changed = snapshots.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    false
                }
                () = stop.cancelled() => true,
            };
            let latest = || {
                let Persisted { project, snapshot } = snapshots.borrow_and_update().clone();
                (project, snapshot)
            };
            if let Err(error) = store.save_latest(latest).await {
                tracing::error!(error = ?error, "failed to persist state");
            }
            if stopping {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests;
