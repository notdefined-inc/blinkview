//! Reporting progress out of long-running work.
//!
//! Face detection over a few thousand photos takes minutes. Without this the UI shows
//! an indeterminate spinner and is indistinguishable from a hang — which is exactly
//! what it looked like during development.
//!
//! The callback is `Sync` because the work it reports on is parallel; counting happens
//! on an atomic so a caller never sees a count go backwards.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Called with (completed, total). Never called with `completed > total`.
pub trait Sink: Fn(usize, usize) + Sync {}
impl<T: Fn(usize, usize) + Sync> Sink for T {}

/// A no-op sink, for callers that do not care.
pub fn silent(_done: usize, _total: usize) {}

/// Counts completions and forwards them, throttled so a fast loop does not flood the
/// channel with an event per item.
pub struct Counter<'a> {
    done: AtomicUsize,
    total: usize,
    every: usize,
    sink: &'a (dyn Fn(usize, usize) + Sync),
}

impl<'a> Counter<'a> {
    pub fn new(total: usize, sink: &'a (dyn Fn(usize, usize) + Sync)) -> Self {
        // Aim for ~100 updates over the whole run, and always at least every item
        // when there are few.
        let every = (total / 100).max(1);
        sink(0, total);
        Self {
            done: AtomicUsize::new(0),
            total,
            every,
            sink,
        }
    }

    /// Record one completed item.
    pub fn tick(&self) {
        let n = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(self.every) || n == self.total {
            (self.sink)(n, self.total);
        }
    }

    pub fn finish(&self) {
        (self.sink)(self.total, self.total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn reports_start_and_finish() {
        let seen = Mutex::new(Vec::new());
        {
            let sink = |d: usize, t: usize| seen.lock().unwrap().push((d, t));
            let c = Counter::new(3, &sink);
            c.tick();
            c.tick();
            c.tick();
        }
        let v = seen.into_inner().unwrap();
        assert_eq!(v.first(), Some(&(0, 3)), "must report a starting point");
        assert_eq!(v.last(), Some(&(3, 3)), "must report completion");
    }

    #[test]
    fn throttles_large_runs() {
        let count = Mutex::new(0usize);
        {
            let sink = |_: usize, _: usize| *count.lock().unwrap() += 1;
            let c = Counter::new(10_000, &sink);
            for _ in 0..10_000 {
                c.tick();
            }
        }
        // ~100 updates plus the initial one — not 10,000.
        assert!(
            *count.lock().unwrap() < 150,
            "emitted {} events",
            count.lock().unwrap()
        );
    }

    #[test]
    fn never_exceeds_total() {
        let max = Mutex::new(0usize);
        {
            let sink = |d: usize, _: usize| {
                let mut m = max.lock().unwrap();
                if d > *m {
                    *m = d
                }
            };
            let c = Counter::new(5, &sink);
            for _ in 0..5 {
                c.tick();
            }
        }
        assert_eq!(*max.lock().unwrap(), 5);
    }
}
