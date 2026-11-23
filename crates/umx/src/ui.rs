//! `umx ui [--graph <a,b>] [--port <n>] [--no-open] [--detach]` (req-104, ws009 s2 plan 095, M1b2 plan 161): starts
//! the graph view (apps/ui) through the UiHost port and supervises it. Runs in the binary like `umx daemon install`,
//! never in the daemon (the daemon's Services hold no UiHost: compose builds it here only); the binary first checks
//! the daemon answers (umx_cli::ui_preflight).
//!
//! The supervisor (ws009 M1b2, design agreed by the architect and security, D1 to D6): one per UMX_HOME, recorded in
//! `$UMX_HOME/run/ui.json` (pid, start time, port, the daemon's start id, PROTOCOL; never a code). It keeps the
//! server's stdin lifeline; it waits on the daemon as its one
//! supervisor wait, and ends (the server's group with it) when the daemon's start id changes or the daemon is
//! unreachable for about 5 s, or the server stops (an idle detached view stops itself after 30 min with no tab
//! polling). An agent's `umx ui open --raise` reaches it as an event: with a tab connected that tab shows it; else it
//! opens a tab with a fresh single-use launch code (60 s, D5) and the graphs. The view has no per-run token: a plain
//! open (no graphs named) is the bare URL, and the server keeps the view to this machine's browsers by Host, Origin
//! and Sec-Fetch-Site (apps/ui lib/guard.ts). A launch code only lets a link name graphs, so an outside link never
//! opens a secured graph or loads an idle one. `--detach` starts the same supervisor in the background
//! (UiHost::detach) and returns once it reports ready.
//!
//! The app folder, first hit wins: UMX_UI_DIR, machine `ui.dir`, apps/ui beside the source umx was built from (all
//! resolved into `config.started`). node: machine `ui.node`, else the first `node` on PATH. Both resolved once to
//! absolute paths.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use umx_core::config::{ConfigValue, Source};
use umx_core::ports::{
    LocalState, LocalStateError, UiCodeUse, UiEnd, UiError, UiHost, UiLaunch, UiServed,
};
use umx_protocol::{Build, Code, Envelope, Fill, Issue, PROTOCOL, Value, json};

use umx_daemon::Config;

const CMD: &str = "ui";

/// How long the view's server may take to say it is ready.
pub const READY_WITHIN: Duration = Duration::from_secs(60);

/// How often umx looks whether the server still runs.
const POLL: Duration = Duration::from_millis(250);

/// How long each of the supervisor's waits is held, so it sees a stopped server within about this.
const HOLD_MS: u64 = 2_000;

/// The daemon unreachable this long ends the view (security D2).
const DAEMON_GONE: Duration = Duration::from_secs(5);

/// How long the view's tabs get to hear why it ends before its server stops (ui-expert N1).
const ENDING_GRACE: Duration = Duration::from_millis(1_500);

/// The least time a supervisor's call is given, once the liveness budget is nearly spent.
const MIN_CALL: Duration = Duration::from_millis(250);

/// How long a new supervisor asks again while the daemon's one supervisor slot is still held by one that just ended.
const SLOT_FREES: Duration = Duration::from_secs(2);

/// A detached view with no tab asking for events this long stops (security D2).
pub const IDLE_STOP: Duration = Duration::from_secs(30 * 60);

/// How long `umx ui stop` waits for the view to go.
const STOP_WITHIN: Duration = Duration::from_secs(5);

static IDLE_MS: AtomicU64 = AtomicU64::new(0);

/// The idle stop: [`IDLE_STOP`], or what umx's test driver set (never in a release build).
fn idle() -> Duration {
    match IDLE_MS.load(Ordering::SeqCst) {
        0 => IDLE_STOP,
        ms => Duration::from_millis(ms),
    }
}

/// Test driver only: a shorter idle stop, so a test sees one.
#[cfg(feature = "test-driver")]
pub fn set_idle(ms: u64) {
    IDLE_MS.store(ms, Ordering::SeqCst);
}

/// What `umx ui` was asked for (umx_cli::UiFlags).
#[derive(Clone, Debug, Default)]
pub struct UiRun {
    /// The graphs to open together (ws009 M1b1), checked by umx_cli::ui_preflight; none opens the picker.
    pub graphs: Vec<String>,
    pub port: Option<u16>,
    pub no_open: bool,
    /// Started by `--detach`: where to report ready, or why not. The idle stop is on.
    pub report_to: Option<String>,
    /// What the launcher's checks said, warnings and hints, said with the ready envelope in the list each kind
    /// names.
    pub notes: Vec<Issue>,
}

/// The view is up.
pub struct UiReady {
    pub port: u16,
    /// `http://127.0.0.1:<port>/`, no fragment: any browser on this machine opens it.
    pub url: String,
    pub graphs: Vec<String>,
    pub opened: bool,
    /// The supervisor's pid (what `umx ui stop` signals).
    pub pid: u32,
    pub detached: bool,
}

impl UiReady {
    /// The JSON envelope: never a launch code. `graphs` lists what opens; `graph` stays as the first, for older
    /// readers. `notes`: what the launcher's checks said (stale_files, F144; graph_loaded_on_demand), each a warning
    /// or a hint by its kind (ws002 X1), never a failure.
    pub fn envelope(&self, notes: &[Issue]) -> Envelope {
        let mut data = json!({
            "url": self.url, "port": self.port, "graph": self.graphs.first(), "graphs": self.graphs,
            "opened": self.opened, "pid": self.pid,
        });
        if self.detached {
            data["detached"] = json!(true);
        }
        notes
            .iter()
            .cloned()
            .fold(Envelope::ok(CMD, data), Envelope::note)
            .settle_hints()
    }
}

/// What the supervisor asks of the daemon: its waits as the supervisor (a trait, so tests script the daemon).
pub(crate) trait Daemon {
    /// The events after `since`, held at most `hold_ms`; the whole call ends `within`, however the daemon behaves.
    /// Err: the daemon did not answer in time, or refused.
    fn wait(
        &self,
        epoch: Option<&str>,
        since: u64,
        hold_ms: u64,
        within: Duration,
    ) -> Result<Waited, Issue>;
}

/// A supervisor wait's answer: the daemon's start id, the latest seq, and the graphs of each open sent with
/// `--raise` after `since`, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Waited {
    pub epoch: String,
    pub seq: u64,
    pub raises: Vec<Vec<String>>,
}

