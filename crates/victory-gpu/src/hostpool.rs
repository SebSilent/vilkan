//! A small bounded pool that compiles host pipelines off the render path.
//!
//! Compilation is kicked at pipeline creation (`hand_out`), not at first
//! use, so by bind time the result is usually ready and the Subrender
//! workers never sleep on a synchronous `Create*PipelineState`. The pool is
//! bounded (a few threads, a bounded queue) so five render workers plus
//! compilation do not oversubscribe the host. Mechanism after five-star
//! (GPL); implementation ours.
//!
//! Two invariants this module guarantees:
//!  * **Completion on every exit path.** A submitted job ALWAYS sets its
//!    `HostSlot` — on success, on a compilation error (`None`), and on a
//!    panic (`catch_unwind` → `None`). A bounded waiter is therefore always
//!    released; a job can never die leaving the slot `Compiling` forever
//!    (the XAsync wait-forever bug). On shutdown the sender drops and the
//!    workers drain every queued job (each completes its slot) before exit.
//!  * **Never block the title's thread.** Enqueue is `try_send`; a full or
//!    closed queue falls back to inline compilation at first use (counted),
//!    never a blocking `send` on `CreatePipelineState`.
#![cfg(windows)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};

use crate::pipeline::host::HostPipeline;
use crate::pipeline::HostSlot;

type Job = Box<dyn FnOnce() + Send + 'static>;

static POOL: OnceLock<Mutex<Option<SyncSender<Job>>>> = OnceLock::new();

const THREADS: usize = 3;
const QUEUE: usize = 256;

// Diagnostics, reported in the fps line. Relaxed: counters only.
/// Jobs accepted onto the queue.
pub(crate) static QUEUED: AtomicUsize = AtomicUsize::new(0);
/// Jobs that finished and set their slot (success or compilation error).
pub(crate) static COMPLETED: AtomicUsize = AtomicUsize::new(0);
/// Jobs that panicked (slot still set to `None`; counted as a failure).
pub(crate) static FAILED: AtomicUsize = AtomicUsize::new(0);
/// Enqueues that fell back to inline-at-first-use (queue full/closed).
pub(crate) static FALLBACK: AtomicUsize = AtomicUsize::new(0);
/// High-water mark of in-flight jobs (queued + running).
pub(crate) static MAX_DEPTH: AtomicUsize = AtomicUsize::new(0);
/// Current in-flight jobs.
static DEPTH: AtomicUsize = AtomicUsize::new(0);
/// Workers actually spawned. If zero (spawn failure), every submit falls
/// back to inline so no job is queued without a worker to complete it.
static WORKERS: AtomicUsize = AtomicUsize::new(0);

fn sender() -> &'static Mutex<Option<SyncSender<Job>>> {
    POOL.get_or_init(|| {
        let (tx, rx) = sync_channel::<Job>(QUEUE);
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..THREADS {
            let rx = Arc::clone(&rx);
            let spawned = std::thread::Builder::new()
                .name("vilkan-pso".into())
                .spawn(move || loop {
                    // Receive one job at a time; the lock is released while
                    // the job runs so the other workers stay busy.
                    let job = {
                        let g = rx.lock().unwrap_or_else(|e| e.into_inner());
                        match g.recv() {
                            Ok(j) => j,
                            // Queue closed and drained: every queued job has
                            // run (and set its slot), so no waiter is left.
                            Err(_) => return,
                        }
                    };
                    // The wrapped job sets its slot on EVERY path (see
                    // `submit`); running it here cannot leave a waiter hung.
                    job();
                    DEPTH.fetch_sub(1, Ordering::Relaxed);
                });
            if spawned.is_ok() {
                WORKERS.fetch_add(1, Ordering::Relaxed);
            }
        }
        Mutex::new(Some(tx))
    })
}

/// Queue a compilation that produces `slot`'s result. Non-blocking: a full
/// or closed queue marks the slot for inline compilation at first use
/// (`FALLBACK`), never blocks the calling (title) thread. The wrapped job
/// guarantees the slot is set on success, error and panic alike.
pub(crate) fn submit(
    inner: impl FnOnce() -> Option<HostPipeline> + Send + 'static,
    slot: Arc<HostSlot>,
) {
    let job_slot = Arc::clone(&slot);
    let wrapped: Job = Box::new(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(inner));
        match result {
            Ok(res) => {
                COMPLETED.fetch_add(1, Ordering::Relaxed);
                job_slot.complete(res);
            }
            Err(_) => {
                // A panicking compilation still releases the waiter.
                FAILED.fetch_add(1, Ordering::Relaxed);
                job_slot.complete(None);
            }
        }
    });
    let g = sender().lock().unwrap_or_else(|e| e.into_inner());
    let Some(tx) = g.as_ref() else {
        // Pool shut down: compile inline at first use.
        FALLBACK.fetch_add(1, Ordering::Relaxed);
        slot.abandon();
        return;
    };
    if WORKERS.load(Ordering::Relaxed) == 0 {
        // No worker could be spawned: never queue a job nothing will run.
        FALLBACK.fetch_add(1, Ordering::Relaxed);
        slot.abandon();
        return;
    }
    let depth = DEPTH.fetch_add(1, Ordering::Relaxed) + 1;
    MAX_DEPTH.fetch_max(depth, Ordering::Relaxed);
    match tx.try_send(wrapped) {
        Ok(()) => {
            QUEUED.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            DEPTH.fetch_sub(1, Ordering::Relaxed);
            FALLBACK.fetch_add(1, Ordering::Relaxed);
            slot.abandon();
        }
    }
}

/// Drop the sender so the workers drain every queued job (each sets its
/// slot) and then exit. Called on host shutdown; releases all waiters.
pub(crate) fn shutdown() {
    if let Some(pool) = POOL.get() {
        let mut g = pool.lock().unwrap_or_else(|e| e.into_inner());
        // Dropping the sender closes the channel after the buffered jobs.
        g.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // A job that returns a compilation error must still set the slot and
    // release a bounded waiter.
    #[test]
    fn completion_on_error_releases_waiter() {
        let slot = Arc::new(HostSlot::new());
        submit(|| None, Arc::clone(&slot));
        // The pool runs the job; the waiter must be released, not hang.
        let _ = slot.wait(Duration::from_secs(10));
        assert!(slot.done(), "error job must complete the slot");
    }

    // A panicking job must set the slot (None) and release the waiter.
    #[test]
    fn completion_on_panic_releases_waiter() {
        let slot = Arc::new(HostSlot::new());
        submit(|| panic!("compile boom"), Arc::clone(&slot));
        let _ = slot.wait(Duration::from_secs(10));
        assert!(slot.done(), "panicking job must complete the slot");
        assert!(FAILED.load(Ordering::Relaxed) >= 1);
    }

    // Shutdown with jobs still queued must drain them so every waiter is
    // released (no slot left Compiling).
    #[test]
    fn shutdown_releases_waiters() {
        let slots: Vec<Arc<HostSlot>> = (0..8).map(|_| Arc::new(HostSlot::new())).collect();
        for s in &slots {
            submit(|| None, Arc::clone(s));
        }
        shutdown();
        for s in &slots {
            let _ = s.wait(Duration::from_secs(10));
            assert!(s.done(), "shutdown must drain and complete every slot");
        }
    }
}
