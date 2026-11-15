//! Pacing for background embedding (workshop 009 section 6, Flowspace's lesson): batches within a token budget,
//! a token rate that halves on a 429 and recovers on success, and a queue in which query embeds go first. Pure
//! state: the caller owns the clock, the sleeping and the threads.

use super::{Counted, EmbedJob, EmbedNote, Planned, Vectors, embed_counted};
use alloc::collections::{BTreeSet, VecDeque};
use alloc::vec::Vec;
use umx_core::ports::{EmbedError, Embedder};

/// Texts per provider call.
pub const MAX_BATCH_TEXTS: usize = 16;
/// Estimated tokens per provider call.
pub const MAX_BATCH_TOKENS: usize = 200_000;
/// Background rate when nothing has been refused: 4M tokens a minute (Flowspace's measured deployment allowed 6.85M).
pub const START_TOKENS_PER_MIN: u64 = 4_000_000;
/// How long to wait before asking an unavailable provider again.
pub const UNAVAILABLE_WAIT_MS: u64 = 30_000;

/// A deliberately high token estimate: two bytes a token (bytes / 3 was too low for dense text in Flowspace).
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(2).max(1)
}

fn tokens(p: &Planned) -> usize {
    p.texts.iter().map(|w| estimate_tokens(&w.text)).sum()
}

/// Splits planned nodes into provider calls of at most [`MAX_BATCH_TEXTS`] texts and [`MAX_BATCH_TOKENS`]
/// estimated tokens, in order. A node's texts are never split; a node over either limit goes alone.
pub fn batches(planned: Vec<Planned>) -> Vec<Vec<Planned>> {
    let mut out: Vec<Vec<Planned>> = Vec::new();
    let (mut texts, mut toks) = (0, 0);
    for p in planned {
        let (n, t) = (p.texts.len(), tokens(&p));
        let fits = texts + n <= MAX_BATCH_TEXTS && toks + t <= MAX_BATCH_TOKENS;
        match out.last_mut() {
            Some(last) if fits || last.is_empty() => {
                last.push(p);
                texts += n;
                toks += t;
            }
            _ => {
                out.push(alloc::vec![p]);
                (texts, toks) = (n, t);
            }
        }
    }
    out
}

/// Jobs waiting to embed. Query embeds go before background work (a search must not wait behind an ingest);
/// a background job already waiting is not queued twice.
#[derive(Debug)]
pub struct EmbedQueue<T: Ord + Clone> {
    query: VecDeque<T>,
    background: VecDeque<T>,
    waiting: BTreeSet<T>,
}

impl<T: Ord + Clone> Default for EmbedQueue<T> {
    fn default() -> Self {
        EmbedQueue {
            query: VecDeque::new(),
            background: VecDeque::new(),
            waiting: BTreeSet::new(),
        }
    }
}

impl<T: Ord + Clone> EmbedQueue<T> {
    pub fn push(&mut self, job: T) {
        if self.waiting.insert(job.clone()) {
            self.background.push_back(job);
        }
    }

    pub fn push_query(&mut self, job: T) {
        self.query.push_back(job);
    }

    /// Puts jobs back at the front of the background lane, in order (after a 429 or an unavailable provider).
    pub fn retry(&mut self, jobs: Vec<T>) {
        for job in jobs.into_iter().rev() {
            if self.waiting.insert(job.clone()) {
                self.background.push_front(job);
            }
        }
    }

