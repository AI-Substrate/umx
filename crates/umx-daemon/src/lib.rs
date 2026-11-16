//! umx-daemon: serves umx-protocol over an owner-only Unix socket (blueprint 001, c-daemon).
//!
//! std only: a blocking accept and one thread per connection, so an idle daemon sleeps in the kernel
//! and uses no CPU (k-1, req-06). There is no TCP or UDP listener anywhere (req-45, SEC-1).

use std::fs::{self, DirBuilder, Metadata, Permissions};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::ops::ControlFlow;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};

mod doctor;
mod embed_worker;
#[cfg(test)]
mod f72_tests;
#[cfg(test)]
mod hold_bench;
mod models;
mod provenance;
mod query_embed;
#[cfg(test)]
mod req75_tests;
mod restart;
#[cfg(test)]
mod s08_tests;
#[cfg(test)]
mod s25_tests;
#[cfg(test)]
mod s25a_tests;
#[cfg(test)]
mod s27_tests;
#[cfg(test)]
mod s31_tests;
#[cfg(test)]
mod s5a_tests;
#[cfg(test)]
mod s6a_tests;
mod service;
mod similarity;
#[cfg(test)]
mod test_alloc;
mod ui_channel;
mod verbs;

pub use doctor::{Skill, doctor};
pub use models::models_fetch;
pub use restart::restart;
pub use service::{install, service, service_status, uninstall};
pub use umx_compose::Config;
use umx_protocol::{
    Build, CANDIDATE_ID, CANDIDATE_TITLE, Code, Envelope, Fill, Issue, MAX_FRAME, MAX_REPLY, Op,
    PROTOCOL, Request, Value, json,
};

/// macOS `sockaddr_un.sun_path` is 104 bytes, including the trailing NUL.
const SUN_PATH_MAX: usize = 104;

/// How long `daemon start` waits for an already-running daemon to say its pid.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a request may take to arrive (F44). A connected client is a local umx process, so a slow first byte is
/// a busy machine, not a stuck client, and gets the longest wait; once bytes flow, a gap longer than `idle` is a
/// stalled frame and is cut off; and no request may take longer than `total`, so a connection never holds a thread
/// for long. Measured: a macOS CI runner under load once took over 10 s to send the first byte of a small request.
/// F54 raised `idle` to 60 s for what looked like a client stalling before its shutdown; F59 found the real cause
/// (a missed wakeup, see [`READ_SLICE`]) and put it back to 10 s.
#[derive(Clone, Copy, Debug)]
struct ReadLimits {
    first_byte: Duration,
    idle: Duration,
    total: Duration,
}

const READ_LIMITS: ReadLimits = ReadLimits {
    first_byte: Duration::from_secs(60),
    idle: Duration::from_secs(10),
    total: Duration::from_secs(120),
};

/// The longest one socket read may block (F59). macOS can miss the wakeup for data or an end of stream that arrives
/// while a read waits: measured with a 1,000-op batch, about 1 run in 20, the whole request and its EOF were in the
/// socket and the read slept until its timeout, then one more read returned the EOF in 2 µs. So no read waits
/// longer than this; the limits are kept from elapsed time, and a missed wakeup costs at most one slice. The same
/// loop is in [`read_request`], [`read_until_end`] (doctor's `ask_by` and `running_pid`) and umx-cli `read_reply`;
/// change one, look at the other two.
pub(crate) const READ_SLICE: Duration = Duration::from_millis(250);

/// A socket read that can be given a timeout: a UnixStream, or a stand-in in tests.
pub(crate) trait TimedRead: Read {
    /// Sets the read timeout. macOS refuses it (EINVAL) once the peer has closed both ways; the data and the end are
    /// then already there, nothing more can arrive to be missed, and the read returns at once, so a failure is not
    /// an error.
    fn arm(&mut self, wait: Duration);
}

impl TimedRead for UnixStream {
    fn arm(&mut self, wait: Duration) {
        let _ = self.set_read_timeout(Some(wait));
    }
}

/// Reads one request frame: everything until the client shuts down its write half, at most [`MAX_FRAME`] bytes
/// (the rest is not read, as before), each read at most [`READ_SLICE`]. The error is the internal code word to
/// report: `request_timed_out` when a limit in `limits` ran out, `request_unreadable` for any other read failure.
fn read_request(stream: &mut impl TimedRead, limits: ReadLimits) -> Result<Vec<u8>, &'static str> {
    let started = Instant::now();
    let mut progress = started;
    let mut bytes = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let room = MAX_FRAME as usize - bytes.len();
        if room == 0 {
            return Ok(bytes);
        }
        let wait = if bytes.is_empty() {
            limits.first_byte
        } else {
            limits.idle
        };
        let deadline = (progress + wait).min(started + limits.total);
        let Some(left) = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
        else {
            return Err("request_timed_out");
        };
        stream.arm(READ_SLICE.min(left));
        let take = room.min(buf.len());
        match stream.read(&mut buf[..take]) {
            Ok(0) => return Ok(bytes),
            Ok(n) => {
                bytes.extend_from_slice(&buf[..n]);
                progress = Instant::now();
            }
            // A slice ran out (or a signal came): look at the limits and read again.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("request_unreadable"),
        }
    }
}

const TOO_LONG: &str = "the path is over the 104-byte limit; set UMX_HOME to a shorter path";
const NOT_OWNER_ONLY: &str =
    "the run directory is open to other users or is not a plain directory; chmod 700 it";
const NOT_YOURS: &str =
    "the run directory belongs to another user; remove it or set UMX_HOME to a folder you own";
const NOT_A_SOCKET: &str = "something that is not a socket is at the socket path";

/// How this daemon starts itself again for `umx daemon restart` (F46): the binary path, resolved once when the
/// process started (on Linux, current_exe() of a replaced binary reads "<path> (deleted)", so a new build at the
/// same path is picked up on purpose), and the exact `daemon start ...` arguments it was started with.
#[derive(Clone, Debug)]
pub struct Relaunch {
    pub exe: std::path::PathBuf,
    pub args: Vec<String>,
}

/// Binds the socket, calls `ready` once, then serves until `umx daemon stop`, which ends the process, or `umx
/// daemon restart`, which replaces it with `relaunch` (same pid). Returns only when the daemon cannot start.
pub fn start(
    config: &Config,
    build: Build,
    relaunch: Relaunch,
    ready: impl FnOnce(&Envelope),
) -> Envelope {
    let listener = match bind(config) {
        Ok(listener) => listener,
        Err(issue) => return Envelope::fail("daemon start", issue),
    };
    let mut daemon = Daemon::new(config, build);
    daemon.relaunch = Some(relaunch);
    let daemon = Arc::new(daemon);
    let mut started = Envelope::ok(
        "daemon start",
        json!({ "pid": std::process::id(), "socket": daemon.socket, "version": daemon.build }),
    );
    if let Some(closed) = &daemon.config_closed {
        started = started.warn(verbs::issue(
            &umx_core::config::problems::config_invalid(closed),
            "daemon start",
        ));
    }
    ready(&started);
    accept(&daemon, listener, SLOTS, &Arc::new(AtomicUsize::new(0)));
    Envelope::fail("daemon start", internal("accept_loop_ended"))
}

/// How many connections the daemon serves at once (F56): `serve` with the full request limits, then `lane` more
/// that answer only `daemon status`, `stop` and `restart` (with [`LANE_LIMITS`]), so a daemon that stuck clients
/// have pinned can still be checked and stopped. Past both, a connection gets `daemon_busy` before its request is
/// read. 72 threads at the default stack is nothing.
#[derive(Clone, Copy, Debug)]
struct Slots {
    serve: usize,
    lane: usize,
}

const SLOTS: Slots = Slots { serve: 64, lane: 8 };

/// A lane connection's limits: a stalled client holds a lane slot for 2 s at most.
const LANE_LIMITS: ReadLimits = ReadLimits {
    first_byte: Duration::from_secs(1),
    idle: Duration::from_secs(1),
    total: Duration::from_secs(1),
};

/// The whole life of a lane connection: [`LANE_LIMITS`] to send its request, the rest to take the reply (review
/// R129-B1: a client that never reads its reply must not hold a lane slot).
const LANE_LIFETIME: Duration = Duration::from_secs(2);

/// How long a served connection may take to read its reply once it is ready. The CLI reads at once; a client that
/// does not frees its slot after this.
const REPLY_WRITE: Duration = Duration::from_secs(30);

/// How long writing a refusal may block the accept loop.
const REFUSE_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Each `similarity pairs` read hold (plan, a graph's copy, finish) and its compute, as (step, time), for the s31
/// bench.
#[cfg(test)]
pub(crate) static PAIRS_TIMES: Mutex<Vec<(&'static str, Duration)>> = Mutex::new(Vec::new());

#[cfg(test)]
fn pairs_time(step: &'static str, since: Instant) {
    PAIRS_TIMES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((step, since.elapsed()));
}

/// The most threads one `umx similarity pairs` computes on (ws004 s31): its parts give the same lists for any number.
const PAIRS_THREADS: usize = 4;

/// How long a restart waits for the accept loop to stop before it execs anyway (F64).
const QUIESCE: Duration = Duration::from_secs(1);

/// Connects once to `socket` to wake a blocked accept (F64). On Linux a full listen backlog makes connect block, so
/// it runs on a thread the restart can abandon at its deadline; std opens that socket with SOCK_CLOEXEC, atomically,
/// so an abandoned connect never reaches the new image. On macOS a full backlog refuses at once, but std sets
/// FD_CLOEXEC in a second step after socket(2), so the connect stays on the calling thread, which execs only after it
/// has returned and the stream is dropped (review R137-B1).
fn wake_accept(socket: &Path) {
    #[cfg(target_os = "linux")]
    {
        let socket = socket.to_path_buf();
        std::thread::spawn(move || drop(UnixStream::connect(socket)));
    }
    #[cfg(not(target_os = "linux"))]
    drop(UnixStream::connect(socket));
}

/// One connection's place in `in_flight`, given back when it drops, a panic in `serve` included.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

enum Admit {
    Serve(Slot),
    Lane(Slot),
    Refuse,
}

fn admit(in_flight: &Arc<AtomicUsize>, slots: Slots) -> Admit {
    let before = in_flight.fetch_add(1, Ordering::SeqCst);
    let slot = Slot(Arc::clone(in_flight));
    if before < slots.serve {
        Admit::Serve(slot)
    } else if before < slots.serve + slots.lane {
        Admit::Lane(slot)
    } else {
        Admit::Refuse
    }
}

/// Serves every connection on `listener`, at most `slots` at once; `in_flight` counts them.
fn accept(
    daemon: &Arc<Daemon>,
    listener: UnixListener,
    slots: Slots,
    in_flight: &Arc<AtomicUsize>,
) {
    loop {
        let Ok((stream, _)) = listener.accept() else {
            continue;
        };
        // F64: a restart is replacing the process. On macOS, std's accept sets FD_CLOEXEC only after accept(2), so a
        // connection taken while the image is replaced could survive into it unread. Stop here, outside accept: drop
        // the stream that woke us and the listener, say so, and never accept again.
        if daemon.relaunching.load(Ordering::SeqCst) {
            drop(stream);
            drop(listener);
            if let Some(ack) = daemon
                .parked
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                let _ = ack.send(());
            }
            loop {
                std::thread::park();
            }
        }
        let daemon = Arc::clone(daemon);
        match admit(in_flight, slots) {
            Admit::Serve(slot) => {
                std::thread::spawn(move || {
                    let _slot = slot;
                    daemon.serve(stream, READ_LIMITS, false);
                });
            }
            Admit::Lane(slot) => {
                std::thread::spawn(move || {
                    let _slot = slot;
                    daemon.serve(stream, LANE_LIMITS, true);
                });
            }
            Admit::Refuse => refuse(stream),
        }
    }
}

/// Answers `daemon_busy` without reading the request (reading could wait on the very client that is stalling).
fn refuse(mut stream: UnixStream) {
    let bytes = busy("unknown").to_json();
    write_reply(
        &mut stream,
        bytes.as_bytes(),
        Instant::now() + REFUSE_WRITE_TIMEOUT,
    );
    let _ = stream.shutdown(Shutdown::Write);
}

/// A socket write that can be given a timeout: a UnixStream, or a stand-in in tests.
pub(crate) trait TimedWrite: Write {
    /// Sets the write timeout; a failure is not an error (the write then fails or completes at once).
    fn arm_write(&mut self, wait: Duration);
}

impl TimedWrite for UnixStream {
    fn arm_write(&mut self, wait: Duration) {
        let _ = self.set_write_timeout(Some(wait));
    }
}

/// Writes `bytes` by `deadline`, each write blocking at most [`READ_SLICE`], so a peer that never reads cannot hold
/// the thread (review R129-B1) and a missed wakeup costs one slice (F59). False if the peer did not take it all.
fn write_reply(stream: &mut impl TimedWrite, bytes: &[u8], deadline: Instant) -> bool {
    let mut at = 0;
    while at < bytes.len() {
        let Some(left) = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
        else {
            return false;
        };
        stream.arm_write(READ_SLICE.min(left));
        match stream.write(&bytes[at..]) {
            Ok(0) => return false,
            Ok(n) => at += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return false,
        }
    }
    true
}

fn busy(command: &str) -> Envelope {
    Envelope::fail(command, Issue::new(Code::DaemonBusy))
}

pub(crate) fn internal(code: &'static str) -> Issue {
    Issue::fill(Code::Internal, &[("code", Fill::word(code))])
}

fn bind(config: &Config) -> Result<UnixListener, Issue> {
    let socket = &config.socket;
    let unavailable = |cause: Fill| {
        Issue::fill(Code::SocketUnavailable, &[("cause", cause)])
            .with_subject(socket.display().to_string())
    };
    if socket.as_os_str().len() >= SUN_PATH_MAX {
        return Err(unavailable(Fill::word(TOO_LONG)));
    }
    owner_only_dir(&config.run_dir, config.uid).map_err(|cause| unavailable(Fill::word(cause)))?;
    if let Ok(meta) = fs::symlink_metadata(socket) {
        if !meta.file_type().is_socket() {
            return Err(unavailable(Fill::word(NOT_A_SOCKET)));
        }
        if let Ok(stream) = UnixStream::connect(socket) {
            let pid = running_pid(stream).map_or(Fill::word("unknown"), Fill::count);
            return Err(Issue::fill(Code::DaemonAlreadyRunning, &[("pid", pid)])
                .with_subject(socket.display().to_string()));
        }
        // Nothing answers: a daemon that was killed left it behind.
        fs::remove_file(socket).map_err(|e| unavailable(Fill::os_error(&e)))?;
    }
    // The run dir is already 0700, so nobody else can reach the socket before the chmod below.
    let listener = UnixListener::bind(socket).map_err(|e| unavailable(Fill::os_error(&e)))?;
    fs::set_permissions(socket, Permissions::from_mode(0o600))
        .map_err(|e| unavailable(Fill::os_error(&e)))?;
    Ok(listener)
}

/// Asks the daemon on the other end of `stream` for its pid, within [`READ_TIMEOUT`].
fn running_pid(mut stream: UnixStream) -> Option<u64> {
    let deadline = Instant::now() + READ_TIMEOUT;
    let unknown = Build {
        version: String::new(),
        commit: String::new(),
        built: String::new(),
    };
    stream.set_write_timeout(Some(READ_TIMEOUT)).ok()?;
    stream
        .write_all(
            Request::new("daemon status", Vec::new(), unknown)
                .to_json()
                .as_bytes(),
        )
        .ok()?;
    stream.shutdown(Shutdown::Write).ok()?;
    Envelope::from_json(&read_until_end(&mut stream, deadline)?)
        .ok()?
        .data["pid"]
        .as_u64()
}

/// Reads a daemon's reply until the other end closes, at most [`MAX_REPLY`] bytes, by `deadline`, each read at most
/// [`READ_SLICE`] (F59, F62); None if the end does not come in time or the reply is too long. The same loop as
/// [`read_request`] (which stops at [`MAX_FRAME`]) and umx-cli `read_reply`; change one, look at the other two.
pub(crate) fn read_until_end(stream: &mut impl TimedRead, deadline: Instant) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())?;
        stream.arm(READ_SLICE.min(left));
        match stream.read(&mut chunk) {
            Ok(0) => return Some(bytes),
            Ok(n) if (bytes.len() + n) as u64 <= MAX_REPLY => bytes.extend_from_slice(&chunk[..n]),
            Ok(_) => return None,
            // A slice ran out (or a signal came): look at the deadline and read again.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return None,
        }
    }
}

