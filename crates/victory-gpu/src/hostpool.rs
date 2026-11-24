//! A small bounded pool that compiles host pipelines off the render path.
//!
//! Compilation is kicked at pipeline creation (`hand_out`), not at first
//! use, so by bind time the result is usually ready and the Subrender
//! workers never sleep on a synchronous `Create*PipelineState`. The pool is
//! bounded (a few threads, a bounded queue) so five render workers plus
//! compilation do not oversubscribe the host. Mechanism after five-star
//! (GPL); implementation ours.
#![cfg(windows)]

use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Mutex, OnceLock};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// The shared queue. `None` until the pool starts; the sender is kept so
/// workers live for the process. On shutdown the sender drops and the
/// workers drain then exit.
static POOL: OnceLock<Mutex<Option<SyncSender<Job>>>> = OnceLock::new();

const THREADS: usize = 3;
const QUEUE: usize = 256;

fn sender() -> &'static Mutex<Option<SyncSender<Job>>> {
    POOL.get_or_init(|| {
        let (tx, rx) = sync_channel::<Job>(QUEUE);
        let rx = std::sync::Arc::new(Mutex::new(rx));
        for _ in 0..THREADS {
            let rx = std::sync::Arc::clone(&rx);
            let _ = std::thread::Builder::new()
                .name("vilkan-pso".into())
                .spawn(move || loop {
                    // Receive one job at a time; the lock is released while
                    // the job runs so the other workers stay busy.
                    let job = {
                        let g = rx.lock().unwrap_or_else(|e| e.into_inner());
                        match g.recv() {
                            Ok(j) => j,
                            Err(_) => return, // queue closed: shutdown
                        }
                    };
                    // A panicking compilation must not kill a worker nor
                    // unwind into the render path; the slot stays Compiling
                    // and the bind times out (counted), never a crash.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                });
        }
        Mutex::new(Some(tx))
    })
}

/// Queue a compilation. Blocks only if the bounded queue is full (back-
/// pressure), never spawns a thread per pipeline.
pub(crate) fn submit(job: impl FnOnce() + Send + 'static) {
    let g = sender().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tx) = g.as_ref() {
        // A full queue means compilation is far behind; waiting here is the
        // bounded back-pressure, still off the critical render wait.
        let _ = tx.send(Box::new(job));
    }
}