//! Query embeds for `umx search`, bounded in time (req-53, ws004 s08; embeddings-expert and architect).
//!
//! Each embedder instance gets one query thread, started on first use, so at most one query embed runs per
//! instance. A search hands its embed to that thread and waits at most [`QUERY_EMBED_BOUND`] (its own budget,
//! counting any wait behind another search's embed); past that it gives up and searches with words only
//! (`similarity_unavailable`). A call that is already running cannot be cancelled (a blocking HTTP or llama.cpp
//! call), so it finishes on its thread; a queued one whose search gave up is skipped. A cold local model loads on
//! the first call, so a timed-out first search warms it for the next. While a query embed waits or runs on an
//! instance, the embed worker sends that instance no new background batch, so a query never queues behind ingest.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use umx_core::ports::{Embedder, Locality, TextKind};
use umx_services::QueryFailed;

/// How long a search waits for its query vector (req-53).
pub(crate) const QUERY_EMBED_BOUND: Duration = Duration::from_secs(5);

enum Work {
    Query {
        text: String,
        reply: Sender<Result<Vec<f32>, QueryFailed>>,
        abandoned: Arc<AtomicBool>,
    },
    /// Load a local model (`ready()`, which embeds nothing) because a graph that uses it loaded.
    Warm,
}

struct Lane {
    work: Sender<Work>,
    /// Queries waiting or running on this instance.
    busy: Arc<AtomicUsize>,
    /// The instance has answered a call (query or warm-up): its model is loaded.
    warm: Arc<AtomicBool>,
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
            let (work, rx) = channel();
            let lane = Lane {
                work,
                busy: Arc::new(AtomicUsize::new(0)),
                warm: Arc::new(AtomicBool::new(false)),
            };
            let (busy, warm, embedder) = (lane.busy.clone(), lane.warm.clone(), embedder.clone());
            std::thread::spawn(move || run(&*embedder, &rx, &busy, &warm));
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
        let deadline = Instant::now() + self.bound();
        let (reply, answer) = channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        let warm = self.with_lane(embedder, |lane| {
            lane.busy.fetch_add(1, Ordering::SeqCst);
            let work = Work::Query {
                text: text.to_string(),
                reply,
                abandoned: abandoned.clone(),
            };
            if lane.work.send(work).is_err() {
                lane.busy.fetch_sub(1, Ordering::SeqCst);
            }
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
                let _ = lane.work.send(Work::Warm);
            }
        });
    }
}

fn run(embedder: &dyn Embedder, rx: &Receiver<Work>, busy: &AtomicUsize, warm: &AtomicBool) {
    for work in rx {
        match work {
            Work::Warm => {
                if !warm.load(Ordering::SeqCst) && embedder.ready().is_ok() {
                    warm.store(true, Ordering::SeqCst);
                }
            }
            Work::Query {
                text,
                reply,
                abandoned,
            } => {
                // A search that gave up while this waited no longer wants it: skip, so a backlog drains at once.
                if !abandoned.load(Ordering::SeqCst) {
                    let result = embedder
                        .embed(&[text], TextKind::Query)
                        .map(|mut e| e.vectors.pop().unwrap_or_default())
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
