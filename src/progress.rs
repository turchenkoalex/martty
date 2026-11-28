//! The terminal's activity indicator while martty is busy: Ghostty's
//! progress bar, Rex's tab spinner (see `terminal/`). It shows only once the
//! work has taken longer than `DELAY`, so fast layouts don't flash it.

use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::terminal::{self, Status};

const DELAY: Duration = Duration::from_millis(150);

/// Shows the indicator while alive (after `DELAY`) and removes it on drop.
pub struct Busy {
    /// Percentages for the indicator; dropping it stops the thread.
    progress: Option<Sender<u8>>,
    thread: Option<JoinHandle<()>>,
}

impl Busy {
    /// `what` is one line saying what martty is doing, such as
    /// `Downloading README.md`.
    pub fn start(what: &str) -> Self {
        if !io::stdout().is_terminal() {
            return Self {
                progress: None,
                thread: None,
            };
        }
        let what = what.to_owned();
        let (progress, updates) = mpsc::channel::<u8>();
        // Nothing else writes to stdout while martty is busy, so these
        // writes can't land inside another escape sequence.
        let thread = thread::spawn(move || {
            let term = terminal::current();
            let report = |progress| {
                let what = &what;
                write(&term.status(Status::Working { what, progress }))
            };
            // Percentages sent before `DELAY` only set where the indicator starts.
            let deadline = Instant::now() + DELAY;
            let mut shown = None;
            loop {
                match updates.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(p) => shown = Some(p),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            let _ = report(shown);
            // Ends once the `Busy` is dropped.
            while let Ok(p) = updates.recv() {
                if shown != Some(p) {
                    shown = Some(p);
                    let _ = report(shown);
                }
            }
            let _ = write(&term.status(Status::Idle));
        });
        Self {
            progress: Some(progress),
            thread: Some(thread),
        }
    }

    /// How much of the work is done, in percent.
    pub fn progress(&self, percent: u8) {
        if let Some(progress) = &self.progress {
            let _ = progress.send(percent.min(100));
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.progress = None;
        // Joined so the indicator is gone before the caller draws.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Tells the terminal what happened outside any busy stretch: news to look
/// at, or the error martty stops on.
pub fn report(status: Status<'_>) {
    if io::stdout().is_terminal() {
        let _ = write(&terminal::current().status(status));
    }
}

fn write(text: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()
}
