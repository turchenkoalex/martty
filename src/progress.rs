//! The terminal's activity indicator while martty is busy: Ghostty's
//! progress bar, Rex's tab spinner (see `terminal/`). It shows only once the
//! work has taken longer than `DELAY`, so fast layouts don't flash it.
//!
//! Busy stretches nest (a layout downloads images and draws diagrams), so
//! there is one indicator for the process, showing the innermost stretch:
//! its message and, when known, its percentage. When that one ends, the one
//! around it shows again.
//!
//! Reports are queued under this module's lock, so they keep the order of
//! the changes, and one writer thread puts them on stdout. Only that thread
//! ever waits for stdout: a worker reporting its share while some code holds
//! stdout's lock across the work goes on, where writing itself it would
//! wait for the holder, which waits for the worker, for ever. Each report is
//! one locked write, so it can't land inside another escape sequence.
//! `flush` lets the queue drain before martty exits.

use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use crate::terminal::{self, Status, Terminal};

const DELAY: Duration = Duration::from_millis(150);
/// How long `flush` waits for the writer, should stdout stay locked.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(1);

/// A stretch of work shown by the indicator while alive; dropping it ends it.
pub struct Busy {
    /// `None` when stdout is no terminal: nothing to show.
    id: Option<u64>,
}

struct Task {
    id: u64,
    what: String,
    progress: Option<u8>,
}

struct State {
    /// Stretches under way, innermost last.
    tasks: Vec<Task>,
    next_id: u64,
    /// Counts the times the indicator went from idle to busy, so a timer
    /// left from an earlier stretch can tell it is stale.
    episode: u64,
    /// What the terminal shows now; `None` while the indicator is off.
    shown: Option<(String, Option<u8>)>,
}

fn lock() -> MutexGuard<'static, State> {
    static STATE: Mutex<State> = Mutex::new(State {
        tasks: Vec::new(),
        next_id: 0,
        episode: 0,
        shown: None,
    });
    // A panic while writing a report leaves the state as usable as before.
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

impl State {
    /// The report of the innermost stretch, if the indicator is on and it
    /// changed; empty otherwise.
    fn changes(&mut self, term: &dyn Terminal) -> String {
        if self.shown.is_none() {
            return String::new();
        }
        let Some(task) = self.tasks.last() else {
            self.shown = None;
            return term.status(Status::Idle);
        };
        let now = (task.what.clone(), task.progress);
        if self.shown.as_ref() == Some(&now) {
            return String::new();
        }
        let report = term.status(Status::Working {
            what: &now.0,
            progress: now.1,
        });
        self.shown = Some(now);
        report
    }

    /// Turns the indicator on, for the timer of `episode`.
    fn show(&mut self, episode: u64, term: &dyn Terminal) -> String {
        if self.episode != episode || self.tasks.is_empty() || self.shown.is_some() {
            return String::new();
        }
        // Differs from any real state, so `changes` reports.
        self.shown = Some((String::new(), Some(u8::MAX)));
        self.changes(term)
    }

    /// Queues what changed. Called with the lock held, so reports keep the
    /// order of the changes.
    fn update(&mut self) {
        queue(self.changes(terminal::current()));
    }
}

impl Busy {
    /// `what` is one line saying what martty is doing, such as
    /// `Downloading README.md`.
    pub fn start(what: &str) -> Self {
        if !io::stdout().is_terminal() {
            return Self { id: None };
        }
        let mut state = lock();
        if state.tasks.is_empty() && state.shown.is_none() {
            state.episode += 1;
            let episode = state.episode;
            thread::spawn(move || {
                thread::sleep(DELAY);
                // Queued under the lock: once released, a stretch may end
                // and queue idle, which this must not follow.
                let mut state = lock();
                let report = state.show(episode, terminal::current());
                queue(report);
            });
        }
        let id = state.next_id;
        state.next_id += 1;
        state.tasks.push(Task {
            id,
            what: what.to_owned(),
            progress: None,
        });
        state.update();
        Self { id: Some(id) }
    }

