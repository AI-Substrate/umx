//! The embed worker (req-35, architect F1) makes vectors off the writer. One dispatcher thread plans under the shared
//! lock, keeps each instance's pacer and pause, and hands batches to that instance's call threads, up to its
//! concurrency ceiling at once (ws004 s25). A call thread runs the provider call holding no lock and posts the
//! outcome back; the dispatcher sends the vectors to the writer as `Job::Apply`, so a slow or retrying provider never
//! delays a write. Pacing and batching are umx-services' (`Pacer`, `settle`, `batches`).

use crate::Job;
use crate::query_embed::QueryLanes;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};
use umx_core::ports::{EmbedError, Embedder, GraphDir};
use umx_services::embedding::{Counted, Instance, embed_counted};
use umx_services::{
    EmbedJob, EmbedQueue, Pacer, Planned, Services, Step, Ticket, Vectors, batch_tokens, batches,
    settle,
};

pub(crate) type Queue = Arc<(Mutex<EmbedQueue<EmbedJob>>, Condvar)>;

/// What the writer and the embed worker share about the backlog (ws004 s25a): how many Applies are in flight, and
/// whether the dispatcher has work in hand, so the writer knows when the backlog has gone quiet.
#[derive(Default)]
pub(crate) struct Drain {
    /// (Applies sent and not yet written, the writer has stopped).
    ahead: Mutex<(usize, bool)>,
    busy: AtomicBool,
    /// Each instance's background calls so far, for `daemon status` (ws004 s25).
    stats: Mutex<BTreeMap<String, LaneStats>>,
}

/// One instance's background embedding, as `daemon status` shows it (ws004 s25; security: name and rate only, never
/// an endpoint, deployment or key).
#[derive(Clone, Debug, Default)]
pub(crate) struct LaneStats {
    pub(crate) slots: usize,
    pub(crate) in_flight: usize,
    pub(crate) calls: u64,
    /// Tokens the provider said it counted (calls that say).
    pub(crate) tokens_counted: u64,
    pub(crate) rate_limited: u64,
    /// The configured pace and the current rate, in tokens a minute; None: unpaced.
    pub(crate) tokens_per_minute: Option<u64>,
    pub(crate) rate: Option<u64>,
    pub(crate) last_halved: Option<Instant>,
}

impl Drain {
    /// Counts one more Apply in flight if fewer than `cap` are; never blocks, so the dispatcher keeps collecting
    /// finished calls meanwhile (ws004 s25 D1). Always room once the writer has stopped: the send then fails and the
    /// worker ends.
    fn take_room(&self, cap: usize) -> bool {
        let mut ahead = self.ahead.lock().unwrap_or_else(|e| e.into_inner());
        if ahead.0 >= cap && !ahead.1 {
            return false;
        }
        ahead.0 += 1;
        true
    }

