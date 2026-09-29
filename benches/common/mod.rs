//! Timing shared by the benchmarks. Each benchmark does the same work
//! through litestd and through std, in one binary, and prints how litestd
//! compares. Run them with `scripts/bench.sh`.

#![allow(
    dead_code,
    clippy::redundant_pub_crate,
    reason = "helpers shared by benchmarks that each use some of them"
)]

use core::time::Duration;
use std::time::Instant;

/// Samples of each side, taken alternately so both see the same noise.
const ROUNDS: usize = 21;
/// The shortest time one sample takes.
const SAMPLE: Duration = Duration::from_millis(5);

/// Whether the benchmark `name` matches the filter: the first argument that
/// is not a flag, since Cargo passes `--bench`.
pub(crate) fn selected(name: &str) -> bool {
    std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .is_none_or(|filter| name.contains(&filter))
}

/// Times `lite` and `std`, which do the same work through litestd and std,
/// and prints the median time per call of each and how litestd compares,
/// with half the interquartile range of the per-round ratios as the spread.
/// The closures pass their inputs and results through `black_box`.
pub(crate) fn compare(
    name: &str,
    mut lite: impl FnMut(),
    mut std: impl FnMut(),
) {
    if !selected(name) {
        return;
    }
    let iters = calibrate(&mut lite).max(calibrate(&mut std));
    let rounds: [(f64, f64); ROUNDS] = core::array::from_fn(|_| {
        (sample(&mut lite, iters), sample(&mut std, iters))
    });
    let mut ratios = rounds.map(|(lite, std)| std / lite);
    let ratio = median(&mut ratios);
    let spread = (ratios[ROUNDS * 3 / 4] - ratios[ROUNDS / 4]) / 2.0 / ratio;
    println!(
        "{name:<40} litestd {:>9}  std {:>9}  {} (+-{:.0}%)",
        nanos(median(&mut rounds.map(|(lite, _)| lite))),
        nanos(median(&mut rounds.map(|(_, std)| std))),
        verdict(ratio),
        spread * 100.0,
    );
}

/// The iteration count that makes one sample of `f` take `SAMPLE`.
fn calibrate(f: &mut impl FnMut()) -> u32 {
    let mut iters = 1;
    // Doubles up to 2^30, so at most 31 passes.
    while iters < 1 << 30 {
        let start = Instant::now();
        for _ in 0..iters {
            f();
        }
        if start.elapsed() >= SAMPLE {
            break;
        }
        iters *= 2;
    }
    iters
}

/// Runs `f` `iters` times and returns the mean nanoseconds per call.
fn sample(f: &mut impl FnMut(), iters: u32) -> f64 {
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    start.elapsed().as_secs_f64() * 1e9 / f64::from(iters)
}

/// Sorts `values` and returns the middle one.
fn median(values: &mut [f64; ROUNDS]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[ROUNDS / 2]
}

/// Formats nanoseconds in the unit that keeps them short.
fn nanos(ns: f64) -> String {
    if ns < 1e3 {
        format!("{ns:.1} ns")
    } else if ns < 1e6 {
        format!("{:.2} us", ns / 1e3)
    } else {
        format!("{:.2} ms", ns / 1e6)
    }
}

/// Describes a ratio of std's time to litestd's.
fn verdict(ratio: f64) -> String {
    if ratio >= 1.0 {
        format!("{ratio:.2}x faster")
    } else {
        format!("{:.0}% slower", (1.0 / ratio - 1.0) * 100.0)
    }
}
