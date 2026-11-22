//! Panics in the libraries that parse untrusted input (merman, usvg, resvg,
//! latex-rust, the image decoders) must not take the viewer down: `catch`
//! turns one into "can't show this", and the block falls back to text.
//!
//! A panic nothing catches while the pager is up would print its message
//! onto the alternate screen, which leaving it erases; the message is kept
//! instead and printed once the terminal is restored.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    /// Inside `catch` on this thread: the panic is expected and handled.
    static CAUGHT: Cell<bool> = const { Cell::new(false) };
}

static ALTERNATE_SCREEN: AtomicBool = AtomicBool::new(false);
static DEFERRED: Mutex<Option<String>> = Mutex::new(None);

/// Replaces the panic hook: silent for caught panics, deferred while the
/// pager owns the screen, the default one otherwise.
pub fn install() {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if CAUGHT.get() {
            return;
        }
        if ALTERNATE_SCREEN.load(Ordering::Relaxed) {
            if let Ok(mut deferred) = DEFERRED.lock() {
                deferred.get_or_insert_with(|| info.to_string());
            }
            return;
        }
        default(info);
    }));
}

/// Runs `f`, turning a panic into `None`.
pub fn catch<T>(f: impl FnOnce() -> Option<T>) -> Option<T> {
    let outer = CAUGHT.replace(true);
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    CAUGHT.set(outer);
    result.ok().flatten()
}

/// Whether the pager's alternate screen is up.
pub fn set_alternate_screen(up: bool) {
    ALTERNATE_SCREEN.store(up, Ordering::Relaxed);
}

/// The message of a panic that happened on the alternate screen.
pub fn take_deferred() -> Option<String> {
    DEFERRED.lock().ok()?.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_should_turn_panic_into_none() {
        let got: Option<u8> = catch(|| panic!("boom"));
        assert_eq!(got, None);
        assert_eq!(catch(|| Some(1)), Some(1));
    }
}