    /// Each instance's background calls so far.
    pub(crate) fn stats(&self) -> BTreeMap<String, LaneStats> {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn note(&self, name: &str, f: impl FnOnce(&mut LaneStats)) {
        f(self
            .stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(name.to_string())
            .or_default());
    }

    /// Whether [`Self::take_room`] would succeed now.
    fn room_for(&self, cap: usize) -> bool {
        let ahead = self.ahead.lock().unwrap_or_else(|e| e.into_inner());
        ahead.0 < cap || ahead.1
    }

    /// The writer has written `n` Applies' last units; the dispatcher may send more.
    pub(crate) fn applied(&self, n: usize, queue: &Queue) {
        if n == 0 {
            return;
        }
        let mut ahead = self.ahead.lock().unwrap_or_else(|e| e.into_inner());
        ahead.0 = ahead.0.saturating_sub(n);
        drop(ahead);
        wake(queue);
    }

    /// The writer has stopped: the dispatcher's next send fails and it ends.
    pub(crate) fn close(&self, queue: &Queue) {
        self.ahead.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        wake(queue);
    }

    /// Nothing queued, no round in progress and no Apply in flight: the backlog has drained (or is waiting on
    /// nothing the writer can see).
    pub(crate) fn quiet(&self, queue: &Queue) -> bool {
        let q = queue.0.lock().unwrap_or_else(|e| e.into_inner());
        q.is_empty()
            && !self.busy.load(Ordering::SeqCst)
            && self.ahead.lock().unwrap_or_else(|e| e.into_inner()).0 == 0
    }
}

/// Whether a graph folder's own config lets its graphs use the named instance: a repo layer may narrow to none or
/// to one local instance, so a graph in such a repository is never sent to another instance the machine built.
pub(crate) type FolderAllows = Arc<dyn Fn(&GraphDir, &str) -> bool + Send + Sync>;

/// `batch` without what may no longer go to `instance`, read right before sending: nodes of graphs whose instance
/// is no longer this one (secured, reembedded or unloaded since planning) and of graphs whose folder config narrows
/// it away. Holds the shared lock only while it reads.
fn sendable(
    services: &RwLock<Services>,
    allows: &FolderAllows,
    instance: &Instance,
    batch: Vec<Planned>,
) -> Vec<Planned> {
    let s = services.read().unwrap_or_else(|e| e.into_inner());
    s.embedding()
        .still_sendable_where(s.graph(), &instance.embedder, batch, &**allows)
}

/// Puts jobs at the BACK of the background lane: a group in backoff waits its turn behind newer work, so it never
/// crowds the next round and holds up another instance's jobs (R106-2).
fn defer(queue: &Queue, jobs: impl IntoIterator<Item = EmbedJob>) {
    let mut q = queue.0.lock().unwrap_or_else(|e| e.into_inner());
    for job in jobs {
        q.push(job);
    }
}

/// How long a batch for an instance with a query in flight waits before it is looked at again.
const QUERY_WAIT_MS: u64 = 20;

/// Nodes planned per round, at most: enough to fill a few provider calls.
const PLAN_PER_ROUND: usize = 64;

/// Nodes planned for each free slot: about one provider batch (16 texts of 2 to 4 per node).
const NODES_PER_BATCH: usize = 8;

/// Queues the writer's new embed jobs and wakes the worker. Without an embedder nothing is ever queued.
pub(crate) fn enqueue(queue: &Queue, jobs: Vec<EmbedJob>) {
    if jobs.is_empty() {
        return;
    }
    let (lock, ready) = &**queue;
    let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
    for job in jobs {
        q.push(job);
    }
    ready.notify_one();
}

/// A diagnostic on the daemon's stderr for a job that could not be planned, rendered through the catalogue (safe
/// text only).
fn report(job: &EmbedJob, problem: &umx_core::Problem) {
    let issue = crate::verbs::issue(problem, "embeddings repair");
    eprintln!(
        "umx embed: graph {} node {}: {} ({})",
        job.alias,
        job.node,
        issue.message(),
        issue.code()
    );
}

/// The kind of a provider failure, in umx's words: a provider's own message may echo input text.
fn failure_kind(error: &umx_core::ports::EmbedError) -> &'static str {
    use umx_core::ports::EmbedError as E;
    match error {
        E::Unavailable { .. } => "embedder unavailable",
        E::EmptyInput { .. } => "an empty input",
        E::InputTooLong { .. } => "an input over the embedder's cap",
        E::BadResponse(_) => "a bad response from the embedder",
        E::RateLimited { .. } => "rate limited",
        E::Config(_) => "an embedder configuration error",
    }
}

/// Wakes the dispatcher: under the queue's lock, so a wakeup is never lost between its check and its wait.
fn wake(queue: &Queue) {
    let _held = queue.0.lock().unwrap_or_else(|e| e.into_inner());
    queue.1.notify_all();
}

