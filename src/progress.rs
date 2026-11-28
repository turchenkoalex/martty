//! The terminal's activity indicator while martty is busy: Ghostty's
//! progress bar, Rex's tab spinner (see `terminal/`). It shows only once the
//! work has taken longer than `DELAY`, so fast layouts don't flash it.

use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::terminal::{self, Status};

const DELAY: Duration = Duration::from_millis(150);

/// Shows the indicator while alive (after `DELAY`) and removes it on drop.
pub struct Busy {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Busy {
    /// `what` is one line saying what martty is doing, such as
    /// `Downloading README.md`.
    pub fn start(what: &str) -> Self {
        if !io::stdout().is_terminal() {
            return Self {
                stop: None,
                thread: None,
            };
        }
        let term = terminal::current();
        let busy = term.status(Status::Working(what));
        let idle = term.status(Status::Idle);
        let (stop, wait) = mpsc::channel::<()>();
        // Nothing else writes to stdout while martty is busy, so the two
        // writes can't land inside another escape sequence.
        let thread = thread::spawn(move || {
            if wait.recv_timeout(DELAY) == Err(RecvTimeoutError::Timeout) {
                let _ = write(&busy);
                // Returns once `stop` is dropped.
                let _ = wait.recv();
                let _ = write(&idle);
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.stop = None;
        // Joined so the indicator is gone before the caller draws.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Tells the terminal that martty stopped on `error`, right before it exits.
pub fn failed(error: &str) {
    if io::stdout().is_terminal() {
        let _ = write(&terminal::current().status(Status::Failed(error)));
    }
}

fn write(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()
}
