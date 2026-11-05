//! Running memory jobs on the memory models (`memory.models`): each job tries the models in
//! order and moves on when one fails, and a job every model failed waits in a small bounded
//! queue for a later try. Pure: the desktop supplies the request and the clock.

use std::collections::VecDeque;
use std::future::Future;

use serde::Serialize;

/// Jobs that may wait for a later try; past it the oldest is dropped.
pub const MAX_PENDING: usize = 8;
/// Tries a waiting job gets (its first included) before it is dropped.
pub const MAX_ATTEMPTS: u32 = 6;
/// First wait before a failed job is tried again; it doubles with each try.
pub const FIRST_RETRY_MS: i64 = 60_000;
/// Longest wait between tries.
pub const MAX_RETRY_MS: i64 = 6 * 60 * 60 * 1000;

/// Why one model did not do a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// The provider refused for its rate (HTTP 429).
    RateLimited,
    /// Any other provider error (HTTP 5xx, a refused key, a network failure).
    ProviderError,
    /// No answer within the request time limit.
    TimedOut,
    /// An answer, but not one the job could read: the model can't do the job.
    Unreadable,
    /// The memory models used up today's `memory.daily_budget_usd`.
    OverBudget,
    /// The model can't be called at all (no key, unknown provider).
    Unavailable,
}

impl Failure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::ProviderError => "provider_error",
            Self::TimedOut => "timed_out",
            Self::Unreadable => "unreadable",
            Self::OverBudget => "over_budget",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Tries `attempt` on each model in order until one succeeds: the index and model that did
/// the job with what it returned, and every failure on the way (index and why). `None` when
/// every model failed.
pub async fn on_models<T, F, Fut>(
    models: &[String],
    mut attempt: F,
) -> (Option<(usize, T)>, Vec<(usize, Failure)>)
where
    F: FnMut(usize, String) -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    let mut failures = Vec::new();
    for (index, model) in models.iter().enumerate() {
        match attempt(index, model.clone()).await {
            Ok(value) => return (Some((index, value)), failures),
            Err(failure) => failures.push((index, failure)),
        }
    }
    (None, failures)
}

/// One job waiting for a later try.
#[derive(Debug, Clone, PartialEq)]
pub struct Waiting<T> {
    pub job: T,
    /// Tries so far that failed on every model.
    pub attempts: u32,
    /// When it may be tried again (milliseconds since the epoch).
    pub next_at: i64,
    /// Why the last try failed, the first model's reason first.
    pub reasons: Vec<Failure>,
}

/// The wait after `attempts` failed tries, starting at `first`.
pub fn backoff_ms(attempts: u32, first: i64) -> i64 {
    let doublings = attempts.saturating_sub(1).min(20);
    first.saturating_mul(1i64 << doublings).min(MAX_RETRY_MS)
}

/// Jobs every memory model failed, oldest first, at most [`MAX_PENDING`].
#[derive(Debug, Clone)]
pub struct Pending<T> {
    jobs: VecDeque<Waiting<T>>,
    /// The first wait; [`FIRST_RETRY_MS`] unless a test shortens it.
    first_retry_ms: i64,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Self::with_first_retry(FIRST_RETRY_MS)
    }
}

