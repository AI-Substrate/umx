//! A counting global allocator: live and peak bytes requested, and every byte ever requested, in atomics. Memory
//! figures come from here, not RSS, which swings by hundreds of MB under load and keeps freed pages (s01 spec,
//! section 4).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Bytes requested since start, never decremented (a realloc that grows counts its growth).
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

fn grow(n: usize) {
    let v = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(v, Relaxed);
    ALLOCATED.fetch_add(n, Relaxed);
    thread_add(n as isize);
}

fn shrink(n: usize) {
    LIVE.fetch_sub(n, Relaxed);
    thread_add(-(n as isize));
}

#[cfg(test)]
std::thread_local! {
    /// Net bytes requested by this thread: unit tests run in parallel, so a test measures its own thread only.
    static THREAD_NET: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn thread_add(n: isize) {
    let _ = THREAD_NET.try_with(|c| c.set(c.get() + n));
}

/// Bench builds count only the shared totals.
#[cfg(not(test))]
#[inline(always)]
fn thread_add(_: isize) {}

/// Net bytes this thread has requested and not freed (tests only).
#[cfg(test)]
pub fn thread_net() -> isize {
    THREAD_NET.with(std::cell::Cell::get)
}

// SAFETY: every call forwards to `System` unchanged; the counters only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            grow(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            grow(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        shrink(l.size());
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            if new > l.size() {
                grow(new - l.size());
            } else {
                shrink(l.size() - new);
            }
        }
        q
    }
}

/// Bytes requested and not yet freed (allocator rounding not included).
pub fn live() -> usize {
    LIVE.load(Relaxed)
}

/// Starts a new peak window at the current live bytes.
pub fn reset_peak() {
    PEAK.store(LIVE.load(Relaxed), Relaxed);
}

pub fn peak() -> usize {
    PEAK.load(Relaxed)
}

/// Bytes requested since start: the difference over a span is what the span allocated, frees not netted out.
pub fn allocated() -> usize {
    ALLOCATED.load(Relaxed)
}

/// Live bytes now less `base`, signed (F219). The counters are process-wide, so another thread freeing memory during
/// the span takes live below `base`: an unsigned difference panicked in debug builds, and a saturating one would read
/// 0. A negative figure says the span was disturbed.
pub fn live_since(base: usize) -> isize {
    signed(live(), base)
}

/// The peak window's high-water mark less `base`, signed, as [`live_since`].
pub fn peak_since(base: usize) -> isize {
    signed(peak(), base)
}

fn signed(now: usize, base: usize) -> isize {
    (now as i128 - base as i128).clamp(isize::MIN as i128, isize::MAX as i128) as isize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_another_thread_shrinks_reads_negative_not_a_panic() {
        // F219: frees elsewhere in the process take the shared counter below a span's base.
        assert_eq!(signed(10, 25), -15);
        assert_eq!(signed(25, 10), 15);
        assert_eq!(signed(0, usize::MAX), isize::MIN);
        assert_eq!(signed(usize::MAX, 0), isize::MAX);
        // A base far above anything a parallel test allocates: the live and peak figures read negative.
        assert!(live_since(live() + (1 << 40)) < 0);
        assert!(peak_since(peak() + (1 << 40)) < 0);
    }
}
