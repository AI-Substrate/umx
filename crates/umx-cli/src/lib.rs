//! umx-cli: the CLI client (blueprint 001, c-cli). It depends only on umx-protocol (r-cli-is-a-client).
//! Every command goes through the daemon except `umx version`, `umx agents-start-here`, `umx docs list|get`,
//! `umx daemon start|status` and `umx doctor`, which also work with the daemon down (ruling 2026-11-09 under
//! D13). `daemon start` and `doctor` run in the binary (crates/umx), which hands them to umx-daemon.
//! Graph, node, edge and ontology commands are parsed here into typed operations; the daemon never parses
//! argv.

mod batch;
mod docs;
mod human;
mod parse;

/// The keys `--human` leaves shown on a flagged record it cannot mask exactly (F224); a test in crates/umx checks
/// each is reserved on nodes and edges.
pub use human::PLAIN_KEYS as HUMAN_PLAIN_KEYS;

use std::io::{BufRead, IsTerminal, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use umx_protocol::{
    ActorClaim, Build, Code, Envelope, Fill, Issue, MAX_REPLY, Op, Request, command_label, json,
};

/// What the CLI needs from the binary that runs it.
pub struct Context {
    pub socket: PathBuf,
    pub build: Build,
    pub output: Output,
    /// Who writes when `--by` is not given (UMX_AUTHOR, else the OS user).
    pub by: String,
    /// The graph alias when `--graph` is not given (UMX_GRAPH).
    pub graph: Option<String>,
    pub actor: Option<ActorClaim>,
}

/// How results are printed: the JSON envelope, or human text with colour per [`Output::color`].
#[derive(Clone, Copy, Debug)]
pub struct Output {
    pub json: bool,
    /// Colour in human text: forced on or off, or `None` for only on a terminal (umx-compose reads the env).
    pub color: Option<bool>,
}

/// Removes the global `--json` and `--human` flags (req-63). Returns whether JSON output is on, and the
/// remaining arguments. The last of the two flags wins; with neither, `json_default` (`UMX_OUTPUT`, else whether
/// stdout is a terminal). A leading `--version` or `-V` is spelled `version` (F70), so it is `umx version` in
/// every way: help, offline, output and exit code.
pub fn global_flags(args: Vec<String>, json_default: bool) -> (bool, Vec<String>) {
    let mut json = json_default;
    let mut rest = Vec::with_capacity(args.len());
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--human" => json = false,
            _ => rest.push(arg),
        }
    }
    if let Some(first) = rest.first_mut()
        && matches!(first.as_str(), "--version" | "-V")
    {
        *first = "version".to_owned();
    }
    (json, rest)
}

/// Help, if the command asks for it: `--help` or `-h` anywhere, or `help` as the first word. Runs offline in
/// the CLI (no daemon, no wire change); the binary checks this before anything else, so even
/// `umx daemon start --help` prints help instead of starting the daemon. A `--help` or `-h` given as a value
/// (say, a title) asks for help too.
pub fn help(args: &[String]) -> Option<Envelope> {
    let flagged = args.iter().any(|a| a == "--help" || a == "-h");
    let named = args.first().is_some_and(|a| a == "help");
    if !flagged && !named {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--help" && *a != "-h")
        .skip(usize::from(named))
        .collect();
    Some(docs::help(&words))
}

/// Runs one command and prints its result. Returns the exit code.
pub fn run(args: &[String], ctx: &Context) -> u8 {
    if let Some(env) = help(args) {
        return emit(&env, ctx.output);
    }
    emit(&dispatch(args, ctx), ctx.output)
}

/// No file reads or discovery: the binary uses this before constructing the client identity.
pub fn needs_actor(args: &[String]) -> bool {
    if help(args).is_some() {
        return false;
    }
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    if let ["batch", rest @ ..] = words.as_slice() {
        return parse::batch(rest).is_ok_and(|(_, _, parsed)| sends_actor(&parsed.op));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    parse::parse(&words, &cwd).is_ok_and(|parsed| sends_actor(&parsed.op))
}

/// The one list of ops the client resolves its identity for: every write, and `umx ui focus|highlight|filter|fit|
/// clear`, whose event the view shows with who sent it (plan 129). UiSelect comes from the view and carries none.
fn sends_actor(op: &Op) -> bool {
    op.writes() || matches!(op, Op::UiCommand { .. })
}

/// The longest `--idle-unload`: 30 days. Longer is refused at parse time, so deadlines stay far from overflow.
pub const MAX_IDLE_UNLOAD_MS: u64 = 720 * 3_600_000;

/// The flags of `umx daemon start`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartFlags {
    /// `--idle-unload <dur>`: the idle time in ms, None when not given.
    pub idle_unload_ms: Option<u64>,
    /// `--service`: started by the service manager (req-86), so a daemon that is already running is not a failure
    /// (exit 0), and the service manager does not start it again and again.
    pub service: bool,
}

/// Whether every `--flag` (or `--flag=value`) in `rest` is in `command`'s [`parse::FLAGS`] row: the offline
/// parsers below admit only what the table names, so the table and the docs check cannot drift from them.
fn in_flags_row(command: &str, rest: &[String]) -> bool {
    rest.iter()
        .filter(|a| a.starts_with("--"))
        .all(|a| parse::flags(command).contains(&a.split_once('=').map_or(a.as_str(), |(f, _)| f)))
}

/// Parses the flags of `umx daemon start`. Wrong flags give the usage error to print.
pub fn daemon_start_flags(rest: &[String]) -> Result<StartFlags, Box<Envelope>> {
    const WHAT: &str = "daemon start takes only --idle-unload <duration>, from 1s to 720h (such as 30m or 2h), or 0 for never, and --service";
    let wrong = || Box::new(usage("daemon start", WHAT));
    if !in_flags_row("daemon start", rest) {
        return Err(wrong());
    }
    let mut flags = StartFlags::default();
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "--service" if !flags.service => {
                flags.service = true;
                continue;
            }
            "--idle-unload" => args.next().ok_or_else(wrong)?.as_str(),
            other => other.strip_prefix("--idle-unload=").ok_or_else(wrong)?,
        };
        if flags.idle_unload_ms.is_some() {
            return Err(wrong());
        }
        let ms = parse::duration_ms(value)
            .filter(|ms| *ms == 0 || (1_000..=MAX_IDLE_UNLOAD_MS).contains(ms))
            .ok_or_else(wrong)?;
        flags.idle_unload_ms = Some(ms);
    }
    Ok(flags)
}