/// Creates `dir` as 0700, or accepts an existing plain directory owned by this user with no group or other bits.
/// The error is the cause to report.
fn owner_only_dir(dir: &Path, uid: Option<u32>) -> Result<(), &'static str> {
    if fs::symlink_metadata(dir).is_err() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|_| NOT_OWNER_ONLY)?;
    }
    let meta = fs::symlink_metadata(dir).map_err(|_| NOT_OWNER_ONLY)?;
    owner_only(&meta, uid.ok_or(NOT_YOURS)?)
}

fn owner_only(meta: &Metadata, uid: u32) -> Result<(), &'static str> {
    if !meta.is_dir() || meta.mode() & 0o077 != 0 {
        Err(NOT_OWNER_ONLY)
    } else if meta.uid() != uid {
        Err(NOT_YOURS)
    } else {
        Ok(())
    }
}

pub(crate) fn usage(command: &str, what: &'static str) -> Issue {
    let noun = if command.starts_with("daemon") {
        "daemon"
    } else {
        "start"
    };
    Issue::fill(
        Code::UsageInvalid,
        &[
            ("cmd", Fill::command(command)),
            ("what", Fill::word(what)),
            ("noun", Fill::word(noun)),
        ],
    )
}

/// One job for the writer thread (d-7): every write and `daemon stop` run here, one at a time, in the order they
/// arrived.
enum Job {
    Write(Box<Request>, Sender<Envelope>),
    /// A reembed planned under the shared lock (architect F1): the writer only checks again, records the key and
    /// queues.
    Reembed(Box<umx_services::ReembedPlan>, Sender<Envelope>),
    /// Vectors the embed worker made, written into their shards like any other write (req-35).
    Apply(Vec<umx_services::Vectors>),
    /// similar_to lists computed off every lock from a snapshot; applied only if the vectors did not move.
    SimilarityApply(
        String,
        Box<umx_services::SimilarityResult>,
        Sender<similarity::Applied>,
    ),
    /// Load what a request missed (req-15, F72): one alias the registry remembers as loaded, or the members of a saved
    /// scope (F99) or a `--graph a,b` list (F118), whose plan the writer decides afresh for the whole scope or list
    /// before loading any. The request is the
    /// one that missed, for its context; the reply carries the loads' warnings, or the error that stopped them.
    LoadRemembered(Miss, Box<Request>, Sender<Envelope>),
    Stop(Sender<Envelope>),
    /// `daemon restart`: like Stop, every write queued before it has been applied when it replies.
    Restart(Sender<Envelope>),
}

/// Client jobs a waiting Apply unit lets go first, at most (graph-storage-expert, s25a): client writes win, but a
/// steady stream of them never starves the embed backlog.
const WRITES_BEFORE_APPLY: usize = 8;

/// How long owed similar_to files wait for the embed backlog to go quiet, from when the debt began (ws004 s25a,
/// R180-3). After that they get a step (one file) after every [`FLUSH_EVERY`] other jobs, however busy the writer is.
const SIMILAR_FLUSH_MAX_AGE_MS: u64 = 60_000;

/// Other jobs between two steps of overdue similar_to writes, at most.
const FLUSH_EVERY: usize = 4;

/// How often the writer looks again, while similar_to files are owed, whether the backlog went quiet.
const QUIET_POLL_MS: u64 = 100;

/// What the writer does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    Client,
    Unit,
    Flush,
    Wait,
}

/// The writer's choice of next job (ws004 s25a), apart from the clock and the queues so it is tested on its own: a
/// waiting client job first, at most [`WRITES_BEFORE_APPLY`] in a row while Apply units wait; owed similar_to
/// writes when the backlog is quiet, or, once their debt is [`SIMILAR_FLUSH_MAX_AGE_MS`] old, a step after every
/// [`FLUSH_EVERY`] other jobs.
#[derive(Debug, Default)]
struct Schedule {
    streak: usize,
    owed_since_ms: Option<u64>,
    since_flush: usize,
}

impl Schedule {
    fn pick(&mut self, now_ms: u64, client: bool, units: bool, owed: bool, quiet: bool) -> Pick {
        if !owed {
            self.owed_since_ms = None;
        } else if self.owed_since_ms.is_none() {
            self.owed_since_ms = Some(now_ms);
        }
        let overdue = self
            .owed_since_ms
            .is_some_and(|since| now_ms.saturating_sub(since) >= SIMILAR_FLUSH_MAX_AGE_MS);
        if !units {
            self.streak = 0;
        }
        if owed
            && ((overdue && self.since_flush >= FLUSH_EVERY)
                || (!client && !units && (quiet || overdue)))
        {
            self.since_flush = 0;
            return Pick::Flush;
        }
        if client && (!units || self.streak < WRITES_BEFORE_APPLY) {
            self.streak += 1;
            self.since_flush += 1;
            return Pick::Client;
        }
        if units {
            self.streak = 0;
            self.since_flush += 1;
            return Pick::Unit;
        }
        Pick::Wait
    }

    /// Owed writes were just found: their age starts now, not when the writer next looks (R180-3).
    fn owed_now(&mut self, now_ms: u64) {
        self.owed_since_ms.get_or_insert(now_ms);
    }
}

/// One lock hold of an Apply (ws004 s25a): the vectors of one graph's one shard file.
struct Unit {
    /// The graph and the shard file.
    at: (String, String),
    vectors: Vec<umx_services::Vectors>,
    /// The Applies its vectors came from (ws004 s25, R201-3).
    applies: Vec<u64>,
    /// How many Applies writing it finishes: set by [`take_unit`], those with no unit still waiting.
    lasts: usize,
}

/// Nodes' vectors in one merged unit, at most (graph-storage-expert, s25 D2): the provider batch size, so the
/// longest hold stays bounded.
const MERGED_UNIT_MAX: usize = umx_services::embedding::MAX_BATCH_TEXTS;

/// An Apply's vectors as units, one per shard file, in order; `apply` names the Apply.
fn units(vectors: Vec<umx_services::Vectors>, apply: u64) -> Vec<Unit> {
    let mut by_shard: std::collections::BTreeMap<(String, String), Vec<umx_services::Vectors>> =
        std::collections::BTreeMap::new();
    for v in vectors {
        let at = (v.job.alias.clone(), umx_core::vectors::shard_of(v.job.node));
        by_shard.entry(at).or_default().push(v);
    }
    by_shard
        .into_iter()
        .map(|(at, vectors)| Unit {
            at,
            vectors,
            applies: vec![apply],
            lasts: 0,
        })
        .collect()
}

/// The first waiting unit, with the later waiting units for the same graph and shard file merged into it, in queue
/// order, up to [`MERGED_UNIT_MAX`] nodes (ws004 s25 D2): still one file per hold, fewer holds per node. A node in
/// two units keeps its later vectors, and apply checks every vector against the node's current text as before.
/// Merging stops at the first unit for that file that does not fit, since taking one past it would write its vectors
/// before that unit's (R201-1). The unit finishes each of its Applies that has no unit left waiting, wherever in the
/// queue merging took its units from (R201-3).
fn take_unit(units: &mut std::collections::VecDeque<Unit>) -> Option<Unit> {
    let mut unit = units.pop_front()?;
    let mut i = 0;
    while i < units.len() {
        if units[i].at != unit.at {
            i += 1;
            continue;
        }
        if unit.vectors.len() + units[i].vectors.len() > MERGED_UNIT_MAX {
            break;
        }
        let more = units.remove(i).expect("in range");
        unit.vectors.extend(more.vectors);
        unit.applies.extend(more.applies);
    }
    unit.applies.sort_unstable();
    unit.applies.dedup();
    unit.lasts = unit
        .applies
        .iter()
        .filter(|a| !units.iter().any(|u| u.applies.contains(a)))
        .count();
    Some(unit)
}

/// The longest single hold of the write lock by an Apply unit or a similar_to flush step, in µs (s25a's bench).
#[cfg(test)]
pub(crate) static LONGEST_UNIT_HOLD_US: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The writer thread (d-7): every write and `daemon stop` run here, one at a time, in the order they arrived. The
/// embed worker's vectors go in between, a shard file per lock hold and only while no client job waits (or after
/// [`WRITES_BEFORE_APPLY`] of them), so a write never waits behind the embed backlog (ws004 s25a).
struct Writer {
    services: Arc<RwLock<umx_services::Services>>,
    ok: Arc<AtomicBool>,
    config: Arc<umx_services::ConfigServices>,
    jobs: embed_worker::Queue,
    warms: Arc<query_embed::QueryLanes>,
    warm_allows: embed_worker::FolderAllows,
    rebuild_tx: Sender<Job>,
    drain: Arc<embed_worker::Drain>,
    /// The next Apply's name, for its units (R201-3).
    next_apply: std::sync::atomic::AtomicU64,
    /// The views' channel: each lock hold hands it what its writes changed (ws009 S5a).
    ui: UiFeed,
}

/// The views' channel and what wakes their waits, shared by the daemon and the writer.
#[derive(Clone)]
struct UiFeed {
    channel: Arc<Mutex<umx_services::ui::UiChannel>>,
    wake: Arc<Condvar>,
}

impl UiFeed {
    /// Hands the channel what the graph store's commits, loads and unloads changed, and wakes the waits if any.
    /// Called with the writer's lock still held, so no read sees a revision the views' log has not got.
    fn feed(&self, services: &mut umx_services::Services) {
        let fed = services.take_changes();
        if fed.is_empty() {
            return;
        }
        let mut channel = self.channel.lock().unwrap_or_else(|e| e.into_inner());
        if channel.record(fed) {
            self.wake.notify_all();
        }
    }
}

/// The services under the writer's exclusive lock. Dropping it feeds the views' change log before the lock goes
/// (ws009 S5a), whatever the hold did: a client write, a load on demand, an idle unload or an embed apply.
struct Writing<'a> {
    services: std::sync::RwLockWriteGuard<'a, umx_services::Services>,
    ui: &'a UiFeed,
}

impl std::ops::Deref for Writing<'_> {
    type Target = umx_services::Services;
    fn deref(&self) -> &Self::Target {
        &self.services
    }
}

impl std::ops::DerefMut for Writing<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.services
    }
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        self.ui.feed(&mut self.services);
    }
}

impl Writer {
    fn run(self, rx: std::sync::mpsc::Receiver<Job>) {
        use std::sync::mpsc::TryRecvError;
        let started = Instant::now();
        let now = || started.elapsed().as_millis() as u64;
        let mut writes: std::collections::VecDeque<Job> = std::collections::VecDeque::new();
        let mut units: std::collections::VecDeque<Unit> = std::collections::VecDeque::new();
        let mut schedule = Schedule::default();
        let mut open = true;
        loop {
            while open {
                match rx.try_recv() {
                    Ok(job) => self.sort(job, &mut writes, &mut units),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => open = false,
                }
            }
            let owed = guarded(&self.ok, || self.read().similar_owed()).unwrap_or(false);
            let quiet = owed && self.drain.quiet(&self.jobs);
            match schedule.pick(now(), !writes.is_empty(), !units.is_empty(), owed, quiet) {
                Pick::Client => {
                    let job = writes.pop_front().expect("a job is waiting");
                    if self.client(job, &mut units) {
                        return;
                    }
                    continue;
                }
                Pick::Unit => {
                    let unit = take_unit(&mut units).expect("a unit is waiting");
                    if self.apply(unit) {
                        schedule.owed_now(now());
                    }
                    continue;
                }
                Pick::Flush => {
                    self.flush_step();
                    continue;
                }
                Pick::Wait => {}
            }
            if !open {
                return;
            }
            // Sleep until the next job or the next idle deadline, never on a tick (req-06). A read may have pushed
            // the deadline back meanwhile; unload_idle checks again and the wait is recomputed. While similar_to
            // files are owed, look again shortly for the backlog to go quiet, and keep unloading idle graphs.
            let idle = guarded(&self.ok, || self.read().idle_wait_ms()).flatten();
            let wait = match (idle, owed.then_some(QUIET_POLL_MS)) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let job = match wait {
                Some(ms) => match rx.recv_timeout(Duration::from_millis(ms)) {
                    Ok(job) => Some(job),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => {
                        open = false;
                        None
                    }
                },
                None => rx.recv().map_err(|_| open = false).ok(),
            };
            match job {
                Some(job) => self.sort(job, &mut writes, &mut units),
                None if open => self.unload_due(),
                None => {}
            }
        }
    }

