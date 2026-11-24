//! A handle a long check reports through and can be cancelled with. The
//! check runs on whatever thread the caller likes (the TUI uses a worker so
//! the screen stays live); this is the only thing the two share.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
}

thread_local! {
    /// The job the current thread is checking for, so the site and id.loc.gov
    /// clients deep down can give up on Esc instead of finishing their retries.
    static CURRENT: RefCell<Option<Arc<Progress>>> = const { RefCell::new(None) };
}

/// While the returned guard lives, network calls on this thread watch `job`.
pub fn enter(job: Arc<Progress>) -> impl Drop {
    struct Guard(Option<Arc<Progress>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let prev = self.0.take();
            CURRENT.with(|c| *c.borrow_mut() = prev);
        }
    }
    Guard(CURRENT.with(|c| c.borrow_mut().replace(job)))
}

/// The job this thread works for, to hand to helper threads it starts.
pub fn current() -> Option<Arc<Progress>> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Is this thread working for a job (a screen that can stop it)?
pub fn has_job() -> bool {
    CURRENT.with(|c| c.borrow().is_some())
}

/// Has the job this thread works for been cancelled?
pub fn cancelled_here() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(|p| p.cancelled()))
}

/// Sleep, waking early on cancel. False when cancelled.
pub fn sleep(d: Duration) -> bool {
    let end = Instant::now() + d;
    loop {
        if cancelled_here() {
            return false;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_wakes_on_cancel() {
        let job = Arc::new(Progress::default());
        let _g = enter(job.clone());
        assert!(sleep(Duration::from_millis(10)));
        job.cancel();
        let t = Instant::now();
        assert!(!sleep(Duration::from_secs(30)));
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}