/// The flags of `umx doctor`: whether `--fix` was given. Wrong flags give the usage error to print.
pub fn doctor_flags(rest: &[String]) -> Result<bool, Box<Envelope>> {
    match rest {
        [] => Ok(false),
        [fix] if fix == "--fix" && in_flags_row("doctor", rest) => Ok(true),
        _ => Err(Box::new(usage("doctor", "doctor takes only --fix"))),
    }
}

/// The arguments of `umx models fetch`: the model's name and whether `--dry-run` was given.
pub fn models_fetch_flags(rest: &[String]) -> Result<(String, bool), Box<Envelope>> {
    let dry_run = rest.iter().any(|a| a == "--dry-run");
    let names: Vec<&String> = rest.iter().filter(|a| *a != "--dry-run").collect();
    match names.as_slice() {
        [name] if !name.starts_with('-') && in_flags_row("models fetch", rest) => {
            Ok(((*name).clone(), dry_run))
        }
        _ => Err(Box::new(usage(
            "models fetch",
            "models fetch takes a model name and optionally --dry-run",
        ))),
    }
}

/// Whether `umx ui <word> ...` is a verb (`umx ui focus`, `umx ui selection`, ws009 S6) rather than `umx ui` with
/// its flags: any first word that is not a flag. The binary runs `umx ui` itself and hands verbs to [`run`], except
/// [`UI_LOCAL_VERBS`].
pub fn is_ui_verb(word: &str) -> bool {
    !word.starts_with('-')
}

/// `umx ui status` and `umx ui stop` (ws009 M1b2): run by the binary, which reads ui.json and the process it names;
/// neither asks the daemon.
pub const UI_LOCAL_VERBS: [&str; 2] = ["status", "stop"];

/// The words after `umx ui status|stop`: none.
pub fn ui_local_flags(verb: &str, rest: &[String]) -> Result<(), Box<Envelope>> {
    if rest.is_empty() {
        Ok(())
    } else if verb == "stop" {
        Err(Box::new(usage("ui stop", "ui stop takes no arguments")))
    } else {
        Err(Box::new(usage("ui status", "ui status takes no arguments")))
    }
}

/// The most graphs `umx ui --graph` opens together (umx-core ui MAX_OPEN_GRAPHS, the view's own cap; ws009 M1b1).
pub const MAX_OPEN_GRAPHS: usize = 12;

/// The flags of `umx ui` (plan 095).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiFlags {
    /// `--graph a,b,c`: open these graphs together, 1 to 12, each once (ws009 M1b1); a secured graph opens when
    /// named here, as clicking it does (d69-7).
    pub graphs: Vec<String>,
    /// `--port <n>`: bind 127.0.0.1:<n> instead of a free port.
    pub port: Option<u16>,
    /// `--no-open`: print the link instead of opening the browser (human output only); with `--detach`, open
    /// nothing and print no link (a later `umx ui open --raise` opens a tab).
    pub no_open: bool,
    /// `--detach` (ws009 M1b2): run the view in the background, tied to the daemon, and return once it is ready.
    pub detach: bool,
    /// `--report-to <socket>`: set only by `--detach` for the background `umx ui` it starts (the UiHost's report
    /// flag): where that one says it is ready, or why not.
    pub report_to: Option<String>,
}

/// Parses the flags of `umx ui`. `--no-open` with JSON output is refused: the link holds the run's token, which goes
/// only to a terminal or the browser, never into an envelope an agent keeps (security, ws009 s2).
pub fn ui_flags(rest: &[String], json: bool) -> Result<UiFlags, Box<Envelope>> {
    const WHAT: &str = "ui takes only --graph <alias>[,<alias>...], --port <1-65535>, --no-open and --detach, each at most once";
    let wrong = || Box::new(usage("ui", WHAT));
    let mut flags = UiFlags::default();
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(wrong)
        };
        match name {
            "--no-open" if !flags.no_open && inline.is_none() => flags.no_open = true,
            "--detach" if !flags.detach && inline.is_none() => flags.detach = true,
            // umx-core's ui_host::REPORT_FLAG (crates/umx keeps the two equal).
            "--report-to" if flags.report_to.is_none() => {
                flags.report_to = Some(value()?);
            }
            "--graph" if flags.graphs.is_empty() => {
                let list = value()?;
                for alias in list.split(',') {
                    Fill::name(alias).ok_or_else(wrong)?;
                    if flags.graphs.iter().any(|g| g == alias) {
                        return Err(Box::new(usage("ui", "--graph names each graph once")));
                    }
                    flags.graphs.push(alias.to_owned());
                }
                if flags.graphs.len() > MAX_OPEN_GRAPHS {
                    return Err(Box::new(usage("ui", "--graph names at most 12 graphs")));
                }
            }
            "--port" if flags.port.is_none() => {
                let port = value()?
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or_else(wrong)?;
                flags.port = Some(port);
            }
            _ => return Err(wrong()),
        }
    }
    if flags.detach && flags.report_to.is_some() {
        return Err(wrong());
    }
    // Detached, nothing prints a link: --no-open then only leaves the browser closed.
    if flags.no_open && json && !flags.detach && flags.report_to.is_none() {
        return Err(Box::new(usage(
            "ui",
            "--no-open prints the link only as text: add --human (JSON output never holds the link's token)",
        )));
    }
    Ok(flags)
}