    /// Unloads the graphs whose idle time has passed; what writing their owed similar_to files found goes to the
    /// daemon's log (R180-4).
    fn unload_due(&self) {
        let due = guarded(&self.ok, || self.read().idle_wait_ms() == Some(0)).unwrap_or(false);
        if !due {
            return;
        }
        let problems = guarded(&self.ok, || {
            let mut services = self.write();
            services.unload_idle();
            services.take_unload_problems()
        })
        .unwrap_or_default();
        for problem in problems {
            let issue = verbs::issue(&problem, "similarity rebuild");
            eprintln!("umx similar: {} ({})", issue.message(), issue.code());
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, umx_services::Services> {
        self.services.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> Writing<'_> {
        Writing {
            services: self.services.write().unwrap_or_else(|e| e.into_inner()),
            ui: &self.ui,
        }
    }

    /// Client jobs keep their order; an Apply becomes one unit per shard file.
    fn sort(
        &self,
        job: Job,
        writes: &mut std::collections::VecDeque<Job>,
        units: &mut std::collections::VecDeque<Unit>,
    ) {
        match job {
            Job::Apply(vectors) => {
                let more = self::units(vectors, self.next_apply.fetch_add(1, Ordering::Relaxed));
                if more.is_empty() {
                    self.drain.applied(1, &self.jobs);
                }
                units.extend(more);
            }
            other => writes.push_back(other),
        }
    }

    /// After a config set or unset: each hosted instance it added, removed or repointed that loaded named or secured
    /// graphs of its model route to warns `embedder_route_changed`, counting them, never naming them (D62, security
    /// s20b). Under D62 that is a heads-up: every machine instance is trusted.
    fn route_changed(
        &self,
        env: Envelope,
        before: &umx_core::config::Resolved,
        command: &str,
    ) -> Envelope {
        if env.status == umx_protocol::Status::Error {
            return env;
        }
        let after = self.config.resolve(None);
        let changes = umx_core::config::route_changes(before, &after);
        if changes.is_empty() {
            return env;
        }
        let services = self.services.read().unwrap_or_else(|e| e.into_inner());
        let embedding = services.embedding();
        // The instances the daemon would build from the config as written: an unchanged one as built now, a changed
        // or new one from its keys (one that cannot be built is no candidate). Every one is in the machine config.
        let text = |name: &str, leaf: &str| {
            after
                .value(&format!("{}.{name}.{leaf}", umx_core::config::INSTANCES))
                .cloned()
        };
        let names = after.instances();
        let candidates: Vec<umx_services::embedding::Candidate<'_>> = names
            .iter()
            .filter_map(|name| {
                let built = embedding.instances().iter().find(|i| &i.name == name);
                if let Some(i) = built.filter(|_| !changes.contains(name)) {
                    return Some(umx_services::embedding::Candidate {
                        name,
                        key: i.embedder.key(),
                        locality: i.embedder.locality(),
                    });
                }
                let kind = text(name, "kind")?;
                let local = kind.as_text() == Some("local");
                if !local
                    && (text(name, "endpoint").is_none() || text(name, "deployment").is_none())
                {
                    return None;
                }
                let model = text(name, "model")?;
                let dims = match text(name, "dims")? {
                    umx_core::config::ConfigValue::Int(d) => d,
                    _ => return None,
                };
                let key = umx_core::ports::ModelKey::parse(&format!("{}@{dims}", model.as_text()?))
                    .ok()?;
                Some(umx_services::embedding::Candidate {
                    name,
                    key,
                    locality: if local {
                        umx_core::ports::Locality::Local
                    } else {
                        umx_core::ports::Locality::Hosted
                    },
                })
            })
            .collect();
        let default = after
            .value("embedder.active")
            .and_then(umx_core::config::ConfigValue::as_text);
        let allows = folder_allows(&self.config);
        changes.iter().fold(env, |env, changed| {
            let graphs =
                embedding.rerouted(services.graph(), &candidates, default, changed, &*allows);
            if graphs == 0 {
                return env;
            }
            env.warn(verbs::issue(
                &umx_services::problems::embedder_route_changed(changed, graphs),
                command,
            ))
        })
    }

    /// Runs one client job; true when the writer must stop.
    fn client(&self, job: Job, units: &mut std::collections::VecDeque<Unit>) -> bool {
        match job {
            Job::Write(request, reply) => {
                let command = request
                    .op
                    .as_ref()
                    .map_or(request.command.as_str(), |op| op.command());
                let was_ok = self.ok.load(Ordering::SeqCst);
                let env = guarded(&self.ok, || {
                    if request.op.as_ref().is_some_and(Op::is_config) {
                        let before = self.config.resolve(None);
                        let env = verbs::config(&self.config, &request);
                        return self.route_changed(env, &before, command);
                    }
                    let mut services = self.write();
                    let env = verbs::run(&mut services, &request);
                    if matches!(request.op, Some(Op::GraphLoad { .. })) {
                        warm_local(&services, &*self.warm_allows, &self.warms);
                    }
                    embed_worker::enqueue(&self.jobs, services.take_embed_jobs());
                    let rebuilds = services.take_similarity_rebuilds();
                    drop(services);
                    similarity::spawn_rebuilds(&self.services, &self.rebuild_tx, rebuilds);
                    env
                })
                .unwrap_or_else(|| {
                    let why = if was_ok {
                        "writer_panicked"
                    } else {
                        "writer_stopped"
                    };
                    Envelope::fail(command, internal(why))
                });
                let _ = reply.send(env);
            }
            Job::LoadRemembered(miss, request, reply) => {
                let command = request.command.clone();
                let env = guarded(&self.ok, || {
                    let mut services = self.write();
                    let ctx = verbs::ctx(&request);
                    let loaded = match &miss {
                        Miss::Alias(alias) => {
                            services
                                .load_registered(&ctx, alias)
                                .map(|done| umx_services::Done {
                                    value: usize::from(done.value.is_some()),
                                    notes: done.notes,
                                })
                        }
                        // Decided for the whole scope here, under the writer's exclusive lock (review R213-B1).
                        Miss::Scope(name) => services.load_scope(&ctx, name),
                        // F118: the same for a --graph list.
                        Miss::Aliases(aliases) => services.load_registered_all(&ctx, aliases),
                    };
                    let env = match loaded {
                        Ok(done) => done.notes.iter().fold(
                            Envelope::ok(&command, json!({ "loaded": done.value })),
                            |env, w| env.note(verbs::issue(w, &command)),
                        ),
                        Err(p) => Envelope::fail(&command, verbs::issue(&p, &command)),
                    };
                    warm_local(&services, &*self.warm_allows, &self.warms);
                    embed_worker::enqueue(&self.jobs, services.take_embed_jobs());
                    let rebuilds = services.take_similarity_rebuilds();
                    drop(services);
                    similarity::spawn_rebuilds(&self.services, &self.rebuild_tx, rebuilds);
                    env
                })
                .unwrap_or_else(|| Envelope::fail(&command, internal("writer_stopped")));
                let _ = reply.send(env);
            }
            Job::Reembed(plan, reply) => {
                let was_ok = self.ok.load(Ordering::SeqCst);
                let env = guarded(&self.ok, || {
                    let mut services = self.write();
                    let env = verbs::reembed_apply(&mut services, *plan);
                    embed_worker::enqueue(&self.jobs, services.take_embed_jobs());
                    let rebuilds = services.take_similarity_rebuilds();
                    drop(services);
                    similarity::spawn_rebuilds(&self.services, &self.rebuild_tx, rebuilds);
                    env
                })
                .unwrap_or_else(|| {
                    let why = if was_ok {
                        "writer_panicked"
                    } else {
                        "writer_stopped"
                    };
                    Envelope::fail("embeddings reembed", internal(why))
                });
                let _ = reply.send(env);
            }
            Job::SimilarityApply(alias, lists, reply) => {
                let applied = guarded(&self.ok, || self.write().similarity_apply(&alias, *lists));
                if let Some(applied) = applied {
                    let _ = reply.send(applied);
                }
            }
            Job::Stop(reply) => {
                // Every write queued before the stop has been applied; the caller exits after replying.
                self.finish(units);
                let _ = reply.send(Envelope::ok(
                    "daemon stop",
                    json!({ "stopped": true, "pid": std::process::id() }),
                ));
                return true;
            }
            Job::Restart(reply) => {
                // The same as stop; the caller replaces the process after replying.
                self.finish(units);
                let _ = reply.send(Envelope::ok(
                    "daemon restart",
                    json!({ "restarting": true, "pid": std::process::id() }),
                ));
                return true;
            }
            Job::Apply(_) => unreachable!("sorted into units"),
        }
        false
    }

    /// Before a stop or restart: the vectors already made are applied too, and the similar_to files they owe are
    /// written, so the next start finds the lists fresh. Then the embed worker is let go.
    fn finish(&self, units: &mut std::collections::VecDeque<Unit>) {
        while let Some(unit) = take_unit(units) {
            self.apply(unit);
        }
        guarded(&self.ok, || self.write().unpause_similar());
        while self.flush_step() {}
        self.drain.close(&self.jobs);
    }

    /// Writes one unit's vectors into its shard (req-35). No client waits for this write, so its outcome goes to the
    /// daemon's stderr (its log): a failed shard write changed nothing (load or repair queues the node again), and a
    /// not-synced one stands and is not undone (ap-13).
    /// Returns whether similar_to files are owed after it.
    fn apply(&self, unit: Unit) -> bool {
        let (problems, owed) = guarded(&self.ok, || {
            let mut services = self.write();
            #[cfg(test)]
            let held = Instant::now();
            #[cfg(test)]
            let mut files = 0;
            let problems = match services.embed_apply_deferred(&unit.vectors) {
                Ok((_applied, warnings)) => {
                    #[cfg(test)]
                    {
                        files = _applied.shards_written + _applied.similar_files_written;
                    }
                    warnings
                }
                Err(problem) => vec![problem],
            };
            let owed = services.similar_owed();
            drop(services);
            #[cfg(test)]
            record_hold(held, Some(files));
            (problems, owed)
        })
        .unwrap_or_default();
        for problem in problems {
            let issue = verbs::issue(&problem, "embeddings repair");
            eprintln!("umx embed: {} ({})", issue.message(), issue.code());
        }
        self.drain.applied(unit.lasts, &self.jobs);
        owed
    }

    /// Writes one owed similar_to file; false when nothing is owed (or the writer has failed).
    fn flush_step(&self) -> bool {
        let Some(problems) = guarded(&self.ok, || {
            let mut services = self.write();
            #[cfg(test)]
            let held = Instant::now();
            let step = services.flush_similar_step();
            drop(services);
            #[cfg(test)]
            record_hold(held, None);
            step
        })
        .flatten() else {
            return false;
        };
        for problem in problems {
            let issue = verbs::issue(&problem, "similarity rebuild");
            eprintln!("umx similar: {} ({})", issue.message(), issue.code());
        }
        true
    }
}

#[cfg(test)]
fn record_hold(since: Instant, files: Option<usize>) {
    let us = since.elapsed().as_micros() as u64;
    LONGEST_UNIT_HOLD_US.fetch_max(us, Ordering::SeqCst);
    HOLDS.lock().unwrap().push((us, files));
}

/// Every hold [`record_hold`] timed, in µs, with the files an Apply unit wrote (`None` for a flush step, which writes
/// one similar_to file), for the hold A/B bench (S201-1).
#[cfg(test)]
pub(crate) static HOLDS: std::sync::Mutex<Vec<(u64, Option<usize>)>> =
    std::sync::Mutex::new(Vec::new());

struct Daemon {
    build: Build,
    socket: String,
    started: Instant,
    /// When this process started serving, in ms since the epoch: tells a restarted daemon from the one before it,
    /// which has the same pid (`umx daemon restart`).
    started_ms: u64,
    mode: &'static str,
    services: Arc<RwLock<umx_services::Services>>,
    config: Arc<umx_services::ConfigServices>,
    /// Set while graph folders are closed by an invalid config (the key or file to name).
    config_closed: Option<String>,
    /// Every named embedder instance, built once at start (req-73). Services hold only the active one.
    embedders: Arc<umx_compose::Embedders>,
    /// The machine config the instances were built from: `embeddings probe` builds a fresh one-off instance when a
    /// name is not in it or its keys changed since (ws004 s27).
    built_from: umx_core::config::Resolved,
    /// Builds one instance from the machine config as it is now (compose's `embedder_now`); a test swaps in a fake.
    build_now: BuildNow,
    /// `az` and `azd` as the adapter runs them: absolute paths when found on PATH at start.
    az: (String, String),
    /// None once `daemon stop` has gone through the writer: new writes are refused.
    writer: Mutex<Option<Sender<Job>>>,
    /// How `daemon restart` starts this process again; None in tests that build a Daemon directly.
    relaunch: Option<Relaunch>,
    /// Set by `daemon restart` before the exec: the accept loop stops, drops the listener and acks on `parked`
    /// (F64), so no connection is mid-accept when the process image is replaced.
    relaunching: AtomicBool,
    parked: Mutex<Option<Sender<()>>>,
    /// False once a write panicked: memory may no longer match the files, so later writes are refused and status
    /// says so. `daemon stop` still works.
    writer_ok: Arc<AtomicBool>,
    /// Search's query embeds, one thread per instance, each bounded at 5 s (req-53).
    queries: Arc<query_embed::QueryLanes>,
    /// What the writer and the embed worker share about the backlog, and each instance's background calls.
    drain: Arc<embed_worker::Drain>,
    /// The graph view's channel (ws009 S6): its own lock beside the services, never inside them (architect W7).
    /// The writer holds it too, to hand it the change feed (S5a); the lock order is services, then this.
    ui: Arc<Mutex<umx_services::ui::UiChannel>>,
    /// Wakes the views' waits on a new event, a change or a stop; a wait sleeps here holding no other lock.
    ui_wake: Arc<Condvar>,
    /// `ui wait` connections from their request read to their reply written: a stop or restart lets them answer
    /// first (`ui_quiesce`).
    ui_replies: Arc<AtomicUsize>,
    /// Each graph's `vectors_digest`, kept by (key, load instance, vectors generation) so a polling view pays the
    /// O(nodes) hash once per change (ws004 s34b); `embeddings status` and `similarity pairs`' copies share it (s36).
    /// Lock order, everywhere: the services read hold first, then this mutex; never this mutex first.
    vectors_digests: Mutex<umx_services::DigestMemo>,
    /// Runs between a `similarity pairs` copy and its compute, with no lock held (tests: a write must not wait).
    #[cfg(test)]
    pairs_hook: Mutex<Option<PairsHook>>,
}

#[cfg(test)]
type PairsHook = Box<dyn Fn() + Send + Sync>;

/// Builds one embedder instance by name from the config as it is now; None when the config does not define it.
type BuildNow = Arc<dyn Fn(&str) -> Option<umx_compose::Instance> + Send + Sync>;

/// Starts loading the local model of each loaded graph's instance that has not answered yet (embeddings-expert and
/// architect, s08): only instances a loaded graph uses, never every configured one, so an unused model costs nothing.
fn warm_local(
    services: &umx_services::Services,
    allows: &(dyn Fn(&umx_core::ports::GraphDir, &str) -> bool + Send + Sync),
    queries: &query_embed::QueryLanes,
) {
    for alias in services.graph().aliases() {
        let Some(dir) = services.graph().loaded(alias).map(|l| l.dir.clone()) else {
            continue;
        };
        if let Some(i) = services
            .embedding()
            .embedder_for_where(services.graph(), alias, &|n| allows(&dir, n))
        {
            queries.warm(&i.embedder);
        }
    }
}

/// Reloads the graphs this machine's registry lists as loaded (F72), before the daemon says it is ready, so a restart
/// (in place, stop then start, or the service) brings them back. Nothing else runs yet, so it holds the services lock
/// as a write would; embeds and similarity rebuilds are queued and local models warmed as for `graph load`. A graph
/// that does not reload is logged here and reported by `daemon status` and `umx doctor`; never fatal.
fn reload(
    services: &Arc<RwLock<umx_services::Services>>,
    config_closed: bool,
    ok: &AtomicBool,
    jobs: &embed_worker::Queue,
    tx: &Sender<Job>,
    allows: &(dyn Fn(&umx_core::ports::GraphDir, &str) -> bool + Send + Sync),
    queries: &query_embed::QueryLanes,
) {
    guarded(ok, || {
        let mut s = services.write().unwrap_or_else(|e| e.into_inner());
        let refused = config_closed.then_some("config_invalid");
        let report = s.reload_remembered(&umx_services::Ctx::new("daemon"), refused);
        if let Some(why) = &report.unreadable {
            eprintln!("umx daemon: the graph registry cannot be used ({why}); nothing reloaded");
        }
        for n in &report.not_reloaded {
            eprintln!(
                "umx daemon: graph {} at {} did not reload ({})",
                n.alias, n.folder, n.code
            );
        }
        warm_local(&s, allows, queries);
        embed_worker::enqueue(jobs, s.take_embed_jobs());
        let rebuilds = s.take_similarity_rebuilds();
        drop(s);
        similarity::spawn_rebuilds(services, tx, rebuilds);
    });
}

/// The alias of the request's own `graph_not_loaded` error: the first error only, never a warning, so a check that
/// reports another graph as a warning (evidence_unchecked) never loads a graph (graph-addressing-expert, F72: cost,
/// and a secured-graph oracle).
fn missed_alias(reply: &Envelope) -> Option<String> {
    match reply.errors.first() {
        Some(e) if e.code() == "graph_not_loaded" => e.subject().map(str::to_owned),
        _ => None,
    }
}

/// The graphs a `--graph a,b` request names, in order without repeats; none for one graph or no `--graph` (F118).
fn graph_list(request: &Request) -> Option<Vec<String>> {
    let given = request.graph.as_deref().filter(|g| g.contains(','))?;
    let mut list: Vec<String> = Vec::new();
    for alias in given.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        if !list.iter().any(|a| a == alias) {
            list.push(alias.to_owned());
        }
    }
    Some(list)
}

/// What a request missed that the writer may load on demand (F72, F99).
enum Miss {
    Alias(String),
    /// F118: a `--graph a,b` read's whole list, in order. The writer works out which are missing under its exclusive
    /// lock and plans them all before it loads any: a graph loaded when the read missed may be unloaded on purpose
    /// before the writer runs this (review R257-B1).
    Aliases(Vec<String>),
    /// A saved scope's name: the writer resolves its members to graph ids and registry rows itself.
    Scope(String),
}

/// The saved scope of a `--scope` request whose first error is `scope_graph_not_loaded` (F99); never a warning.
fn missed_scope(request: &Request, reply: &Envelope) -> Option<String> {
    match reply.errors.first() {
        Some(e) if e.code() == "scope_graph_not_loaded" => request.scope.clone(),
        _ => None,
    }
}

/// Every instance the daemon built, as services hold them (ws004 s17): each graph picks its own by key and policy.
fn built(embedders: &umx_compose::Embedders) -> Vec<umx_services::embedding::Instance> {
    embedders
        .instances()
        .iter()
        .filter_map(|i| {
            let embedder = i.built.as_ref().ok()?;
            Some(umx_services::embedding::Instance {
                name: i.name.clone(),
                embedder: Arc::clone(embedder),
            })
        })
        .collect()
}

/// The one folder rule (req-72; security s11, s17), decided on the repo layer itself: a repo layer that is refused
/// or does not parse (`repo_layer_ok`) allows none, so its text is never sent. A repo layer that sets
/// `embedder.active` allows only the instance it names (`none` allows none), even when it names the same value as
/// the machine, and even if a hosted instance makes the graph's model. A repo layer whose file sets it to anything
/// that is not a valid narrowing (so `resolve` drops it and the machine's value shows through) allows none: fail
/// closed, as for a broken repo layer (F1). A folder whose repo layer does not set it may use the machine's
/// instances.
fn folder_allows(config: &Arc<umx_services::ConfigServices>) -> embed_worker::FolderAllows {
    let config = Arc::clone(config);
    Arc::new(move |dir, instance| {
        if !config.repo_layer_ok(dir) {
            return false;
        }
        let resolved = config.resolve(Some(dir));
        match resolved.get("embedder.active") {
            Some(s) if s.source == umx_core::config::Source::Repo => {
                s.value
                    .as_ref()
                    .and_then(umx_core::config::ConfigValue::as_text)
                    == Some(instance)
            }
            _ => !config.repo_layer_sets(dir, "embedder.active"),
        }
    })
}

/// Runs one writer step; a panic marks the writer failed instead of killing it, so `daemon stop` still works.
fn guarded<T>(ok: &AtomicBool, step: impl FnOnce() -> T) -> Option<T> {
    if !ok.load(Ordering::SeqCst) {
        return None;
    }
    let result = catch_unwind(AssertUnwindSafe(step)).ok();
    if result.is_none() {
        ok.store(false, Ordering::SeqCst);
    }
    result
}

/// Stub in a `stub` build (fakes, req-51), else real.
fn mode() -> umx_compose::Mode {
    #[cfg(feature = "stub")]
    return umx_compose::Mode::Stub;
    #[cfg(not(feature = "stub"))]
    umx_compose::Mode::Real
}

impl Daemon {
    fn new(config: &Config, build: Build) -> Daemon {
        let mode = mode();
        // One config service for config commands and graph writes' `root:` lookups (in stub mode, one fake file).
        let (mut composed, settings) = umx_compose::compose_with_config(mode, config);
        // Built once, here, never mid-query. One that cannot be built is reported, and the daemon still starts.
        let embedders = umx_compose::embedders(mode, config, &settings);
        // Real mode wires the instance embedder.active names, or none (stub mode keeps compose's fake). Fixed until
        // the daemon restarts, so the embed worker and every graph use one embedder and models never mix.
        if mode == umx_compose::Mode::Real {
            let default = embedders
                .active(&settings.resolve(None))
                .ok()
                .flatten()
                .map(|(name, _)| name);
            composed.set_instances(built(&embedders), default.as_deref());
            composed.set_pace(embedders.pace());
            // D62 (security): every hosted instance is trusted with named graphs' text, so each is named with its host
            // at start.
            let resolved = settings.resolve(None);
            for i in embedders
                .instances()
                .iter()
                .filter(|i| i.kind != "local" && i.built.is_ok())
            {
                let endpoint = resolved
                    .value(&format!(
                        "{}.{}.endpoint",
                        umx_core::config::INSTANCES,
                        i.name
                    ))
                    .and_then(umx_core::config::ConfigValue::as_text)
                    .unwrap_or("");
                eprintln!(
                    "umx daemon: embedder {} is trusted at {} (it is in the machine config)",
                    i.name,
                    doctor::host_of(endpoint)
                );
            }
        }
        composed.set_floors(embedders.floors(&settings.resolve(None)));
        Daemon::with_services(config, build, composed, settings, mode, embedders)
    }

