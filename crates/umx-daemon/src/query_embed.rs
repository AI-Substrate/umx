//! Query embeds for `umx search`, bounded in time (req-53, ws004 s08; embeddings-expert and architect).
//!
//! Each embedder instance gets one query thread, started on first use, so at most one query embed runs per
//! instance. A search hands its embed to that thread and waits at most [`QUERY_EMBED_BOUND`] (its own budget,
//! counting any wait behind another search's embed); past that it gives up and searches with words only
//! (`similarity_unavailable`). A call that is already running cannot be cancelled (a blocking HTTP or llama.cpp
//! call), so it finishes on its thread; a queued one whose search gave up is skipped. A cold local model loads on
//! the first call, so a timed-out first search warms it for the next. While a query embed waits or runs on an
//! instance, the embed worker sends that instance no new background batch, so a query never queues behind ingest.
//!
//! A write's related check (req-123) embeds its texts in one request on the same thread, under the same bound, but a
//! search waiting there always goes first (embeddings-expert, ws004 s28): the query slot is the search's, and a
//! related check waits at most one search's embed. One already running finishes first; its cap keeps it near 2 s.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use umx_core::ports::{Embedder, Locality, TextKind};
use umx_services::QueryFailed;

/// How long a search waits for its query vector (req-53).
pub(crate) const QUERY_EMBED_BOUND: Duration = Duration::from_secs(5);

enum Work {
    Query {
        texts: Vec<String>,
        kind: TextKind,
        reply: Sender<Result<Vec<Vec<f32>>, QueryFailed>>,
        abandoned: Arc<AtomicBool>,
    },
    /// Load a local model (`ready()`, which embeds nothing) because a graph that uses it loaded.
    Warm,
}

/// A lane's waiting work: searches first, then related checks, then a warm-up.
#[derive(Default)]
struct Pending {
    searches: VecDeque<Work>,
    related: VecDeque<Work>,
    warm: VecDeque<Work>,
    closed: bool,
}

#[derive(Default)]
struct Inbox {
    pending: Mutex<Pending>,
    ready: Condvar,
}

/// Which queue a piece of work joins.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    Search,
    Related,
}

impl Inbox {
    fn send(&self, turn: Option<Turn>, work: Work) {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        match turn {
            Some(Turn::Search) => p.searches.push_back(work),
            Some(Turn::Related) => p.related.push_back(work),
            None => p.warm.push_back(work),
        }
        self.ready.notify_one();
    }

    /// The next work, a waiting search first; None once the lane is closed.
    fn next(&self) -> Option<Work> {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(w) = p
                .searches
                .pop_front()
                .or_else(|| p.related.pop_front())
                .or_else(|| p.warm.pop_front())
            {
                return Some(w);
            }
            if p.closed {
                return None;
            }
            p = self.ready.wait(p).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close(&self) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        self.ready.notify_all();
    }
}

struct Lane {
    work: Arc<Inbox>,
    /// Queries waiting or running on this instance.
    busy: Arc<AtomicUsize>,
    /// The instance has answered a call (query or warm-up): its model is loaded.
    warm: Arc<AtomicBool>,
}

impl Drop for Lane {
    fn drop(&mut self) {
        self.work.close();
    }
}

/// The query threads, by instance (the address of its adapter, which lives as long as the daemon's services).
#[derive(Default)]
pub(crate) struct QueryLanes {
    lanes: Mutex<BTreeMap<usize, Lane>>,
    bound: Option<Duration>,
}

fn id(embedder: &Arc<dyn Embedder>) -> usize {
    Arc::as_ptr(embedder) as *const () as usize
}

impl QueryLanes {
    pub(crate) fn new() -> QueryLanes {
        QueryLanes::default()
    }

    /// For tests: a shorter bound than [`QUERY_EMBED_BOUND`].
    #[cfg(test)]
    pub(crate) fn with_bound(bound: Duration) -> QueryLanes {
        QueryLanes {
            bound: Some(bound),
            ..QueryLanes::default()
        }
    }

    fn bound(&self) -> Duration {
        self.bound.unwrap_or(QUERY_EMBED_BOUND)
    }

    /// Runs `f` on `embedder`'s lane, starting its thread on first use.
    fn with_lane<T>(&self, embedder: &Arc<dyn Embedder>, f: impl FnOnce(&Lane) -> T) -> T {
        let mut lanes = self.lanes.lock().unwrap_or_else(|e| e.into_inner());
        let lane = lanes.entry(id(embedder)).or_insert_with(|| {
            let lane = Lane {
                work: Arc::new(Inbox::default()),
                busy: Arc::new(AtomicUsize::new(0)),
                warm: Arc::new(AtomicBool::new(false)),
            };
            let (inbox, busy, warm, embedder) = (
                lane.work.clone(),
                lane.busy.clone(),
                lane.warm.clone(),
                embedder.clone(),
            );
            std::thread::spawn(move || run(&*embedder, &inbox, &busy, &warm));
            lane
        });
        f(lane)
    }