/// One wait as `umx ui`'s supervisor (ws009 M1b2): every graph's opens after `since`, held at most `hold_ms`. The
/// reply's `epoch` is the daemon's start id. The whole call (connect, write, reply) ends `within`, however the
/// daemon behaves (review B3). Err: the daemon did not answer in time.
pub fn ui_supervisor_wait(
    socket: &Path,
    build: &Build,
    epoch: Option<String>,
    since: u64,
    hold_ms: u64,
    within: Duration,
) -> Result<Envelope, Issue> {
    let op = Op::UiWait {
        epoch,
        since,
        timeout_ms: hold_ms,
        graphs: Vec::new(),
        revs: Default::default(),
        supervisor: true,
    };
    let limits = ReplyLimits {
        first_byte: within,
        idle: within,
        total: within,
        end: Some(Instant::now() + within),
    };
    call_with(socket, &Request::op(op, None, None, build.clone()), limits)
}

/// What `umx ui` checks with the daemon before it starts anything: the daemon answers (the usual refusal when it does
/// not), and each `--graph` alias names a graph it can read. Each check is a named read (`ontology show --graph
/// <alias>`), so a graph that unloaded when idle loads on demand as for any named read (F72, plan 129), and a graph
/// unloaded on purpose or never known is refused with the daemon's own graph_not_loaded and its next (ws009 M1b1:
/// every alias, before anything opens). The reads' warnings and hints come back (stale_files, F144;
/// graph_loaded_on_demand, a hint since ws002 X1): the launch goes on, and says them when it is ready, each in the
/// list its kind names (review B5, review R6).
pub fn ui_preflight(
    socket: &Path,
    build: &Build,
    graphs: &[String],
) -> Result<Vec<Issue>, Box<Envelope>> {
    let requests: Vec<Request> = if graphs.is_empty() {
        vec![Request::op(
            Op::GraphList {
                all: false,
                graphfile: None,
            },
            None,
            None,
            build.clone(),
        )]
    } else {
        graphs
            .iter()
            .map(|alias| Request::op(Op::OntologyShow, Some(alias.clone()), None, build.clone()))
            .collect()
    };
    let mut notes: Vec<Issue> = Vec::new();
    for request in &requests {
        let reply =
            call_at(socket, request).map_err(|issue| Box::new(Envelope::fail("ui", issue)))?;
        if let Some(issue) = reply.errors.into_iter().next() {
            return Err(Box::new(Envelope::fail("ui", issue)));
        }
        for w in reply.warnings.into_iter().chain(reply.hints) {
            if !notes.contains(&w) {
                notes.push(w);
            }
        }
    }
    Ok(notes)
}

/// The error for a command whose arguments are wrong.
pub fn usage(command: &str, what: &'static str) -> Envelope {
    let noun = match command.split(' ').next() {
        Some("daemon") => "daemon",
        Some("doctor") => "doctor",
        Some("models") => "models",
        Some("graph") => "graph",
        Some("node") => "node",
        Some("edge") => "edge",
        Some("ontology") => "ontology",
        Some("config") => "config",
        Some("embeddings") => "embeddings",
        Some("neighbours" | "walk" | "path" | "resolve") => "explore",
        Some("search") => "search",
        Some("query") => "query",
        Some("who") => "who",
        Some("batch") => "batch",
        Some("scope") => "scope",
        Some("ui") => "ui",
        Some("root") => "evidence",
        _ => "start",
    };
    let fills = [
        ("cmd", Fill::command(command)),
        ("what", Fill::word(what)),
        ("noun", Fill::word(noun)),
    ];
    Envelope::fail(command, Issue::fill(Code::UsageInvalid, &fills))
}

fn dispatch(args: &[String], ctx: &Context) -> Envelope {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["version"] => version(ctx),
        ["daemon", "status"] => status(&ctx.socket, &ctx.build),
        ["agents-start-here"] => docs::agents_start_here(),
        ["agents-start-here", ..] => usage("agents-start-here", "unexpected arguments"),
        [
            "graph" | "node" | "edge" | "ontology" | "check" | "config" | "embeddings"
            | "similarity" | "neighbours" | "walk" | "path" | "search" | "query" | "scope"
            | "resolve" | "root" | "entity" | "resource" | "who",
            ..,
        ] => typed(&words, ctx),
        ["ui", verb, ..] if is_ui_verb(verb) => typed(&words, ctx),
        ["batch", rest @ ..] => batch(rest, ctx),
        ["docs", "list"] => docs::list(),
        ["docs", "get", topic] => docs::get(topic),
        ["docs", "get"] => usage("docs get", "name a topic"),
        ["docs" | "version", ..] => usage(command_label(&words), "unexpected arguments"),
        // `models fetch` runs in the binary (umx main); any other models verb is local too, never the daemon's.
        ["models", ..] => usage(command_label(&words), "the models verb is fetch"),
        [] => usage("", "no command given"),
        [first, ..] if first.starts_with('-') => usage("", "unknown flag before the command"),
        [noun, verb, rest @ ..] if !verb.starts_with('-') => {
            send(ctx, &format!("{noun} {verb}"), rest)
        }
        [noun, rest @ ..] => send(ctx, noun, rest),
    }
}

/// Parses a graph, node, edge or ontology command and sends the typed operation.
fn typed(words: &[&str], ctx: &Context) -> Envelope {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    match parse::parse(words, &cwd) {
        Ok(p) => match spine_decl(p) {
            Ok(p) => send_op(p, ctx),
            Err(env) => *env,
        },
        Err(u) => usage(u.command, u.what),
    }
}