    /// Up to `n` jobs: queries first, then background.
    pub fn pop(&mut self, n: usize) -> Vec<T> {
        let mut out = Vec::new();
        while out.len() < n {
            if let Some(q) = self.query.pop_front() {
                out.push(q);
            } else if let Some(b) = self.background.pop_front() {
                self.waiting.remove(&b);
                out.push(b);
            } else {
                break;
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.query.len() + self.background.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// How long with no 429 before a halved rate steps back up an eighth of the configured rate (embeddings-expert, s25).
pub const QUIET_RECOVERY_MS: u64 = 60_000;

/// The background token rate of one instance (pure: the caller owns the clock, the threads and the sleeping). A call
/// reserves its estimated tokens when it is dispatched, so calls in flight at once cannot all pass the rate; what
/// the provider says it counted corrects the charge. A 429 starts an episode that lasts until the first success
/// after its wait: the rate halves once for the whole episode (floor: an eighth of the configured rate), however
/// many calls in flight come back 429, and dispatch waits at least the provider's `Retry-After`. The rate steps back
/// up by time, an eighth of the configured rate for each quiet minute, never above it. An unpaced instance (local)
/// has no rate; only a provider's wait applies (ws004 s25).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pacer {
    max: Option<u64>,
    rate: u64,
    next_ms: u64,
    pause_until_ms: u64,
    episode: bool,
    quiet_since_ms: u64,
}

/// What [`Pacer::reserve`] charged one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    tokens: u64,
}

impl Default for Pacer {
    fn default() -> Self {
        Pacer::new(START_TOKENS_PER_MIN)
    }
}

impl Pacer {
    pub fn new(tokens_per_min: u64) -> Pacer {
        Pacer::for_rate(Some(tokens_per_min))
    }

    /// Paced at `tokens_per_min`, or unpaced for None.
    pub fn for_rate(tokens_per_min: Option<u64>) -> Pacer {
        let max = tokens_per_min.map(|r| r.max(1));
        Pacer {
            max,
            rate: max.unwrap_or(0),
            next_ms: 0,
            pause_until_ms: 0,
            episode: false,
            quiet_since_ms: 0,
        }
    }

    /// The current rate, or None when unpaced.
    pub fn rate(&self) -> Option<u64> {
        self.max.map(|_| self.rate)
    }

    /// Whether a 429 episode is running (the rate is held down until a success after its wait).
    pub fn rate_limited_now(&self) -> bool {
        self.episode
    }

    /// How long to wait before the next dispatch.
    pub fn wait_ms(&self, now_ms: u64) -> u64 {
        self.next_ms.max(self.pause_until_ms).saturating_sub(now_ms)
    }

    fn cost(&self, tokens: u64) -> u64 {
        match self.max {
            Some(_) => tokens.saturating_mul(60_000) / self.rate.max(1),
            None => 0,
        }
    }

    /// Charges a call about to be sent with `tokens` estimated tokens.
    pub fn reserve(&mut self, tokens: usize, now_ms: u64) -> Ticket {
        self.recover(now_ms);
        let tokens = tokens as u64;
        let cost = self.cost(tokens);
        self.next_ms = self.next_ms.max(now_ms).saturating_add(cost);
        Ticket { tokens }
    }

    /// A call came back with vectors. Its charge is corrected by what the provider counted, when it says; the first
    /// success after a 429's wait ends the episode.
    pub fn succeeded(&mut self, ticket: Ticket, counted: Option<u64>, now_ms: u64) {
        if let Some(counted) = counted {
            if counted > ticket.tokens {
                self.next_ms = self
                    .next_ms
                    .saturating_add(self.cost(counted - ticket.tokens));
            } else {
                self.next_ms = self
                    .next_ms
                    .saturating_sub(self.cost(ticket.tokens - counted));
            }
        }
        if self.episode && now_ms >= self.pause_until_ms {
            self.episode = false;
            self.quiet_since_ms = now_ms;
        }
    }

    /// A call came back 429: the first in an episode halves the rate; every one waits at least `Retry-After`.
    pub fn rate_limited(&mut self, retry_after_ms: Option<u64>, now_ms: u64) {
        if !self.episode
            && let Some(max) = self.max
        {
            self.rate = (self.rate / 2).max((max / 8).max(1));
        }
        self.episode = true;
        self.quiet_since_ms = now_ms;
        let until = now_ms.saturating_add(retry_after_ms.unwrap_or(1_000));
        self.pause_until_ms = self.pause_until_ms.max(until);
    }

    fn recover(&mut self, now_ms: u64) {
        let Some(max) = self.max else { return };
        if self.episode {
            return;
        }
        while self.rate < max && now_ms.saturating_sub(self.quiet_since_ms) >= QUIET_RECOVERY_MS {
            self.rate = self.rate.saturating_add((max / 8).max(1)).min(max);
            self.quiet_since_ms = self.quiet_since_ms.saturating_add(QUIET_RECOVERY_MS);
        }
    }
}

/// What one paced provider call came to.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Vectors to apply through the writer, and what was cut, refused or truncated on the way (safe to log).
    Done {
        vectors: Vec<Vectors>,
        notes: Vec<EmbedNote>,
    },
    /// The provider said not now (429) or could not be reached: put the jobs back and wait.
    Retry { jobs: Vec<EmbedJob>, wait_ms: u64 },
    /// The provider refused the batch for good (bad input or response): these jobs are dropped.
    Failed {
        jobs: Vec<EmbedJob>,
        error: EmbedError,
    },
}

/// The estimated tokens of a batch, as [`Pacer::reserve`] charges them.
pub fn batch_tokens(batch: &[Planned]) -> usize {
    batch.iter().map(tokens).sum()
}

/// What a provider call for `batch` came to, settled with its instance's pacer (pure; the call itself ran with no
/// lock held, ws004 s25). Rate limits and an unavailable provider never fail a job; they come back as `Retry`.
pub fn settle(
    batch: &[Planned],
    result: Result<Counted, EmbedError>,
    pacer: &mut Pacer,
    ticket: Ticket,
    now_ms: u64,
) -> Step {
    let jobs = || batch.iter().map(|p| p.job.clone()).collect();
    match result {
        Ok((vectors, notes, counted)) => {
            pacer.succeeded(ticket, counted, now_ms);
            Step::Done { vectors, notes }
        }
        Err(EmbedError::RateLimited { retry_after_ms }) => {
            pacer.rate_limited(retry_after_ms, now_ms);
            Step::Retry {
                jobs: jobs(),
                wait_ms: pacer.wait_ms(now_ms),
            }
        }
        Err(EmbedError::Unavailable { .. }) => Step::Retry {
            jobs: jobs(),
            wait_ms: UNAVAILABLE_WAIT_MS,
        },
        Err(error) => Step::Failed {
            jobs: jobs(),
            error,
        },
    }
}

/// One provider call for `batch`, paced by `pacer`: [`Pacer::reserve`], the call, then [`settle`]. The caller waits
/// [`Pacer::wait_ms`] first.
pub fn step(embedder: &dyn Embedder, batch: &[Planned], pacer: &mut Pacer, now_ms: u64) -> Step {
    let ticket = pacer.reserve(batch_tokens(batch), now_ms);
    settle(batch, embed_counted(embedder, batch), pacer, ticket, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::Wanted;
    use alloc::string::String;
    use alloc::vec;
    use umx_core::Id;
    use umx_core::vectors::TextHash;

    fn planned(node: u128, texts: usize, bytes: usize) -> Planned {
        Planned {
            job: EmbedJob {
                alias: "g".into(),
                node: Id(node),
            },
            texts: (0..texts)
                .map(|i| Wanted {
                    field: alloc::format!("f{i}"),
                    hash: TextHash::of("x"),
                    text: String::from("a").repeat(bytes),
                })
                .collect(),
        }
    }

    #[test]
    fn batches_respect_the_text_and_token_limits_and_never_split_a_node() {
        let sizes: Vec<usize> = batches((0..10).map(|n| planned(n, 4, 10)).collect())
            .iter()
            .map(Vec::len)
            .collect();
        assert_eq!(sizes, [4, 4, 2], "16 texts a call");
        let big = MAX_BATCH_TOKENS; // about half the budget in tokens at 2 bytes a token
        let sizes: Vec<usize> = batches(vec![
            planned(1, 1, big),
            planned(2, 1, big),
            planned(3, 1, big),
        ])
        .iter()
        .map(Vec::len)
        .collect();
        assert_eq!(sizes, [2, 1]);
        let lone = batches(vec![planned(1, 20, 1), planned(2, 1, 1)]);
        assert_eq!(lone.iter().map(Vec::len).collect::<Vec<_>>(), [1, 1]);
    }

    #[test]
    fn queries_go_first_and_background_jobs_are_not_queued_twice() {
        let mut q = EmbedQueue::default();
        q.push(1);
        q.push(2);
        q.push(1);
        q.push_query(9);
        assert_eq!(q.len(), 3);
        assert_eq!(q.pop(2), [9, 1]);
        q.retry(vec![1]);
        assert_eq!(q.pop(10), [1, 2]);
        assert!(q.is_empty());
    }

    #[test]
    fn reserving_spaces_dispatches_and_the_providers_count_corrects_the_charge() {
        let mut p = Pacer::new(60_000);
        let t = p.reserve(1_000, 0);
        assert_eq!(p.wait_ms(0), 1_000, "1k tokens at 1k a second");
        p.reserve(1_000, 0);
        assert_eq!(p.wait_ms(0), 2_000, "calls in flight at once each pay");
        p.succeeded(t, Some(400), 10);
        assert_eq!(p.wait_ms(0), 1_400, "the provider counted 400, not 1000");
        p.succeeded(t, Some(1_600), 10);
        assert_eq!(p.wait_ms(0), 2_000, "or more");
        let mut free = Pacer::for_rate(None);
        free.reserve(1_000_000, 0);
        assert_eq!((free.rate(), free.wait_ms(0)), (None, 0), "unpaced");
        free.rate_limited(Some(3_000), 0);
        assert_eq!(free.wait_ms(0), 3_000, "a provider's wait still applies");
    }

    /// embeddings-expert (s25): one halving per 429 episode, however many calls in flight come back 429; the
    /// episode lasts until the first success after the wait; successes alone never win the rate back, quiet
    /// minutes do, an eighth of the configured rate each, never above it; the floor is an eighth.
    #[test]
    fn a_429_episode_halves_once_and_quiet_minutes_win_the_rate_back() {
        let mut p = Pacer::new(800_000);
        let tickets: Vec<Ticket> = (0..31).map(|_| p.reserve(10, 0)).collect();
        for _ in 0..31 {
            p.rate_limited(Some(2_000), 1_000);
        }
        assert_eq!((p.rate(), p.wait_ms(1_000)), (Some(400_000), 2_000));
        p.succeeded(tickets[0], None, 2_000);
        assert!(
            p.rate_limited_now(),
            "a success before the wait ends does not end the episode"
        );
        p.succeeded(tickets[1], None, 3_000);
        assert!(!p.rate_limited_now());
        for t in &tickets[2..] {
            p.succeeded(*t, None, 4_000);
        }
        p.reserve(10, 3_000 + QUIET_RECOVERY_MS - 1);
        assert_eq!(p.rate(), Some(400_000), "31 successes keep the rate halved");
        p.reserve(10, 3_000 + QUIET_RECOVERY_MS);
        assert_eq!(p.rate(), Some(500_000), "a quiet minute: +1/8");
        p.reserve(10, 3_000 + 10 * QUIET_RECOVERY_MS);
        assert_eq!(p.rate(), Some(800_000), "never above the configured rate");
        for episode in 0..10 {
            p.rate_limited(None, 1_000_000 + episode * 10_000);
            p.succeeded(tickets[0], None, 1_000_000 + episode * 10_000 + 5_000);
        }
        assert_eq!(p.rate(), Some(100_000), "the floor is an eighth");
    }
}
