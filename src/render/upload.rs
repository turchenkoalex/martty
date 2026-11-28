//! Images whose upload waits until they are about to be seen. Layout only
//! sizes them (their placeholder rows are ordinary lines either way);
//! rasterizing, compressing and base64 happen when the pager comes near
//! them, or all at once for printing, spread over the CPU's cores, with the
//! share done on the terminal's activity indicator.

use std::ops::Range;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;

use crate::kitty::Transmit;
use crate::panics;
use crate::progress::{self, Busy};

/// Makes an image's upload; `None` when it can't be drawn after all (its
/// rows then stay empty).
pub type Job = Box<dyn FnOnce() -> Option<Arc<Transmit>> + Send>;

pub struct Upload {
    pub id: u32,
    /// Lines that show the image.
    pub lines: Range<usize>,
    pub job: Job,
}

/// The uploads of `jobs` in order, each as kitty commands for its id. A job
/// that panics (a decoder choking on a broken file) uploads nothing.
pub fn run(jobs: Vec<(u32, Job)>) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    let busy = Busy::start(&match jobs.len() {
        1 => "Drawing an image".to_owned(),
        n => format!("Drawing {n} images"),
    });
    let total = jobs.len() as u64;
    let finished = AtomicU64::new(0);
    let run_counted = |job: Job| {
        let transmit = panics::catch(job);
        busy.progress(progress::percent(
            finished.fetch_add(1, Ordering::Relaxed) + 1,
            total,
        ));
        transmit
    };
    let threads = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(jobs.len());
    let results: Vec<(u32, Option<Arc<Transmit>>)> = if threads <= 1 {
        jobs.into_iter()
            .map(|(id, job)| (id, run_counted(job)))
            .collect()
    } else {
        let ids: Vec<u32> = jobs.iter().map(|(id, _)| *id).collect();
        let jobs: Vec<Mutex<Option<Job>>> = jobs
            .into_iter()
            .map(|(_, job)| Mutex::new(Some(job)))
            .collect();
        let done: Vec<Mutex<Option<Arc<Transmit>>>> =
            jobs.iter().map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(slot) = jobs.get(i) else { break };
                        let job = slot.lock().ok().and_then(|mut j| j.take());
                        if let (Some(job), Ok(mut out)) = (job, done[i].lock()) {
                            *out = run_counted(job);
                        }
                    }
                });
            }
        });
        ids.into_iter()
            .zip(done)
            .map(|(id, d)| (id, d.into_inner().ok().flatten()))
            .collect()
    };
    let mut out = String::new();
    for (id, transmit) in results {
        if let Some(t) = transmit {
            t.write(&mut out, id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(byte: u8) -> Job {
        Box::new(move || Some(Arc::new(Transmit::png(&[byte], 1, 1))))
    }

    #[test]
    fn run_should_keep_order_of_jobs() {
        let out = run((1..=8).map(|i| (u32::from(i), job(i))).collect());
        let ids: Vec<&str> = out
            .split("\x1b_G")
            .filter_map(|c| c.split(',').next())
            .filter(|c| !c.is_empty())
            .collect();
        assert_eq!(
            ids,
            ["i=1", "i=2", "i=3", "i=4", "i=5", "i=6", "i=7", "i=8"]
        );
    }

    #[test]
    fn run_should_survive_panicking_jobs() {
        let broken: Job = Box::new(|| panic!("broken image"));
        let out = run(vec![(1, broken), (2, job(2))]);
        assert!(out.starts_with("\x1b_Gi=2,"), "{out:?}");
    }

    #[test]
    fn run_should_skip_failed_jobs() {
        let failed: Job = Box::new(|| None);
        assert_eq!(run(vec![(1, failed)]), "");
    }
}