/// `graph spine set --file <decl.json|->` (ws008 TM5): the parse holds the file's name; the declaration is read here,
/// through batch's capped read, and sent as JSON. The daemon never sees the path.
fn spine_decl(mut p: parse::Parsed) -> Result<parse::Parsed, Box<Envelope>> {
    if let Op::GraphSpineSet { decl, .. } = &mut p.op
        && let umx_protocol::Value::String(file) = decl
    {
        let bytes = batch::read(file, batch::Input::Spine)?;
        *decl = core::str::from_utf8(&bytes)
            .ok()
            .and_then(umx_protocol::parse_json)
            .ok_or_else(|| {
                Box::new(usage(
                    "graph spine set",
                    "the --file is not JSON (one spine declaration object)",
                ))
            })?;
    }
    Ok(p)
}

/// `umx batch --file <ops.jsonl|-> [--refs-from <receipt>]... [--dry-run]`: the ops and receipts are read and
/// checked here, then sent as one request.
fn batch(rest: &[&str], ctx: &Context) -> Envelope {
    let (file, receipts, p) = match parse::batch(rest) {
        Ok(parsed) => parsed,
        Err(u) => return usage(u.command, u.what),
    };
    let lines = match batch::read(&file, batch::Input::Ops).and_then(|bytes| batch::parse(&bytes)) {
        Ok(lines) => lines,
        Err(env) => return *env,
    };
    let batch::Imports { refs, graph, from } = match batch::refs_from(&receipts) {
        Ok(imports) => imports,
        Err(env) => return *env,
    };
    let Op::Batch {
        dry_run,
        reason,
        echo_sentences,
        raised_in,
        related,
        ..
    } = p.op
    else {
        return Envelope::fail("batch", internal("batch_parse"));
    };
    let reply = send_op(
        parse::Parsed {
            op: Op::Batch {
                lines,
                dry_run,
                reason,
                echo_sentences,
                refs,
                refs_graph: graph,
                raised_in,
                related,
            },
            ..p
        },
        ctx,
    );
    batch::name_receipts(reply, &from)
}

/// Sends a parsed operation (the author and default graph from the context when not given) and returns the reply.
fn send_op(p: parse::Parsed, ctx: &Context) -> Envelope {
    let by = p.by.unwrap_or_else(|| ctx.by.clone());
    let mut request = Request::op(p.op, p.graph, Some(by), ctx.build.clone());
    request.default_graph = ctx.graph.clone();
    request.scope = p.scope;
    request.actor = ctx.actor.clone();
    request.all = p.all;
    request.include_named = p.include_named;
    request.include_provenance = p.include_provenance;
    let command = request.command.clone();
    let reply = call(ctx, &request).unwrap_or_else(|issue| Envelope::fail(&command, issue));
    match confirmed_on_tty(&request, &reply) {
        Some(again) => call(ctx, &again).unwrap_or_else(|issue| Envelope::fail(&command, issue)),
        None => reply,
    }
}

/// `graph confirm-policy` when the daemon says lowering the policy needs a person (ws004 s06 A2): asks on /dev/tty,
/// never stdin, so a pipe, a flag or an env var cannot answer, and the person types the graph's alias, not y. The
/// request again with what they typed, or None (no terminal) so the daemon's refusal stands. A pty wrapper can still
/// reach a terminal: a speed bump, accepted as proportionate (C33); every downgrade is recorded and shown, so such a
/// bypass is visible.
fn confirmed_on_tty(request: &Request, reply: &Envelope) -> Option<Request> {
    let (name, needs, what) = match &request.op {
        Some(Op::GraphConfirmPolicy { alias, typed: None }) => (
            alias,
            Code::PolicyConfirmNeedsHuman,
            "To lower it, type the graph's alias",
        ),
        _ => return None,
    };
    let issue = reply.errors.first()?;
    if issue.code() != needs.spec().code {
        return None;
    }
    let tty = open_tty()?;
    // The name came from argv; show it only if it is a plain snake_case name.
    let shown = Fill::name(name).map_or("<name>", |_| name.as_str());
    write!(
        &tty,
        "{}\n{what} ({shown}); anything else cancels: ",
        issue.message()
    )
    .ok()?;
    let mut line = String::new();
    std::io::BufReader::new(&tty).read_line(&mut line).ok()?;
    let typed = Some(line.trim().to_owned());
    let mut again = request.clone();
    again.op = Some(match &request.op {
        Some(Op::GraphConfirmPolicy { alias, .. }) => Op::GraphConfirmPolicy {
            alias: alias.clone(),
            typed,
        },
        _ => return None,
    });
    Some(again)
}

fn open_tty() -> Option<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()
}

fn internal(code: &'static str) -> Issue {
    Issue::fill(Code::Internal, &[("code", Fill::word(code))])
}

fn send(ctx: &Context, command: &str, args: &[&str]) -> Envelope {
    let request = Request::new(
        command,
        args.iter().map(|a| (*a).to_owned()).collect(),
        ctx.build.clone(),
    );
    call(ctx, &request).unwrap_or_else(|issue| Envelope::fail(command, issue))
}

fn call(ctx: &Context, request: &Request) -> Result<Envelope, Issue> {
    call_at(&ctx.socket, request)
}

fn call_at(socket: &Path, request: &Request) -> Result<Envelope, Issue> {
    call_with(socket, request, ReplyLimits::for_request(request))
}

/// How long the CLI waits for the daemon's reply (F50). The daemon writes a reply in one go once the command has
/// run, so the first byte waits for the command itself and gets the longest limit; once the reply has started, a
/// gap longer than `idle`, or a reply still arriving `total` after its first byte, is cut off. The daemon reads
/// requests the same way (umx-daemon `read_request`, F44); change one, look at the other.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ReplyLimits {
    first_byte: Duration,
    idle: Duration,
    total: Duration,
    /// When the whole call (connect, write, reply) must be done, whatever the limits above allow: only `umx ui`'s
    /// supervisor sets one, so a daemon that stops answering ends the view in about 5 s (ws009 M1b2, review B3).
    end: Option<Instant>,
}