    /// The daemon over composed services. With an embedder, an embed worker thread makes vectors off the writer
    /// and hands them back to it as `Job::Apply`.
    fn with_services(
        config: &Config,
        build: Build,
        mut composed: umx_services::Services,
        config_services: Arc<umx_services::ConfigServices>,
        mode: umx_compose::Mode,
        embedders: umx_compose::Embedders,
    ) -> Daemon {
        composed.set_idle_unload_ms(config.idle_unload_ms);
        let built_from = config_services.resolve(None);
        let embedders = Arc::new(embedders);
        let embeds = !composed.embedding().instances().is_empty();
        let ui = composed.graph().ui_channel();
        let services = Arc::new(RwLock::new(composed));
        let config_writes = Arc::clone(&config_services);
        let (tx, rx) = channel::<Job>();
        let queue = Arc::new((
            Mutex::new(umx_services::EmbedQueue::default()),
            Condvar::new(),
        ));
        let queries = Arc::new(query_embed::QueryLanes::new());
        let drain = Arc::new(embed_worker::Drain::default());
        if embeds {
            embed_worker::spawn(
                Arc::clone(&services),
                Arc::clone(&queue),
                tx.clone(),
                folder_allows(&config_services),
                Arc::clone(&queries),
                Arc::clone(&drain),
            );
        }
        let writer_ok = Arc::new(AtomicBool::new(true));
        reload(
            &services,
            config.config_closed.is_some(),
            &writer_ok,
            &queue,
            &tx,
            &*folder_allows(&config_services),
            &queries,
        );
        let ui = UiFeed {
            channel: Arc::new(Mutex::new(ui)),
            wake: Arc::new(Condvar::new()),
        };
        // The graphs the registry reloaded are the views' first steps.
        ui.feed(&mut services.write().unwrap_or_else(|e| e.into_inner()));
        let warms = Arc::clone(&queries);
        let warm_allows = folder_allows(&config_services);
        let jobs = Arc::clone(&queue);
        let writes = Arc::clone(&services);
        let rebuild_tx = tx.clone();
        let ok = Arc::clone(&writer_ok);
        let status_drain = Arc::clone(&drain);
        let writer = Writer {
            services: writes,
            ok,
            config: config_writes,
            jobs,
            warms,
            warm_allows,
            rebuild_tx,
            drain,
            next_apply: std::sync::atomic::AtomicU64::new(0),
            ui: ui.clone(),
        };
        std::thread::spawn(move || writer.run(rx));
        let build_now: BuildNow = {
            let (setup, settings) = (config.clone(), Arc::clone(&config_services));
            Arc::new(move |name: &str| umx_compose::embedder_now(mode, &setup, &settings, name))
        };
        Daemon {
            build,
            socket: config.socket.display().to_string(),
            started: Instant::now(),
            started_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            mode: mode.name(),
            services,
            config: config_services,
            config_closed: config.config_closed.clone(),
            embedders,
            built_from,
            build_now,
            az: umx_compose::az_programs(config),
            writer: Mutex::new(Some(tx)),
            writer_ok,
            queries,
            drain: status_drain,
            relaunch: None,
            relaunching: AtomicBool::new(false),
            parked: Mutex::new(None),
            ui: ui.channel,
            ui_wake: ui.wake,
            ui_replies: Arc::new(AtomicUsize::new(0)),
            vectors_digests: Mutex::new(umx_services::DigestMemo::new()),
            #[cfg(test)]
            pairs_hook: Mutex::new(None),
        }
    }

    /// Serves one connection. In the lane (`lane`), only a lifecycle request runs; anything else is `daemon_busy`.
    fn serve(&self, mut stream: UnixStream, limits: ReadLimits, lane: bool) {
        let accepted = Instant::now();
        let mut view_reply = None;
        let reply = match read_request(&mut stream, limits) {
            Ok(bytes) if lane => match Request::peek(&bytes) {
                Some(head) if head.is_lifecycle() => self.answer(&bytes),
                Some(head) => busy(&head.command),
                None => busy("unknown"),
            },
            Ok(bytes) => {
                if Request::peek(&bytes).is_some_and(|h| h.command == "ui wait") {
                    self.ui_replies.fetch_add(1, Ordering::SeqCst);
                    view_reply = Some(Slot(Arc::clone(&self.ui_replies)));
                    // The wait watches this connection, so a client that hangs up frees its slot (ws009 M1b2).
                    ui_channel::WAIT_CLIENT.with(|c| *c.borrow_mut() = stream.try_clone().ok());
                    let reply = self.answer(&bytes);
                    ui_channel::WAIT_CLIENT.with(|c| c.borrow_mut().take());
                    reply
                } else {
                    self.answer(&bytes)
                }
            }
            Err(code) => Envelope::fail("unknown", internal(code)),
        };
        let ok = reply.errors.is_empty();
        let stopping = ok && reply.command == "daemon stop";
        let restarting = ok && reply.command == "daemon restart";
        // The whole reply is written (or its time ran out) and the stream shut before the socket goes away or the
        // process is replaced. A lane connection lives LANE_LIFETIME in all; any other has REPLY_WRITE to take it.
        let write_by = if lane {
            accepted + LANE_LIFETIME
        } else {
            Instant::now() + REPLY_WRITE
        };
        // A reply with rows already written as JSON goes out in three parts, so its rows are held once (ws004 s34a).
        let (head, rows, tail) = reply.json_parts();
        let _ = write_reply(&mut stream, head.as_bytes(), write_by)
            && write_reply(&mut stream, rows.as_bytes(), write_by)
            && write_reply(&mut stream, tail.as_bytes(), write_by);
        let _ = stream.shutdown(Shutdown::Both);
        drop(stream);
        drop(view_reply);
        if restarting && !self.quiesce_accept(Path::new(&self.socket)) {
            eprintln!(
                "umx daemon restart: the accept loop did not stop within 1 s; restarting anyway"
            );
        }
        if stopping || restarting {
            self.ui_quiesce();
            let _ = fs::remove_file(&self.socket);
        }
        if stopping {
            self.exit(0);
        }
        if restarting {
            self.relaunch();
        }
    }

    /// Stops the accept loop before an exec (F64, architect note f:64): sets the flag, connects once to `socket` to
    /// wake accept, and waits for the loop to drop the listener and ack, all of it, the connect included, by one
    /// deadline [`QUIESCE`] away (review R137-B1). False if it did not ack in time; the caller then logs and execs.
    fn quiesce_accept(&self, socket: &Path) -> bool {
        let deadline = Instant::now() + QUIESCE;
        let (ack, acked) = channel();
        *self.parked.lock().unwrap_or_else(|e| e.into_inner()) = Some(ack);
        self.relaunching.store(true, Ordering::SeqCst);
        wake_accept(socket);
        acked
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }

    /// Replaces this process with a new `umx daemon start` (F46): same pid, stdout and stderr (the log). Every other
    /// file descriptor umx holds (the listener, client streams, graph and config files) is close-on-exec, as std
    /// opens them, so the new process inherits none; a client still connected sees end of stream. A pending embed
    /// round is dropped; the next load finds and queues those nodes again.
    fn relaunch(&self) -> ! {
        use std::os::unix::process::CommandExt;
        let Some(r) = &self.relaunch else {
            eprintln!("umx daemon restart: this daemon cannot start itself again");
            self.exit(1);
        };
        let err = std::process::Command::new(&r.exe).args(&r.args).exec();
        eprintln!(
            "umx daemon restart: could not start {} again: {err}",
            r.exe.display()
        );
        self.exit(1);
    }

    /// Ends the process after a stop (or a restart that cannot exec). The writer has applied every queued write and
    /// stopped (`Job::Stop` and `Job::Restart` reply only after [`Daemon::finish`]); only then is each local model
    /// released, its sessions joined and their contexts and model freed, so exit's ggml Metal teardown finds none
    /// loaded (F207). Rust drops nothing at exit, and other threads still hold the embedders, so dropping cannot do it.
    /// An exec runs no exit handlers, so a restart needs none of this.
    fn exit(&self, code: i32) -> ! {
        for instance in self.embedders.instances() {
            instance.release_local();
        }
        std::process::exit(code);
    }

    /// Refuses a request from another protocol version before it is parsed or run (A7): its fields may mean
    /// something else here, so a write could run without what was asked. `daemon status` and `daemon stop` still
    /// answer, so the `next` (stop, then start the CLI's build) works.
    fn answer(&self, bytes: &[u8]) -> Envelope {
        if let Some(head) = Request::peek(bytes)
            && head.umx != PROTOCOL
            && !head.is_lifecycle()
        {
            let versions = [
                ("cli protocol", Fill::count(head.umx.into())),
                ("daemon protocol", Fill::count(PROTOCOL.into())),
            ];
            return Envelope::fail(
                &head.command,
                Issue::fill(Code::ProtocolMismatch, &versions),
            );
        }
        match Request::from_json(bytes) {
            Ok(request) => self.handle(&request),
            Err(_) => Envelope::fail("unknown", internal("request_unreadable")),
        }
    }

    /// Sends a job to the writer and waits for its reply. After `daemon stop`, writes are refused.
    fn through_writer(
        &self,
        command: &str,
        job: impl FnOnce(Sender<Envelope>) -> Job,
        stop: bool,
    ) -> Envelope {
        let (tx, rx) = channel();
        let sent = {
            let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
            let sent = writer.as_ref().is_some_and(|w| w.send(job(tx)).is_ok());
            if stop {
                *writer = None;
            }
            sent
        };
        if !sent {
            return Envelope::fail(command, Issue::new(Code::DaemonNotRunning));
        }
        rx.recv()
            .unwrap_or_else(|_| Envelope::fail(command, internal("writer_stopped")))
    }

    fn handle(&self, request: &Request) -> Envelope {
        let reply = self.dispatch(request);
        // Hints in catalogue order, at most MAX_HINTS of them (ws002 X1), once for the whole reply.
        let reply = self.flag_candidates(self.on_demand(request, reply).settle_hints());
        if request.client == self.build {
            reply
        } else {
            let builds = [
                ("daemon build", Fill::build(&self.build)),
                ("cli build", Fill::build(&request.client)),
            ];
            reply.warn(Issue::fill(Code::DaemonVersionMismatch, &builds))
        }
    }

    /// F234: names `field:title` on each problem candidate whose title umx's secret check flags (a near duplicate, an
    /// ambiguous or missing node's matches, a spine's anchors), as a read names a flagged title on its row. Here, once
    /// for the whole reply, because candidates are made on many paths that hold no secret allows. `--human` prints no
    /// candidate, so only JSON shows the title, under its flag.
    ///
    /// F242 (architect N1 on #402, R405, security): a candidate's title is unflagged only when every graph loaded now
    /// that holds a node of that id with that exact title allows it. The check reads the current holders and allows
    /// (a second services read), not the snapshot that made the reply, so changes to the loaded holders or to allows
    /// during the request can clear a flag or add one. For example, with copies G1 (not allowing the title) and G2
    /// (allowing it), unloading G1 or retitling its node before the flag pass clears the flag. A known limit: the
    /// window is one request; a cleared flag always means text some graph's owner explicitly allowed; the JSON carries
    /// the title either way; and the flag is advisory. Candidates are found by [`CANDIDATE_ID`] and
    /// [`CANDIDATE_TITLE`], the keys umx-core's `Candidate` writes (a test in verbs keeps them equal).
    fn flag_candidates(&self, mut reply: Envelope) -> Envelope {
        let mut titled: Vec<&mut Value> = reply
            .warnings
            .iter_mut()
            .chain(reply.hints.iter_mut())
            .chain(reply.errors.iter_mut())
            .flat_map(|issue| issue.candidates_mut().iter_mut())
            .filter(|c| c.get(CANDIDATE_TITLE).is_some_and(Value::is_string))
            .collect();
        if titled.is_empty() {
            return reply;
        }
        let text =
            |c: &Value, key: &str| c.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        let pairs: Vec<(String, String)> = titled
            .iter()
            .map(|c| (text(c, CANDIDATE_ID), text(c, CANDIDATE_TITLE)))
            .collect();
        let refs: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(id, title)| (id.as_str(), title.as_str()))
            .collect();
        let flagged = self
            .services
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .candidate_titles_flagged(&refs);
        for (c, on) in titled.iter_mut().zip(flagged) {
            if on {
                c["secret_suspected"] = json!([umx_core::place::named_place(CANDIDATE_TITLE)]);
            }
        }
        reply
    }

    /// req-15, F72 (architect): a request that failed graph_not_loaded for an alias this machine remembers as loaded
    /// (it unloaded when idle, or did not reload at start) loads that graph through the writer, then runs once more.
    /// Never a loop: the rerun's reply stands. A failed load returns the load's own error. The rerun's envelope
    /// carries the load's warnings, graph_loaded_on_demand first. A graph another request loaded since this one
    /// missed it is not loaded again: the request just runs once more (review R156-B1).
    /// F99: a `--scope` read refused `scope_graph_not_loaded` loads every member the registry remembers as loaded,
    /// then runs once more, the same way; a member unloaded on purpose loads nothing.
    /// F118: a `--graph a,b` read that missed one of its graphs loads every listed graph the registry remembers as
    /// loaded, as a scope does, then runs once more; one listed graph unloaded on purpose loads nothing.
    /// A view's wait never loads a graph (architect): it may name a graph it saw unloaded, and must not bring
    /// it back.
    fn on_demand(&self, request: &Request, reply: Envelope) -> Envelope {
        match self.miss_for(request, reply) {
            ControlFlow::Continue(miss) => self.load_then_rerun(request, miss),
            ControlFlow::Break(env) => env,
        }
    }

