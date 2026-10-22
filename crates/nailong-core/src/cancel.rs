//! Process-wide cancellation flag for the single foreground install task.
//!
//! nailong allows one install at a time. A global flag keeps cancellation out
//! of every download and processor signature; callers reset it before a new
//! task and the UI sets it when the user presses Cancel.

use crate::error::{Error, Result};
use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);

pub fn reset() {
    CANCELLED.store(false, Ordering::SeqCst);
}

pub fn cancel() {
    CANCELLED.store(true, Ordering::SeqCst);
}

pub fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

pub fn check() -> Result<()> {
    if is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_and_cancel_round_trip() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reset();
        assert!(!is_cancelled());
        assert!(check().is_ok());
        cancel();
        assert!(is_cancelled());
        assert!(matches!(check(), Err(Error::Cancelled)));
        reset();
        assert!(!is_cancelled());
    }
}