impl ReplyLimits {
    /// Writes (`Op::writes`) and op-less requests but `daemon status` (`daemon stop` and `daemon restart`, which
    /// drain the writer): they wait their turn on the one writer, behind a graph load, a batch or a rebuild.
    const WRITER: ReplyLimits = ReplyLimits::first(600);
    /// Reads, served under the shared lock: room for a hosted query embed and Cypher's 10 s backstop.
    const READ: ReplyLimits = ReplyLimits::first(120);
    /// `daemon status` (also behind `umx version`): never behind the writer, but a daemon that is still starting
    /// has bound its socket and answers only once it has composed its services, which a loaded runner stretched
    /// past 10 s (macOS CI, run 42835609032). So the daemon's own first-byte budget for a request (60 s).
    const STATUS: ReplyLimits = ReplyLimits::first(60);

    const fn first(secs: u64) -> ReplyLimits {
        ReplyLimits {
            first_byte: Duration::from_secs(secs),
            idle: Duration::from_secs(10),
            total: Duration::from_secs(60),
            end: None,
        }
    }

    fn for_request(request: &Request) -> ReplyLimits {
        match &request.op {
            Some(op) if op.writes() => ReplyLimits::WRITER,
            Some(_) => ReplyLimits::READ,
            None if request.command == "daemon status" => ReplyLimits::STATUS,
            None => ReplyLimits::WRITER,
        }
    }
}

/// How long writing a request may take: the daemon's own limit for reading one (F44).
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

fn call_with(socket: &Path, request: &Request, limits: ReplyLimits) -> Result<Envelope, Issue> {
    let not_running =
        || Issue::new(Code::DaemonNotRunning).with_subject(socket.display().to_string());
    // std has no connect timeout for a Unix socket, and a connect can block on a full backlog (Linux).
    let (tx, rx) = std::sync::mpsc::channel();
    let path = socket.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(path));
    });
    let left = |most: Duration| {
        limits.end.map_or(most, |end| {
            most.min(end.saturating_duration_since(Instant::now()))
        })
    };
    let mut stream = match rx.recv_timeout(left(limits.first_byte)) {
        Ok(Ok(stream)) => stream,
        Ok(Err(_)) => return Err(not_running()),
        Err(_) => return Err(internal("reply_timed_out")),
    };
    let write_within = left(WRITE_TIMEOUT);
    if write_within.is_zero() {
        return Err(internal("reply_timed_out"));
    }
    let _ = stream.set_write_timeout(Some(write_within));
    // A busy daemon answers and closes before it reads the request (daemon_busy, F56), so a failed write or
    // shutdown is reported only when no reply can be read after it.
    let sent = stream
        .write_all(request.to_json().as_bytes())
        .and_then(|()| stream.shutdown(Shutdown::Write));
    match (sent, read_reply(&mut stream, limits)) {
        (Ok(()), Ok(bytes)) => {
            Envelope::from_json(&bytes).map_err(|_| internal("reply_unreadable"))
        }
        (Ok(()), Err(code)) => Err(internal(code)),
        (Err(_), Ok(bytes)) => Envelope::from_json(&bytes).map_err(|_| internal("socket_io")),
        (Err(_), Err(_)) => Err(internal("socket_io")),
    }
}

/// The longest one socket read may block (F59): macOS can miss the wakeup for data or an end of stream that arrives
/// while a read waits, and the read then sleeps until its timeout. So no read waits longer than this; the limits
/// are kept from elapsed time. The same loop is in umx-daemon `read_request` and `read_until_end` (doctor `ask_by`
/// and `running_pid`, F62); change one, look at the other two.
const READ_SLICE: Duration = Duration::from_millis(250);

/// A socket read that can be given a timeout: a UnixStream, or a stand-in in tests.
trait TimedRead: Read {
    /// Sets the read timeout; macOS refuses it (EINVAL) once the peer has closed both ways, when nothing more can
    /// arrive and the read returns at once, so a failure is not an error.
    fn arm(&mut self, wait: Duration);
}

impl TimedRead for UnixStream {
    fn arm(&mut self, wait: Duration) {
        let _ = self.set_read_timeout(Some(wait));
    }
}

/// Reads the reply until the daemon closes the stream, each read at most [`READ_SLICE`]. The error is the internal
/// code word to report: `reply_timed_out` when a limit ran out, `reply_too_large` once the bytes read would pass
/// [`MAX_REPLY`] (refused before they are kept), `socket_io` for any other failure.
fn read_reply(stream: &mut impl TimedRead, limits: ReplyLimits) -> Result<Vec<u8>, &'static str> {
    let asked = Instant::now();
    let mut first: Option<Instant> = None;
    let mut progress = asked;
    let mut bytes = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let deadline = match first {
            None => asked + limits.first_byte,
            Some(at) => (progress + limits.idle).min(at + limits.total),
        };
        let deadline = limits.end.map_or(deadline, |end| deadline.min(end));
        let Some(left) = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
        else {
            return Err("reply_timed_out");
        };
        stream.arm(READ_SLICE.min(left));
        match stream.read(&mut buf) {
            Ok(0) => return Ok(bytes),
            Ok(n) => {
                progress = Instant::now();
                first.get_or_insert(progress);
                if (bytes.len() + n) as u64 > MAX_REPLY {
                    return Err("reply_too_large");
                }
                bytes.extend_from_slice(&buf[..n]);
            }
            // A slice ran out (or a signal came): look at the limits and read again.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err("socket_io"),
        }
    }
}

/// `umx daemon status`. Works with the daemon down (ruling 2026-11-09 under D13): it then says so and exits
/// 0. The binary adds the `service` object (req-86).
pub fn status(socket: &Path, build: &Build) -> Envelope {
    let request = Request::new("daemon status", Vec::new(), build.clone());
    match call_at(socket, &request) {
        Ok(reply) => reply,
        Err(issue) if issue.code() == Code::DaemonNotRunning.spec().code => Envelope::ok(
            "daemon status",
            json!({ "running": false, "socket": socket.display().to_string() }),
        ),
        Err(issue) => Envelope::fail("daemon status", issue),
    }
}