    /// What a read that missed asks the writer to load, read under the shared lock; or the reply to give instead (its
    /// own, or a rerun's when another request loaded what it missed).
    fn miss_for(&self, request: &Request, reply: Envelope) -> ControlFlow<Envelope, Miss> {
        if matches!(request.op, Some(Op::UiWait { .. })) {
            return ControlFlow::Break(reply);
        }
        let listed = missed_alias(&reply)
            .and_then(|alias| graph_list(request).filter(|list| list.contains(&alias)));
        let miss = if let Some(list) = listed {
            // A cheap filter only: the writer decides the plan again before it loads anything.
            let missing: Option<Vec<String>> = match self.services.read() {
                Ok(s) => {
                    let missing: Vec<String> = list
                        .iter()
                        .filter(|a| s.graph().loaded(a).is_none())
                        .cloned()
                        .collect();
                    missing
                        .iter()
                        .all(|a| s.graph().remembered_loaded(a).is_some())
                        .then_some(missing)
                }
                Err(_) => None,
            };
            match missing {
                None => return ControlFlow::Break(reply),
                // Another request loaded them since this one missed: run once more (review R156-B1).
                Some(missing) if missing.is_empty() => {
                    return ControlFlow::Break(self.dispatch(request));
                }
                // The whole list, not the missing ones: the writer works them out again (review R257-B1).
                Some(_) => Miss::Aliases(list),
            }
        } else if let Some(alias) = missed_alias(&reply) {
            let (now_loaded, remembered) = match self.services.read() {
                Ok(s) => (
                    s.graph().loaded(&alias).is_some(),
                    s.graph().remembered_loaded(&alias).is_some(),
                ),
                Err(_) => (false, false),
            };
            if now_loaded {
                return ControlFlow::Break(self.dispatch(request));
            }
            if !remembered {
                return ControlFlow::Break(reply);
            }
            Miss::Alias(alias)
        } else if let Some(scope) = missed_scope(request, &reply) {
            // A cheap filter only: the writer decides the plan again before it loads anything.
            let plan = match self.services.read() {
                Ok(s) => s.graph().scope_load_plan(&scope).ok(),
                Err(_) => None,
            };
            match plan {
                None => return ControlFlow::Break(reply),
                // Another request loaded them since this one missed: run once more (review R156-B1).
                Some(rows) if rows.is_empty() => return ControlFlow::Break(self.dispatch(request)),
                Some(_) => Miss::Scope(scope),
            }
        } else {
            return ControlFlow::Break(reply);
        };
        ControlFlow::Continue(miss)
    }

    /// Loads what a read missed through the writer, then runs the read once more with the loads' warnings first.
    fn load_then_rerun(&self, request: &Request, miss: Miss) -> Envelope {
        let command = request.command.as_str();
        let loaded = self.through_writer(
            command,
            |tx| Job::LoadRemembered(miss, Box::new(request.clone()), tx),
            false,
        );
        if !loaded.errors.is_empty() {
            return loaded;
        }
        // The loads' warnings and hints go first, each in its own list; only a warning moves the status (ws002 X1).
        let mut env = self.dispatch(request);
        if !loaded.warnings.is_empty() && env.errors.is_empty() {
            env.status = umx_protocol::Status::Warning;
        }
        env.warnings.splice(0..0, loaded.warnings);
        env.hints.splice(0..0, loaded.hints);
        env
    }