/// Background calls an instance may have in flight at once (ws004 s25): its concurrency ceiling less one, kept for
/// its query lane (`query_embed`), and at least one. Azure (32): 31; llama.cpp with one session: 1, as before.
pub(crate) fn slots(embedder: &dyn Embedder) -> usize {
    embedder
        .concurrency_ceiling()
        .saturating_sub(1)
        .clamp(1, 32)
}

/// One provider call handed to a call thread.
struct Call {
    instance: Instance,
    batch: Vec<Planned>,
    ticket: Ticket,
}

/// A call's outcome, posted back to the dispatcher.
struct Finished {
    instance: Instance,
    batch: Vec<Planned>,
    ticket: Ticket,
    result: Result<Counted, EmbedError>,
}

type Outcomes = Arc<Mutex<Vec<Finished>>>;

/// An instance's calls: its call threads (started on first use, ended when the dispatcher drops the sender), how
/// many are in flight, its pacer, and how long it is paused (429 or unreachable: only this instance waits, R106-2).
struct Lane {
    calls: Sender<Call>,
    slots: usize,
    in_flight: usize,
    pacer: Pacer,
    paused_until: u64,
}

impl Lane {
    fn start(instance: &Instance, pace: Option<u64>, outcomes: &Outcomes, queue: &Queue) -> Lane {
        let slots = slots(&*instance.embedder);
        let (calls, rx) = channel::<Call>();
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..slots {
            let (rx, outcomes, queue) = (Arc::clone(&rx), Arc::clone(outcomes), Arc::clone(queue));
            std::thread::spawn(move || call_thread(&rx, &outcomes, &queue));
        }
        Lane {
            calls,
            slots,
            in_flight: 0,
            pacer: Pacer::for_rate(pace),
            paused_until: 0,
        }
    }
}

/// Runs provider calls one at a time, holding no lock, until its lane is dropped. A call that panics fails its
/// batch, never the thread's count.
fn call_thread(rx: &Mutex<Receiver<Call>>, outcomes: &Outcomes, queue: &Queue) {
    loop {
        let call = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let Ok(Call {
            instance,
            batch,
            ticket,
        }) = call
        else {
            return;
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            embed_counted(&*instance.embedder, &batch)
        }))
        .unwrap_or_else(|_| Err(EmbedError::BadResponse("the embedder panicked".into())));
        outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Finished {
                instance,
                batch,
                ticket,
                result,
            });
        wake(queue);
    }
}

/// Plans up to `nodes` (at most [`PLAN_PER_ROUND`]) queued nodes under the shared lock, each graph's policy read once
/// (architect) and its nodes grouped by the instance that graph embeds with, as provider-sized batches. A job for an
/// instance in `blocked` (paused, paced or with every slot busy) goes back to the end of the queue unplanned and the
/// round reads on, at most once through the queue, so one instance's wait never hides another's work (R201-2). The flag
/// is true when the queue held only such jobs: plan again when a wait ends or work arrives.
fn plan_round(
    services: &RwLock<Services>,
    queue: &Queue,
    allows: &FolderAllows,
    drain: &Drain,
    nodes: usize,
    blocked: &BTreeSet<String>,
) -> (Vec<(Instance, Vec<Planned>)>, bool) {
    let want = nodes.clamp(1, PLAN_PER_ROUND);
    let mut left = {
        let q = queue.0.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() {
            return (Vec::new(), false);
        }
        // Set under the queue's lock, so the writer never sees an empty queue with this round unseen.
        drain.busy.store(true, Ordering::SeqCst);
        q.len()
    };
    let mut jobs: Vec<EmbedJob> = Vec::new();
    let mut put_back = 0;
    while jobs.len() < want && left > 0 {
        let popped = queue
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop((want - jobs.len()).min(left));
        if popped.is_empty() {
            break;
        }
        left -= popped.len();
        let mut back = Vec::new();
        {
            let s = services.read().unwrap_or_else(|e| e.into_inner());
            let mut chosen: BTreeMap<String, Option<Instance>> = BTreeMap::new();
            for j in popped {
                let instance = chosen
                    .entry(j.alias.clone())
                    .or_insert_with(|| instance_for(&s, allows, &j.alias));
                match instance {
                    Some(i) if blocked.contains(&i.name) => back.push(j),
                    _ => jobs.push(j),
                }
            }
        }
        put_back += back.len();
        defer(queue, back);
    }
    let only_blocked = jobs.is_empty() && put_back > 0;
    let s = services.read().unwrap_or_else(|e| e.into_inner());
    let mut groups: BTreeMap<String, (Instance, Vec<Planned>)> = BTreeMap::new();
    let mut chosen: BTreeMap<String, Option<Instance>> = BTreeMap::new();
    for (j, planned) in jobs.iter().zip(s.embed_plan_all(&jobs)) {
        let planned = match planned {
            Ok(Some(planned)) => planned,
            Ok(None) => continue,
            Err(problem) => {
                report(j, &problem);
                continue;
            }
        };
        let instance = chosen
            .entry(j.alias.clone())
            .or_insert_with(|| instance_for(&s, allows, &j.alias));
        if let Some(instance) = instance {
            groups
                .entry(instance.name.clone())
                .or_insert_with(|| (instance.clone(), Vec::new()))
                .1
                .push(planned);
        }
    }
    let batches = groups
        .into_values()
        .flat_map(|(instance, planned)| {
            batches(planned)
                .into_iter()
                .map(move |b| (instance.clone(), b))
        })
        .collect();
    (batches, only_blocked)
}

