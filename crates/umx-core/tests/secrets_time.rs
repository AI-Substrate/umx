//! Security, ws004 s08: no secret-scan rule goes quadratic on a long run. Each adversarial shape is scanned at sizes
//! growing 4x a step up to 1 MiB, and its time must grow about 4x a step (linear), never about 16x (quadratic).
//! Growth, not a wall-clock bound (F108): a loaded gate slows both sizes of a step alike, so the ratio holds where a
//! fixed bound flaked (3.1 s against 3 s at load 25). Here, not in the crate's unit tests, because it times with std
//! and umx-core is no_std.

use std::time::{Duration, Instant};
use umx_core::secrets::scan;

/// Each step's larger input is this many times its smaller.
const STEP: usize = 4;
/// The largest input: what the guard was written for.
const MIB: usize = 1 << 20;
/// The smallest input. A quadratic rule already dominates here, so a regression fails at the first step, in seconds.
const SMALLEST: usize = 16 << 10;
/// Linear time grows about [`STEP`]x a step and quadratic about 16x; this is the line between them.
const MAX_GROWTH: f64 = 8.0;
/// A time below this is too short to divide by (an early hit, timer grain): it counts as this much.
const FLOOR: Duration = Duration::from_millis(5);
/// Runs of each size a step: the fastest counts, so a run the machine paused part-way does not.
const RUNS: usize = 2;

fn repeat(unit: &str, n: usize) -> String {
    let mut s = String::with_capacity(n + unit.len());
    while s.len() < n {
        s.push_str(unit);
    }
    s.truncate(n);
    s
}

fn timed(input: &str) -> Duration {
    let started = Instant::now();
    let _ = scan(input);
    started.elapsed()
}

#[test]
fn adversarial_megabyte_inputs_scan_in_linear_time() {
    let shapes = [
        "sk-",
        "AKIA",
        "eyJ",
        "-----BEGIN ",
        "password=",
        "eyJ-",
        "ghp_",
        "://",
        "bearer ",
        "api_key=",
        "Q7mZp2Kx9VbR4tLw8NcY3hJd6FsG1aE5",
        "a",
        "aws ",
    ];
    for shape in shapes {
        let mut n = SMALLEST;
        while n < MIB {
            let (small, large) = (repeat(shape, n), repeat(shape, n * STEP));
            // Interleaved, so a change in load between the sizes touches both.
            let (mut t_small, mut t_large) = (Duration::MAX, Duration::MAX);
            for _ in 0..RUNS {
                t_small = t_small.min(timed(&small));
                t_large = t_large.min(timed(&large));
            }
            let growth = t_large.as_secs_f64() / t_small.max(FLOOR).as_secs_f64();
            eprintln!(
                "{shape:?} {n} -> {}: {t_small:?} -> {t_large:?} ({growth:.2}x)",
                n * STEP
            );
            assert!(
                growth < MAX_GROWTH,
                "{shape:?}: {n} bytes took {t_small:?}, {} took {t_large:?} ({growth:.1}x, linear is about {STEP}x)",
                n * STEP
            );
            n *= STEP;
        }
    }
}
