//! A handle a long check reports through and can be cancelled with. The
//! check runs on whatever thread the caller likes (the TUI uses a worker so
//! the screen stays live); this is the only thing the two share.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub struct Progress {
    msg: Mutex<String>,
    cancel: AtomicBool,
}

impl Progress {
    /// What the check is doing right now ("fetching vocabulary subject").
    pub fn say(&self, msg: impl Into<String>) {
        if let Ok(mut m) = self.msg.lock() {
            *m = msg.into();
        }
    }
    pub fn message(&self) -> String {
        self.msg.lock().map(|m| m.clone()).unwrap_or_default()
    }
    /// Ask the running check to stop at the next row.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// Ready for the next job.
    pub fn reset(&self) {
        self.cancel.store(false, Ordering::Relaxed);
        self.say("");
    }
}