impl Waited {
    fn from(data: &Value, since: u64) -> Waited {
        let raises = data["events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["seq"].as_u64().is_some_and(|s| s > since))
            .map(|e| &e["command"])
            .filter(|c| c["kind"] == "open" && c["raise"] == true)
            .map(|c| {
                c["graphs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|g| Fill::name(g).is_some())
                    .map(str::to_owned)
                    .collect()
            })
            .collect();
        Waited {
            epoch: data["epoch"].as_str().unwrap_or_default().to_owned(),
            seq: data["seq"].as_u64().unwrap_or(since),
            raises,
        }
    }
}

/// The daemon at its socket.
pub(crate) struct Socket<'a> {
    pub socket: &'a Path,
    pub build: &'a Build,
}

impl Daemon for Socket<'_> {
    fn wait(
        &self,
        epoch: Option<&str>,
        since: u64,
        hold_ms: u64,
        within: Duration,
    ) -> Result<Waited, Issue> {
        let env = umx_cli::ui_supervisor_wait(
            self.socket,
            self.build,
            epoch.map(str::to_owned),
            since,
            hold_ms,
            within,
        )?;
        match env.errors.into_iter().next() {
            Some(refused) => Err(refused),
            None => Ok(Waited::from(&env.data, since)),
        }
    }
}

/// ui.json: the view this UMX_HOME runs (security D3 and the architect's condition 3: no secret, no code).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub pid: u32,
    pub started: String,
    pub port: u16,
    pub daemon: String,
}

impl Record {
    fn bytes(&self) -> Vec<u8> {
        json!({ "pid": self.pid, "started": self.started, "port": self.port, "daemon": self.daemon, "umx": PROTOCOL })
            .to_string()
            .into_bytes()
    }

    fn parse(bytes: &[u8]) -> Option<Record> {
        let v = umx_protocol::parse_json(std::str::from_utf8(bytes).ok()?)?;
        Some(Record {
            pid: u32::try_from(v["pid"].as_u64()?).ok()?,
            started: v["started"].as_str()?.to_owned(),
            port: u16::try_from(v["port"].as_u64()?).ok()?,
            daemon: v["daemon"].as_str()?.to_owned(),
        })
    }
}

fn state_issue(e: &LocalStateError) -> Issue {
    failed(&format!("ui.json cannot be used: {e}"))
}

/// What ui.json records, if anything (an empty file is none: a view that ended clears it).
fn read_record(state: &dyn LocalState) -> Result<Option<Record>, Issue> {
    match state.read().map_err(|e| state_issue(&e))? {
        Some(bytes) if !bytes.is_empty() => Ok(Record::parse(&bytes)),
        _ => Ok(None),
    }
}

/// Clears ui.json if it still names `pid`.
fn clear_record(state: &dyn LocalState, pid: u32) {
    if read_record(state).is_ok_and(|r| r.is_some_and(|r| r.pid == pid)) {
        let _ = state.write(b"");
    }
}

/// Whether `argv` is a `umx ui` run: an exe named umx..., any global flags, `ui`, then flags only (not a verb).
fn is_umx_ui(argv: &str) -> bool {
    let words: Vec<&str> = argv.split_whitespace().collect();
    let Some((exe, rest)) = words.split_first() else {
        return false;
    };
    let umx = Path::new(exe)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("umx"));
    let mut rest = rest.iter().skip_while(|w| w.starts_with('-'));
    umx && rest.next() == Some(&"ui") && rest.next().is_none_or(|w| w.starts_with('-'))
}

/// Whether the recorded view still runs: the same pid, start time and a `umx ui` command line (security D3).
fn live(host: &dyn UiHost, record: &Record) -> bool {
    host.process(record.pid)
        .is_some_and(|p| p.started == record.started && is_umx_ui(&p.argv))
}

/// The next for a second view: open the graphs in the running one, else see it.
fn already(graphs: &[String]) -> Issue {
    let fix = if graphs.is_empty() {
        Fill::command("ui status")
    } else {
        Fill::checked_command(&format!("umx ui open --graph {} --raise", graphs.join(",")))
    };
    Issue::fill(Code::UiAlreadyRunning, &[("fix", fix)])
}

/// The parts the supervisor runs with.
pub(crate) struct Parts<'a> {
    pub host: &'a dyn UiHost,
    pub state: &'a dyn LocalState,
    pub daemon: &'a dyn Daemon,
    /// How often the server is looked at, and the gap between failed waits.
    pub poll: Duration,
    /// How long a server told why it ends gets to tell its tabs.
    pub grace: Duration,
}

/// `umx ui` (and the background one `--detach` starts): composes the host, starts and supervises the view, calls
/// `ready` once it is up, and returns when it ends (an error envelope saying why). Ctrl-C ends umx itself; the
/// server sees its stdin close and stops its group.
pub fn ui(config: &Config, build: &Build, run: &UiRun, ready: impl FnOnce(&UiReady)) -> Envelope {
    let host = umx_compose::ui_host_opening_with(crate::mode(), config, crate::ui_opener());
    let state = umx_compose::ui_state(crate::mode(), config);
    let daemon = Socket {
        socket: &config.socket,
        build,
    };
    let parts = Parts {
        host: host.as_ref(),
        state: state.as_ref(),
        daemon: &daemon,
        poll: POLL,
        grace: ENDING_GRACE,
    };
    let launch = match (app_dir(config), node(config)) {
        (Err(issue), _) | (_, Err(issue)) => {
            return report_failure(&parts, run, Envelope::fail(CMD, issue));
        }
        (Ok(dir), Ok(node)) => (dir, node),
    };
    let umx = std::env::current_exe()
        .and_then(fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("umx"));
    let mut env = vec![
        ("PATH".to_owned(), search_path(&config.search_path)),
        ("UMX_HOME".to_owned(), config.home.display().to_string()),
        ("UMX_BIN".to_owned(), umx.display().to_string()),
        ("NODE_ENV".to_owned(), "production".to_owned()),
    ];
    if run.report_to.is_some() {
        env.push(("UMX_UI_IDLE_MS".to_owned(), idle().as_millis().to_string()));
    }
    serve(&parts, &launch.0, &launch.1, env, run, ready)
}

/// Sends `said` to the `umx ui --detach` waiting at `to` (a refusal before anything started).
pub fn report(config: &Config, to: &str, said: &Envelope) {
    let host = umx_compose::ui_host(crate::mode(), config);
    let _ = host.report(to, said.to_json().as_bytes());
}

/// A background view that ends before it said it was ready tells the waiting `umx ui --detach` why.
fn report_failure(parts: &Parts, run: &UiRun, ended: Envelope) -> Envelope {
    if let Some(to) = &run.report_to {
        let _ = parts.host.report(to, ended.to_json().as_bytes());
    }
    ended
}