    /// The query's vector from `embedder`, waiting at most the bound from now (any wait behind another search's
    /// embed included). On time-out the search goes on with words only; `warming` says the instance has never
    /// answered, so a local model may still be loading.
    pub(crate) fn embed(
        &self,
        embedder: &Arc<dyn Embedder>,
        text: &str,
    ) -> Result<Vec<f32>, QueryFailed> {
        self.ask(
            embedder,
            vec![text.to_string()],
            TextKind::Query,
            Turn::Search,
        )
        .map(|mut v| v.pop().unwrap_or_default())
    }

    /// A write's related check (req-123): several texts of one `kind` in one request, under the same bound, taken
    /// only when no search is waiting on the instance.
    pub(crate) fn embed_related(
        &self,
        embedder: &Arc<dyn Embedder>,
        texts: Vec<String>,
        kind: TextKind,
    ) -> Result<Vec<Vec<f32>>, QueryFailed> {
        self.ask(embedder, texts, kind, Turn::Related)
    }

    fn ask(
        &self,
        embedder: &Arc<dyn Embedder>,
        texts: Vec<String>,
        kind: TextKind,
        turn: Turn,
    ) -> Result<Vec<Vec<f32>>, QueryFailed> {
        let deadline = Instant::now() + self.bound();
        let (reply, answer) = channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        let warm = self.with_lane(embedder, |lane| {
            lane.busy.fetch_add(1, Ordering::SeqCst);
            let work = Work::Query {
                texts,
                kind,
                reply,
                abandoned: abandoned.clone(),
            };
            lane.work.send(Some(turn), work);
            lane.warm.clone()
        });
        match answer.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(_) => {
                abandoned.store(true, Ordering::SeqCst);
                Err(QueryFailed::TimedOut {
                    warming: embedder.locality() == Locality::Local && !warm.load(Ordering::SeqCst),
                })
            }
        }
    }

    /// Whether a query embed is waiting or running on `embedder` (the embed worker holds its batches meanwhile).
    pub(crate) fn busy(&self, embedder: &Arc<dyn Embedder>) -> bool {
        self.lanes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id(embedder))
            .is_some_and(|l| l.busy.load(Ordering::SeqCst) > 0)
    }

    /// Loads a local instance's model in the background if it has not answered yet (a graph that uses it loaded).
    /// Hosted instances have nothing to load.
    pub(crate) fn warm(&self, embedder: &Arc<dyn Embedder>) {
        if embedder.locality() != Locality::Local {
            return;
        }
        self.with_lane(embedder, |lane| {
            if !lane.warm.load(Ordering::SeqCst) {
                lane.work.send(None, Work::Warm);
            }
        });
    }
}