impl<T> Pending<T> {
    /// An empty queue whose first wait is `first_retry_ms`.
    pub fn with_first_retry(first_retry_ms: i64) -> Self {
        Self {
            jobs: VecDeque::new(),
            first_retry_ms,
        }
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The earliest time a waiting job may be tried again.
    pub fn next_at(&self) -> Option<i64> {
        self.jobs.iter().map(|waiting| waiting.next_at).min()
    }

    /// Why the newest waiting job's last try failed.
    pub fn last_reasons(&self) -> &[Failure] {
        self.jobs
            .back()
            .map_or(&[][..], |waiting| waiting.reasons.as_slice())
    }

    /// Keeps a job that just failed on every model for a later try (at `retry_at` when given,
    /// else after [`backoff_ms`]). Returns a job dropped to make room, or this job when it has
    /// had all its tries. A try that failed only on the daily cap does not count against them.
    pub fn keep(&mut self, mut waiting: Waiting<T>, now: i64, retry_at: Option<i64>) -> Option<T> {
        let capped_only = !waiting.reasons.is_empty()
            && waiting
                .reasons
                .iter()
                .all(|reason| *reason == Failure::OverBudget);
        if !capped_only {
            waiting.attempts += 1;
        }
        if waiting.attempts >= MAX_ATTEMPTS {
            return Some(waiting.job);
        }
        let wait = backoff_ms(waiting.attempts.max(1), self.first_retry_ms);
        waiting.next_at = retry_at.unwrap_or(now + wait);
        self.jobs.push_back(waiting);
        if self.jobs.len() > MAX_PENDING {
            return self.jobs.pop_front().map(|waiting| waiting.job);
        }
        None
    }

    /// Takes the jobs whose wait is over, oldest first.
    pub fn take_due(&mut self, now: i64) -> Vec<Waiting<T>> {
        let (due, waiting): (VecDeque<_>, VecDeque<_>) =
            self.jobs.drain(..).partition(|job| job.next_at <= now);
        self.jobs = waiting;
        due.into()
    }
}

/// A job as it first fails: no tries counted yet.
pub fn first_failure<T>(job: T, failures: &[(usize, Failure)]) -> Waiting<T> {
    Waiting {
        job,
        attempts: 0,
        next_at: 0,
        reasons: failures.iter().map(|(_, failure)| *failure).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn models() -> Vec<String> {
        ["first", "second", "third"].map(str::to_owned).to_vec()
    }

    #[tokio::test]
    async fn a_job_tries_the_models_in_order_and_stops_at_the_first_that_does_it() {
        let tried = RefCell::new(Vec::new());
        let (done, failures) = on_models(&models(), |index, model| {
            tried.borrow_mut().push(model.clone());
            async move {
                match index {
                    0 => Err(Failure::RateLimited),
                    _ => Ok(format!("{model} did it")),
                }
            }
        })
        .await;
        assert_eq!(done, Some((1, "second did it".to_owned())));
        assert_eq!(failures, [(0, Failure::RateLimited)]);
        assert_eq!(
            *tried.borrow(),
            ["first", "second"],
            "the third is never asked"
        );
        // Each reason moves on, the last model included; all failing gives no result.
        let (done, failures) = on_models(&models(), |index, _| async move {
            Err::<(), _>(
                [
                    Failure::ProviderError,
                    Failure::TimedOut,
                    Failure::Unreadable,
                ][index],
            )
        })
        .await;
        assert_eq!(done, None);
        assert_eq!(
            failures,
            [
                (0, Failure::ProviderError),
                (1, Failure::TimedOut),
                (2, Failure::Unreadable)
            ]
        );
        // The first model doing it asks no other.
        let calls = RefCell::new(0);
        let (done, failures) = on_models(&models(), |_, _| {
            *calls.borrow_mut() += 1;
            async { Ok(()) }
        })
        .await;
        assert_eq!(
            (done, failures.len(), *calls.borrow()),
            (Some((0, ())), 0, 1)
        );
    }

    #[test]
    fn failed_jobs_wait_with_a_growing_bounded_backoff_and_are_dropped_after_their_tries() {
        let mut pending = Pending::default();
        let failed = |job| first_failure(job, &[(0, Failure::ProviderError)]);
        assert_eq!(pending.keep(failed("a"), 0, None), None);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.next_at(), Some(FIRST_RETRY_MS));
        assert!(pending.take_due(FIRST_RETRY_MS - 1).is_empty());
        let mut due = pending.take_due(FIRST_RETRY_MS);
        assert_eq!(due.len(), 1);
        assert!(pending.is_empty());
        // Each further failure doubles the wait, up to the longest; the last try drops it.
        let mut waiting = due.remove(0);
        let mut now = FIRST_RETRY_MS;
        for attempt in 2..MAX_ATTEMPTS {
            assert_eq!(pending.keep(waiting, now, None), None);
            let next = pending.next_at().unwrap();
            assert_eq!(next - now, backoff_ms(attempt, FIRST_RETRY_MS));
            now = next;
            waiting = pending.take_due(now).remove(0);
        }
        assert_eq!(pending.keep(waiting, now, None), Some("a"));
        assert!(pending.is_empty());
        assert_eq!(backoff_ms(40, FIRST_RETRY_MS), MAX_RETRY_MS);
    }

    #[test]
    fn the_queue_is_bounded_and_the_daily_cap_costs_no_tries() {
        let mut pending = Pending::default();
        for job in 0..MAX_PENDING {
            assert_eq!(
                pending.keep(first_failure(job, &[(0, Failure::TimedOut)]), 0, None),
                None
            );
        }
        // One more pushes the oldest out.
        assert_eq!(
            pending.keep(first_failure(99, &[(0, Failure::TimedOut)]), 0, None),
            Some(0)
        );
        assert_eq!(pending.len(), MAX_PENDING);
        assert_eq!(pending.last_reasons(), [Failure::TimedOut]);
        // Held back only by the cap: tried at the given time, its tries untouched.
        let mut capped = Pending::default();
        let reasons = [(0, Failure::OverBudget), (1, Failure::OverBudget)];
        assert_eq!(
            capped.keep(first_failure("c", &reasons), 0, Some(5_000)),
            None
        );
        let waiting = capped.take_due(5_000).remove(0);
        assert_eq!((waiting.attempts, waiting.next_at), (0, 5_000));
    }
}