/// The verb over its parts, so tests can give it fakes.
pub(crate) fn serve(
    parts: &Parts,
    dir: &Path,
    node: &Path,
    env: Vec<(String, String)>,
    run: &UiRun,
    ready: impl FnOnce(&UiReady),
) -> Envelope {
    let host = parts.host;
    let (server, first) = match start(parts, dir, node, env, run) {
        Ok(started) => started,
        Err(refused) => return report_failure(parts, run, *refused),
    };
    let me = std::process::id();
    let url = format!("http://127.0.0.1:{}/", server.port);
    let up = UiReady {
        port: server.port,
        url: url.clone(),
        graphs: run.graphs.clone(),
        opened: !run.no_open,
        pid: me,
        detached: run.report_to.is_some(),
    };
    if let Some(to) = &run.report_to {
        let _ = host.report(to, up.envelope(&run.notes).to_json().as_bytes());
    }
    ready(&up);
    let (why, end) = watch(parts, &server, &url, &first);
    // Its tabs say why before it goes (ui-expert N1); a server that cannot be told just stops. The record
    // stays until the server is gone, so status never says stopped while it still answers.
    if let Some(end) = end
        && host.ending(&server, end).is_ok()
    {
        std::thread::sleep(parts.grace);
    }
    host.stop(&server);
    clear_record(parts.state, me);
    Envelope::fail(CMD, failed(why))
}

/// Everything before the view is ready: one view per UMX_HOME, the daemon's start id, the server, the first open
/// and ui.json.
fn start(
    parts: &Parts,
    dir: &Path,
    node: &Path,
    env: Vec<(String, String)>,
    run: &UiRun,
) -> Result<(UiServed, Waited), Box<Envelope>> {
    let host = parts.host;
    let me = std::process::id();
    let fail = |issue: Issue| Box::new(Envelope::fail(CMD, issue));
    // One view per UMX_HOME (review B1): taken before anything starts or opens, and held for this process's
    // life (the kernel lets go when it ends, however it ends). ui.json only records the view once it is ours.
    if !host.own_view().map_err(|e| fail(issue(&e)))? {
        return Err(fail(already(&run.graphs)));
    }
    // The daemon's start id now; a second supervisor is refused here (one supervisor wait at a time). One that just
    // ended frees its slot within the daemon's hang-up check, so a busy answer is asked again for a moment.
    let until = Instant::now() + SLOT_FREES;
    let first = loop {
        match parts.daemon.wait(None, 0, 0, DAEMON_GONE) {
            Err(busy) if busy.code() == "ui_waiters_busy" && Instant::now() < until => {
                std::thread::sleep(parts.poll);
            }
            other => break other.map_err(fail)?,
        }
    };
    let launch = UiLaunch {
        node: node.display().to_string(),
        app_dir: dir.display().to_string(),
        port: run.port,
        env,
        ready_ms: READY_WITHIN.as_millis() as u64,
    };
    let server = host.serve(&launch).map_err(|e| fail(issue(&e)))?;
    let url = format!("http://127.0.0.1:{}/", server.port);
    let record = Record {
        pid: me,
        started: host.process(me).map(|p| p.started).unwrap_or_default(),
        port: server.port,
        daemon: first.epoch.clone(),
    };
    let opened = if run.no_open {
        Ok(())
    } else {
        open(host, &server, &url, &run.graphs, true).map_err(|e| issue(&e))
    };
    if let Err(issue) = opened.and_then(|()| {
        parts
            .state
            .write(&record.bytes())
            .map(|_| ())
            .map_err(|e| state_issue(&e))
    }) {
        host.stop(&server);
        return Err(fail(issue));
    }
    Ok((server, first))
}

/// Supervises until the view must end, and says why.
fn watch(
    parts: &Parts,
    server: &UiServed,
    url: &str,
    first: &Waited,
) -> (&'static str, Option<UiEnd>) {
    let host = parts.host;
    let mut since = first.seq;
    let mut answered = Instant::now();
    loop {
        if !host.running(server) {
            return ("the view's server stopped", None);
        }
        // The call ends with what is left of the liveness budget, so a daemon that accepts and never answers ends
        // the view near DAEMON_GONE too (review B3); the hold is well inside it.
        let left = DAEMON_GONE.saturating_sub(answered.elapsed()).max(MIN_CALL);
        let hold = HOLD_MS.min(left.as_millis() as u64 / 2);
        match parts.daemon.wait(Some(&first.epoch), since, hold, left) {
            Ok(w) if w.epoch != first.epoch => {
                return (
                    "the daemon restarted, so the view closed; start it again",
                    Some(UiEnd::Restarted),
                );
            }
            Ok(w) => {
                answered = Instant::now();
                since = w.seq;
                for graphs in &w.raises {
                    // A failed raise leaves the view as it is; the agent's own reply already went.
                    let _ = open(host, server, url, graphs, false);
                }
            }
            Err(_) if answered.elapsed() >= DAEMON_GONE => {
                return (
                    "the daemon stopped answering, so the view closed",
                    Some(UiEnd::Unreachable),
                );
            }
            Err(_) => std::thread::sleep(parts.poll),
        }
    }
}

/// Opens a tab at the view, or, unless `fresh`, leaves it to a connected tab. Named graphs go with a fresh
/// single-use launch code (the view opens a named graph only with a code this run took); none named is the bare URL.
fn open(
    host: &dyn UiHost,
    server: &UiServed,
    url: &str,
    graphs: &[String],
    fresh: bool,
) -> Result<(), UiError> {
    if fresh && graphs.is_empty() {
        return host.open(url);
    }
    let code = host.code()?;
    match host.launch_code(server, &code)? {
        UiCodeUse::Tab if !fresh => Ok(()),
        _ if graphs.is_empty() => host.open(url),
        _ => host.open(&link(url, &code, graphs)),
    }
}

/// The view's link with a launch code and the graphs (commas as %2C: the link stays within what ui_host::url_ok lets
/// umx open; the view decodes g=).
fn link(url: &str, code: &str, graphs: &[String]) -> String {
    format!("{url}#t={code}&g={}", graphs.join("%2C"))
}

/// `umx ui --detach`: refuses while a view runs, else starts `umx ui` in the background and returns what it
/// reported (ready, or why not). The checks before anything starts run here too, so most refusals are immediate.
pub fn detach(config: &Config, run: &UiRun) -> Envelope {
    let host = umx_compose::ui_host_opening_with(crate::mode(), config, crate::ui_opener());
    let state = umx_compose::ui_state(crate::mode(), config);
    if let Err(issue) = app_dir(config).and_then(|_| node(config)) {
        return Envelope::fail(CMD, issue);
    }
    let exe = std::env::current_exe()
        .and_then(fs::canonicalize)
        .unwrap_or_else(|_| PathBuf::from("umx"));
    start_detached(
        host.as_ref(),
        state.as_ref(),
        &exe.display().to_string(),
        run,
    )
}

