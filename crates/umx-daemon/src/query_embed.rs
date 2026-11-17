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

    /// The query's vectors from `embedder`, one per phrasing (PRD 002 search-09: every phrasing in one call),
    /// waiting at most the bound from now for the whole call (any wait behind another search's embed included). On
    /// time-out the search goes on with words only; `warming` says the instance has never answered, so a local model
    /// may still be loading.
    pub(crate) fn embed_all(
        &self,
        embedder: &Arc<dyn Embedder>,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, QueryFailed> {
        self.ask(embedder, texts.to_vec(), TextKind::Query, Turn::Search)
    }

    /// [`Self::embed_all`] of one text.
    #[cfg(test)]
    pub(crate) fn embed(
        &self,
        embedder: &Arc<dyn Embedder>,
        text: &str,
    ) -> Result<Vec<f32>, QueryFailed> {
        self.embed_all(embedder, &[text.to_string()])
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

    /// For tests: the searches and related checks waiting (not running) on `embedder`'s lane.
    #[cfg(test)]
    fn waiting(&self, embedder: &Arc<dyn Embedder>) -> (usize, usize) {
        self.with_lane(embedder, |lane| {
            let p = lane.work.pending.lock().unwrap_or_else(|e| e.into_inner());
            (p.searches.len(), p.related.len())
        })
    }

    /// For tests: whether `embedder`'s lane has answered a call or warmed.
    #[cfg(test)]
    fn answered(&self, embedder: &Arc<dyn Embedder>) -> bool {
        self.with_lane(embedder, |lane| lane.warm.load(Ordering::SeqCst))
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

    /// Waits until `ready` holds. Never a test's proof: the 60 s only stops a broken test from hanging.
    fn wait_until(what: &str, ready: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(60);
        while !ready() {
            assert!(Instant::now() < until, "never: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The next call the gate started (its first text). A search that times out before the lane takes it is skipped
    /// and starts no call, so this waits at most 60 s and fails, never hangs.
    fn started(starts: &std::sync::mpsc::Receiver<String>) -> String {
        starts
            .recv_timeout(Duration::from_secs(60))
            .expect("no call started within 60 s")
    }

    /// Each call says it started (its first text) and then holds until the test lets one call go, so a test drives
    /// the lane's order itself and nothing in it depends on how long a step takes. Dropping the release lets every
    /// call go.
    struct Gate {
        started: Mutex<std::sync::mpsc::Sender<String>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        calls: AtomicUsize,
        readies: AtomicUsize,
    }

    /// A gate, the calls it started (in order) and its release (one send lets one call go).
    fn gate() -> (
        Arc<Gate>,
        std::sync::mpsc::Receiver<String>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (started, starts) = channel();
        let (go, release) = channel();
        let gate = Arc::new(Gate {
            started: Mutex::new(started),
            release: Mutex::new(release),
            calls: AtomicUsize::new(0),
            readies: AtomicUsize::new(0),
        });
        (gate, starts, go)
    }

    impl Embedder for Gate {
        fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.started.lock().unwrap().send(texts[0].clone());
            let _ = self.release.lock().unwrap().recv();
            Ok(Embedded {
                vectors: texts.iter().map(|_| vec![1.0, 0.0]).collect(),
                ..Embedded::default()
            })
        }
        fn key(&self) -> ModelKey {
            ModelKey::parse("gate@2").unwrap()
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
            self.readies.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn credential(&self) -> Option<Credential> {
            None
        }
        fn adapter_id(&self) -> &'static str {
            "test.gate"
        }
    }

    /// A query embed is bounded: with its call held, the search still comes back, timed out, and says warming while
    /// the instance has never answered. The held call finishes on its thread and warms the instance; the next timed-out
    /// search no longer says warming.
    #[test]
    fn a_slow_query_embed_times_out_at_the_bound_and_says_warming_until_one_answers() {
        let lanes = QueryLanes::with_bound(Duration::from_secs(1));
        let (inner, starts, go) = gate();
        let e: Arc<dyn Embedder> = inner.clone();
        assert_eq!(
            lanes.embed(&e, "q"),
            Err(QueryFailed::TimedOut { warming: true })
        );
        assert_eq!(started(&starts), "q");
        go.send(()).unwrap();
        wait_until("the held call finished", || !lanes.busy(&e));
        assert!(lanes.answered(&e), "the timed-out call warmed the instance");
        assert_eq!(
            lanes.embed(&e, "q2"),
            Err(QueryFailed::TimedOut { warming: false })
        );
        drop(go);
        wait_until("the lane drained", || !lanes.busy(&e));
    }

    /// PRD 002 search-09: a search's phrasings go to the instance in one call, one vector each, and the bound covers
    /// the whole call: with the call held, the search times out as one query does, warming until the instance has
    /// answered. Let go, the instance has made one call for all five.
    #[test]
    fn every_phrasing_goes_in_one_call_under_one_bound() {
        let lanes = QueryLanes::with_bound(Duration::from_secs(1));
        let (inner, starts, go) = gate();
        let e: Arc<dyn Embedder> = inner.clone();
        let five: Vec<String> = (1..=5).map(|i| format!("phrasing {i}")).collect();
        assert_eq!(
            lanes.embed_all(&e, &five),
            Err(QueryFailed::TimedOut { warming: true })
        );
        assert_eq!(started(&starts), "phrasing 1");
        drop(go);
        wait_until("the held call finished", || !lanes.busy(&e));
        assert_eq!(lanes.embed_all(&e, &five).map(|v| v.len()), Ok(5));
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2, "one call per search");
    }

    /// A search's budget counts its wait behind another search's embed, and a search that gave up is never run. The
    /// first search's call is held until the end, so the others can only time out waiting behind it, whatever the
    /// machine's speed; once it is let go, none of theirs runs. Not warming: the lane warmed first.
    #[test]
    fn a_second_search_waits_within_its_own_budget_and_abandoned_queries_are_skipped() {
        let lanes = QueryLanes::with_bound(Duration::from_secs(1));
        let (inner, starts, go) = gate();
        let e: Arc<dyn Embedder> = inner.clone();
        lanes.warm(&e);
        wait_until("the lane warmed", || lanes.answered(&e));
        std::thread::scope(|s| {
            let a = s.spawn(|| lanes.embed(&e, "a"));
            assert_eq!(started(&starts), "a");
            // a's call is held: b waits behind it and its own budget runs out meanwhile.
            let b = s.spawn(|| lanes.embed(&e, "b"));
            assert_eq!(
                b.join().unwrap(),
                Err(QueryFailed::TimedOut { warming: false })
            );
            // More searches pile up behind the held call and give up too.
            for _ in 0..5 {
                assert_eq!(
                    lanes.embed(&e, "c"),
                    Err(QueryFailed::TimedOut { warming: false })
                );
            }
            assert_eq!(lanes.waiting(&e), (6, 0), "b and the five wait, abandoned");
            // a outlived its own budget too: it finishes on its thread, its search long gone.
            assert_eq!(
                a.join().unwrap(),
                Err(QueryFailed::TimedOut { warming: false })
            );
            // Lets a go, and any call after it: one that ran would count.
            drop(go);
        });
        wait_until("the lane drained", || !lanes.busy(&e));
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            1,
            "only a ran: the abandoned searches were skipped"
        );
    }

    /// ws004 s28 (embeddings-expert): a related check never takes the query slot from a waiting search. With one
    /// related check running and another queued, a search that arrives next runs before the queued one. Each step
    /// waits for the lane to hold what it should (a call started, a check or search queued), never for time.
    #[test]
    fn a_search_queued_behind_a_related_check_still_gets_its_slot() {
        let lanes = QueryLanes::with_bound(Duration::from_secs(60));
        let (inner, starts, go) = gate();
        let e: Arc<dyn Embedder> = inner.clone();
        std::thread::scope(|s| {
            let r1 = s.spawn(|| {
                lanes.embed_related(&e, vec!["r1".into(), "r1b".into()], TextKind::Query)
            });
            assert_eq!(started(&starts), "r1");
            let r2 = s.spawn(|| lanes.embed_related(&e, vec!["r2".into()], TextKind::Query));
            wait_until("r2 queued", || lanes.waiting(&e) == (0, 1));
            let q = s.spawn(|| lanes.embed(&e, "q"));
            wait_until("q queued", || lanes.waiting(&e) == (1, 1));
            let mut order = vec!["r1".to_string()];
            for _ in 0..2 {
                go.send(()).unwrap();
                order.push(started(&starts));
            }
            go.send(()).unwrap();
            assert_eq!(order, ["r1", "q", "r2"]);
            assert_eq!(q.join().unwrap(), Ok(vec![1.0, 0.0]));
            assert_eq!(r1.join().unwrap().map(|v| v.len()), Ok(2));
            assert!(r2.join().unwrap().is_ok());
        });
    }

    /// A graph's load warms its local instance once (`ready()`, which embeds nothing), so its first search finds
    /// the model loaded; a hosted instance has nothing to load and gets no lane. Waits for the warm-up, never for time.
    #[test]
    fn warming_loads_a_local_instance_once_and_never_a_hosted_one() {
        let lanes = QueryLanes::with_bound(Duration::from_secs(60));
        let (inner, _starts, go) = gate();
        drop(go);
        let local: Arc<dyn Embedder> = inner.clone();
        lanes.warm(&local);
        wait_until("the local instance warmed", || lanes.answered(&local));
        lanes.warm(&local);
        assert_eq!(lanes.embed(&local, "q").map(|v| v.len()), Ok(2));
        assert_eq!(inner.readies.load(Ordering::SeqCst), 1, "warmed once");
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            1,
            "warming embeds nothing"
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
