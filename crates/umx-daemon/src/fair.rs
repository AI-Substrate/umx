//! The services lock, fair to readers. std's `RwLock` lets the writer thread, which runs its jobs back to back, take
//! the lock again before a woken reader runs: on macOS a reader waited 4 s behind 1 ms holds and 29 s behind 20 ms
//! ones, so a stream of embed applies starved every read. Here a writer first lets the readers already waiting in,
//! for at most [`YIELD_MAX`], so one job's hold is the longest a read waits behind the writer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LockResult, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

/// The longest a write waits for the readers before it: a steady stream of readers never stops the writer.
const YIELD_MAX: Duration = Duration::from_millis(50);

pub(crate) struct FairLock<T> {
    lock: RwLock<T>,
    waiting: AtomicUsize,
}

impl<T> FairLock<T> {
    pub(crate) fn new(value: T) -> Self {
        FairLock {
            lock: RwLock::new(value),
            waiting: AtomicUsize::new(0),
        }
    }

    pub(crate) fn read(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let guard = self.lock.read();
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        guard
    }

    pub(crate) fn write(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        let until = Instant::now() + YIELD_MAX;
        while self.waiting.load(Ordering::SeqCst) > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_micros(200));
        }
        self.lock.write()
    }

    pub(crate) fn is_poisoned(&self) -> bool {
        self.lock.is_poisoned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// The writer loop's shape: a reader comes while a write holds the lock, the write ends and the writer at once
    /// asks for the next. The reader goes first every time; std's RwLock alone lets the writer take it again (on
    /// macOS a reader waited seconds behind back-to-back 5 ms holds).
    #[test]
    fn a_waiting_reader_goes_before_the_writers_next_write() {
        let lock = Arc::new(FairLock::new(()));
        for round in 0..50 {
            let first = lock.write().unwrap();
            let read = Arc::new(AtomicBool::new(false));
            let reader = {
                let (lock, read) = (Arc::clone(&lock), Arc::clone(&read));
                std::thread::spawn(move || {
                    let _guard = lock.read().unwrap();
                    read.store(true, Ordering::SeqCst);
                })
            };
            while lock.waiting.load(Ordering::SeqCst) == 0 {
                std::thread::yield_now();
            }
            drop(first);
            let _next = lock.write().unwrap();
            assert!(
                read.load(Ordering::SeqCst),
                "round {round}: the writer went again before the waiting reader"
            );
            drop(_next);
            reader.join().unwrap();
        }
    }
}