pub(crate) fn start_detached(
    host: &dyn UiHost,
    state: &dyn LocalState,
    exe: &str,
    run: &UiRun,
) -> Envelope {
    match read_record(state) {
        Err(issue) => return Envelope::fail(CMD, issue),
        Ok(Some(record)) if live(host, &record) => {
            return Envelope::fail(CMD, already(&run.graphs));
        }
        Ok(_) => {}
    }
    let mut args = vec!["ui".to_owned()];
    if !run.graphs.is_empty() {
        args.extend(["--graph".to_owned(), run.graphs.join(",")]);
    }
    if let Some(port) = run.port {
        args.extend(["--port".to_owned(), port.to_string()]);
    }
    if run.no_open {
        args.push("--no-open".to_owned());
    }
    // The child's own checks and start, plus its first daemon wait.
    let within = READY_WITHIN + Duration::from_secs(30);
    match host.detach(exe, &args, within.as_millis() as u64) {
        Ok(done) => Envelope::from_json(&done.said).unwrap_or_else(|_| {
            Envelope::fail(CMD, failed("the background view said nothing umx can read"))
        }),
        Err(UiError::Exited(_)) => Envelope::fail(
            CMD,
            failed(
                "the background view stopped before it was ready; run umx ui --human to see why",
            ),
        ),
        Err(e) => Envelope::fail(CMD, issue(&e)),
    }
}

/// `umx ui status` (security D3): whether a view runs for this UMX_HOME, its port, pid and when it started. Never a
/// launch code, and never the graphs it shows.
pub fn status(config: &Config) -> Envelope {
    let host = umx_compose::ui_host(crate::mode(), config);
    let state = umx_compose::ui_state(crate::mode(), config);
    status_of(host.as_ref(), state.as_ref())
}

pub(crate) fn status_of(host: &dyn UiHost, state: &dyn LocalState) -> Envelope {
    match read_record(state) {
        Err(issue) => Envelope::fail("ui status", issue),
        Ok(Some(r)) if live(host, &r) => Envelope::ok(
            "ui status",
            json!({ "running": true, "port": r.port, "pid": r.pid, "started": r.started }),
        ),
        Ok(_) => Envelope::ok("ui status", json!({ "running": false })),
    }
}

/// `umx ui stop` (security D3 and the stop condition): any caller of this uid; signals only the live process ui.json
/// names, with the same start time and a `umx ui` command line, and never another (a reused pid is left alone and
/// the stale record removed).
pub fn stop(config: &Config) -> Envelope {
    let host = umx_compose::ui_host(crate::mode(), config);
    let state = umx_compose::ui_state(crate::mode(), config);
    stop_of(host.as_ref(), state.as_ref(), Duration::from_millis(50))
}

pub(crate) fn stop_of(host: &dyn UiHost, state: &dyn LocalState, poll: Duration) -> Envelope {
    let record = match read_record(state) {
        Err(issue) => return Envelope::fail("ui stop", issue),
        Ok(None) => return Envelope::ok("ui stop", json!({ "stopped": false })),
        Ok(Some(r)) => r,
    };
    if !live(host, &record) {
        clear_record(state, record.pid);
        return Envelope::ok("ui stop", json!({ "stopped": false }));
    }
    if let Err(e) = host.signal(record.pid) {
        return Envelope::fail("ui stop", issue(&e));
    }
    let deadline = Instant::now() + STOP_WITHIN;
    while live(host, &record) {
        if Instant::now() >= deadline {
            return Envelope::fail(
                "ui stop",
                failed("the view was asked to stop but still runs after 5 s"),
            );
        }
        std::thread::sleep(poll);
    }
    clear_record(state, record.pid);
    Envelope::ok("ui stop", json!({ "stopped": true, "pid": record.pid }))
}

fn failed(reason: &str) -> Issue {
    Issue::fill(Code::UiFailed, &[("reason", Fill::text(reason))])
}

fn issue(e: &UiError) -> Issue {
    match e {
        UiError::TimedOut => Issue::fill(
            Code::UiStartTimedOut,
            &[("seconds", Fill::count(READY_WITHIN.as_secs()))],
        ),
        UiError::Open(m) => Issue::fill(Code::UiOpenFailed, &[("reason", Fill::text(m))]),
        other => failed(&other.to_string()),
    }
}

fn text(config: &Config, key: &str) -> Option<String> {
    match config.started.value(key) {
        Some(ConfigValue::Text(t)) => Some(t.clone()),
        _ => None,
    }
}

fn refused(dir: &Path, reason: &'static str) -> Issue {
    Issue::fill(
        Code::UiDirRefused,
        &[("dir", Fill::path(dir)), ("reason", Fill::word(reason))],
    )
}

/// The app folder, canonical: a folder this user owns that others cannot write, holding bin/serve.mjs and the
/// standalone build's .next/standalone/server.js, both with the same checks (security F1: node runs both).
pub(crate) fn app_dir(config: &Config) -> Result<PathBuf, Issue> {
    let given = PathBuf::from(text(config, "ui.dir").unwrap_or_else(umx_compose::default_ui_dir));
    // Only the default can be the source folder umx was built from; when it is gone, umx is a binary install
    // (D76a Q11: releases ship binaries only, and the view is built from source).
    let built_from = config
        .started
        .get("ui.dir")
        .is_none_or(|s| s.source == Source::Default);
    let dir = fs::canonicalize(&given).map_err(|_| {
        if built_from {
            // The view pins its protocol, so only this binary's own commit builds a view its daemon accepts (s9 B1).
            Issue::fill(
                Code::UiDirRefused,
                &[
                    ("dir", Fill::path(&given)),
                    ("reason", Fill::source_build(env!("UMX_COMMIT_FULL"))),
                ],
            )
        } else {
            refused(&given, "it does not exist")
        }
    })?;
    let trusted = |p: &Path, what: &'static str| -> Result<(), Issue> {
        let m = fs::metadata(p).map_err(|_| refused(&dir, what))?;
        if config.uid.is_some_and(|uid| m.uid() != uid) {
            return Err(refused(
                &dir,
                "it or bin/serve.mjs or the build's server.js is owned by another user",
            ));
        }
        if m.permissions().mode() & 0o022 != 0 {
            return Err(refused(
                &dir,
                "others can write to it or bin/serve.mjs or the build's server.js (chmod go-w it)",
            ));
        }
        Ok(())
    };
    if !dir.is_dir() {
        return Err(refused(&dir, "it is not a folder"));
    }
    trusted(&dir, "it cannot be read")?;
    let script = dir.join("bin").join("serve.mjs");
    let built = dir.join(".next").join("standalone").join("server.js");
    if !script.is_file() || !built.is_file() {
        return Err(Issue::fill(Code::UiNotBuilt, &[("dir", Fill::path(&dir))]));
    }
    trusted(&script, "bin/serve.mjs cannot be read")?;
    trusted(&built, "the build's server.js cannot be read")?;
    Ok(dir)
}