    /// How much of this stretch is done, in percent. Callable from the
    /// threads doing the work.
    pub fn progress(&self, percent: u8) {
        let Some(id) = self.id else {
            return;
        };
        let mut state = lock();
        if let Some(task) = state.tasks.iter_mut().find(|t| t.id == id) {
            task.progress = Some(percent.min(100));
        }
        state.update();
    }
}

impl Drop for Busy {
    /// Ends the stretch: the indicator goes off, or back to the stretch
    /// around it.
    fn drop(&mut self) {
        if let Some(id) = self.id {
            let mut state = lock();
            state.tasks.retain(|t| t.id != id);
            state.update();
        }
    }
}

/// `done` of `total` steps in whole percent, at most 100.
pub fn percent(done: u64, total: u64) -> u8 {
    u8::try_from(done.saturating_mul(100) / total.max(1))
        .unwrap_or(100)
        .min(100)
}

/// Tells the terminal what happened outside any busy stretch: news to look
/// at, or the error martty stops on.
pub fn report(status: Status<'_>) {
    if io::stdout().is_terminal() {
        let _ordered = lock();
        queue(terminal::current().status(status));
    }
}

/// Waits, a second at most, until every queued report is on stdout: the
/// writer thread ends with the process.
pub fn flush() {
    let Some(writer) = WRITER.get() else {
        return;
    };
    let (done, wait) = mpsc::channel();
    if writer.send(Message::Flush(done)).is_ok() {
        let _ = wait.recv_timeout(FLUSH_TIMEOUT);
    }
}

enum Message {
    Report(String),
    Flush(Sender<()>),
}

static WRITER: OnceLock<Sender<Message>> = OnceLock::new();

/// Hands `report` to the writer thread, started on first use.
fn queue(report: String) {
    if report.is_empty() {
        return;
    }
    let writer = WRITER.get_or_init(|| {
        let (writer, messages) = mpsc::channel();
        thread::spawn(move || {
            for message in messages {
                match message {
                    Message::Report(report) => {
                        let mut out = io::stdout().lock();
                        let _ = out.write_all(report.as_bytes()).and_then(|()| out.flush());
                    }
                    Message::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        writer
    });
    let _ = writer.send(Message::Report(report));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reports as their `Debug` text.
    struct Echo;

    impl Terminal for Echo {
        fn font_family(&self) -> Option<String> {
            None
        }

        fn status(&self, status: Status<'_>) -> String {
            format!("{status:?}")
        }
    }

    fn task(id: u64, what: &str) -> Task {
        Task {
            id,
            what: what.to_owned(),
            progress: None,
        }
    }

    #[test]
    fn indicator_should_show_innermost_stretch_and_fall_back() {
        let mut state = State {
            tasks: vec![task(0, "Laying out")],
            next_id: 1,
            episode: 1,
            shown: None,
        };
        // Before the timer, nothing is shown; a stale timer shows nothing.
        assert_eq!(state.changes(&Echo), "");
        assert_eq!(state.show(0, &Echo), "");
        assert!(state.show(1, &Echo).contains("Laying out"));
        state.tasks.push(task(1, "Downloading 2 images"));
        assert!(state.changes(&Echo).contains("Downloading 2 images"));
        state.tasks[1].progress = Some(50);
        assert!(state.changes(&Echo).contains("Some(50)"));
        assert_eq!(state.changes(&Echo), "");
        state.tasks.pop();
        assert!(state.changes(&Echo).contains("Laying out"));
        state.tasks.pop();
        assert_eq!(state.changes(&Echo), "Idle");
        assert_eq!(state.changes(&Echo), "");
    }

    #[test]
    fn percent_should_round_down_and_stop_at_100() {
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(3, 3), 100);
        assert_eq!(percent(5, 3), 100);
        assert_eq!(percent(1, 0), 100);
    }
}