fn version(ctx: &Context) -> Envelope {
    let mut env = Envelope::ok("version", json!({ "cli": ctx.build, "daemon": null }));
    let status = Request::new("daemon status", Vec::new(), ctx.build.clone());
    if let Ok(reply) = call(ctx, &status)
        && reply.errors.is_empty()
    {
        env.data["daemon"] = reply.data["version"].clone();
        for warning in reply.warnings {
            env = env.warn(warning);
        }
    }
    env
}

/// Prints an envelope: one JSON object on stdout, or human text (data on stdout, issues on stderr).
/// Returns its exit code.
pub fn emit(env: &Envelope, output: Output) -> u8 {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    if output.json {
        let _ = writeln!(out, "{}", env.to_json());
    } else {
        let (o, e) = human::render(
            env,
            human::Style::new(output.color, out.is_terminal()),
            human::Style::new(output.color, err.is_terminal()),
        );
        let _ = out.write_all(o.as_bytes());
        let _ = err.write_all(e.as_bytes());
    }
    let _ = out.flush();
    env.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// Plan 129: an agent's `umx ui` command resolves who sent it, as a write does; reading the selection does not.
    #[test]
    fn ui_commands_resolve_the_sender_and_reads_do_not() {
        let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(needs_actor(&words(&["ui", "clear"])));
        assert!(needs_actor(&words(&[
            "ui", "focus", "01JC8X", "--hops", "2"
        ])));
        assert!(needs_actor(&words(&[
            "node", "add", "--type", "q", "--title", "T"
        ])));
        assert!(!needs_actor(&words(&["ui", "selection"])));
        assert!(!needs_actor(&words(&["node", "list"])));
    }

    /// The scale for the reply-limit tests: limits and sleeps are multiples of it.
    const U: Duration = Duration::from_millis(250);

    const LIMITS: ReplyLimits = ReplyLimits {
        first_byte: Duration::from_millis(8 * 250),
        idle: Duration::from_millis(2 * 250),
        total: Duration::from_millis(12 * 250),
        end: None,
    };

    /// How late past its limit a cut-off may come on a loaded machine (F105: load average 46 to 74 stretched one
    /// to 11.8 s). Tests only: the cut-off itself is proven by the peer holding its end open (see [`held`]).
    const LOADED: Duration = Duration::from_secs(20);

    /// How long a held peer keeps its end open before giving up, so a broken limit fails the test (the CLI then
    /// reads an end of stream, not `reply_timed_out`) instead of hanging it. Longer than any limit plus [`LOADED`].
    const BACKSTOP: Duration = Duration::from_secs(30);

    /// A release for a held peer: the peer keeps its end open until this is dropped (or [`BACKSTOP`]), so the only
    /// way the CLI can stop reading before then is its own limit (F105, the pattern: wait on the system's own
    /// signal, not on a sleep racing it).
    fn held() -> (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>) {
        std::sync::mpsc::channel()
    }

    fn build() -> Build {
        Build {
            version: "0".into(),
            commit: "test".into(),
            built: "now".into(),
        }
    }

    /// A stand-in daemon on a fresh socket: it reads one request, then does `reply` with the stream.
    fn peer(name: &str, reply: impl FnOnce(UnixStream) + Send + 'static) -> PathBuf {
        let socket = PathBuf::from(format!("/tmp/umx-cli-{}-{name}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let _ = stream.read_to_end(&mut request);
            reply(stream);
        });
        socket
    }

    /// A stand-in daemon that answers with `reply` as soon as it accepts, reading nothing.
    fn peer_unread(name: &str, reply: impl FnOnce(UnixStream) + Send + 'static) -> PathBuf {
        let socket = PathBuf::from(format!("/tmp/umx-cli-{}-{name}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            reply(stream);
        });
        socket
    }

    /// Asks `socket` with the test limits; returns the internal code word on failure and the time it took.
    fn ask(socket: &Path) -> (Result<Envelope, String>, Duration) {
        let started = Instant::now();
        let request = Request::new("daemon status", Vec::new(), build());
        let got = call_with(socket, &request, LIMITS).map_err(|issue| issue.message().to_owned());
        let _ = std::fs::remove_file(socket);
        (got, started.elapsed())
    }

    fn timed_out(got: &Result<Envelope, String>) -> bool {
        matches!(got, Err(m) if m.contains("reply_timed_out"))
    }

    #[test]
    fn the_launchers_reads_keep_their_warnings() {
        let stale = Issue::fill(
            Code::StaleFiles,
            &[
                ("graph", Fill::name("beta").unwrap()),
                ("files", Fill::text("nodes.jsonl")),
                ("fix", Fill::command("graph load /work/beta --as beta")),
            ],
        );
        let reply = Envelope::ok("ontology show", json!({ "types": [] })).warn(stale.clone());
        let socket = peer("preflight-warns", move |mut s| {
            let _ = s.write_all(reply.to_json().as_bytes());
        });
        let got = ui_preflight(&socket, &build(), &["beta".to_owned()]);
        let _ = std::fs::remove_file(&socket);
        assert_eq!(got.map_err(|e| e.to_json()), Ok(vec![stale]));
    }

    /// ws002 X1 (review R6): a read that loaded an idle graph says so with a hint, and the launcher keeps it.
    #[test]
    fn the_launchers_reads_keep_their_hints() {
        let loaded = Issue::fill(
            Code::GraphLoadedOnDemand,
            &[
                ("alias", Fill::name("beta").unwrap()),
                ("path", Fill::text("/work/beta")),
            ],
        );
        let reply = Envelope::ok("ontology show", json!({ "types": [] })).hint(loaded.clone());
        let socket = peer("preflight-hints", move |mut s| {
            let _ = s.write_all(reply.to_json().as_bytes());
        });
        let got = ui_preflight(&socket, &build(), &["beta".to_owned()]);
        let _ = std::fs::remove_file(&socket);
        assert_eq!(got.map_err(|e| e.to_json()), Ok(vec![loaded]));
    }

    #[test]
    fn a_slow_first_byte_is_served() {
        let socket = peer("slow", |mut s| {
            std::thread::sleep(5 * U);
            let ok = Envelope::ok("daemon status", json!({ "running": true }));
            let _ = s.write_all(ok.to_json().as_bytes());
        });
        let (got, _) = ask(&socket);
        assert_eq!(got.unwrap().data["running"], true);
    }

    #[test]
    fn a_silent_daemon_is_cut_off_at_the_first_byte_limit() {
        let (release, hold) = held();
        let socket = peer("silent", move |s| {
            let _ = hold.recv_timeout(BACKSTOP);
            drop(s);
        });
        let (got, took) = ask(&socket);
        drop(release);
        assert!(timed_out(&got), "{got:?}");
        assert!(
            took >= LIMITS.first_byte && took < LIMITS.first_byte + LOADED,
            "{took:?}"
        );
    }

    #[test]
    fn a_reply_that_stalls_after_its_first_byte_is_cut_off_at_the_idle_limit() {
        let (release, hold) = held();
        let socket = peer("stall", move |mut s| {
            let _ = s.write_all(b"{");
            let _ = hold.recv_timeout(BACKSTOP);
        });
        // Total past the backstop, so only the idle limit can end it before the peer gives up.
        let limits = ReplyLimits {
            total: BACKSTOP + LOADED,
            ..LIMITS
        };
        let request = Request::new("daemon status", Vec::new(), build());
        let started = Instant::now();
        let got = call_with(&socket, &request, limits).map_err(|issue| issue.message().to_owned());
        let took = started.elapsed();
        let _ = std::fs::remove_file(&socket);
        drop(release);
        assert!(timed_out(&got), "{got:?}");
        assert!(
            took >= LIMITS.idle && took < LIMITS.idle + LOADED,
            "{took:?}"
        );
    }

    /// Review B3: `umx ui`'s supervisor's wait ends `within`, end to end, whatever the daemon does: one that
    /// accepts and never answers, and one that sends the wait's space heartbeat and then stops. The ordinary read
    /// limits (120 s to the first byte, 10 s idle) would keep either going far longer.
    #[test]
    fn the_supervisors_wait_ends_within_its_deadline_on_a_daemon_that_stops_answering() {
        let within = Duration::from_millis(1_500);
        for (name, heartbeat) in [("silent", false), ("beat", true)] {
            let (release, hold) = held();
            let socket = peer(name, move |mut s| {
                if heartbeat {
                    let _ = s.write_all(b"  ");
                }
                let _ = hold.recv_timeout(BACKSTOP);
            });
            let started = Instant::now();
            let got = ui_supervisor_wait(&socket, &build(), None, 0, 2_000, within)
                .map_err(|issue| issue.message().to_owned());
            let took = started.elapsed();
            let _ = std::fs::remove_file(&socket);
            drop(release);
            assert!(timed_out(&got), "{name}: {got:?}");
            assert!(took >= within && took < within + LOADED, "{name}: {took:?}");
            assert!(
                took < ReplyLimits::READ.idle,
                "{name}: past the ordinary idle limit: {took:?}"
            );
        }
    }

    #[test]
    fn a_trickling_reply_is_cut_off_at_the_total_limit() {
        let started = Instant::now();
        let socket = peer("trickle", move |mut s| {
            while started.elapsed() < BACKSTOP && s.write_all(b" ").is_ok() {
                std::thread::sleep(U / 5);
            }
        });
        // Idle no shorter than total, so only the total limit can end it, however late a starved peer writes.
        let limits = ReplyLimits {
            idle: LIMITS.total,
            ..LIMITS
        };
        let request = Request::new("daemon status", Vec::new(), build());
        let got = call_with(&socket, &request, limits).map_err(|issue| issue.message().to_owned());
        let took = started.elapsed();
        let _ = std::fs::remove_file(&socket);
        assert!(timed_out(&got), "{got:?}");
        assert!(
            took >= LIMITS.total && took < LIMITS.total + LOADED,
            "{took:?}"
        );
    }

    #[test]
    fn the_total_limit_counts_from_the_first_byte() {
        let socket = peer("late-trickle", |mut s| {
            std::thread::sleep(5 * U);
            while s.write_all(b" ").is_ok() {
                std::thread::sleep(U / 5);
            }
        });
        let (got, took) = ask(&socket);
        assert!(timed_out(&got), "{got:?}");
        assert!(took >= 5 * U + LIMITS.total, "{took:?}");
    }

    #[test]
    fn a_reply_cut_off_by_the_daemon_closing_is_unreadable_not_a_timeout() {
        let socket = peer("eof", |mut s| {
            let _ = s.write_all(br#"{"umx":"#);
        });
        let (got, took) = ask(&socket);
        assert!(
            matches!(&got, Err(m) if m.contains("reply_unreadable")),
            "{got:?}"
        );
        assert!(took < 2 * U, "{took:?}");
    }

    /// F56: a busy daemon answers and closes without reading; the CLI's write then fails, and it still reads the
    /// answer instead of reporting socket_io.
    #[test]
    fn an_answer_sent_before_the_request_was_read_is_still_read() {
        let socket = peer_unread("unread", |mut s| {
            let busy = Envelope::fail("unknown", Issue::new(Code::DaemonBusy));
            let _ = s.write_all(busy.to_json().as_bytes());
            let _ = s.shutdown(Shutdown::Write);
        });
        // Bigger than any socket buffer, so the write cannot complete into a closed peer.
        let request = Request::new("graph list", vec!["x".repeat(4 << 20)], build());
        let got = call_with(&socket, &request, LIMITS);
        let _ = std::fs::remove_file(&socket);
        assert_eq!(
            got.map(|e| e.errors[0].code().to_owned()),
            Ok("daemon_busy".to_owned())
        );
    }

    /// A stream whose reply and end are there, but whose first `missed` reads sleep their whole timeout and return
    /// WouldBlock, as on macOS when a read's wakeup is missed (F59).
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

    #[test]
    fn a_read_that_misses_its_wakeup_is_read_again_within_a_slice() {
        let mut stream = MissedWakeup {
            data: b"{}".to_vec(),
            at: 0,
            missed: 3,
            armed: Duration::ZERO,
            longest: Duration::ZERO,
        };
        let started = Instant::now();
        assert_eq!(
            read_reply(&mut stream, ReplyLimits::STATUS).as_deref(),
            Ok(&b"{}"[..])
        );
        assert!(stream.longest <= READ_SLICE, "{:?}", stream.longest);
        assert!(
            started.elapsed() < 4 * READ_SLICE + U * 4,
            "{:?}",
            started.elapsed()
        );
    }

    /// A daemon reply of `left` bytes, made as it is read.
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

    /// F194 (ws004 s34a): a reply past the request cap but within MAX_REPLY is read whole (one `similarity pairs`
    /// call's every row); one byte over MAX_REPLY is refused, at the cap and not before.
    #[test]
    fn a_reply_is_read_up_to_max_reply_and_refused_one_byte_over() {
        let big = 2 * umx_protocol::MAX_FRAME;
        let got = read_reply(&mut Spaces { left: big }, ReplyLimits::STATUS);
        assert_eq!(got.map(|b| b.len() as u64), Ok(big));
        let mut over = Spaces {
            left: MAX_REPLY + 1,
        };
        assert_eq!(
            read_reply(&mut over, ReplyLimits::STATUS),
            Err("reply_too_large")
        );
        assert_eq!(over.left, 0, "every byte up to the cap was read first");
    }

    #[test]
    fn the_wait_for_a_reply_follows_what_the_request_does() {
        let cwd = PathBuf::from("/tmp");
        let op = |words: &[&str]| match parse::parse(words, &cwd) {
            Ok(p) => p.op,
            Err(u) => panic!("{words:?}: {}", u.what),
        };
        let limits = |request: Request| ReplyLimits::for_request(&request);
        let with = |o: Op| limits(Request::op(o, None, None, build()));
        let bare = |command: &str| limits(Request::new(command, Vec::new(), build()));
        assert_eq!(with(op(&["graph", "unload", "demo"])), ReplyLimits::WRITER);
        assert_eq!(with(op(&["graph", "list"])), ReplyLimits::READ);
        assert_eq!(bare("daemon status"), ReplyLimits::STATUS);
        // A starting daemon answers status only after composing its services: never less than 60 s.
        assert!(ReplyLimits::STATUS.first_byte >= Duration::from_secs(60));
        assert_eq!(bare("daemon stop"), ReplyLimits::WRITER);
        assert_eq!(bare("daemon restart"), ReplyLimits::WRITER);
    }

    #[test]
    fn idle_unload_is_zero_or_one_second_to_thirty_days() {
        let flags = |v: &str| {
            daemon_start_flags(&["--idle-unload".to_owned(), v.to_owned()])
                .ok()
                .map(|f| f.idle_unload_ms)
        };
        assert_eq!(flags("0"), Some(Some(0)));
        assert_eq!(flags("1s"), Some(Some(1_000)));
        assert_eq!(flags("720h"), Some(Some(MAX_IDLE_UNLOAD_MS)));
        for refused in [
            "999ms",
            "721h",
            "18446744073709551615ms",
            "99999999999999999999h",
        ] {
            assert_eq!(flags(refused), None, "{refused}");
        }
        assert_eq!(daemon_start_flags(&[]).ok(), Some(StartFlags::default()));
    }

    #[test]
    fn service_is_a_flag_given_at_most_once() {
        let flags = |args: &[&str]| {
            daemon_start_flags(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>()).ok()
        };
        let both = StartFlags {
            idle_unload_ms: Some(60_000),
            service: true,
        };
        assert_eq!(flags(&["--service", "--idle-unload", "1m"]), Some(both));
        assert_eq!(flags(&["--idle-unload=1m", "--service"]), Some(both));
        for refused in [
            &["--service", "--service"][..],
            &["--idle-unload", "1m", "--idle-unload", "2m"],
            &["--idle-unload"],
            &["--servce"],
        ] {
            assert_eq!(flags(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn the_offline_flag_rows_are_what_their_parsers_take() {
        // These commands parse their own flags but admit only their parse::FLAGS row (in_flags_row). Each row's
        // flag must be taken, and no other flag umx knows may be; removing a row's flag breaks its parser's tests.
        let takes = |command: &str, flag: &str| {
            let tries = [vec![flag], vec![flag, "1m"]];
            tries.iter().any(|t| {
                let args: Vec<String> = t.iter().map(|a| (*a).to_owned()).collect();
                match command {
                    "daemon start" => daemon_start_flags(&args).is_ok(),
                    "doctor" => doctor_flags(&args).is_ok(),
                    _ => models_fetch_flags(&[&["some-model".to_owned()], &args[..]].concat())
                        .is_ok(),
                }
            })
        };
        let known: Vec<&str> = parse::FLAGS
            .iter()
            .flat_map(|(_, f)| f.iter().copied())
            .chain(parse::COMMON)
            .collect();
        let mut bad = Vec::new();
        for command in ["daemon start", "doctor", "models fetch"] {
            let row = parse::flags(command);
            assert!(!row.is_empty(), "{command} has no row in parse::FLAGS");
            for flag in &known {
                if takes(command, flag) != row.contains(flag) {
                    bad.push(format!("{command} {flag}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "parse::FLAGS and the parser disagree: {bad:?}"
        );
    }
}
