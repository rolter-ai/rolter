//! Shutdown handle shared by the gateway's background sinks (#1924).
//!
//! The request-log writer, the health-event writer and the usage-recording
//! workers all batch or queue work on a channel and only finish it when every
//! sender is dropped. At shutdown that never happens: `AppState` clones live on
//! in the prober, scraper and watcher tasks, so the runtime used to drop the
//! writers mid-batch and lose up to a flush window of rows.
//!
//! A [`SinkTasks`] pairs a [`CancellationToken`] with the join handles of the
//! tasks it stops. Cancelling it makes each task close its receiver, which still
//! yields everything already queued and then `None`, so the ordinary "senders
//! gone" path flushes the remainder and exits. [`SinkTasks::stop`] then awaits
//! the handles.

use parking_lot::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The stop signal and join handles for one sink's background tasks.
#[derive(Default)]
pub(crate) struct SinkTasks {
    stop: CancellationToken,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl SinkTasks {
    /// A token the sink's tasks select on; cancelled by [`SinkTasks::stop`].
    pub(crate) fn token(&self) -> CancellationToken {
        self.stop.clone()
    }

    /// Register a spawned task so [`SinkTasks::stop`] can wait for it.
    pub(crate) fn track(&self, handle: JoinHandle<()>) {
        self.handles.lock().push(handle);
    }

    /// Ask every tracked task to flush what it holds and stop, and wait until
    /// they have. Unbounded by itself: the caller wraps it in a timeout so a dead
    /// ClickHouse or Redis cannot hold the process open.
    pub(crate) async fn stop(&self) {
        self.stop.cancel();
        let handles = std::mem::take(&mut *self.handles.lock());
        for handle in handles {
            // a panicked writer has nothing left to flush
            let _ = handle.await;
        }
    }
}