fn run(embedder: &dyn Embedder, inbox: &Inbox, busy: &AtomicUsize, warm: &AtomicBool) {
    while let Some(work) = inbox.next() {
        match work {
            Work::Warm => {
                if !warm.load(Ordering::SeqCst) && embedder.ready().is_ok() {
                    warm.store(true, Ordering::SeqCst);
                }
            }
            Work::Query {
                texts,
                kind,
                reply,
                abandoned,
            } => {
                // A search that gave up while this waited no longer wants it: skip, so a backlog drains at once.
                if !abandoned.load(Ordering::SeqCst) {
                    let result = embedder
                        .embed(&texts, kind)
                        .map(|e| e.vectors)
                        .map_err(QueryFailed::Embed);
                    if result.is_ok() {
                        warm.store(true, Ordering::SeqCst);
                    }
                    let _ = reply.send(result);
                }
                busy.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_core::ports::{Credential, EmbedError, Embedded, ModelKey};

    /// Sleeps `delay` per call, then gives a fixed vector; counts calls.
    struct Slow {
        delay: Duration,
        calls: AtomicUsize,
        locality: Locality,
    }

    impl Embedder for Slow {
        fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            Ok(Embedded {
                vectors: texts.iter().map(|_| vec![1.0, 0.0]).collect(),
                ..Embedded::default()
            })
        }
        fn key(&self) -> ModelKey {
            ModelKey::parse("slow@2").unwrap()
        }
        fn concurrency_ceiling(&self) -> usize {
            1
        }
        fn max_input_tokens(&self) -> usize {
            512
        }
        fn locality(&self) -> Locality {
            self.locality
        }
        fn ready(&self) -> Result<(), EmbedError> {
            std::thread::sleep(self.delay);
            Ok(())
        }
        fn credential(&self) -> Option<Credential> {
            None
        }
        fn adapter_id(&self) -> &'static str {
            "test.slow"
        }
    }

    fn slow(ms: u64) -> Arc<dyn Embedder> {
        Arc::new(Slow {
            delay: Duration::from_millis(ms),
            calls: AtomicUsize::new(0),
            locality: Locality::Local,
        })
    }

    #[test]
    fn a_slow_query_embed_times_out_at_the_bound_and_says_warming_until_one_answers() {
        let lanes = QueryLanes::with_bound(Duration::from_millis(200));
        let e = slow(600);
        let started = Instant::now();
        assert_eq!(
            lanes.embed(&e, "q"),
            Err(QueryFailed::TimedOut { warming: true })
        );
        let took = started.elapsed();
        assert!(took < Duration::from_millis(450), "bounded: {took:?}");
        // The timed-out call finishes on its thread and warms the instance; then the lane is idle again.
        std::thread::sleep(Duration::from_millis(700));
        assert!(!lanes.busy(&e));
        let fast = QueryLanes::with_bound(Duration::from_secs(5));
        assert_eq!(fast.embed(&e, "q"), Ok(vec![1.0, 0.0]));
    }

    #[test]
    fn a_second_search_waits_within_its_own_budget_and_abandoned_queries_are_skipped() {
        let lanes = QueryLanes::with_bound(Duration::from_millis(300));
        let inner = Arc::new(Slow {
            delay: Duration::from_millis(200),
            calls: AtomicUsize::new(0),
            locality: Locality::Local,
        });
        let e: Arc<dyn Embedder> = inner.clone();
        // Two searches at once: the second waits behind the first (200 ms) and still answers inside its 300 ms.
        std::thread::scope(|s| {
            let a = s.spawn(|| lanes.embed(&e, "a"));
            std::thread::sleep(Duration::from_millis(20));
            let b = s.spawn(|| lanes.embed(&e, "b"));
            assert!(a.join().unwrap().is_ok());
            // b waited ~180 ms then ran 200 ms: past its budget, so it timed out (not warming: a answered).
            assert_eq!(
                b.join().unwrap(),
                Err(QueryFailed::TimedOut { warming: false })
            );
        });
        // Five searches pile up behind a slow call; those that gave up before their turn are never run.
        let lanes = QueryLanes::with_bound(Duration::from_millis(50));
        let before = inner.calls.load(Ordering::SeqCst);
        for _ in 0..5 {
            assert!(lanes.embed(&e, "c").is_err());
        }
        std::thread::sleep(Duration::from_millis(1200));
        let ran = inner.calls.load(Ordering::SeqCst) - before;
        assert!(ran < 5, "abandoned queries were skipped: {ran} ran");
        assert!(!lanes.busy(&e));
    }

    /// Embeds each call's first text after `delay`, logging it, so a test sees the order calls ran in.
    struct Logged {
        delay: Duration,
        seen: Mutex<Vec<String>>,
    }

    impl Embedder for Logged {
        fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
            std::thread::sleep(self.delay);
            self.seen.lock().unwrap().push(texts[0].clone());
            Ok(Embedded {
                vectors: texts.iter().map(|_| vec![1.0, 0.0]).collect(),
                ..Embedded::default()
            })
        }
        fn key(&self) -> ModelKey {
            ModelKey::parse("logged@2").unwrap()
        }
        fn concurrency_ceiling(&self) -> usize {
            1
        }
        fn max_input_tokens(&self) -> usize {
            512
        }
        fn locality(&self) -> Locality {
            Locality::Local
        }
        fn ready(&self) -> Result<(), EmbedError> {
            Ok(())
        }
        fn credential(&self) -> Option<Credential> {
            None
        }
        fn adapter_id(&self) -> &'static str {
            "test.logged"
        }
    }

    /// ws004 s28 (embeddings-expert): a related check never takes the query slot from a waiting search. With one
    /// related check running and another queued, a search that arrives next runs before the queued one and answers
    /// inside its bound.
    #[test]
    fn a_search_queued_behind_a_related_check_still_gets_its_slot() {
        let lanes = QueryLanes::with_bound(Duration::from_secs(5));
        let inner = Arc::new(Logged {
            delay: Duration::from_millis(150),
            seen: Mutex::new(Vec::new()),
        });
        let e: Arc<dyn Embedder> = inner.clone();
        std::thread::scope(|s| {
            let r1 = s.spawn(|| {
                lanes.embed_related(&e, vec!["r1".into(), "r1b".into()], TextKind::Query)
            });
            std::thread::sleep(Duration::from_millis(40));
            let r2 = s.spawn(|| lanes.embed_related(&e, vec!["r2".into()], TextKind::Query));
            std::thread::sleep(Duration::from_millis(40));
            let q = s.spawn(|| lanes.embed(&e, "q"));
            assert_eq!(q.join().unwrap(), Ok(vec![1.0, 0.0]));
            assert_eq!(r1.join().unwrap().map(|v| v.len()), Ok(2));
            assert!(r2.join().unwrap().is_ok());
        });
        assert_eq!(*inner.seen.lock().unwrap(), ["r1", "q", "r2"]);
    }

    #[test]
    fn warming_loads_a_local_instance_once_and_never_a_hosted_one() {
        let lanes = QueryLanes::with_bound(Duration::from_millis(50));
        let local = slow(10);
        lanes.warm(&local);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            lanes.embed(&local, "q").map(|v| v.len()),
            Ok(2),
            "warm: answered inside 50 ms"
        );
        let hosted: Arc<dyn Embedder> = Arc::new(Slow {
            delay: Duration::from_millis(10),
            calls: AtomicUsize::new(0),
            locality: Locality::Hosted,
        });
        lanes.warm(&hosted);
        assert!(lanes.lanes.lock().unwrap().get(&id(&hosted)).is_none());
    }
}