/// The instance a graph embeds with now: the folder rule filters first, then the tie-break picks among what is
/// allowed (R106-3).
fn instance_for(s: &Services, allows: &FolderAllows, alias: &str) -> Option<Instance> {
    let dir = s.loaded(alias).map(|l| l.dir.clone())?;
    s.embedding()
        .embedder_for_where(s.graph(), alias, &|n| allows(&dir, n))
}

pub(crate) fn spawn(
    services: Arc<RwLock<Services>>,
    queue: Queue,
    writer: Sender<Job>,
    allows: FolderAllows,
    queries: Arc<QueryLanes>,
    drain: Arc<Drain>,
) {
    std::thread::spawn(move || dispatch(&services, &queue, &writer, &allows, &queries, &drain));
}

/// The dispatcher's loop (ws004 s25): collect finished calls, hand vectors to the writer while it has room, plan
/// when fewer batches are ready than slots are free, start calls where an instance has a free slot, is not paced or
/// paused and has no query waiting, then sleep until something changes.
fn dispatch(
    services: &RwLock<Services>,
    queue: &Queue,
    writer: &Sender<Job>,
    allows: &FolderAllows,
    queries: &QueryLanes,
    drain: &Drain,
) {
    let started = Instant::now();
    let now = || started.elapsed().as_millis() as u64;
    let outcomes: Outcomes = Arc::default();
    let mut lanes: BTreeMap<String, Lane> = BTreeMap::new();
    let mut ready: VecDeque<(Instance, Vec<Planned>)> = VecDeque::new();
    let mut to_apply: VecDeque<Vec<Vectors>> = VecDeque::new();
    loop {
        // 1. Finished calls.
        let finished = std::mem::take(&mut *outcomes.lock().unwrap_or_else(|e| e.into_inner()));
        for f in finished {
            let at = now();
            let lane = lanes
                .get_mut(&f.instance.name)
                .expect("a call came from a lane");
            lane.in_flight -= 1;
            let counted = match &f.result {
                Ok((_, _, counted)) => *counted,
                Err(_) => None,
            };
            let limited = matches!(f.result, Err(EmbedError::RateLimited { .. }));
            let before = lane.pacer.rate();
            let step = settle(&f.batch, f.result, &mut lane.pacer, f.ticket, at);
            let (in_flight, rate) = (lane.in_flight, lane.pacer.rate());
            drain.note(&f.instance.name, |s| {
                s.in_flight = in_flight;
                s.tokens_counted += counted.unwrap_or(0);
                s.rate_limited += u64::from(limited);
                s.rate = rate;
                if rate < before {
                    s.last_halved = Some(Instant::now());
                }
            });
            match step {
                Step::Done { vectors, notes } => {
                    // To the daemon's stderr (its log under `just dev`): graph, node and field, never text.
                    for note in notes {
                        eprintln!("umx embed: {note}");
                    }
                    to_apply.push_back(vectors);
                }
                Step::Retry { jobs, wait_ms } => {
                    // Only this instance waits: its calls in flight finish, it starts no new one until then, and
                    // its jobs (this batch and those ready for it) go to the back of the queue (R106-2).
                    lane.paused_until = lane.paused_until.max(at.saturating_add(wait_ms));
                    let mut back = jobs;
                    ready.retain(|(i, b)| {
                        let mine = i.name == f.instance.name;
                        if mine {
                            back.extend(b.iter().map(|p| p.job.clone()));
                        }
                        !mine
                    });
                    defer(queue, back);
                }
                // One bad input fails the whole call: try each node alone, so only that node is dropped.
                Step::Failed { .. } if f.batch.len() > 1 => {
                    for one in f.batch.into_iter().rev() {
                        ready.push_front((f.instance.clone(), vec![one]));
                    }
                }
                // Nothing more to try: say which node and what kind of failure, never the text.
                Step::Failed { jobs, error } => {
                    for job in jobs {
                        eprintln!(
                            "umx embed: graph {} node {}: not embedded ({})",
                            job.alias,
                            job.node,
                            failure_kind(&error)
                        );
                    }
                }
            }
        }
        // 2. Vectors to the writer, while it has room (D1: never blocks).
        let cap = lanes.values().map(|l| l.slots).sum::<usize>() + 1;
        while !to_apply.is_empty() && drain.take_room(cap) {
            let vectors = to_apply.pop_front().expect("one is waiting");
            if writer.send(Job::Apply(vectors)).is_err() {
                return;
            }
        }
        // 3. Plan while fewer batches are ready than slots are free, only as many nodes as those slots could take.
        // An instance that cannot start a call now (paused, paced or every slot busy) neither counts its free slots
        // nor its ready batches, and its queued jobs are passed over, so it never keeps another's work from being
        // planned (R201-2).
        let mut soonest: Option<u64> = None;
        let mut wait_for = |ms: u64| soonest = Some(soonest.map_or(ms, |s| s.min(ms)));
        let blocked = |lanes: &BTreeMap<String, Lane>, at: u64| -> BTreeSet<String> {
            lanes
                .iter()
                .filter(|(_, l)| {
                    l.paused_until > at || l.in_flight >= l.slots || l.pacer.wait_ms(at) > 0
                })
                .map(|(name, _)| name.clone())
                .collect()
        };
        let want = |lanes: &BTreeMap<String, Lane>,
                    ready: &VecDeque<(Instance, Vec<Planned>)>,
                    blocked: &BTreeSet<String>| {
            let free = lanes
                .iter()
                .filter(|(name, _)| !blocked.contains(*name))
                .map(|(_, l)| l.slots - l.in_flight)
                .sum::<usize>()
                .max(1);
            let waiting = ready
                .iter()
                .filter(|(i, _)| !blocked.contains(&i.name))
                .count();
            free.saturating_sub(waiting)
        };
        let at = now();
        let stuck = blocked(&lanes, at);
        for lane in lanes.values() {
            let wait = lane
                .paused_until
                .saturating_sub(at)
                .max(lane.pacer.wait_ms(at));
            if wait > 0 {
                wait_for(wait);
            }
        }
        let mut deferred = false;
        if to_apply.is_empty() && want(&lanes, &ready, &stuck) > 0 {
            let nodes = want(&lanes, &ready, &stuck) * NODES_PER_BATCH;
            let (planned, only_blocked) = plan_round(services, queue, allows, drain, nodes, &stuck);
            ready.extend(planned);
            deferred |= only_blocked;
        }
        // 4. Start calls, in order, where the instance can take one. Nothing starts while finished vectors wait for
        // the writer, so what the dispatcher holds stays bounded. A batch whose instance is paused, has a query
        // waiting, or already has a slot's worth of batches waiting goes back to the end of the queue, so an
        // instance that cannot take work never keeps another's jobs from being planned (R106-2, R120-3).
        let mut kept: BTreeMap<String, usize> = BTreeMap::new();
        let mut i = 0;
        while i < ready.len() && to_apply.is_empty() {
            let at = now();
            let name = ready[i].0.name.clone();
            let lane = lanes.entry(name.clone()).or_insert_with(|| {
                let pace = services
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .embedding()
                    .pace(&name);
                let lane = Lane::start(&ready[i].0, pace, &outcomes, queue);
                drain.note(&name, |s| {
                    s.slots = lane.slots;
                    s.tokens_per_minute = pace;
                    s.rate = lane.pacer.rate();
                });
                lane
            });
            let paused = lane.paused_until.saturating_sub(at);
            // A search's query embed goes first: an instance with a query waiting or running gets no new batch
            // meanwhile (embeddings-expert, s08); only that instance waits (R120-3).
            let held = queries.busy(&ready[i].0.embedder);
            let waiting = kept.get(&name).copied().unwrap_or(0);
            if paused > 0 || held || waiting >= lane.slots {
                let (_, batch) = ready.remove(i).expect("in range");
                defer(queue, batch.into_iter().map(|p| p.job));
                deferred = true;
                if paused > 0 || held {
                    wait_for(if held { QUERY_WAIT_MS } else { paused });
                }
                continue;
            }
            let paced = lane.pacer.wait_ms(at);
            if lane.in_flight >= lane.slots || paced > 0 {
                if paced > 0 {
                    wait_for(paced);
                }
                *kept.entry(name).or_default() += 1;
                i += 1;
                continue;
            }
            let (instance, batch) = ready.remove(i).expect("in range");
            let batch = sendable(services, allows, &instance, batch);
            if batch.is_empty() {
                continue;
            }
            let ticket = lane.pacer.reserve(batch_tokens(&batch), at);
            lane.in_flight += 1;
            let (in_flight, rate) = (lane.in_flight, lane.pacer.rate());
            drain.note(&instance.name, |s| {
                s.calls += 1;
                s.in_flight = in_flight;
                s.rate = rate;
            });
            if lane
                .calls
                .send(Call {
                    instance,
                    batch,
                    ticket,
                })
                .is_err()
            {
                return;
            }
        }
        // 5. Sleep until a call finishes, the writer takes vectors, work arrives, or a wait ends.
        let in_flight: usize = lanes.values().map(|l| l.in_flight).sum();
        let holding = !ready.is_empty() || in_flight > 0 || !to_apply.is_empty();
        let (lock, ready_cv) = &**queue;
        let q = lock.lock().unwrap_or_else(|e| e.into_inner());
        drain.busy.store(holding || !q.is_empty(), Ordering::SeqCst);
        if !outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            continue;
        }
        // Jobs just put back are not planned again until a wait ends or something changes.
        let can_plan = !deferred
            && to_apply.is_empty()
            && want(&lanes, &ready, &blocked(&lanes, now())) > 0
            && !q.is_empty();
        let can_send = !to_apply.is_empty() && drain.room_for(cap);
        if can_plan || can_send {
            continue;
        }
        match soonest {
            Some(ms) => drop(
                ready_cv
                    .wait_timeout(q, Duration::from_millis(ms.clamp(1, 1_000)))
                    .unwrap_or_else(|e| e.into_inner()),
            ),
            None => drop(ready_cv.wait(q).unwrap_or_else(|e| e.into_inner())),
        }
    }
}
