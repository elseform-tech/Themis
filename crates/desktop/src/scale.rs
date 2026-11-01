//! Helpers for scale tests, which count work instead of timing it. See docs/scale-tests.md.

use std::sync::{mpsc, Arc};
use std::time::Duration;

/// The longest one measurement may take. This protects the machine and is never the thing a
/// test checks: past it, the measurement is abandoned and larger sizes are skipped.
const MEASUREMENT_LIMIT: Duration = Duration::from_secs(30);

/// Runs `work` at each size, smallest first, on a thread of its own, and returns the counts.
/// Thread-local counters must be read inside `work`.
pub(crate) fn measure<F>(sizes: &[usize], work: F) -> Vec<(usize, u64)>
where
    F: Fn(usize) -> u64 + Send + Sync + 'static,
{
    let work = Arc::new(work);
    let mut samples = Vec::new();
    for &size in sizes {
        let (sender, receiver) = mpsc::channel();
        let work = Arc::clone(&work);
        std::thread::spawn(move || {
            let _ = sender.send(work(size));
        });
        match receiver.recv_timeout(MEASUREMENT_LIMIT) {
            Ok(count) => samples.push((size, count)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                eprintln!("scale: size {size} passed {MEASUREMENT_LIMIT:?}; skipping larger sizes");
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("scale measurement at size {size} panicked")
            }
        }
    }
    samples
}

/// How the work grows, from the first and last sample: 0 is flat, 1 linear, 2 quadratic.
pub(crate) fn growth_rate(samples: &[(usize, u64)]) -> f64 {
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        panic!("scale test measured nothing");
    };
    assert!(
        samples.len() > 1,
        "only size {} finished inside the limit, which is too slow in itself",
        first.0
    );
    let work = (last.1.max(1) as f64 / first.1.max(1) as f64).ln();
    work / (last.0 as f64 / first.0 as f64).ln()
}

#[test]
fn growth_rate_tells_flat_linear_and_quadratic_apart() {
    let rate = |work: fn(u64) -> u64| {
        growth_rate(&[1000, 8000].map(|size: usize| (size, work(size as u64))))
    };
    assert!(rate(|_| 40).abs() < 0.01);
    assert!((rate(|size| 3 * size) - 1.0).abs() < 0.01);
    assert!((rate(|size| size * size) - 2.0).abs() < 0.01);
}

#[test]
fn measure_runs_every_size_in_order() {
    let samples = measure(&[1, 2, 3], |size| size as u64 * 10);
    assert_eq!(samples, [(1, 10), (2, 20), (3, 30)]);
}
