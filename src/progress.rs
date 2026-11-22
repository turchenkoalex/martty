//! Ghostty's progress indicator (OSC 9;4) while a slow layout renders,
//! mostly diagrams. It shows only once the layout has taken longer than
//! `DELAY`, so fast layouts don't flash it.

use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DELAY: Duration = Duration::from_millis(150);
/// OSC 9;4 state 3: indeterminate.
const BUSY: &[u8] = b"\x1b]9;4;3\x1b\\";
/// OSC 9;4 state 0: remove the indicator.
const DONE: &[u8] = b"\x1b]9;4;0\x1b\\";

/// Shows the indicator while alive (after `DELAY`) and removes it on drop.
pub struct Busy {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Busy {
    pub fn start() -> Self {
        if !io::stdout().is_terminal() {
            return Self {
                stop: None,
                thread: None,
            };
        }
        let (stop, wait) = mpsc::channel::<()>();
        // Nothing else writes to stdout while a layout renders, so the two
        // writes can't land inside another escape sequence.
        let thread = thread::spawn(move || {
            if wait.recv_timeout(DELAY) == Err(RecvTimeoutError::Timeout) {
                let _ = write(BUSY);
                // Returns once `stop` is dropped.
                let _ = wait.recv();
                let _ = write(DONE);
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

fn write(bytes: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()
}