fn executable(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// node, canonical: `ui.node`, else the first executable `node` on PATH.
pub(crate) fn node(config: &Config) -> Result<PathBuf, Issue> {
    let not_found =
        |reason: &'static str| Issue::fill(Code::UiNodeNotFound, &[("reason", Fill::word(reason))]);
    let found = match text(config, "ui.node") {
        Some(set) => Some(PathBuf::from(set))
            .filter(|p| executable(p))
            .ok_or_else(|| not_found("ui.node is not an executable file"))?,
        None => config
            .search_path
            .iter()
            .map(|d| d.join("node"))
            .find(|p| executable(p))
            .ok_or_else(|| not_found("ui.node is not set and there is no node on PATH"))?,
    };
    fs::canonicalize(&found).map_err(|_| not_found("node cannot be resolved"))
}

fn search_path(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use umx_core::ports::UiProcess;
    use umx_fakes::{FakeLocalState, FakeUiHost};
    use umx_protocol::{Build, Value};

    fn temp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "umx-uiverb-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    fn config(home: &Path) -> Config {
        Config::at(home.join("home"), Some(true))
    }

    fn with(config: Config, key: &str, value: &Path) -> Config {
        let mut c = config;
        c.settings.env.remove("UMX_UI_DIR");
        c.settings.flags.insert(
            key.to_owned(),
            ConfigValue::Text(value.display().to_string()),
        );
        c.started = umx_core::config::resolve(&c.settings);
        c
    }

    /// An app folder like a built apps/ui.
    fn app(root: &Path) -> PathBuf {
        let dir = root.join("app");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join(".next/standalone")).unwrap();
        fs::write(dir.join("bin/serve.mjs"), "").unwrap();
        fs::write(dir.join(".next/standalone/server.js"), "").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    fn code(issue: Result<PathBuf, Issue>) -> String {
        issue.unwrap_err().code().to_owned()
    }

    #[test]
    fn a_built_app_folder_is_found_and_an_unbuilt_one_says_how_to_build_it() {
        let root = temp();
        let dir = app(&root);
        let c = with(config(&root), "ui.dir", &dir);
        assert_eq!(app_dir(&c).unwrap(), dir);
        fs::remove_file(dir.join(".next/standalone/server.js")).unwrap();
        let issue = app_dir(&c).unwrap_err();
        assert_eq!(issue.code(), "ui_not_built");
        assert!(
            issue.message().contains(&format!(
                "pnpm --dir {} install --frozen-lockfile && pnpm --dir {} build",
                dir.display(),
                dir.display()
            )),
            "{}",
            issue.message()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_or_open_app_folder_is_refused() {
        let root = temp();
        let c = with(config(&root), "ui.dir", &root.join("nowhere"));
        assert_eq!(code(app_dir(&c)), "ui_dir_refused");
        let dir = app(&root);
        let c = with(config(&root), "ui.dir", &dir);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(code(app_dir(&c)), "ui_dir_refused");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(dir.join("bin/serve.mjs"), fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(code(app_dir(&c)), "ui_dir_refused");
        fs::set_permissions(dir.join("bin/serve.mjs"), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(app_dir(&c).unwrap(), dir);
        // The build node runs is checked the same way (security F1).
        let built = dir.join(".next/standalone/server.js");
        fs::set_permissions(&built, fs::Permissions::from_mode(0o646)).unwrap();
        assert_eq!(code(app_dir(&c)), "ui_dir_refused");
        let _ = fs::remove_dir_all(root);
    }

    /// ws012 s9 (D76a Q11): in a binary install the source folder umx was built from (the default ui.dir) is not on
    /// the machine, and the refusal says the view ships only with a source build and how to get it. A folder set by
    /// UMX_UI_DIR or ui.dir that is missing is only missing.
    #[test]
    fn a_binary_install_is_told_the_view_comes_from_a_source_build() {
        let root = temp();
        let mut c = config(&root);
        c.settings.env.remove("UMX_UI_DIR");
        c.settings.defaults.insert(
            "ui.dir".to_owned(),
            ConfigValue::Text(root.join("runner/umx/apps/ui").display().to_string()),
        );
        c.started = umx_core::config::resolve(&c.settings);
        let issue = app_dir(&c).unwrap_err();
        assert_eq!(
            (issue.code(), issue.next()),
            ("ui_dir_refused", "umx docs get ui")
        );
        // The source to build is this binary's own commit (s9 B1), or, unknown, the source it was built from.
        let commit = env!("UMX_COMMIT_FULL");
        let pinned = format!(
            "git clone https://github.com/AI-Substrate/umx && git -C umx checkout {commit}, build umx/apps/ui"
        );
        let known = commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit());
        let source = if known {
            pinned.as_str()
        } else {
            "does not know its commit: use the source this umx was built from or update umx first"
        };
        for part in [
            "as in a binary install",
            "ships only with a source build of the commit this umx was built from",
            source,
            "(umx docs get ui)",
            "set UMX_UI_DIR or ui.dir",
        ] {
            assert!(
                issue.message().contains(part),
                "{part}: {}",
                issue.message()
            );
        }
        let c = with(config(&root), "ui.dir", &root.join("nowhere"));
        let issue = app_dir(&c).unwrap_err();
        assert!(
            issue.message().ends_with(": it does not exist."),
            "{}",
            issue.message()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn node_comes_from_ui_node_else_path_else_a_refusal() {
        let root = temp();
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let mut c = config(&root);
        c.search_path = vec![root.join("empty"), bin.clone()];
        assert_eq!(code(node(&c)), "ui_node_not_found");
        fs::write(bin.join("node"), "").unwrap();
        assert_eq!(code(node(&c)), "ui_node_not_found", "not executable");
        fs::set_permissions(bin.join("node"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(node(&c).unwrap(), bin.join("node"));
        let other = root.join("other-node");
        fs::write(&other, "").unwrap();
        fs::set_permissions(&other, fs::Permissions::from_mode(0o755)).unwrap();
        let c = with(c, "ui.node", &other);
        assert_eq!(node(&c).unwrap(), other);
        let c = with(c, "ui.node", &root.join("missing"));
        assert_eq!(code(node(&c)), "ui_node_not_found");
        let _ = fs::remove_dir_all(root);
    }

    /// A daemon that answers each supervisor wait from a script; past its end, a restarted daemon (another epoch).
    /// A `Tab` step sets whether the fake's tab is connected before the next answer.
    enum Step {
        Answer(Result<Waited, Issue>),
        Tab(bool),
    }

    struct Script {
        steps: std::sync::Mutex<std::collections::VecDeque<Step>>,
        watch: umx_fakes::FakeUiWatch,
        asked: std::sync::Mutex<Vec<(Option<String>, u64)>>,
    }

    impl Daemon for Script {
        fn wait(
            &self,
            epoch: Option<&str>,
            since: u64,
            _hold: u64,
            _within: Duration,
        ) -> Result<Waited, Issue> {
            self.asked
                .lock()
                .unwrap()
                .push((epoch.map(str::to_owned), since));
            loop {
                match self.steps.lock().unwrap().pop_front() {
                    Some(Step::Tab(live)) => self.watch.tab(live),
                    Some(Step::Answer(a)) => return a,
                    None => {
                        return Ok(Waited {
                            epoch: "restarted".into(),
                            ..Waited::default()
                        });
                    }
                }
            }
        }
    }

    fn at(epoch: &str, seq: u64, raises: &[&[&str]]) -> Step {
        Step::Answer(Ok(Waited {
            epoch: epoch.into(),
            seq,
            raises: raises
                .iter()
                .map(|r| r.iter().map(|g| g.to_string()).collect())
                .collect(),
        }))
    }

    fn script(host: &FakeUiHost, steps: Vec<Step>) -> Script {
        Script {
            steps: std::sync::Mutex::new(steps.into()),
            watch: host.watch(),
            asked: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn run_with(
        host: &FakeUiHost,
        state: &FakeLocalState,
        daemon: &Script,
        node: &str,
        run: &UiRun,
    ) -> (Envelope, Option<(String, Value)>) {
        let mut seen = None;
        let parts = Parts {
            host,
            state,
            daemon,
            poll: Duration::from_millis(1),
            grace: Duration::ZERO,
        };
        let env = serve(
            &parts,
            Path::new("/fake/apps/ui"),
            Path::new(node),
            vec![],
            run,
            |r| seen = Some((r.url.clone(), r.envelope(&[]).data)),
        );
        (env, seen)
    }

    fn run_fake(host: &FakeUiHost, node: &str, run: &UiRun) -> (Envelope, Option<(String, Value)>) {
        let daemon = script(host, vec![at("e1", 0, &[])]);
        run_with(
            host,
            &FakeLocalState::new().with_absent_ok(true),
            &daemon,
            node,
            run,
        )
    }

    fn me() -> UiProcess {
        UiProcess {
            started: "Thu Nov 12 20:00:00 2026".into(),
            argv: "/opt/umx/bin/umx --json ui --graph alpha".into(),
        }
    }

    #[test]
    fn named_graphs_open_with_a_single_use_code_and_no_envelope_holds_it() {
        let host = FakeUiHost::new();
        host.watch().add_process(std::process::id(), me());
        let state = FakeLocalState::new().with_absent_ok(true);
        let daemon = script(&host, vec![at("e1", 7, &[])]);
        let run = UiRun {
            graphs: vec!["alpha".into(), "beta".into(), "gamma".into()],
            port: Some(4747),
            ..UiRun::default()
        };
        let (end, seen) = run_with(&host, &state, &daemon, "/fake/node", &run);
        let (url, data) = seen.expect("ready");
        let codes = host.watch().codes();
        assert_eq!(codes.len(), 1, "one code for the one open");
        assert_eq!(
            host.watch().stdin_seen(),
            [format!("code {}", codes[0])],
            "the server got the code and no token"
        );
        assert_eq!(url, "http://127.0.0.1:4747/");
        assert_eq!(
            host.opened(),
            vec![format!(
                "http://127.0.0.1:4747/#t={}&g=alpha%2Cbeta%2Cgamma",
                codes[0]
            )],
            "the browser gets the code with the graphs (D5)"
        );
        assert_eq!(
            data,
            json!({ "url": url, "port": 4747, "graph": "alpha", "graphs": ["alpha", "beta", "gamma"], "opened": true, "pid": std::process::id() })
        );
        for said in [data.to_string(), end.to_json()] {
            assert!(!said.contains(&codes[0]), "{said}");
        }
        // The supervisor watched from the first wait's seq, under its epoch, and ended when the epoch changed.
        assert_eq!(
            daemon.asked.lock().unwrap().clone(),
            [(None, 0), (Some("e1".to_string()), 7)]
        );
        assert!(
            end.errors[0].message().contains("the daemon restarted"),
            "{}",
            end.to_json()
        );
        assert!(!host.alive(), "the server goes with it");
        assert_eq!(
            host.watch().endings(),
            [UiEnd::Restarted],
            "told first, so its tabs can say why"
        );
        assert_eq!(state.bytes(), Some(Vec::new()), "ui.json cleared");
        // What the launcher's checks warned is said when it is ready, as a warning with its next.
        let stale = Issue::fill(
            Code::StaleFiles,
            &[
                ("graph", Fill::name("beta").unwrap()),
                ("files", Fill::text("nodes.jsonl")),
                ("fix", Fill::command("graph load")),
            ],
        );
        let ready = UiReady {
            port: 4747,
            url: url.clone(),
            graphs: vec!["beta".into()],
            opened: true,
            pid: 1,
            detached: true,
        }
        .envelope(std::slice::from_ref(&stale));
        assert_eq!(ready.status, umx_protocol::Status::Warning);
        assert_eq!(ready.warnings, [stale]);
        assert_eq!(ready.data["detached"], true);
        // ws002 X1 (review R6): a load on demand is a hint, said as one and never as a warning.
        let loaded = Issue::fill(
            Code::GraphLoadedOnDemand,
            &[
                ("alias", Fill::name("beta").unwrap()),
                ("path", Fill::text("/work/beta")),
            ],
        );
        let hinted = UiReady {
            port: 4747,
            url,
            graphs: vec!["beta".into()],
            opened: true,
            pid: 1,
            detached: false,
        }
        .envelope(std::slice::from_ref(&loaded));
        assert_eq!(hinted.status, umx_protocol::Status::Ok);
        assert_eq!(
            (hinted.warnings.len(), hinted.hints.clone()),
            (0, vec![loaded])
        );
    }

    /// ui.json holds exactly pid, start time, port, the daemon's start id and PROTOCOL (security, architect).
    #[test]
    fn ui_json_records_the_view_while_it_runs_and_never_a_secret() {
        let host = FakeUiHost::new();
        host.watch().add_process(std::process::id(), me());
        let state = FakeLocalState::new().with_absent_ok(true);
        let watch = host.watch();
        let seen = std::sync::Mutex::new(None);
        struct Peek<'a>(
            &'a FakeLocalState,
            &'a std::sync::Mutex<Option<Vec<u8>>>,
            Script,
        );
        impl Daemon for Peek<'_> {
            fn wait(
                &self,
                epoch: Option<&str>,
                since: u64,
                hold: u64,
                within: Duration,
            ) -> Result<Waited, Issue> {
                if epoch.is_some() {
                    *self.1.lock().unwrap() = self.0.bytes();
                }
                self.2.wait(epoch, since, hold, within)
            }
        }
        let daemon = Peek(&state, &seen, script(&host, vec![at("e1", 0, &[])]));
        let parts = Parts {
            host: &host,
            state: &state,
            daemon: &daemon,
            poll: Duration::from_millis(1),
            grace: Duration::ZERO,
        };
        serve(
            &parts,
            Path::new("/a"),
            Path::new("/fake/node"),
            vec![],
            &UiRun::default(),
            |_| {},
        );
        let bytes = seen.lock().unwrap().clone().expect("written while it ran");
        let v = umx_protocol::parse_json(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({ "pid": std::process::id(), "started": me().started, "port": 4747, "daemon": "e1", "umx": PROTOCOL })
        );
        assert_eq!(watch.served(), 1);
    }

    /// M1b2 raise: with no tab connected the supervisor opens one with a fresh code each time; with a tab connected
    /// it opens nothing (the tab shows it). Raises seen before it started are never replayed.
    #[test]
    fn a_raise_opens_a_tab_with_a_fresh_code_unless_a_tab_is_connected() {
        let host = FakeUiHost::new();
        let state = FakeLocalState::new().with_absent_ok(true);
        let daemon = script(
            &host,
            vec![
                at("e1", 3, &[]),
                at("e1", 4, &[&["beta"]]),
                at("e1", 5, &[&["beta", "gamma"]]),
                Step::Tab(true),
                at("e1", 6, &[&["gamma"]]),
                // The daemon down for a moment: the view stays.
                Step::Answer(Err(Issue::new(Code::DaemonNotRunning))),
                at("e1", 6, &[]),
            ],
        );
        let run = UiRun {
            no_open: true,
            ..UiRun::default()
        };
        let (end, _) = run_with(&host, &state, &daemon, "/fake/node", &run);
        let codes = host.watch().codes();
        assert_eq!(codes.len(), 2, "{codes:?}");
        assert_ne!(codes[0], codes[1], "each raise has its own code");
        assert_eq!(
            host.opened(),
            [
                format!("http://127.0.0.1:4747/#t={}&g=beta", codes[0]),
                format!("http://127.0.0.1:4747/#t={}&g=beta%2Cgamma", codes[1]),
            ]
        );
        assert!(end.errors[0].message().contains("the daemon restarted"));
    }

    #[test]
    fn the_supervisor_ends_when_its_server_stops_or_the_daemon_stays_unreachable() {
        let host = FakeUiHost::new();
        let daemon = script(&host, vec![at("e1", 0, &[])]);
        let state = FakeLocalState::new().with_absent_ok(true);
        let run = UiRun {
            no_open: true,
            ..UiRun::default()
        };
        let (end, _) = run_with(&host, &state, &daemon, "/fake/node-ends", &run);
        assert!(
            end.errors[0].message().contains("server stopped"),
            "{}",
            end.to_json()
        );
        let gone = (0..10_000)
            .map(|_| Step::Answer(Err(Issue::new(Code::DaemonNotRunning))))
            .collect::<Vec<_>>();
        let mut steps = vec![at("e1", 0, &[])];
        steps.extend(gone);
        let daemon = script(&host, steps);
        let started = Instant::now();
        let (end, _) = run_with(&host, &state, &daemon, "/fake/node", &run);
        assert!(
            end.errors[0].message().contains("stopped answering"),
            "{}",
            end.to_json()
        );
        assert!(started.elapsed() >= DAEMON_GONE);
    }

    /// Review B3: a daemon that keeps accepting but stops answering (suspended, say) ends the view near 5 s,
    /// through the real socket client: the first wait is answered (the start id), every later one is accepted and
    /// left hanging, either silent or after the wait's space heartbeat.
    #[test]
    fn a_daemon_that_accepts_but_never_answers_ends_the_view_near_five_seconds() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        for heartbeat in [false, true] {
            let socket = PathBuf::from(format!(
                "/tmp/umx-ui-mute-{}-{heartbeat}.sock",
                std::process::id()
            ));
            let _ = fs::remove_file(&socket);
            let listener = UnixListener::bind(&socket).unwrap();
            std::thread::spawn(move || {
                let mut held = Vec::new();
                for (n, stream) in listener.incoming().enumerate() {
                    let Ok(mut stream) = stream else { return };
                    let mut request = Vec::new();
                    let _ = stream.read_to_end(&mut request);
                    if n == 0 {
                        let reply = Envelope::ok(
                            "ui wait",
                            json!({ "epoch": "e1", "seq": 0, "reset": false, "events": [], "changes": [] }),
                        );
                        let _ = stream.write_all(reply.to_json().as_bytes());
                    } else {
                        if heartbeat {
                            let _ = stream.write_all(b" ");
                        }
                        held.push(stream);
                    }
                }
            });
            let host = FakeUiHost::new();
            let state = FakeLocalState::new().with_absent_ok(true);
            let build = Build {
                version: "t".into(),
                commit: "t".into(),
                built: "t".into(),
            };
            let daemon = Socket {
                socket: &socket,
                build: &build,
            };
            let parts = Parts {
                host: &host,
                state: &state,
                daemon: &daemon,
                poll: Duration::from_millis(20),
                grace: Duration::ZERO,
            };
            let run = UiRun {
                no_open: true,
                ..UiRun::default()
            };
            let mut up = None;
            let end = serve(
                &parts,
                Path::new("/a"),
                Path::new("/fake/node"),
                vec![],
                &run,
                |r| {
                    up = Some(Instant::now());
                    let _ = r;
                },
            );
            let took = up.expect("it was up").elapsed();
            let _ = fs::remove_file(&socket);
            assert!(
                end.errors[0].message().contains("stopped answering"),
                "{}",
                end.to_json()
            );
            assert!(
                took >= DAEMON_GONE && took < DAEMON_GONE + Duration::from_secs(3),
                "heartbeat {heartbeat}: {took:?}"
            );
            assert!(!host.alive(), "the server went with it");
            assert_eq!(host.watch().endings(), [UiEnd::Unreachable]);
        }
    }

    /// One view per UMX_HOME (review B1): while another process holds the view, a second is refused before
    /// it starts a server or opens anything, naming what to do instead, whatever ui.json says (empty here, as while
    /// the first is still starting). `--detach` reports a live recorded view at once without starting a child; a
    /// record whose pid now runs something else (another start time, or not umx ui) is stale.
    #[test]
    fn a_second_view_is_refused_while_the_recorded_one_runs() {
        let host = FakeUiHost::new();
        let state = FakeLocalState::new().with_absent_ok(true);
        host.watch().held_elsewhere(true);
        let run = UiRun {
            graphs: vec!["alpha".into(), "beta".into()],
            ..UiRun::default()
        };
        let (end, seen) = run_with(&host, &state, &script(&host, vec![]), "/fake/node", &run);
        assert!(seen.is_none());
        assert_eq!(end.errors[0].code(), "ui_already_running");
        assert_eq!(
            end.errors[0].next(),
            "umx ui open --graph alpha,beta --raise"
        );
        assert_eq!(host.served(), 0, "no server started");
        assert!(
            host.opened().is_empty() && host.watch().codes().is_empty(),
            "nothing opened"
        );
        assert_eq!(state.bytes(), None, "ui.json untouched");
        host.watch().held_elsewhere(false);
        let record = Record {
            pid: 999_001,
            started: me().started,
            port: 5151,
            daemon: "e0".into(),
        };
        state.set_bytes(&record.bytes());
        host.watch().add_process(999_001, me());
        let detached = start_detached(&host, &state, "/opt/umx", &run);
        assert_eq!(detached.errors[0].code(), "ui_already_running");
        assert!(host.watch().detached().is_empty(), "nothing started");
        for other in [
            UiProcess {
                started: "Fri Nov 13 09:00:00 2026".into(),
                ..me()
            },
            UiProcess {
                argv: "/bin/sleep 600".into(),
                ..me()
            },
            UiProcess {
                argv: "/opt/umx ui status".into(),
                ..me()
            },
        ] {
            host.watch().add_process(999_001, other);
            assert_eq!(status_of(&host, &state).data["running"], false);
        }
    }

    #[test]
    fn detach_starts_umx_ui_in_the_background_and_returns_what_it_reported() {
        let host = FakeUiHost::new();
        let state = FakeLocalState::new().with_absent_ok(true);
        let said = Envelope::ok(
            CMD,
            json!({ "url": "http://127.0.0.1:5000/", "pid": 4242, "detached": true }),
        );
        host.watch().detach_says(Some(said.to_json().into_bytes()));
        let run = UiRun {
            graphs: vec!["alpha".into(), "beta".into()],
            port: Some(5000),
            no_open: true,
            ..UiRun::default()
        };
        let got = start_detached(&host, &state, "/opt/umx", &run);
        assert_eq!(got.data, said.data);
        assert_eq!(
            host.watch().detached(),
            [[
                "/opt/umx",
                "ui",
                "--graph",
                "alpha,beta",
                "--port",
                "5000",
                "--no-open",
                umx_core::ports::ui_host::REPORT_FLAG,
                "/fake/home/run/ui-report.sock"
            ]
            .map(String::from)
            .to_vec()]
        );
        // The CLI reads the host's report flag (umx-cli cannot name umx-core's constant).
        let flags = umx_cli::ui_flags(&[
            umx_core::ports::ui_host::REPORT_FLAG.to_string(),
            "/s".into(),
        ])
        .unwrap();
        assert_eq!(flags.report_to.as_deref(), Some("/s"));
        host.watch().detach_says(None);
        let died = start_detached(&host, &state, "/opt/umx-dies", &run);
        assert_eq!(died.errors[0].code(), "ui_failed");
        assert!(
            died.errors[0].message().contains("--human"),
            "{}",
            died.to_json()
        );
    }

    /// `umx ui stop` signals only the live process ui.json names (same start time, a `umx ui` command line); a
    /// reused pid is never signalled, and its stale record goes (security D3 and the stop condition).
    #[test]
    fn stop_signals_only_the_recorded_umx_ui_and_status_says_no_more_than_port_pid_and_start() {
        let host = FakeUiHost::new();
        let state = FakeLocalState::new().with_absent_ok(true);
        assert_eq!(
            stop_of(&host, &state, Duration::ZERO).data,
            json!({ "stopped": false })
        );
        let record = Record {
            pid: 999_002,
            started: me().started,
            port: 5152,
            daemon: "e0".into(),
        };
        // Reused: the pid runs something else.
        state.set_bytes(&record.bytes());
        host.watch().add_process(
            999_002,
            UiProcess {
                argv: "/usr/bin/vim notes".into(),
                ..me()
            },
        );
        assert_eq!(
            stop_of(&host, &state, Duration::ZERO).data,
            json!({ "stopped": false })
        );
        assert!(host.watch().signalled().is_empty(), "never a bare kill");
        assert_eq!(state.bytes(), Some(Vec::new()), "the stale record went");
        // The recorded view, live.
        state.set_bytes(&record.bytes());
        host.watch().add_process(999_002, me());
        let status = status_of(&host, &state);
        assert_eq!(
            status.data,
            json!({ "running": true, "port": 5152, "pid": 999_002, "started": me().started })
        );
        let stopped = stop_of(&host, &state, Duration::ZERO);
        assert_eq!(stopped.data, json!({ "stopped": true, "pid": 999_002 }));
        assert_eq!(host.watch().signalled(), [999_002]);
        assert_eq!(state.bytes(), Some(Vec::new()));
        assert_eq!(status_of(&host, &state).data, json!({ "running": false }));
        // ui.json refused (a symlink, another owner, open to others): said, never read.
        state.set_bytes(&record.bytes());
        state.refuse(Some("it is a symlink"));
        assert_eq!(
            stop_of(&host, &state, Duration::ZERO).errors[0].code(),
            "ui_failed"
        );
        assert_eq!(status_of(&host, &state).errors[0].code(), "ui_failed");
    }

    #[test]
    fn a_timeout_and_a_failed_open_have_their_own_codes() {
        assert_eq!(issue(&UiError::TimedOut).code(), "ui_start_timed_out");
        let open = issue(&UiError::Open("no xdg-open on PATH".into()));
        assert_eq!(open.code(), "ui_open_failed");
        assert!(open.message().contains("--no-open"), "{}", open.message());
        assert_eq!(issue(&UiError::Exited(Some(3))).code(), "ui_failed");
    }

    #[test]
    fn no_open_opens_nothing() {
        let host = FakeUiHost::new();
        let run = UiRun {
            no_open: true,
            ..UiRun::default()
        };
        let (_, seen) = run_fake(&host, "/fake/node-ends", &run);
        assert_eq!(seen.unwrap().0, "http://127.0.0.1:4747/");
        assert!(host.opened().is_empty());
    }

    /// No token: a plain start opens the bare URL and arms no code; the server's stdin gets nothing.
    #[test]
    fn a_plain_start_opens_the_bare_url_with_no_token_or_code() {
        let host = FakeUiHost::new();
        let (_, seen) = run_fake(&host, "/fake/node-ends", &UiRun::default());
        assert_eq!(seen.unwrap().0, "http://127.0.0.1:4747/");
        assert_eq!(host.opened(), ["http://127.0.0.1:4747/"]);
        assert!(host.watch().codes().is_empty() && host.watch().stdin_seen().is_empty());
    }

    #[test]
    fn a_server_that_never_gets_ready_or_dies_is_reported_and_nothing_opens() {
        let host = FakeUiHost::new();
        let (env, seen) = run_fake(&host, "/fake/node-silent", &UiRun::default());
        assert!(seen.is_none());
        assert_eq!(env.errors[0].code(), "ui_start_timed_out");
        let (env, _) = run_fake(&host, "/fake/node-dies", &UiRun::default());
        assert_eq!(env.errors[0].code(), "ui_failed");
        assert!(host.opened().is_empty());
        assert!(!host.alive());
    }
}