    fn dispatch(&self, request: &Request) -> Envelope {
        let command = request.command.as_str();
        if (request.all || request.include_named || request.include_provenance)
            && let Some(problem) = verbs::scope_usage(request)
        {
            return Envelope::fail(command, verbs::issue(&problem, command));
        }
        let writer_ok = self.writer_ok();
        let lifecycle = request.is_lifecycle();
        match (&request.op, command, request.args.as_slice()) {
            // The label must be the operation's own: a write never runs under another command's name.
            (Some(op), _, _) if op.command() != command => {
                Envelope::fail(command, internal("command_does_not_match_op"))
            }
            // Memory may no longer match the files once the writer has failed (A6): serve nothing from it.
            (op, _, _) if !writer_ok && !lifecycle => {
                let label = op.as_ref().map_or(command, |op| op.command());
                Envelope::fail(label, internal("writer_stopped"))
            }
            // An invalid config closes every graph folder; the config verbs still run so it can be fixed.
            (Some(op), _, _) if !op.is_config() && self.config_closed.is_some() => {
                let closed = self.config_closed.as_deref().unwrap_or_default();
                Envelope::fail(
                    op.command(),
                    verbs::issue(
                        &umx_core::config::problems::config_invalid(closed),
                        op.command(),
                    ),
                )
            }
            // Never on the writer and never under the services lock: a hosted call can retry for ~40 s (F1).
            (Some(Op::EmbeddingsProbe { embedder }), _, _) => {
                self.probe(request, embedder.as_deref())
            }
            // Planned under the shared lock, so reading every shard never holds the writer (architect F1).
            (Some(Op::EmbeddingsReembed { dry_run: false, .. }), _, _) => {
                let planned = match self.services.read() {
                    Ok(services) => verbs::reembed_plan(&services, request),
                    Err(_) => Err(Box::new(Envelope::fail(
                        command,
                        internal("writer_stopped"),
                    ))),
                };
                match planned {
                    Ok(plan) => {
                        self.through_writer(command, |tx| Job::Reembed(Box::new(plan), tx), false)
                    }
                    Err(env) => *env,
                }
            }
            // Planned and finished under the shared lock; the query embed between holds none, so a hosted embedder's
            // seconds never hold the writer (architect, s11).
            (Some(Op::Search { .. }), _, _) => {
                // A folder whose repo config names another embedder (or none) gets no vector leg (security, s11).
                let allows = folder_allows(&self.config);
                let planned = match self.services.read() {
                    Ok(services) => verbs::search_plan(&services, request, &*allows),
                    Err(_) => Err(Box::new(Envelope::fail(
                        command,
                        internal("writer_stopped"),
                    ))),
                };
                match planned {
                    Ok(plan) => {
                        // Each instance re-checked right before it is asked, under a brief read lock: every graph whose
                        // leg uses it must still admit it, its folder too (D53, R176-1).
                        let admitted = |alias: &str, name: &str| {
                            self.services.read().is_ok_and(|s| {
                                s.embedding().admits_now(s.graph(), alias, name)
                                    && s.loaded(alias).is_some_and(|l| allows(&l.dir, name))
                            })
                        };
                        let queries =
                            plan.embed_with(&admitted, &|e, text| self.queries.embed(e, text));
                        match self.services.read() {
                            Ok(services) => verbs::search_finish(&services, request, plan, queries),
                            Err(_) => Envelope::fail(command, internal("writer_stopped")),
                        }
                    }
                    Err(env) => *env,
                }
            }
            // Read under the shared lock with the one folder rule; sends nothing (architect, ws004 s27).
            (Some(Op::EmbeddingsStatus), _, _) => {
                let allows = folder_allows(&self.config);
                match self.services.read() {
                    Ok(services) => verbs::embeddings_status(
                        &services,
                        &|d, n| allows(d, n),
                        &mut self
                            .vectors_digests
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()),
                    ),
                    Err(_) => Envelope::fail(command, internal("writer_stopped")),
                }
            }
            // A config verb (the writer writes it unless it is a dry run), then what the import means for the loaded
            // graphs, read under the shared lock (embeddings-expert F4).
            (Some(Op::ConfigImport { dry_run, .. }), _, _) => {
                let env = if *dry_run {
                    verbs::config(&self.config, request)
                } else {
                    self.through_writer(
                        command,
                        |tx| Job::Write(Box::new(request.clone()), tx),
                        false,
                    )
                };
                self.import_warnings(env)
            }
            // Computed with no lock held, applied under the writer (architect, ws004 s09).
            (Some(Op::SimilarityRebuild), _, _) => self.similarity_rebuild(request),
            // Planned and each graph copied under its own short read hold, computed with no lock held, finished under
            // one more (graph-storage-expert and architect, ws004 s31): a write never waits on it.
            (Some(Op::SimilarityPairs { .. }), _, _) => self.similarity_pairs(request),
            // The graph view's channel (ws009 S6): daemon memory only, never the writer; the wait holds no lock.
            (
                Some(Op::UiWait {
                    epoch,
                    since,
                    timeout_ms,
                    graphs,
                    revs,
                    supervisor,
                }),
                _,
                _,
            ) => self.ui_wait(
                epoch.as_deref(),
                *since,
                *timeout_ms,
                graphs,
                revs,
                *supervisor,
            ),
            (Some(Op::UiCommand { .. } | Op::UiSelect { .. } | Op::UiSelection), _, _) => {
                self.ui_op(request)
            }
            (Some(op), _, _) if op.writes() => self.through_writer(
                op.command(),
                |tx| Job::Write(Box::new(request.clone()), tx),
                false,
            ),
            (Some(op), _, _) if op.is_config() => verbs::config(&self.config, request),
            // req-116: each open graph's repository is found through the config files port.
            (
                Some(Op::GraphList {
                    all: true,
                    graphfile: None,
                }),
                _,
                _,
            ) => match self.services.read() {
                Ok(services) => verbs::graph_index(&services, Some(&self.config), request),
                Err(_) => Envelope::fail(command, internal("writer_stopped")),
            },
            // A poisoned lock means a write panicked while holding it, maybe before the flag above was set.
            (Some(op), _, _) => match self.services.read() {
                Ok(services) => verbs::read(&services, request),
                Err(_) => Envelope::fail(op.command(), internal("writer_stopped")),
            },
            (None, "daemon status", []) => {
                self.status_warnings(Envelope::ok(command, self.status()))
            }
            (None, "daemon stop", []) => self.through_writer(command, Job::Stop, true),
            (None, "daemon restart", []) => self.through_writer(command, Job::Restart, true),
            (None, "daemon status" | "daemon stop" | "daemon restart", _) => {
                Envelope::fail(command, usage(command, "this command takes no arguments"))
            }
            _ => Envelope::fail(command, usage(command, "unknown command")),
        }
    }

    /// `umx similarity rebuild`: snapshot under the shared lock, compute with no lock, apply under the writer; a graph
    /// whose lists are not set up yet goes through the writer's one-call rebuild instead.
    fn similarity_rebuild(&self, request: &Request) -> Envelope {
        let command = "similarity rebuild";
        let started = match self.services.read() {
            Ok(services) => services.similarity_rebuild_start(&verbs::ctx(request)),
            Err(_) => return Envelope::fail(command, internal("writer_stopped")),
        };
        let (alias, snapshot) = match started {
            Ok(s) => s,
            Err(problem) => return Envelope::fail(command, verbs::issue(&problem, command)),
        };
        let Some(writer) = self
            .writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return Envelope::fail(command, internal("writer_stopped"));
        };
        if snapshot.is_none() {
            return self.through_writer(
                command,
                |tx| Job::Write(Box::new(request.clone()), tx),
                false,
            );
        }
        match similarity::rebuild(&self.services, &writer, &alias, snapshot) {
            similarity::Rebuilt::Done {
                files_written,
                warnings,
                ..
            } => {
                let summary = self
                    .services
                    .read()
                    .ok()
                    .and_then(|s| s.similarity_summary(&alias, files_written));
                match summary {
                    Some(value) => verbs::similarity_rebuilt(
                        command,
                        Ok(umx_services::Done {
                            value,
                            notes: warnings,
                        }),
                    ),
                    None => Envelope::fail(command, internal("graph_unloaded")),
                }
            }
            similarity::Rebuilt::NotSetUp => self.through_writer(
                command,
                |tx| Job::Write(Box::new(request.clone()), tx),
                false,
            ),
            similarity::Rebuilt::Failed(problem) => {
                Envelope::fail(command, verbs::issue(&problem, command))
            }
            similarity::Rebuilt::Moved => {
                Envelope::fail(command, internal("vectors_kept_changing"))
            }
            similarity::Rebuilt::Stopped => Envelope::fail(command, internal("writer_stopped")),
        }
    }

    /// `umx similarity pairs` (ws004 s31). Each step takes the services' read lock only for itself, so a write waits at
    /// most one graph's copy; the compute runs on up to [`PAIRS_THREADS`] threads and stops at the backstop or when the
    /// daemon begins a stop or restart. A graph unloaded, reloaded or no longer readable meanwhile runs it once more,
    /// then refuses.
    fn similarity_pairs(&self, request: &Request) -> Envelope {
        let command = "similarity pairs";
        let ask = match verbs::pairs_ask(request) {
            Ok(ask) => ask,
            Err(env) => return *env,
        };
        let ctx = verbs::ctx(request);
        let fail = |p: &umx_core::Problem| Envelope::fail(command, verbs::issue(p, command));
        let stopped = || Envelope::fail(command, internal("writer_stopped"));
        let deadline = Instant::now() + Duration::from_millis(umx_services::PAIRS_BACKSTOP_MS);
        let stop_cause = || {
            if self.relaunching.load(Ordering::SeqCst)
                || self
                    .writer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_none()
            {
                Some("the daemon is stopping or restarting")
            } else if Instant::now() >= deadline {
                Some("it ran past its 60 s backstop")
            } else {
                None
            }
        };
        for _ in 0..2 {
            #[cfg(test)]
            let held = Instant::now();
            let planned = match self.services.read() {
                Ok(services) => services.pairs_plan(&ctx, &ask),
                Err(_) => return stopped(),
            };
            #[cfg(test)]
            pairs_time("plan", held);
            let mut job = match planned {
                Ok(job) => job,
                Err(p) => return fail(&p),
            };
            let mut moved = false;
            for g in 0..job.graphs() {
                #[cfg(test)]
                let held = Instant::now();
                let copied = match self.services.read() {
                    // The memo's mutex inside the services read hold (the lock order at `vectors_digests`).
                    Ok(services) => services.pairs_copy(
                        &mut job,
                        g,
                        &mut self
                            .vectors_digests
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()),
                    ),
                    Err(_) => return stopped(),
                };
                #[cfg(test)]
                pairs_time("copy", held);
                match copied {
                    Ok(true) => {}
                    Ok(false) => {
                        moved = true;
                        break;
                    }
                    Err(p) => return fail(&p),
                }
            }
            if moved {
                continue;
            }
            #[cfg(test)]
            if let Some(hook) = &*self.pairs_hook.lock().unwrap_or_else(|e| e.into_inner()) {
                hook();
            }
            let threads = PAIRS_THREADS.min(
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1),
            );
            let cancel = || stop_cause().is_some();
            #[cfg(test)]
            let computing = Instant::now();
            let parts: Vec<Result<Option<umx_services::PairsPart>, ()>> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..threads)
                    .map(|p| {
                        let (job, cancel) = (&job, &cancel);
                        s.spawn(move || job.part(p, threads, cancel))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().map_err(|_| ()))
                    .collect()
            });
            let mut done = Vec::with_capacity(parts.len());
            for part in parts {
                match part {
                    Ok(Some(part)) => done.push(part),
                    Ok(None) => {
                        let cause = stop_cause().unwrap_or("it was stopped");
                        return fail(&umx_services::problems::similarity_pairs_timed_out(cause));
                    }
                    Err(()) => return Envelope::fail(command, internal("pairs_part_panicked")),
                }
            }
            let found = job.merge(done);
            #[cfg(test)]
            pairs_time("compute", computing);
            #[cfg(test)]
            let held = Instant::now();
            let finished = match self.services.read() {
                Ok(services) => services.pairs_finish(&ctx, found),
                Err(_) => return stopped(),
            };
            #[cfg(test)]
            pairs_time("finish", held);
            match finished {
                Ok(Some(done)) => return verbs::pairs_reply(command, Ok(done)),
                Ok(None) => continue,
                Err(p) => return fail(&p),
            }
        }
        fail(&umx_services::problems::usage_invalid(
            "graphs changed during the read (retry)",
            "embeddings",
        ))
    }

    /// `umx config import`'s warnings about the key it makes (embeddings-expert F4): no loaded graph records it, and
    /// its similarity floor is not calibrated.
    fn import_warnings(&self, mut env: Envelope) -> Envelope {
        let key = env.data["key"]
            .as_str()
            .and_then(|k| umx_core::ports::ModelKey::parse(k).ok());
        let (Some(key), Some(embedder)) = (key, env.data["embedder"].as_str().map(str::to_owned))
        else {
            return env;
        };
        let key_fill = || Fill::model_key(&key.to_string()).unwrap_or(Fill::word("(that key)"));
        let recorded = self
            .services
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .graph_list()
            .iter()
            .any(|g| g.embedding.as_ref().and_then(|e| e.key.as_ref()) == Some(&key));
        if !recorded {
            let fills = [("key", key_fill()), ("embedder", Fill::text(&embedder))];
            env = env.warn(Issue::fill(Code::ImportKeyUnmatched, &fills));
        }
        // The floor the key's similar_to lists would use now: the machine config's, else the calibrated table's (s26).
        let settings = {
            let services = self.services.read().unwrap_or_else(|e| e.into_inner());
            umx_core::similar::Settings::of(&key, None, services.graph().floors()).0
        };
        if !settings.calibrated() {
            let floor = format!("{:.2}", f64::from(settings.floor) / 10_000.0);
            let fills = [("key", key_fill()), ("floor", Fill::text(&floor))];
            env = env.warn(Issue::fill(Code::SimilarityFloorUncalibrated, &fills));
        }
        env
    }

    /// `umx embeddings probe`: picks the instance (for `--graph`, with that graph's folder config and policy), then
    /// runs ready() and one embed of umx's fixed sentence on this request's thread. The services lock is held only
    /// to read the graph's folder, policy and key.
    fn probe(&self, request: &Request, requested: Option<&str>) -> Envelope {
        const COMMAND: &str = "embeddings probe";
        let fail = |p: &umx_core::Problem| Envelope::fail(COMMAND, verbs::issue(p, COMMAND));
        let allows = folder_allows(&self.config);
        let graph = match request.graph.as_deref() {
            None => None,
            Some(alias) => {
                let services = self.services.read().unwrap_or_else(|e| e.into_inner());
                let Some(loaded) = services.loaded(alias) else {
                    return fail(&services.graph().not_loaded(alias));
                };
                // The policy umx acts on, read fresh: the stricter of graph.json and this machine's record, secured
                // when the record cannot be read (A2). graph.json's own word is never enough.
                let policy = services
                    .effective_policy(alias)
                    .unwrap_or(umx_core::Policy::SECURED);
                let key = loaded.meta.embedding.as_ref().and_then(|e| e.key.clone());
                let name = umx_core::ValidName::alias(alias);
                // The instance the graph embeds with (s17): its recorded key, allowed by its policy and, first, by the
                // folder rule (R106-3).
                let own = services
                    .embedding()
                    .embedder_for_where(services.graph(), alias, &|n| allows(&loaded.dir, n));
                Some((loaded.dir.clone(), policy, key, name, own))
            }
        };
        let resolved = self.config.resolve(graph.as_ref().map(|g| &g.0));
        // A graph's own instance, unless --embedder asks for another.
        let own = graph
            .as_ref()
            .and_then(|g| g.4.clone().filter(|_| requested.is_none()));
        // An instance added or changed since start is built now from the config, used for this probe and dropped
        // (ws004 s27); the running daemon keeps the instances it started with until it restarts.
        let not_in_use: std::cell::Cell<Option<&'static str>> = std::cell::Cell::new(None);
        let lookup = |name: &str| {
            let running = self.embedders.get(name);
            let now = self.config.resolve(None);
            if let Some(i) = running
                && umx_compose::instance_unchanged(&self.built_from, &now, name)
            {
                return Some(i.built.clone());
            }
            let fresh = (self.build_now)(name)?;
            not_in_use.set(Some(if running.is_some() {
                "changed"
            } else {
                "added"
            }));
            Some(fresh.built)
        };
        let for_graph = graph
            .as_ref()
            .and_then(|(_, policy, _, name, _)| name.as_ref().map(|n| (n, *policy)));
        let (instance, embedder) = match own {
            Some(own) => (own.name, own.embedder),
            None => match umx_services::embedding::choose(&resolved, requested, for_graph, &lookup)
            {
                Ok(chosen) => chosen,
                Err(p) => return fail(&p),
            },
        };
        // The folder rule bounds every choice for a graph, --embedder included: nothing is asked of an instance the
        // graph's folder may not use.
        if let Some((dir, ..)) = &graph
            && !allows(dir, &instance)
        {
            return fail(&umx_services::problems::embedder_unavailable(
                &instance,
                requested,
                "the graph's folder config cannot be read or names another embedder",
                "fix it (umx config check) or choose the instance it names",
            ));
        }
        let started = Instant::now();
        let probed = match umx_services::embedding::probe(&instance, requested, &*embedder) {
            Ok(p) => p,
            Err(p) => return fail(&p),
        };
        let latency = started.elapsed().as_millis();
        let alias = request.graph.as_deref();
        let mut env = Envelope::ok(COMMAND, verbs::probed(&instance, alias, &probed, latency));
        if let Some(how) = not_in_use.get() {
            let fills = [
                ("embedder", Fill::text(&instance)),
                ("how", Fill::word(how)),
            ];
            env = env.warn(Issue::fill(Code::EmbedderNotInUse, &fills));
        }
        if let Some((_, _, Some(graph_key), Some(name), _)) = &graph
            && *graph_key != probed.key
        {
            let p = umx_services::problems::embedder_mismatch(name, graph_key, &probed.key);
            env = env.note(verbs::issue(&p, COMMAND));
        }
        env
    }

    /// The embedder rows of `daemon status`: the active instance, its credential once one worked, and where `az`
    /// and `azd` are. Never a token or key; it fetches nothing.
    fn embedder_status(&self) -> Value {
        let services = self.services.read().unwrap_or_else(|e| e.into_inner());
        let wired = services.embedding();
        let resolved = self.config.resolve(None);
        let problem = self.embedders.active(&resolved).err();
        let stats = self.drain.stats();
        json!({
            "active": self.embedders.active_name(&resolved),
            "key": wired.key().map(|k| k.to_string()),
            "credential": wired.credential().map(|c| json!({ "kind": c.kind, "from": c.from })),
            "problem": problem,
            "instances": self.embedders.instances().iter().map(|i| {
                let s = stats.get(&i.name).cloned().unwrap_or_default();
                json!({
                    "name": i.name,
                    "kind": i.kind,
                    "built": i.built.is_ok(),
                    "problem": i.built.as_ref().err(),
                    "backend": i.local_backend().map(|(b, _)| b),
                    // ws004 s25: background calls at once, the pace and what the provider counted.
                    "background": {
                        "slots": s.slots,
                        "in_flight": s.in_flight,
                        "calls": s.calls,
                        "tokens_counted": s.tokens_counted,
                        "rate_limited": s.rate_limited,
                        "tokens_per_minute": i.tokens_per_minute,
                        "rate": s.rate.or(i.tokens_per_minute),
                        "last_halved_s_ago": s.last_halved.map(|t| t.elapsed().as_secs()),
                    },
                })
            }).collect::<Vec<_>>(),
            "az": self.az.0,
            "azd": self.az.1,
        })
    }

    /// `daemon status`'s registry rows (F72): the file, and what the start-up reload did. Folders and aliases are
    /// this user's own machine state, as `graph list` shows them.
    fn registry_status(&self) -> Value {
        let services = self.services.read().unwrap_or_else(|e| e.into_inner());
        let store = services.graph();
        let r = store.reload_report();
        json!({
            "file": store.registry_file().as_str(),
            "unreadable": r.unreadable,
            "skipped_rows": r.skipped_rows,
            "reloaded": r.reloaded.iter().map(|g| json!({
                "alias": g.alias,
                "folder": g.folder,
                "warnings": g.warnings,
            })).collect::<Vec<_>>(),
            "not_reloaded": r.not_reloaded.iter().map(|g| json!({
                "alias": g.alias,
                "folder": g.folder,
                "code": g.code,
            })).collect::<Vec<_>>(),
            "primary_restored": r.primary_restored,
        })
    }

    /// `daemon status`'s warnings: each graph the start-up reload made primary again (F80).
    fn status_warnings(&self, env: Envelope) -> Envelope {
        let services = self.services.read().unwrap_or_else(|e| e.into_inner());
        let report = services.graph().reload_report();
        let restored = report.primary_restored.clone();
        let saved: Vec<Issue> = report
            .warnings
            .iter()
            .map(|p| verbs::issue(p, "daemon status"))
            .collect();
        drop(services);
        let env = restored
            .iter()
            .filter_map(|alias| Fill::name(alias))
            .fold(env, |env, alias| {
                env.warn(Issue::fill(Code::PrimaryRestored, &[("alias", alias)]))
            });
        saved.into_iter().fold(env, Envelope::warn)
    }

    /// False once a write panicked: the flag the writer sets, or the lock a panicking write poisoned.
    fn writer_ok(&self) -> bool {
        self.writer_ok.load(Ordering::SeqCst) && !self.services.is_poisoned()
    }

    fn status(&self) -> Value {
        let graphs = self
            .services
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .graph_list()
            .len();
        let writer_ok = self.writer_ok();
        let adapters: serde_json::Map<String, Value> = umx_compose::describe(
            &self.services.read().unwrap_or_else(|e| e.into_inner()),
            &self.config,
        )
        .into_iter()
        .map(|(port, adapter)| (port.replace('-', "_"), json!(adapter)))
        .collect();
        json!({
            // A daemon whose writer failed answers, but is not running as it should: stop and start it again.
            "running": writer_ok,
            "writer": if writer_ok { "ok" } else { "failed" },
            "pid": std::process::id(),
            "version": self.build,
            "socket": self.socket,
            "uptime_s": self.started.elapsed().as_secs(),
            "started_ms": self.started_ms,
            "mode": self.mode,
            "graphs_loaded": graphs,
            "registry": self.registry_status(),
            "adapters": adapters,
            // `invalid` while an invalid config keeps graph folders closed (`umx config check` says why).
            "config": if self.config_closed.is_some() { "invalid" } else { "ok" },
            "embedder": self.embedder_status(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The read tests' time unit. Every limit and every wait below is a whole number of units, and each margin is
    /// at least a unit, so a busy CI runner cannot blur which limit ended a read (a 100 ms gap against a 150 ms
    /// limit flaked on macOS CI; review R93-2).
    const U: u64 = 250;

    fn units(n: u64) -> Duration {
        Duration::from_millis(n * U)
    }

    /// First byte 12 units (3 s), then 4 units (1 s) between bytes, 24 units (6 s) in all. Clients that keep a
    /// request alive send a byte every 50 ms, 20 times inside the idle limit.
    const TEST_LIMITS: ReadLimits = ReadLimits {
        first_byte: Duration::from_millis(12 * U),
        idle: Duration::from_millis(4 * U),
        total: Duration::from_millis(24 * U),
    };

    /// F59: idle is shorter than the first byte again. F54 had raised it to 60 s for what looked like a client
    /// stalling before its shutdown (CI runs 42820953819, 42821026928, 42827880434); the witness showed the whole
    /// request and its EOF already in the socket while the read slept on (a missed wakeup, handled by READ_SLICE).
    #[test]
    fn the_request_limits_are_ordered() {
        assert!(READ_LIMITS.idle < READ_LIMITS.first_byte);
        assert!(READ_LIMITS.first_byte < READ_LIMITS.total);
    }

    /// A stream whose data and end are there, but whose first `missed` reads sleep their whole timeout and return
    /// WouldBlock, as a read does on macOS when its wakeup is missed (F59). Records the longest timeout armed.
    struct MissedWakeup {
        data: Vec<u8>,
        at: usize,
        missed: usize,
        armed: Duration,
        longest: Duration,
    }

    impl Read for MissedWakeup {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.missed > 0 {
                self.missed -= 1;
                // Capped, so a reader that arms a long wait fails the test quickly instead of sleeping it out.
                std::thread::sleep(self.armed.min(Duration::from_secs(1)));
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            let n = buf.len().min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    impl TimedRead for MissedWakeup {
        fn arm(&mut self, wait: Duration) {
            self.armed = wait;
            self.longest = self.longest.max(wait);
        }
    }

    /// A peer that never takes a byte: every write sleeps its armed timeout and returns WouldBlock, as a full socket
    /// does. With `accepts_after`, it takes everything from that write on (a missed wakeup, F59).
    struct NeverReads {
        armed: Duration,
        longest: Duration,
        accepts_after: Option<usize>,
        writes: usize,
    }

    impl Write for NeverReads {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.writes += 1;
            if self.accepts_after.is_some_and(|n| self.writes > n) {
                return Ok(buf.len());
            }
            std::thread::sleep(self.armed.min(Duration::from_secs(1)));
            Err(std::io::ErrorKind::WouldBlock.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl TimedWrite for NeverReads {
        fn arm_write(&mut self, wait: Duration) {
            self.armed = wait;
            self.longest = self.longest.max(wait);
        }
    }

    /// Review R129-B1: a reply nobody reads gives up at its deadline, never blocking more than a slice at a time.
    #[test]
    fn a_reply_nobody_reads_gives_up_at_its_deadline() {
        let mut peer = NeverReads {
            armed: Duration::ZERO,
            longest: Duration::ZERO,
            accepts_after: None,
            writes: 0,
        };
        let started = Instant::now();
        assert!(!write_reply(
            &mut peer,
            &[b'x'; 40_000],
            Instant::now() + units(4)
        ));
        assert!(
            started.elapsed() >= units(4) && started.elapsed() < units(8),
            "{:?}",
            started.elapsed()
        );
        assert!(peer.longest <= READ_SLICE, "{:?}", peer.longest);
    }

    #[test]
    fn a_reply_write_that_misses_its_wakeup_is_tried_again() {
        let mut peer = NeverReads {
            armed: Duration::ZERO,
            longest: Duration::ZERO,
            accepts_after: Some(3),
            writes: 0,
        };
        assert!(write_reply(
            &mut peer,
            &[b'x'; 40_000],
            Instant::now() + REPLY_WRITE
        ));
        assert!(peer.longest <= READ_SLICE, "{:?}", peer.longest);
    }

    /// F62: doctor's ask_by and running_pid read through read_until_end, which re-reads after a missed wakeup too.
    #[test]
    fn reading_to_the_end_survives_a_missed_wakeup_within_a_slice() {
        let mut stream = MissedWakeup {
            data: br#"{"pid":7}"#.to_vec(),
            at: 0,
            missed: 3,
            armed: Duration::ZERO,
            longest: Duration::ZERO,
        };
        let started = Instant::now();
        let got = read_until_end(&mut stream, Instant::now() + READ_TIMEOUT);
        assert_eq!(got.as_deref(), Some(&br#"{"pid":7}"#[..]));
        assert!(stream.longest <= READ_SLICE, "{:?}", stream.longest);
        assert!(
            started.elapsed() < 4 * READ_SLICE + units(4),
            "{:?}",
            started.elapsed()
        );
    }

    /// A reply of `left` spaces, made as it is read.
    struct Spaces {
        left: u64,
    }

    impl Read for Spaces {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.left as usize);
            buf[..n].fill(b' ');
            self.left -= n as u64;
            Ok(n)
        }
    }

    impl TimedRead for Spaces {
        fn arm(&mut self, _: Duration) {}
    }

    /// F194 (ws004 s34a): a daemon reply is read up to MAX_REPLY, past the request cap, and one byte over is refused.
    #[test]
    fn a_reply_is_read_up_to_max_reply_and_refused_one_byte_over() {
        let deadline = || Instant::now() + READ_TIMEOUT;
        let big = 2 * MAX_FRAME;
        let got = read_until_end(&mut Spaces { left: big }, deadline());
        assert_eq!(got.map(|b| b.len() as u64), Some(big));
        let mut over = Spaces {
            left: MAX_REPLY + 1,
        };
        assert_eq!(read_until_end(&mut over, deadline()), None);
        assert_eq!(over.left, 0, "every byte up to the cap was read first");
    }

    #[test]
    fn a_read_that_misses_its_wakeup_is_read_again_within_a_slice() {
        let mut stream = MissedWakeup {
            data: br#"{"whole":"request"}"#.to_vec(),
            at: 0,
            missed: 3,
            armed: Duration::ZERO,
            longest: Duration::ZERO,
        };
        let started = Instant::now();
        let got = read_request(&mut stream, READ_LIMITS);
        assert_eq!(got.as_deref(), Ok(&br#"{"whole":"request"}"#[..]));
        assert!(stream.longest <= READ_SLICE, "{:?}", stream.longest);
        assert!(
            started.elapsed() < 4 * READ_SLICE + units(4),
            "{:?}",
            started.elapsed()
        );
    }

    /// A real socket's read end that keeps, for F237, the instant each read's slice ran out (WouldBlock or TimedOut),
    /// how many bytes it has read, whether a read is under way, and the longest timeout armed.
    struct Watched {
        inner: UnixStream,
        ran_out: Vec<Instant>,
        read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        reading: std::sync::Arc<std::sync::atomic::AtomicBool>,
        longest: Duration,
    }

    impl Read for Watched {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            use std::sync::atomic::Ordering::SeqCst;
            self.reading.store(true, SeqCst);
            let got = self.inner.read(buf);
            self.reading.store(false, SeqCst);
            match &got {
                Ok(n) => {
                    self.read.fetch_add(*n, SeqCst);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    self.ran_out.push(Instant::now());
                }
                Err(_) => {}
            }
            got
        }
    }

    impl TimedRead for Watched {
        fn arm(&mut self, wait: Duration) {
            self.longest = self.longest.max(wait);
            self.inner.arm(wait);
        }
    }

    /// When the writer of [`read_watched`] shuts its end: before the read begins, or once the reader has every byte
    /// and is in its next read, after a pause.
    #[derive(Clone, Copy, Debug)]
    enum End {
        Before,
        WhileReading(Duration),
    }

    /// Writes `len` bytes, shuts down as `end` says, and reads the request with `read_request` through a [`Watched`]
    /// stream. Returns what was read, the stream, and the two instants the end went in between: `t0` just before the
    /// shutdown, `t1` just after.
    fn read_watched(
        len: usize,
        end: End,
    ) -> (Result<usize, &'static str>, Watched, Instant, Instant) {
        use std::sync::atomic::Ordering::SeqCst;
        let (server, mut client) = UnixStream::pair().unwrap();
        let mut stream = Watched {
            inner: server,
            ran_out: Vec::new(),
            read: Default::default(),
            reading: Default::default(),
            longest: Duration::ZERO,
        };
        let data = vec![b'x'; len];
        let End::WhileReading(pause) = end else {
            client.write_all(&data).unwrap();
            let t0 = Instant::now();
            client.shutdown(Shutdown::Write).unwrap();
            let t1 = Instant::now();
            let got = read_request(&mut stream, READ_LIMITS).map(|b| b.len());
            return (got, stream, t0, t1);
        };
        let (read, reading) = (stream.read.clone(), stream.reading.clone());
        let writer = std::thread::spawn(move || {
            client.write_all(&data).unwrap();
            // Until the reader has every byte and is in its next read; bounded, so a broken reader still ends.
            let until = Instant::now() + Duration::from_secs(10);
            while !(read.load(SeqCst) == len && reading.load(SeqCst)) && Instant::now() < until {
                std::thread::yield_now();
            }
            std::thread::sleep(pause);
            let t0 = Instant::now();
            client.shutdown(Shutdown::Write).unwrap();
            let t1 = Instant::now();
            (client, t0, t1)
        });
        let got = read_request(&mut stream, READ_LIMITS).map(|b| b.len());
        let (_client, t0, t1) = writer.join().unwrap();
        (got, stream, t0, t1)
    }

    /// F237 (review R1): of the instants slices ran out, how many were certain missed wakeups and how many cannot be
    /// told. The end went in between `t0` and `t1`, so a slice that ran out half a slice or more after `t1` had the
    /// end in the socket that long (F59's missed wakeup); one that ran out before `t0` waited on a writer still
    /// writing or holding its end back (a starved writer under load, not the bug); one in between is only reported.
    /// Classified by when each slice ran out, after the writer is done, never by sampling a flag after a read.
    fn missed_wakeups(ran_out: &[Instant], t0: Instant, t1: Instant) -> (usize, usize) {
        let missed = ran_out
            .iter()
            .filter(|at| t1 + READ_SLICE / 2 <= **at)
            .count();
        let unclear = ran_out.iter().filter(|at| t0 <= **at).count() - missed;
        (missed, unclear)
    }

    /// F59 condition 3: a request written and shut down is read to its end within one slice, never by a read
    /// sleeping out its timeout while the request and its end are in the socket (the missed wakeup F59 found):
    /// whether the end was there when the read began, or came while a read waited. F237: judged by when each slice
    /// ran out against when the end went in, not by how long the whole read took, which a loaded machine stretches
    /// with no slice running out. Every read arms a timeout, and none longer than a slice (review R2).
    #[test]
    fn a_request_and_its_end_are_read_within_one_slice() {
        // Every schedule is read before any assertion, so a failure names each schedule it holds for.
        let mut wrong = Vec::new();
        for (len, end) in [
            (2_000, End::Before),
            (300_000, End::WhileReading(Duration::from_millis(20))),
        ] {
            let (got, stream, t0, t1) = read_watched(len, end);
            if got != Ok(len) {
                wrong.push(format!("{end:?}: read {got:?}"));
            }
            if !(Duration::ZERO < stream.longest && stream.longest <= READ_SLICE) {
                wrong.push(format!(
                    "{end:?}: the longest timeout armed was {:?}",
                    stream.longest
                ));
            }
            let (missed, unclear) = missed_wakeups(&stream.ran_out, t0, t1);
            if missed > 0 {
                wrong.push(format!(
                    "{end:?}: {missed} reads slept out their slice with the end in the socket ({} slices ran out, \
                     {unclear} too near the end to tell)",
                    stream.ran_out.len()
                ));
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// F237 (review R1): a writer that holds its end back is not a missed wakeup. It keeps the end two slices after
    /// the reader has every byte, so the reader's slices run out as they do when a loaded machine starves the writer:
    /// they are seen, and none is counted.
    #[test]
    fn a_writer_that_holds_its_end_back_is_not_a_missed_wakeup() {
        let (got, stream, t0, t1) = read_watched(2_000, End::WhileReading(2 * READ_SLICE));
        assert_eq!(got, Ok(2_000));
        assert!(
            !stream.ran_out.is_empty(),
            "the reader's slices ran out while it waited"
        );
        assert_eq!(missed_wakeups(&stream.ran_out, t0, t1).0, 0);
    }

    /// Runs `client` on one end of a socket pair and reads a request from the other with [`TEST_LIMITS`].
    fn read_from(
        client: impl FnOnce(UnixStream) + Send + 'static,
    ) -> (Result<Vec<u8>, &'static str>, Duration) {
        let (mut server, peer) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || client(peer));
        let started = Instant::now();
        let got = read_request(&mut server, TEST_LIMITS);
        let took = started.elapsed();
        drop(server);
        writer.join().unwrap();
        (got, took)
    }

    fn send_then_close(mut s: UnixStream, chunks: &[&[u8]], gap: Duration) {
        for chunk in chunks {
            std::thread::sleep(gap);
            let _ = s.write_all(chunk);
        }
        let _ = s.shutdown(Shutdown::Write);
    }

    #[test]
    fn a_slow_but_live_client_is_served() {
        // The first byte comes late (more than the idle limit, within the first-byte limit), then a byte at a time
        // with gaps under the idle limit: the whole request arrives.
        let (got, _) = read_from(|s| {
            // Two units past the idle limit and four before the first-byte limit.
            std::thread::sleep(units(8));
            send_then_close(
                s,
                &[b"{", b"\"a\"", b":", b"1", b"}"],
                Duration::from_millis(50),
            );
        });
        assert_eq!(got.as_deref(), Ok(&b"{\"a\":1}"[..]));
    }

    #[test]
    fn a_client_that_wrote_and_closed_before_the_read_is_served() {
        // The whole request is in the socket and the peer is gone before the daemon reads (a fast client on a busy
        // daemon); macOS then refuses to set a read timeout, which must not fail the request.
        let (mut server, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(b"{\"a\":1}").unwrap();
        drop(peer);
        assert_eq!(
            read_request(&mut server, TEST_LIMITS).as_deref(),
            Ok(&b"{\"a\":1}"[..])
        );
    }

    #[test]
    fn a_stalled_partial_frame_is_cut_off() {
        let (got, took) = read_from(|mut s| {
            s.write_all(b"{\"half").unwrap();
            std::thread::sleep(units(20));
        });
        assert_eq!(got, Err("request_timed_out"));
        assert!(
            took < units(8),
            "cut at the idle limit, not the first-byte one: {took:?}"
        );
    }

    #[test]
    fn a_silent_client_is_cut_off_at_the_first_byte_limit() {
        let (got, took) = read_from(|_s| std::thread::sleep(units(16)));
        assert_eq!(got, Err("request_timed_out"));
        assert!(
            took >= units(12) - Duration::from_millis(10) && took < units(20),
            "{took:?}"
        );
    }

    #[test]
    fn a_trickle_that_never_ends_is_cut_off_at_the_total_limit() {
        // A byte every 50 ms never trips the idle limit, so only the total limit ends it.
        let (got, took) = read_from(|mut s| {
            for _ in 0..(36 * U / 50) {
                std::thread::sleep(Duration::from_millis(50));
                if s.write_all(b"x").is_err() {
                    return;
                }
            }
        });
        assert_eq!(got, Err("request_timed_out"));
        assert!(
            took >= units(24) - Duration::from_millis(10) && took < units(32),
            "{took:?}"
        );
    }

    #[test]
    fn a_request_is_read_up_to_max_frame_and_no_further() {
        let (got, _) = read_from(|mut s| {
            let chunk = vec![b'a'; 1 << 20];
            for _ in 0..(MAX_FRAME / (1 << 20) + 2) {
                if s.write_all(&chunk).is_err() {
                    return;
                }
            }
        });
        assert_eq!(got.map(|b| b.len() as u64), Ok(MAX_FRAME));
    }

    #[test]
    fn a_panicking_step_marks_the_writer_failed_and_later_steps_do_not_run() {
        let ok = AtomicBool::new(true);
        assert_eq!(guarded(&ok, || 1), Some(1));
        assert_eq!(guarded(&ok, || -> i32 { panic!("planted") }), None);
        assert!(!ok.load(Ordering::SeqCst));
        let mut ran = false;
        assert_eq!(guarded(&ok, || ran = true), None);
        assert!(!ran);
    }

    fn this_uid() -> Option<u32> {
        Config::at(std::path::PathBuf::from("/"), None).uid
    }

    #[test]
    fn the_run_dir_must_be_owner_only_and_owned_by_this_user() {
        let root = std::path::PathBuf::from(format!("/tmp/umx-u-{}-owner", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("run");
        assert_eq!(owner_only_dir(&dir, this_uid()), Ok(()));

        // Second origin: the filesystem's owner of the folder this process just made.
        let meta = fs::symlink_metadata(&dir).unwrap();
        let uid = this_uid().expect("effective uid");
        assert_eq!(uid, meta.uid());

        assert_eq!(owner_only(&meta, uid.wrapping_add(1)), Err(NOT_YOURS));
        fs::set_permissions(&dir, Permissions::from_mode(0o750)).unwrap();
        assert_eq!(owner_only_dir(&dir, this_uid()), Err(NOT_OWNER_ONLY));
        fs::remove_dir_all(&root).unwrap();
    }

    /// F1: a probe whose embedder is stuck inside `embed` holds neither the writer nor the services lock, so a write
    /// that needs both finishes meanwhile.
    #[test]
    fn a_stuck_probe_holds_neither_the_writer_nor_the_services_lock() {
        use std::sync::mpsc;
        use umx_core::ports::{
            Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind,
        };
        struct Stuck {
            entered: Mutex<mpsc::Sender<()>>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl Embedder for Stuck {
            fn embed(&self, texts: &[String], _: TextKind) -> Result<Embedded, EmbedError> {
                let _ = self.entered.lock().unwrap().send(());
                let _ = self.release.lock().unwrap().recv();
                Ok(Embedded {
                    vectors: texts.iter().map(|_| vec![1.0, 0.0]).collect(),
                    truncated: Vec::new(),
                    tokens: None,
                })
            }
            fn key(&self) -> ModelKey {
                ModelKey::new("m", 2).unwrap()
            }
            fn concurrency_ceiling(&self) -> usize {
                1
            }
            fn max_input_tokens(&self) -> usize {
                64
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
                "fake.embedder"
            }
        }
        let home = std::path::PathBuf::from(format!("/tmp/umx-unit-{}-stuck", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            "embedder.active = \"stuck\"\nembedders.stuck.kind = \"local\"\n\
             embedders.stuck.model = \"m\"\nembedders.stuck.dims = 2\n",
        )
        .unwrap();
        let (entered, entered_rx) = mpsc::channel();
        let (release_tx, release) = mpsc::channel();
        let stuck = Stuck {
            entered: Mutex::new(entered),
            release: Mutex::new(release),
        };
        let mut d = Daemon::new(
            &Config::at(home.clone(), Some(true)).roots([home.to_str().unwrap()]),
            build(),
        );
        d.embedders = Arc::new(umx_compose::Embedders::from_instances(vec![
            umx_compose::Instance {
                name: "stuck".into(),
                kind: "local".into(),
                built: Ok(Arc::new(stuck)),
                local: None,
                tokens_per_minute: None,
            },
        ]));
        let write = op(umx_protocol::Op::GraphNew {
            path: home.join("g").display().to_string(),
            alias: "demo".into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured: false,
            no_graphfile: false,
            graphfile: false,
        });
        std::thread::scope(|s| {
            let probe =
                s.spawn(|| d.handle(&op(umx_protocol::Op::EmbeddingsProbe { embedder: None })));
            entered_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("the probe reached embed");
            let (done, written) = mpsc::channel();
            let (d, write) = (&d, &write);
            s.spawn(move || {
                let _ = done.send(d.handle(write));
            });
            let finished = written.recv_timeout(Duration::from_secs(10));
            // Release the probe before asserting, so a blocked write fails the test instead of hanging it.
            release_tx.send(()).unwrap();
            let env = finished.expect("the write finished while the probe was stuck");
            assert!(env.errors.is_empty(), "{}", env.to_json());
            let env = probe.join().unwrap();
            assert!(env.errors.is_empty(), "{}", env.to_json());
            assert_eq!(env.data["dims"], 2);
        });
        fs::remove_dir_all(&home).unwrap();
    }

    fn build() -> Build {
        Build {
            version: "t".into(),
            commit: "t".into(),
            built: "t".into(),
        }
    }

    /// A daemon that is not bound to a socket; `handle` and `answer` are called directly.
    fn daemon() -> Daemon {
        let home = std::path::PathBuf::from(format!("/tmp/umx-unit-{}", std::process::id()));
        Daemon::new(&Config::at(home, Some(true)), build())
    }

    fn op(op: umx_protocol::Op) -> Request {
        Request::op(op, None, None, build())
    }

    /// A daemon on a fresh socket with `slots`; returns the socket and the in-flight count.
    fn listening(name: &str, slots: Slots) -> (std::path::PathBuf, Arc<AtomicUsize>) {
        let socket =
            std::path::PathBuf::from(format!("/tmp/umx-unit-{}-{name}.sock", std::process::id()));
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let in_flight = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&in_flight);
        let d = Arc::new(daemon());
        std::thread::spawn(move || accept(&d, listener, slots, &counted));
        (socket, in_flight)
    }

    /// Review R137-B1: with a full backlog and nobody accepting, quiescing still gives up at its bound, so the restart
    /// logs and execs. On Linux the old blocking self-connect hung here until the listener closed.
    #[test]
    fn quiescing_gives_up_at_its_bound_when_nothing_accepts() {
        let socket =
            std::path::PathBuf::from(format!("/tmp/umx-unit-{}-stuck.sock", std::process::id()));
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        // Fill the backlog: on Linux the next connect blocks, on macOS it is refused.
        let filling = socket.clone();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok(s) = UnixStream::connect(&filling) {
                held.push(s);
                if held.len() > 8192 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        });
        std::thread::sleep(Duration::from_millis(500));
        let d = daemon();
        let started = Instant::now();
        assert!(!d.quiesce_accept(&socket), "acked with nothing accepting");
        assert!(
            started.elapsed() < QUIESCE + Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        drop(listener);
        let _ = fs::remove_file(&socket);
    }

    /// F64: on the flag, the accept loop drops the listener and acks within the bound; nothing connects after.
    #[test]
    fn the_accept_loop_parks_and_drops_the_listener_before_a_restart() {
        let socket =
            std::path::PathBuf::from(format!("/tmp/umx-unit-{}-park.sock", std::process::id()));
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let d = Arc::new(daemon());
        let looping = Arc::clone(&d);
        std::thread::spawn(move || {
            accept(&looping, listener, SLOTS, &Arc::new(AtomicUsize::new(0)))
        });
        // Control: before the flag, a connection is accepted.
        assert!(UnixStream::connect(&socket).is_ok());
        let started = Instant::now();
        assert!(d.quiesce_accept(&socket), "no ack");
        assert!(started.elapsed() < Duration::from_secs(1));
        // The listener is gone: the socket file is still there, but nothing accepts on it.
        assert!(UnixStream::connect(&socket).is_err());
        let _ = fs::remove_file(&socket);
    }

    fn settle(in_flight: &AtomicUsize, want: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while in_flight.load(Ordering::SeqCst) != want {
            assert!(
                Instant::now() < deadline,
                "in flight: {}",
                in_flight.load(Ordering::SeqCst)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sends `request` (or nothing, for None) and reads the whole reply.
    fn send(socket: &Path, request: Option<&Request>) -> Envelope {
        let mut stream = UnixStream::connect(socket).unwrap();
        if let Some(r) = request {
            let _ = stream.write_all(r.to_json().as_bytes());
        }
        let _ = stream.shutdown(Shutdown::Write);
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        Envelope::from_json(&bytes)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&bytes)))
    }

    fn code(env: &Envelope) -> &str {
        env.errors.first().map_or("", |e| e.code())
    }

    /// F56: past its slots the daemon answers daemon_busy before reading; the lane still serves lifecycle requests
    /// and nothing else; a slot comes back when its connection ends.
    #[test]
    fn past_its_slots_the_daemon_is_busy_but_still_answers_status() {
        let (socket, in_flight) = listening("busy", Slots { serve: 2, lane: 1 });
        let status = Request::new("daemon status", Vec::new(), build());
        let other = Request::new("graph list", Vec::new(), build());
        // Two stalled clients hold the serve slots, a third the lane.
        let mut held: Vec<UnixStream> = (0..3)
            .map(|_| UnixStream::connect(&socket).unwrap())
            .collect();
        settle(&in_flight, 3);
        // Refused without a request being read: nothing is sent at all.
        assert_eq!(code(&send(&socket, None)), "daemon_busy");
        settle(&in_flight, 3);
        // The lane slot frees when its stalled client goes; the lane then answers status, and only status.
        drop(held.pop());
        settle(&in_flight, 2);
        let env = send(&socket, Some(&status));
        assert!(env.errors.is_empty(), "{}", env.to_json());
        assert_eq!(code(&send(&socket, Some(&other))), "daemon_busy");
        settle(&in_flight, 2);
        drop(held);
        let _ = fs::remove_file(&socket);
    }

    #[test]
    fn a_freed_serve_slot_serves_any_request_again() {
        let (socket, in_flight) = listening("freed", Slots { serve: 1, lane: 0 });
        let other = Request::new("graph list", Vec::new(), build());
        let held = UnixStream::connect(&socket).unwrap();
        settle(&in_flight, 1);
        assert_eq!(code(&send(&socket, Some(&other))), "daemon_busy");
        drop(held);
        settle(&in_flight, 0);
        assert_ne!(code(&send(&socket, Some(&other))), "daemon_busy");
        settle(&in_flight, 0);
        let _ = fs::remove_file(&socket);
    }

    #[test]
    fn a_slot_is_given_back_when_its_thread_panics() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let taken = Arc::clone(&in_flight);
        let panicked = std::thread::spawn(move || {
            let _slot = admit(&taken, Slots { serve: 1, lane: 0 });
            panic!("serve panicked");
        })
        .join();
        assert!(panicked.is_err());
        assert_eq!(in_flight.load(Ordering::SeqCst), 0);
    }

    fn internal_code(env: &Envelope) -> &str {
        assert_eq!(env.errors.len(), 1, "{}", env.to_json());
        assert_eq!(env.errors[0].code(), "internal", "{}", env.to_json());
        env.errors[0].message()
    }

    #[test]
    fn after_the_writer_fails_every_request_but_status_and_stop_is_refused() {
        let d = daemon();
        let list = op(umx_protocol::Op::GraphList {
            all: false,
            graphfile: None,
        });
        assert_eq!(
            d.handle(&list).errors.len(),
            0,
            "a healthy daemon serves reads"
        );
        d.writer_ok.store(false, Ordering::SeqCst);
        let add = op(umx_protocol::Op::NodeAdd {
            on_key: None,
            raised_in: None,
            kind: "problem".into(),
            title: "x".into(),
            status: None,
            note: None,
            sets: Vec::new(),
            parent: None,
            evidence: Vec::new(),
            evidence_kind: None,
            locator: None,
        });
        for request in [&list, &add] {
            assert_eq!(
                internal_code(&d.handle(request)),
                "Internal error writer_stopped."
            );
        }
        let status = d.handle(&Request::new("daemon status", Vec::new(), build()));
        assert_eq!(
            (&status.data["writer"], &status.data["running"]),
            (&json!("failed"), &json!(false))
        );
    }

    #[test]
    fn a_poisoned_working_copy_is_not_read() {
        // A write that panicked while holding the lock: a read that arrives before the flag is set must not
        // see half-changed memory.
        let d = daemon();
        let services = Arc::clone(&d.services);
        let _ = std::thread::spawn(move || {
            let _guard = services.write().unwrap();
            panic!("planted: a write that panics");
        })
        .join();
        assert_eq!(
            internal_code(&d.handle(&op(umx_protocol::Op::GraphList {
                all: false,
                graphfile: None,
            }))),
            "Internal error writer_stopped."
        );
        let status = d.handle(&Request::new("daemon status", Vec::new(), build()));
        assert_eq!(status.data["writer"], json!("failed"));
    }

    /// An embedder that blocks inside `embed` until opened, so a test can hold the provider "busy".
    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        opened: Condvar,
        entered: std::sync::atomic::AtomicUsize,
    }

    impl Gate {
        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.opened.notify_all();
        }
    }

    impl umx_core::ports::Embedder for Gate {
        fn credential(&self) -> Option<umx_core::ports::Credential> {
            None
        }
        fn ready(&self) -> Result<(), umx_core::ports::EmbedError> {
            Ok(())
        }
        fn adapter_id(&self) -> &'static str {
            "test.gate"
        }
        fn embed(
            &self,
            texts: &[String],
            _kind: umx_core::ports::TextKind,
        ) -> Result<umx_core::ports::Embedded, umx_core::ports::EmbedError> {
            if texts.is_empty() {
                return Ok(Default::default());
            }
            self.entered.fetch_add(1, Ordering::SeqCst);
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.opened.wait(open).unwrap();
            }
            Ok(umx_core::ports::Embedded {
                vectors: texts
                    .iter()
                    .map(|t| vec![1.0, t.len() as f32, 0.5, -0.5])
                    .collect(),
                truncated: Vec::new(),
                tokens: None,
            })
        }
        fn key(&self) -> umx_core::ports::ModelKey {
            umx_core::ports::ModelKey::new("gate", 4).unwrap()
        }
        fn concurrency_ceiling(&self) -> usize {
            1
        }
        fn max_input_tokens(&self) -> usize {
            512
        }
        fn locality(&self) -> umx_core::ports::Locality {
            umx_core::ports::Locality::Local
        }
    }

    /// Waits up to s08's shared deadline (F112): the s17 test's instances wait on the embed worker, as s08's do.
    fn eventually(what: &str, check: impl Fn() -> bool) {
        let until = Instant::now() + crate::s08_tests::SETTLE;
        while !check() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The Retry-After of `Counted`'s 429: far beyond [`crate::s08_tests::SETTLE`], so anything a test sees within
    /// its wait happens inside the pause (F112).
    const PAUSE_MS: u64 = 20 * 60_000;

    /// An instance with a fixed key and locality that counts its calls; `limited` answers every call with a 429.
    struct Counted {
        key: &'static str,
        limited: bool,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl umx_core::ports::Embedder for Counted {
        fn credential(&self) -> Option<umx_core::ports::Credential> {
            None
        }
        fn adapter_id(&self) -> &'static str {
            "fake.embedder"
        }
        fn ready(&self) -> Result<(), umx_core::ports::EmbedError> {
            Ok(())
        }
        fn embed(
            &self,
            texts: &[String],
            _: umx_core::ports::TextKind,
        ) -> Result<umx_core::ports::Embedded, umx_core::ports::EmbedError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.limited {
                return Err(umx_core::ports::EmbedError::RateLimited {
                    retry_after_ms: Some(PAUSE_MS),
                });
            }
            Ok(umx_core::ports::Embedded {
                vectors: texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect(),
                truncated: Vec::new(),
                tokens: None,
            })
        }
        fn key(&self) -> umx_core::ports::ModelKey {
            umx_core::ports::ModelKey::parse(self.key).unwrap()
        }
        fn concurrency_ceiling(&self) -> usize {
            1
        }
        fn max_input_tokens(&self) -> usize {
            512
        }
        fn locality(&self) -> umx_core::ports::Locality {
            umx_core::ports::Locality::Local
        }
    }

    /// R106-2: pacing is per instance. The alphabetically first instance answers every call with a 429 (wait
    /// [`PAUSE_MS`]); the other instance's graph, written only once the 429 has paused it, still embeds meanwhile,
    /// never held behind it.
    #[test]
    fn a_rate_limited_instance_never_starves_another() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("umx-embed-starve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
        let limited = Arc::new(Counted {
            key: "limited@4",
            limited: true,
            calls: Default::default(),
        });
        let free = Arc::new(Counted {
            key: "free@4",
            limited: false,
            calls: Default::default(),
        });
        let (mut services, settings) =
            umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
        services.set_instances(
            vec![
                umx_services::embedding::Instance {
                    name: "a".into(),
                    embedder: limited.clone(),
                },
                umx_services::embedding::Instance {
                    name: "b".into(),
                    embedder: free.clone(),
                },
            ],
            Some("a"),
        );
        let d = Daemon::with_services(
            &config,
            build(),
            services,
            settings,
            umx_compose::Mode::Real,
            umx_compose::Embedders::from_instances(Vec::new()),
        );
        for (alias, key) in [("busy", "limited@4"), ("calm", "free@4")] {
            let new = d.handle(&op(umx_protocol::Op::GraphNew {
                path: root.join(alias).display().to_string(),
                alias: alias.into(),
                title: None,
                empty: false,
                embedding_model: Some(key.into()),
                secured: false,
                no_graphfile: false,
                graphfile: false,
            }));
            assert!(new.errors.is_empty(), "{}", new.to_json());
        }
        let add = |alias: &str| {
            let mut add = op(umx_protocol::Op::NodeAdd {
                on_key: None,
                raised_in: None,
                kind: "problem".into(),
                title: format!("a node in {alias}"),
                status: None,
                note: None,
                sets: Vec::new(),
                parent: None,
                evidence: Vec::new(),
                evidence_kind: None,
                locator: None,
            });
            add.graph = Some(alias.into());
            let env = d.handle(&add);
            assert!(env.errors.is_empty(), "{}", env.to_json());
        };
        add("busy");
        add("busy");
        // The dispatcher has read a's 429 (the daemon's own count, noted in the round that pauses a); only then is
        // the calm graph written, so its batch is planned while a is paused, never beside a's first call.
        eventually("the limited instance's 429 to pause it", || {
            d.drain.stats().get("a").is_some_and(|s| s.rate_limited > 0)
        });
        add("calm");
        eventually("the free instance to embed while the other waits", || {
            free.calls.load(Ordering::SeqCst) > 0
        });
        assert_eq!(
            limited.calls.load(Ordering::SeqCst),
            1,
            "a not asked again: still in its first pause"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn writes_return_before_their_vectors_and_a_busy_embedder_never_holds_the_writer() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("umx-embed-worker-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let config = Config::at(root.join("home"), Some(true)).roots([root.to_str().unwrap()]);
        let gate = Arc::new(Gate::default());
        let (services, settings) =
            umx_compose::compose_with_config(umx_compose::Mode::Real, &config);
        let services = services.with_embedder(gate.clone());
        let d = Daemon::with_services(
            &config,
            build(),
            services,
            settings,
            umx_compose::Mode::Real,
            umx_compose::Embedders::from_instances(Vec::new()),
        );
        let ok = |env: Envelope| assert!(env.errors.is_empty(), "{}", env.to_json());
        ok(d.handle(&op(umx_protocol::Op::GraphNew {
            path: root.join("g").to_str().unwrap().into(),
            alias: "g".into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured: false,
            no_graphfile: false,
            graphfile: false,
        })));
        let add = |title: &str| {
            op(umx_protocol::Op::NodeAdd {
                on_key: None,
                raised_in: None,
                kind: "problem".into(),
                title: title.into(),
                status: None,
                note: None,
                sets: Vec::new(),
                parent: None,
                evidence: Vec::new(),
                evidence_kind: None,
                locator: None,
            })
        };
        ok(d.handle(&add("First")));
        eventually("the worker to call the embedder", || {
            gate.entered.load(Ordering::SeqCst) == 1
        });
        // The provider is busy with the first node; the writer is not.
        ok(d.handle(&add("Second")));
        let shards = root.join("g/embeddings/gate@4");
        assert!(
            !shards.exists(),
            "no vector is written while embedding runs"
        );
        gate.release();
        let lines = || {
            // Shard files only: a write's temp file comes and goes beside them.
            fs::read_dir(&shards)
                .map(|dir| {
                    dir.flatten()
                        .filter(|e| e.file_name().to_string_lossy().ends_with(".tsv"))
                        .filter(|e| !e.file_name().to_string_lossy().starts_with(".umx-tmp-"))
                        .map(|e| {
                            fs::read_to_string(e.path())
                                .unwrap_or_default()
                                .lines()
                                .count()
                        })
                        .sum::<usize>()
                })
                .unwrap_or(0)
        };
        // Two nodes, each a title and a whole-node vector.
        eventually("both nodes' vectors in their shards", || lines() == 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_shard_warning_names_the_real_path() {
        let shard = umx_core::vectors::ShardFile::of(
            umx_core::ports::ModelKey::new("fake", 8).unwrap(),
            umx_core::Id(1),
        );
        let problem = umx_services::problems::vector_line_kept(
            &umx_core::ValidName::alias("demo").unwrap(),
            &shard,
            1,
        );
        let issue = crate::verbs::issue(&problem, "graph load");
        assert_eq!(
            issue.message(),
            "embeddings/fake@8/01.tsv line 1 is not a vector line umx can read; it is kept."
        );
    }

    #[test]
    fn another_protocol_version_is_refused_before_anything_runs() {
        let d = daemon();
        let mut add = op(umx_protocol::Op::GraphNew {
            path: "/tmp/umx-unit-never/g".into(),
            alias: "never".into(),
            title: None,
            empty: false,
            embedding_model: None,
            secured: false,
            no_graphfile: false,
            graphfile: false,
        });
        add.umx = PROTOCOL + 1;
        let env = d.answer(add.to_json().as_bytes());
        assert_eq!(
            env.errors[0].code(),
            "protocol_mismatch",
            "{}",
            env.to_json()
        );
        assert_eq!(env.errors[0].next(), "umx daemon stop && umx daemon start");
        assert_eq!(env.command, "graph new");
        // A request from a newer CLI whose op this build cannot even parse is still named, not unreadable.
        let future = format!(
            r#"{{"umx":{},"command":"node add","args":[],"client":{{"version":"","commit":"","built":""}},"op":{{"op":"node_teleport"}}}}"#,
            PROTOCOL + 1
        );
        assert_eq!(
            d.answer(future.as_bytes()).errors[0].code(),
            "protocol_mismatch"
        );
        for command in ["daemon status", "daemon stop"] {
            let mut r = Request::new(command, Vec::new(), build());
            r.umx = PROTOCOL - 1;
            let env = d.answer(r.to_json().as_bytes());
            assert!(env.errors.is_empty(), "{command}: {}", env.to_json());
        }
    }

    #[test]
    fn a_lifecycle_label_does_not_carry_an_operation_past_the_gates() {
        // Round 1: an old-protocol write labelled `daemon status` skipped the version check and ran.
        let d = daemon();
        for label in ["daemon status", "daemon stop", "daemon restart"] {
            let mut sneaky = op(umx_protocol::Op::GraphNew {
                path: "/tmp/umx-unit-never/g".into(),
                alias: "never".into(),
                title: None,
                empty: false,
                embedding_model: None,
                secured: false,
                no_graphfile: false,
                graphfile: false,
            });
            sneaky.command = label.into();
            sneaky.umx = PROTOCOL - 1;
            let env = d.answer(sneaky.to_json().as_bytes());
            assert_eq!(
                env.errors[0].code(),
                "protocol_mismatch",
                "{label}: {}",
                env.to_json()
            );
            // Same protocol: the label must still be the operation's own.
            sneaky.umx = PROTOCOL;
            let env = d.answer(sneaky.to_json().as_bytes());
            assert_eq!(
                internal_code(&env),
                "Internal error command_does_not_match_op.",
                "{label}"
            );
        }
        assert!(
            d.services.read().unwrap().graph_list().is_empty(),
            "nothing ran"
        );
    }
}
