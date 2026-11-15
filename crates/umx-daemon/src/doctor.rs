//! `umx doctor` (req-85): checks the whole setup in a fixed order and reports each finding as a catalogue code with
//! a runnable next. It runs in the binary, not through the daemon, so it works with the daemon down (like `daemon
//! start` and `daemon status`). With the daemon up it asks only what any client may ask: `daemon status`, `graph
//! list` and `check`. It never starts a process and never reads graph files itself.
//!
//! The run state (socket, run folder, $UMX_HOME and the two files in it) is checked here with std, beside the code
//! that creates it; config, the embedder and graph folders go through the services and their ports.
//!
//! `--fix` repairs only a closed list ([`Fix`]): remove a socket nothing answers on, tighten (never widen) the mode of
//! a path in [`run_state`] that this user owns, and restart the service's daemon through the service manager when it
//! runs another build. Each fix is printed on stderr before it runs; the checks then run again, so the report shows
//! what is true after the fixes.

use std::fs::{self, Permissions};
use std::io::Write;
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use umx_protocol::{Build, Code, Envelope, Fill, Issue, Op, PROTOCOL, Request, Value, json};

use umx_compose::SkillReads;
use umx_core::ports::ServiceState;
use umx_services::Services;

use crate::{Config, mode, read_until_end};

/// How long doctor waits for each answer from the daemon.
const ASK_TIMEOUT: Duration = Duration::from_secs(10);

/// One row of the report: which check, about what, and the finding if it is not ok.
struct Row {
    check: &'static str,
    subject: String,
    issue: Option<Issue>,
    fix: Option<Fix>,
    /// The embedder row's credential that worked: kind and program path or env var NAME, never a token or key.
    credential: Option<umx_core::ports::Credential>,
}

impl Row {
    fn ok(check: &'static str, subject: impl Into<String>) -> Row {
        Row {
            check,
            subject: subject.into(),
            issue: None,
            fix: None,
            credential: None,
        }
    }

    fn found(check: &'static str, subject: impl Into<String>, issue: Issue) -> Row {
        Row {
            issue: Some(issue),
            ..Row::ok(check, subject)
        }
    }

    fn fixable(mut self, fix: Fix) -> Row {
        self.fix = Some(fix);
        self
    }

    /// ok, info (exit 0), degraded (5) or broken (6).
    fn outcome(&self) -> &'static str {
        match self.issue.as_ref().map(Issue::exit) {
            None => "ok",
            Some(0) => "info",
            Some(5) => "degraded",
            Some(_) => "broken",
        }
    }
}

/// The only repairs `--fix` makes. Each re-checks its precondition right before it acts.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fix {
    /// Remove a socket that nothing answers on (a daemon was killed).
    RemoveStaleSocket(PathBuf),
    /// Clear the group and other bits of a path this user owns.
    Tighten { path: PathBuf, to: u32 },
    /// Restart the service's daemon through the service manager, so the installed build runs. Never by hand.
    RestartService,
}

impl Fix {
    fn describe(&self) -> String {
        match self {
            Fix::RemoveStaleSocket(p) => format!("remove the stale socket {}", p.display()),
            Fix::Tighten { path, to } => format!("chmod {to:o} {}", path.display()),
            Fix::RestartService => "restart the umx service".to_owned(),
        }
    }

    fn word(&self) -> &'static str {
        match self {
            Fix::RemoveStaleSocket(_) => "remove_stale_socket",
            Fix::Tighten { .. } => "tighten_mode",
            Fix::RestartService => "restart_service",
        }
    }

    fn path(&self) -> Option<&Path> {
        match self {
            Fix::RemoveStaleSocket(p) | Fix::Tighten { path: p, .. } => Some(p),
            Fix::RestartService => None,
        }
    }

    fn apply(&self, config: &Config, services: &Services) -> Result<(), &'static str> {
        match self {
            Fix::RestartService => {
                services
                    .service()
                    .restart()
                    .map_err(|_| "the service manager failed")?;
                // Wait for the restarted daemon to answer, so the re-check sees it.
                let deadline = Instant::now() + ASK_TIMEOUT;
                while UnixStream::connect(&config.socket).is_err() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(())
            }
            Fix::RemoveStaleSocket(path) | Fix::Tighten { path, .. } => {
                let meta = fs::symlink_metadata(path).map_err(|_| "it is gone")?;
                if meta.file_type().is_symlink() || Some(meta.uid()) != config.uid {
                    return Err("it is a symlink or not yours");
                }
                match self {
                    Fix::Tighten { to, .. } => {
                        // Only ever clears bits: never widens what is there now.
                        let to = meta.mode() & 0o777 & to;
                        fs::set_permissions(path, Permissions::from_mode(to))
                            .map_err(|_| "chmod failed")
                    }
                    _ => {
                        if !meta.file_type().is_socket() || UnixStream::connect(path).is_ok() {
                            return Err("it is not a stale socket");
                        }
                        fs::remove_file(path).map_err(|_| "remove failed")
                    }
                }
            }
        }
    }
}

/// The /umx skill this binary was built with (crates/umx/build.rs): its fingerprint and the full commit that holds
/// it, each `unknown` when the build could not say (the commit also when skills/umx was not as committed).
#[derive(Clone, Copy, Debug)]
pub struct Skill<'a> {
    pub fingerprint: &'a str,
    pub commit: &'a str,
}

/// Runs every check, applies the safe fixes when `fix` is set, and returns the report.
pub fn doctor(config: &Config, build: &Build, skill: Skill<'_>, fix: bool) -> Envelope {
    let mut fixed = Vec::new();
    let services = umx_compose::compose(mode(), config);
    // The config file is read again for every pass, so the checks after a repair see the repaired file (B3).
    let mut rows = checks(&config.reread(), build, skill, &services);
    if fix {
        let fixes: Vec<Fix> = rows.iter().filter_map(|r| r.fix.clone()).collect();
        for f in &fixes {
            // Said before it runs, on stderr so a JSON reader still gets one envelope on stdout.
            eprintln!("umx doctor --fix: {}", f.describe());
            let done = f.apply(config, &services);
            fixed.push(json!({
                "fix": f.word(),
                "path": f.path().map(|p| p.display().to_string()),
                "done": done.is_ok(),
                "why_not": done.err(),
            }));
        }
        if !fixes.is_empty() {
            rows = checks(&config.reread(), build, skill, &services);
        }
    }
    report(rows, fixed)
}

fn report(rows: Vec<Row>, fixed: Vec<Value>) -> Envelope {
    let table: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "check": r.check,
                "subject": r.subject,
                "outcome": r.outcome(),
                "code": r.issue.as_ref().map(Issue::code),
                "fixable": r.fix.is_some(),
                "credential": r.credential.as_ref().map(|c| json!({ "kind": c.kind, "from": c.from })),
            })
        })
        .collect();
    let mut env = Envelope::ok(
        "doctor",
        json!({
            "verdict": "ok",
            "columns": ["check", "subject", "outcome", "code"],
            "rows": table,
            "total": rows.len(),
            "truncated": false,
            "fixed": fixed,
        }),
    );
    for issue in rows.into_iter().filter_map(|r| r.issue) {
        env = env.warn(issue);
    }
    env.data["verdict"] = json!(match env.exit_code() {
        0 => "ok",
        5 => "degraded",
        _ => "broken",
    });
    env
}

/// Every check, in order: the daemon, the run state's modes, the embedder, the graphs, the service, then the skill.
fn checks(config: &Config, build: &Build, skill: Skill<'_>, services: &Services) -> Vec<Row> {
    let mut rows = Vec::new();
    let service = services.service().status();
    let (service_pid, managed) = match &service {
        Ok(ServiceState::Installed { pid, loaded, .. }) => (*pid, *loaded),
        _ => (None, false),
    };
    let up = daemon(config, build, service_pid, managed, &mut rows);
    for (path, kind, mask) in run_state(config) {
        // A socket's mode matters only while a daemon serves it; a stale one is reported and removed instead.
        let socket = fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_socket());
        if kind == Kind::Socket && up.is_none() && socket {
            continue;
        }
        rows.extend(path_mode(&path, kind, mask, config.uid));
    }
    let config_services = umx_compose::compose_config(mode(), config);
    rows.push(config_row(config, build, up.is_some(), &config_services));
    // The configured embedder goes only into doctor's own probe Services, for ready(); the daemon wires none.
    // The one builder (ws004 s05): every instance, the active one checked with ready() only.
    let all = umx_compose::embedders(mode(), config, &config_services);
    let resolved = config_services.resolve(None);
    let built = all.active(&resolved).map(|c| c.map(|(_, e)| e));
    let active = all.active_name(&resolved);
    let local = active
        .as_deref()
        .and_then(|n| all.get(n))
        .and_then(umx_compose::Instance::local_backend);
    let probe = umx_compose::compose(mode(), config);
    let probe = match &built {
        Ok(Some(embedder)) => probe.with_embedder(embedder.clone()),
        _ => probe,
    };
    let fallback = active
        .as_deref()
        .and_then(|n| all.get(n))
        .and_then(umx_compose::Instance::local_fallback);
    rows.push(embedder(
        &probe,
        built.err(),
        key_auth(&config_services),
        local,
        fallback,
    ));
    // A local embedder on the CPU only is never silent: a warning under the CPU floor of 2 texts a second, a note
    // with its rate at or above it (ws004 s16).
    let cpu_rate = active
        .as_deref()
        .and_then(|n| all.get(n))
        .and_then(umx_compose::Instance::local_cpu_rate);
    if let (Some((backend, false)), Some(name), Some(rate)) = (local, active.as_deref(), cpu_rate) {
        rows.push(Row::found(
            "embedder",
            name,
            Issue::fill(
                Code::DoctorLocalEmbedderCpu,
                &[
                    ("embedder", Fill::text(name)),
                    ("backend", Fill::word(backend)),
                    ("rate", Fill::text(&format!("{rate:.1}"))),
                ],
            ),
        ));
    }
    if let (Some((backend, true)), Some(name)) = (local, active.as_deref()) {
        rows.push(Row::found(
            "embedder",
            name,
            Issue::fill(
                Code::DoctorLocalEmbedderSlow,
                &[
                    ("embedder", Fill::text(name)),
                    ("backend", Fill::word(backend)),
                ],
            ),
        ));
    }
    // D62: every hosted instance in the machine config is trusted with named graphs' text, so each is named with its
    // host (the endpoint shows only here and in config output).
    for i in all.instances().iter().filter(|i| i.kind != "local") {
        let endpoint = resolved
            .value(&format!(
                "{}.{}.endpoint",
                umx_core::config::INSTANCES,
                i.name
            ))
            .and_then(umx_core::config::ConfigValue::as_text)
            .unwrap_or("");
        rows.push(Row::ok(
            "trust",
            format!("{} at {}", i.name, host_of(endpoint)),
        ));
    }
    // With the daemon down there are no loaded graphs to check (it reloads the registry's graphs when it starts), and
    // the daemon row's finding says so.
    if let Some(reply) = &up {
        registry_rows(reply, &mut rows);
        graphs(config, build, &probe, &mut rows);
    }
    service_rows(config, service, up.as_ref(), &mut rows);
    rows.extend(skill_row(config, build, skill, umx_compose::skill_reads()));
    rows
}

/// The /umx agent skill row (F77): the skill in `config.skill_dir` must be the one this binary was built with,
/// compared by fingerprint, offline. None when there is no HOME or the build does not know its skill. The install
/// line it prints pins the build's own commit when the build knows it (F94).
fn skill_row(config: &Config, build: &Build, skill: Skill<'_>, reads: SkillReads) -> Option<Row> {
    let dir = config.skill_dir.as_ref()?;
    let built_with = skill.fingerprint;
    if built_with == "unknown" {
        return None;
    }
    let source = Fill::skill_source(skill.commit);
    let subject = dir.display().to_string();
    // The root's kind and link target come from the fs adapter (F111), as the walk's listings and reads do.
    let Ok(link) = (reads.root)(dir) else {
        return Some(Row::found(
            "skill",
            subject,
            Issue::fill(
                Code::DoctorSkillMissing,
                &[("path", Fill::path(dir)), ("source", source)],
            ),
        ));
    };
    // Unreadable, over a cap, or holding a link or special file below its root: never the same skill.
    if skill_fingerprint(dir, reads).as_deref() == Some(built_with) {
        return Some(Row::ok("skill", subject));
    }
    let issue = if let Some(target) = link {
        Issue::fill(
            Code::DoctorSkillLinkOutOfDate,
            &[
                ("path", Fill::path(dir)),
                ("target", Fill::path(&target)),
                ("build", Fill::build(build)),
            ],
        )
    } else {
        Issue::fill(
            Code::DoctorSkillOutOfDate,
            &[
                ("path", Fill::path(dir)),
                ("build", Fill::build(build)),
                ("source", source),
            ],
        )
    };
    Some(Row::found("skill", subject, issue))
}

/// The most entries (files and folders) doctor visits in a skill, the most bytes it reads, and how deep it looks
/// (the real skill is about 10 files and 60 KB), so a huge, deep or hostile folder stays cheap.
const SKILL_ENTRIES: usize = 512;
const SKILL_BYTES: u64 = 8 << 20;
const SKILL_DEPTH: usize = 8;

/// The skill's fingerprint: FNV-1a 64 over each file under `root` (sorted by `/`-joined relative path; names
/// starting with `.` skipped, such as .DS_Store) as path, a zero byte, the length as 8 little-endian bytes, then
/// the bytes. Must stay equal to crates/umx/build.rs `skill_fingerprint`; crates/umx/tests/cli.rs proves the two
/// agree. `root` itself may be a link (just skill-link); below it only plain folders and regular files are
/// allowed, checked without following links before anything is opened, so the walk never leaves the skill and
/// never blocks on a FIFO. None when anything else is there, a read fails, or a cap is passed.
fn skill_fingerprint(root: &Path, reads: SkillReads) -> Option<String> {
    fn walk(
        reads: SkillReads,
        dir: &Path,
        rel: &str,
        depth: usize,
        seen: &mut usize,
        out: &mut Vec<(String, PathBuf)>,
    ) -> Option<()> {
        if depth > SKILL_DEPTH {
            return None;
        }
        // The fs adapter lists the folder (F94): opened without following a link, checked to be the same folder
        // once listed, and refused when it holds a link or a special file.
        // The entries left in the budget go to the listing, which stops past them (R230 B1).
        for (name, is_dir) in (reads.folder)(dir, SKILL_ENTRIES - *seen).ok()? {
            if name.starts_with('.') {
                continue;
            }
            *seen += 1;
            if *seen > SKILL_ENTRIES {
                return None;
            }
            let path = dir.join(&name);
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if is_dir {
                walk(reads, &path, &rel, depth + 1, seen, out)?;
            } else {
                out.push((rel, path));
            }
        }
        Some(())
    }
    let root = fs::canonicalize(root).ok()?;
    let mut files = Vec::new();
    walk(reads, &root, "", 0, &mut 0, &mut files)?;
    files.sort();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    let mut left = SKILL_BYTES;
    for (rel, path) in files {
        let data = (reads.file)(&path, left)?;
        left -= data.len() as u64;
        eat(rel.as_bytes());
        eat(&[0]);
        eat(&(data.len() as u64).to_le_bytes());
        eat(&data);
    }
    Some(format!("{h:016x}"))
}

/// The service rows (req-86): installed, loaded, running this daemon, and running this umx.
fn service_rows(
    config: &Config,
    service: Result<ServiceState, umx_core::ports::ServiceError>,
    up: Option<&Envelope>,
    rows: &mut Vec<Row>,
) {
    // The tool in use is the subject, so an override (UMX_SERVICE_TOOL) shows in every service row.
    let tool = config.service_tool.as_str();
    let found = |issue: Issue| Row::found("service", tool, issue);
    let (spec, loaded, pid) = match service {
        Err(e) => {
            return rows.push(found(Issue::fill(
                Code::DoctorServiceUnreadable,
                &[("reason", Fill::text(&e.to_string()))],
            )));
        }
        Ok(ServiceState::NotInstalled) => {
            return rows.push(found(Issue::new(Code::DoctorServiceNone)));
        }
        Ok(ServiceState::Installed { spec, loaded, pid }) => (spec, loaded, pid),
    };
    // A service file umx cannot read back cannot be trusted to start at the next login, whatever runs now (review
    // B1): unreadable, never ok and never "another install".
    if spec.is_none() {
        return rows.push(found(Issue::fill(
            Code::DoctorServiceUnreadable,
            &[(
                "reason",
                Fill::word("the service file is not one umx daemon install wrote"),
            )],
        )));
    }
    let daemon_pid = up.and_then(|r| r.data["pid"].as_u64());
    let row = if spec
        .as_ref()
        .is_some_and(|s| !Path::new(&s.program).is_file())
    {
        let program = spec.as_ref().map(|s| s.program.as_str()).unwrap_or("");
        found(Issue::fill(
            Code::DoctorServiceMissing,
            &[("program", Fill::path(Path::new(program)))],
        ))
    } else if !loaded {
        found(Issue::new(Code::DoctorServiceNotLoaded))
    } else if pid.is_none() {
        match daemon_pid {
            Some(p) => found(Issue::fill(
                Code::DoctorServiceBeside,
                &[("pid", Fill::count(p))],
            )),
            // The daemon row already said doctor_service_down, with the service's next.
            None => return,
        }
    } else {
        Row::ok("service", tool)
    };
    rows.push(row);
    // Information: the service runs another umx binary or UMX_HOME than this command.
    let this = crate::service::this_spec(config).ok();
    let differs = match (&spec, &this) {
        (Some(s), Some(t)) => s.program != t.program || s.home != t.home,
        _ => spec.is_none(),
    };
    if differs {
        let path = |p: Option<&str>| p.map_or(Fill::word("unknown"), |p| Fill::path(Path::new(p)));
        rows.push(Row::found(
            "service program",
            "",
            Issue::fill(
                Code::DoctorServiceElsewhere,
                &[
                    ("program", path(spec.as_ref().map(|s| s.program.as_str()))),
                    ("home", path(spec.as_ref().map(|s| s.home.as_str()))),
                    ("this", path(this.as_ref().map(|t| t.program.as_str()))),
                ],
            ),
        ));
    }
}

/// The daemon rows. Returns the daemon's status reply when one answers. `service_pid` is the service's daemon, if the
/// service runs one: a stale build there is restarted through the service manager by `--fix`. `managed` is true when
/// a loaded service owns the daemon, so a down daemon is the service's finding (restart it with `umx daemon install`,
/// not by hand).
fn daemon(
    config: &Config,
    build: &Build,
    service_pid: Option<u32>,
    managed: bool,
    rows: &mut Vec<Row>,
) -> Option<Envelope> {
    let socket = &config.socket;
    let subject = socket.display().to_string();
    let down = || {
        let code = if managed {
            Code::DoctorServiceDown
        } else {
            Code::DoctorDaemonDown
        };
        Row::found("daemon", subject.clone(), Issue::new(code))
    };
    let Ok(meta) = fs::symlink_metadata(socket) else {
        rows.push(down());
        return None;
    };
    if !meta.file_type().is_socket() {
        // path_mode reports what is there instead of a socket.
        rows.push(down());
        return None;
    }
    let Some(reply) = ask(
        socket,
        &Request::new("daemon status", Vec::new(), build.clone()),
    ) else {
        rows.push(down());
        rows.push(
            Row::found(
                "socket",
                subject.clone(),
                Issue::fill(Code::DoctorSocketStale, &[("path", Fill::path(socket))]),
            )
            .fixable(Fix::RemoveStaleSocket(socket.clone())),
        );
        return None;
    };
    rows.push(Row::ok("daemon", subject));
    let pid = reply.data["pid"].to_string();
    if reply.umx != PROTOCOL {
        rows.push(Row::found(
            "protocol",
            pid.clone(),
            Issue::fill(
                Code::DoctorDaemonProtocol,
                &[
                    ("daemon protocol", Fill::count(reply.umx.into())),
                    ("cli protocol", Fill::count(PROTOCOL.into())),
                ],
            ),
        ));
        return Some(reply);
    }
    rows.push(Row::ok("protocol", pid.clone()));
    match build_of(&reply.data["version"]) {
        Some(theirs) if theirs == *build => rows.push(Row::ok("build", pid.clone())),
        theirs => {
            let theirs = theirs.map_or(Fill::word("unknown"), |b| Fill::build(&b));
            let fills = [("daemon build", theirs), ("cli build", Fill::build(build))];
            let by_service =
                service_pid.is_some() && reply.data["pid"].as_u64() == service_pid.map(u64::from);
            rows.push(if by_service {
                Row::found(
                    "build",
                    pid.clone(),
                    Issue::fill(Code::DoctorServiceBuild, &fills),
                )
                .fixable(Fix::RestartService)
            } else {
                Row::found(
                    "build",
                    pid.clone(),
                    Issue::fill(Code::DoctorDaemonBuild, &fills),
                )
            });
        }
    }
    if reply.data["writer"] == "failed" {
        rows.push(Row::found(
            "writer",
            pid.clone(),
            Issue::new(Code::DoctorWriterFailed),
        ));
    } else {
        rows.push(Row::ok("writer", pid.clone()));
    }
    // The config the daemon runs with, not the file now: a daemon started on a bad config keeps every graph folder
    // closed until it restarts, even after the file is fixed (B2).
    if reply.data["config"] == "invalid" {
        rows.push(Row::found(
            "daemon config",
            pid,
            Issue::new(Code::DoctorDaemonConfigClosed),
        ));
    } else {
        rows.push(Row::ok("daemon config", pid));
    }
    Some(reply)
}

fn build_of(v: &Value) -> Option<Build> {
    Some(Build {
        version: v["version"].as_str()?.to_owned(),
        commit: v["commit"].as_str()?.to_owned(),
        built: v["built"].as_str()?.to_owned(),
    })
}

/// What a path in the run state must be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Socket,
}

/// The run state and the mode bits each must not have: $UMX_HOME, the run folder, the socket and secrets.env none for
/// group or others (0700, 0600); config.toml no group or other write, the bits the config adapter refuses (req-72).
/// Paths that do not exist are skipped: umx creates them when it needs them.
fn run_state(config: &Config) -> [(PathBuf, Kind, u32); 5] {
    [
        (config.home.clone(), Kind::Dir, 0o077),
        (config.run_dir.clone(), Kind::Dir, 0o077),
        (config.socket.clone(), Kind::Socket, 0o077),
        (config.home.join("config.toml"), Kind::File, 0o022),
        (config.home.join("secrets.env"), Kind::File, 0o077),
    ]
}

fn path_mode(path: &Path, kind: Kind, mask: u32, uid: Option<u32>) -> Option<Row> {
    let meta = fs::symlink_metadata(path).ok()?;
    let subject = path.display().to_string();
    let not_yours = |what: &'static str| {
        Some(Row::found(
            "permissions",
            subject.clone(),
            Issue::fill(
                Code::DoctorNotYours,
                &[("path", Fill::path(path)), ("what", Fill::word(what))],
            ),
        ))
    };
    let t = meta.file_type();
    if t.is_symlink() {
        return not_yours("is a symlink");
    }
    let right_kind = match kind {
        Kind::Dir => t.is_dir(),
        Kind::File => t.is_file(),
        Kind::Socket => t.is_socket(),
    };
    if !right_kind {
        return not_yours(match kind {
            Kind::Dir => "is not a folder",
            Kind::File => "is not a plain file",
            Kind::Socket => "is not a socket",
        });
    }
    if Some(meta.uid()) != uid {
        return not_yours("is owned by another user");
    }
    let mode = meta.mode() & 0o777;
    if mode & mask != 0 {
        // 0o755 shows as 755: its octal digits, read as a number.
        let octal = |m: u32| Fill::count(format!("{m:o}").parse().unwrap_or(0));
        return Some(
            Row::found(
                "permissions",
                subject,
                Issue::fill(
                    Code::DoctorModeOpen,
                    &[
                        ("path", Fill::path(path)),
                        ("mode", octal(mode)),
                        ("want", octal(mode & !mask)),
                    ],
                ),
            )
            .fixable(Fix::Tighten {
                path: path.to_path_buf(),
                to: mode & !mask,
            }),
        );
    }
    Some(Row::ok("permissions", subject))
}

/// The embedder row: none configured is information, not a fault; a configured one must be ready.
/// The embedder row: none chosen is information, not a fault; a chosen one must build from config and be ready.
/// Only ready() runs, which sends no text. Reasons name keys and env var names, never a value.
fn embedder(
    services: &umx_services::Services,
    unbuilt: Option<String>,
    key_auth: bool,
    local: Option<(&'static str, bool)>,
    fallback: Option<&'static str>,
) -> Row {
    let unready = |subject: String, reason: &str| {
        Row::found(
            "embedder",
            subject,
            Issue::fill(
                Code::DoctorEmbedderUnready,
                &[("reason", Fill::text(reason))],
            ),
        )
    };
    if let Some(reason) = unbuilt {
        return unready(String::new(), &reason);
    }
    let key = services
        .embedding()
        .key()
        .map(|k| k.to_string())
        .unwrap_or_default();
    // A local instance names what runs its model: metal, cuda or cpu, and why a cuda build fell back to the cpu.
    let key = match (local, fallback) {
        (Some((backend, _)), Some(why)) => format!("{key} ({backend}: {why})"),
        (Some((backend, _)), None) => format!("{key} ({backend})"),
        (None, _) => key,
    };
    // The credential that worked, read from the same instance right after ready() (req-73): az or azd with the
    // program it ran, or api_key with the env var name. Never a token or key.
    let signed_in = |mut row: Row| {
        row.credential = services.embedding().credential();
        row
    };
    match services.embedding().ready() {
        None => Row::found("embedder", "", Issue::new(Code::DoctorEmbedderNone)),
        // An api key is only checked for presence: ready() never calls the endpoint (security F38).
        Some(Ok(())) if key_auth => signed_in(Row::ok(
            "embedder",
            format!("{key} (api key present, not verified)"),
        )),
        Some(Ok(())) => signed_in(Row::ok("embedder", key)),
        Some(Err(umx_core::ports::EmbedError::Unavailable { reason, next })) => {
            unready(key, &format!("{reason} (then {next})"))
        }
        Some(Err(other)) => unready(key, &other.to_string()),
    }
}

/// The host of an https URL (config refuses any other form): what doctor and daemon start show for a trusted
/// instance, never a path or query.
pub(crate) fn host_of(url: &str) -> String {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    rest.split(['/', '?', '#']).next().unwrap_or("").to_owned()
}

/// Whether the chosen embedder signs in with an api key (whose value doctor never sees or checks).
fn key_auth(config_services: &umx_services::ConfigServices) -> bool {
    use umx_core::config::{ConfigValue, INSTANCES, embedder_for};
    let resolved = config_services.resolve(None);
    match embedder_for(&resolved, umx_core::EmbedderRule::Any, None) {
        Ok(Some(e)) => {
            resolved
                .value(&format!("{INSTANCES}.{}.auth", e.name))
                .and_then(ConfigValue::as_text)
                == Some("api_key")
        }
        _ => false,
    }
}

/// The config row: a machine config umx cannot use closes every graph folder (broken); other problems are what
/// `umx config check` lists. With the daemon up, the daemon's own check is asked, so a setting changed since it
/// started (config_restart_needed) counts too (B2).
fn config_row(
    config: &Config,
    build: &Build,
    up: bool,
    config_services: &umx_services::ConfigServices,
) -> Row {
    if config.config_closed.is_some() {
        return Row::found("config", "", Issue::new(Code::DoctorConfigClosed));
    }
    let cwd = std::env::current_dir().ok();
    let cwd_text = cwd.as_ref().and_then(|d| d.to_str()).map(str::to_owned);
    let from_daemon = up
        .then(|| {
            let check = Op::ConfigCheck {
                cwd: cwd_text.clone(),
            };
            ask(
                &config.socket,
                &Request::op(check, None, None, build.clone()),
            )
        })
        .flatten()
        .map(|r| r.warnings.len() + r.errors.len());
    let count = from_daemon.unwrap_or_else(|| {
        let dir = cwd_text
            .as_deref()
            .and_then(|p| umx_core::ports::GraphDir::parse(p).ok());
        config_services.check(dir.as_ref()).len()
    });
    match count {
        0 => Row::ok("config", ""),
        n => Row::found(
            "config",
            "",
            Issue::fill(
                Code::DoctorConfigProblems,
                &[("count", Fill::count(n as u64))],
            ),
        ),
    }
}

/// Each graph the daemon has loaded: its folder resolves inside the roots and still holds graph.json, and `check`
/// finds nothing in its files.
fn graphs(config: &Config, build: &Build, services: &umx_services::Services, rows: &mut Vec<Row>) {
    let list = Request::op(
        Op::GraphList {
            all: false,
            graphfile: None,
        },
        None,
        None,
        build.clone(),
    );
    // A live daemon that does not list its graphs (a refusal, no answer, an unreadable reply) is a finding of its
    // own, never "no graphs" and never "daemon down" (B2).
    let listed = ask(&config.socket, &list).filter(|r| r.errors.is_empty());
    let Some(graphs) = listed.as_ref().and_then(|r| r.data["rows"].as_array()) else {
        rows.push(Row::found(
            "graphs",
            "",
            Issue::new(Code::DoctorGraphsUnlisted),
        ));
        return;
    };
    if graphs.is_empty() {
        rows.push(Row::ok("graphs", "none loaded"));
    }
    let totals = named_check(config, build, graphs);
    for g in graphs {
        let (Some(alias), Some(path)) = (g["alias"].as_str(), g["path"].as_str()) else {
            continue;
        };
        // The alias came back from the daemon, which only loads snake_case aliases; checked again here.
        let Some(graph) = Fill::name(alias) else {
            continue;
        };
        let folder = |what: &'static str| {
            Row::found(
                "graph folder",
                alias,
                Issue::fill(
                    Code::DoctorGraphFolder,
                    &[
                        ("graph", graph.clone()),
                        ("path", Fill::path(Path::new(path))),
                        ("what", Fill::word(what)),
                    ],
                ),
            )
        };
        match services.graph().folder(path) {
            // With the config closed there are no roots to judge by; doctor_config_closed already says why.
            Err(_) if config.config_closed.is_some() => {}
            Err(_) => rows.push(folder("is outside the roots")),
            Ok(dir) if !Path::new(dir.as_str()).join("graph.json").is_file() => {
                rows.push(folder("is gone"));
            }
            Ok(_) => rows.push(Row::ok("graph folder", alias)),
        }
        // Only a successful check with a count is a result; anything else (a refusal such as graph_invalid, no
        // answer, no count) is reported as not checked, never as clean (B1). Its next runs the check, which says why.
        // F113: the count comes from the one check that named every loaded graph; when that check is refused, each
        // graph is checked on its own, as before, so the one that fails is the one reported.
        let total = totals
            .as_ref()
            .and_then(|t| t.get(alias).copied())
            .or_else(|| {
                let check = Request::op(
                    Op::Check {
                        lint: false,
                        kind: None,
                        offset: None,
                    },
                    Some(alias.to_owned()),
                    None,
                    build.clone(),
                );
                ask(&config.socket, &check)
                    .filter(|r| r.errors.is_empty())
                    .and_then(|r| r.data["total"].as_u64())
            });
        rows.push(match total {
            Some(0) => Row::ok("graph check", alias),
            Some(n) => Row::found(
                "graph check",
                alias,
                Issue::fill(
                    Code::DoctorGraphProblems,
                    &[("graph", graph.clone()), ("count", Fill::count(n))],
                ),
            ),
            None => Row::found(
                "graph check",
                alias,
                Issue::fill(Code::DoctorGraphUnchecked, &[("graph", graph.clone())]),
            ),
        });
    }
}

/// F113: each loaded graph's check count from one `umx check` that names every graph `graph list` returned, so links
/// between loaded graphs resolve whatever their visibility (D53). Doctor names them only to count: its output carries
/// counts, never a row's subject, and its next is the explicit `umx check --graph <name>` (security ruling b). None
/// when there are fewer than two graphs, or the check is refused or gives no per-graph counts.
fn named_check(
    config: &Config,
    build: &Build,
    graphs: &[Value],
) -> Option<std::collections::BTreeMap<String, u64>> {
    let aliases: Vec<&str> = graphs
        .iter()
        .filter_map(|g| g["alias"].as_str())
        .filter(|a| Fill::name(a).is_some())
        .collect();
    if aliases.len() < 2 {
        return None;
    }
    let check = Request::op(
        Op::Check {
            lint: false,
            kind: None,
            offset: None,
        },
        Some(aliases.join(",")),
        None,
        build.clone(),
    );
    let reply = ask(&config.socket, &check).filter(|r| r.errors.is_empty())?;
    let counted = reply.data["graphs"].as_array()?;
    Some(
        counted
            .iter()
            .filter_map(|g| Some((g["graph"].as_str()?.to_owned(), g["total"].as_u64()?)))
            .collect(),
    )
}

/// The registry rows (F72), from the running daemon's status: whether it could use this machine's graph registry
/// when it started, and each remembered graph that did not reload, with the command that loads it. A daemon that
/// reports no registry (an older build) adds no rows; the build row already says it differs.
fn registry_rows(reply: &Envelope, rows: &mut Vec<Row>) {
    let r = &reply.data["registry"];
    let Some(file) = r["file"].as_str() else {
        return;
    };
    let skipped: Vec<String> = r["skipped_rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(Value::to_string)
        .collect();
    let why = match r["unreadable"].as_str() {
        Some(why) => Some(format!("{why} (nothing reloaded)")),
        None if !skipped.is_empty() => Some(format!("rows {} were skipped", skipped.join(" "))),
        None => None,
    };
    rows.push(match why {
        Some(why) => Row::found(
            "registry",
            file,
            Issue::fill(
                Code::DoctorRegistryUnreadable,
                &[
                    ("path", Fill::path(Path::new(file))),
                    ("reason", Fill::text(&why)),
                ],
            ),
        ),
        None => Row::ok("registry", file),
    });
    for g in r["not_reloaded"].as_array().into_iter().flatten() {
        let (Some(alias), Some(folder), Some(code)) = (
            g["alias"].as_str(),
            g["folder"].as_str(),
            g["code"].as_str(),
        ) else {
            continue;
        };
        // Aliases were checked when the registry was read; checked again here.
        let Some(graph) = Fill::name(alias) else {
            continue;
        };
        rows.push(Row::found(
            "graph reload",
            alias,
            Issue::fill(
                Code::DoctorGraphNotReloaded,
                &[
                    ("graph", graph),
                    ("path", Fill::path(Path::new(folder))),
                    ("reason", Fill::text(code)),
                ],
            ),
        ));
    }
}

/// One request and its answer, or None if nothing answers in time.
pub(crate) fn ask(socket: &Path, request: &Request) -> Option<Envelope> {
    ask_by(socket, request, Instant::now() + ASK_TIMEOUT)
}

/// One request and its whole answer by `deadline`, or None. Connect, write and every read wait at most for what is
/// left, so a peer that trickles bytes cannot hold the caller past it (review B4).
pub(crate) fn ask_by(socket: &Path, request: &Request, deadline: Instant) -> Option<Envelope> {
    let left = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
    };
    // std has no connect timeout for a Unix socket, and a full backlog can block a connect on Linux.
    let (tx, rx) = std::sync::mpsc::channel();
    let path = socket.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(path));
    });
    let mut stream = rx.recv_timeout(left()?).ok()?.ok()?;
    // macOS refuses a socket timeout (EINVAL) once the peer has closed; the read then returns at once, so a failed
    // arm is ignored, as in the daemon's read_request.
    let _ = stream.set_write_timeout(Some(left()?));
    // A busy daemon answers and closes before it reads (daemon_busy, F56), so a failed write or shutdown still
    // reads what came back (review R129-B2), as umx-cli's call_with does.
    let _ = stream
        .write_all(request.to_json().as_bytes())
        .and_then(|()| stream.shutdown(Shutdown::Write));
    Envelope::from_json(&read_until_end(&mut stream, deadline)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Fix {
        /// A path fix, with a throwaway config and services.
        fn apply_alone(&self) -> Result<(), &'static str> {
            let config = Config::at(std::env::temp_dir().join("umx-doctor-unused"), None);
            self.apply(&config, &umx_compose::compose(mode(), &config))
        }
    }

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("umx-doctor-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn tighten_only_ever_clears_bits() {
        let dir = temp("tighten");
        let f = dir.join("f");
        fs::write(&f, b"x").unwrap();
        fs::set_permissions(&f, Permissions::from_mode(0o640)).unwrap();
        // Asked to set 0o700, it may only clear: 0o640 & 0o700 = 0o600.
        Fix::Tighten {
            path: f.clone(),
            to: 0o700,
        }
        .apply_alone()
        .unwrap();
        assert_eq!(fs::metadata(&f).unwrap().mode() & 0o777, 0o600);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fixes_refuse_a_symlink() {
        let dir = temp("link");
        let target = dir.join("target");
        fs::write(&target, b"x").unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o644)).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let tighten = Fix::Tighten {
            path: link.clone(),
            to: 0o600,
        };
        assert!(tighten.apply_alone().is_err());
        assert!(Fix::RemoveStaleSocket(link.clone()).apply_alone().is_err());
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o644);
        assert!(link.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_plain_file_is_never_removed_as_a_stale_socket() {
        let dir = temp("plain");
        let f = dir.join("umx.sock");
        fs::write(&f, b"x").unwrap();
        assert!(Fix::RemoveStaleSocket(f.clone()).apply_alone().is_err());
        assert!(f.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_skill_walk_reads_only_through_the_injected_reads() {
        // F94 (architect): doctor lists and reads the skill only through the fns compose hands it.
        fn folder(dir: &Path, _budget: usize) -> std::io::Result<Vec<(String, bool)>> {
            Ok(match dir.file_name().and_then(|n| n.to_str()) {
                Some("refs") => vec![("b.md".into(), false)],
                _ => vec![("a.md".into(), false), ("refs".into(), true)],
            })
        }
        fn file(path: &Path, _cap: u64) -> Option<Vec<u8>> {
            Some(path.file_name()?.to_str()?.as_bytes().to_vec())
        }
        fn swapped(_: &Path, _: usize) -> std::io::Result<Vec<(String, bool)>> {
            Err(std::io::Error::other(
                "the folder changed while it was listed",
            ))
        }
        fn unreadable(_: &Path, _: u64) -> Option<Vec<u8>> {
            None
        }
        let root = std::env::temp_dir();
        fn not_link(_: &Path) -> std::io::Result<Option<PathBuf>> {
            Ok(None)
        }
        let reads = SkillReads {
            root: not_link,
            folder,
            file,
        };
        let first = skill_fingerprint(&root, reads).unwrap();
        assert_eq!(skill_fingerprint(&root, reads), Some(first.clone()));
        let other = skill_fingerprint(
            &root,
            SkillReads {
                root: not_link,
                folder,
                file: |p, c| {
                    file(p, c).map(|mut b| {
                        b.push(b'!');
                        b
                    })
                },
            },
        );
        assert_ne!(
            other,
            Some(first),
            "the file fn's bytes are what is fingerprinted"
        );
        assert_eq!(
            skill_fingerprint(
                &root,
                SkillReads {
                    root: not_link,
                    folder: swapped,
                    file
                }
            ),
            None
        );
        assert_eq!(
            skill_fingerprint(
                &root,
                SkillReads {
                    root: not_link,
                    folder,
                    file: unreadable
                }
            ),
            None
        );
        // The walk hands each listing what is left of the entry budget (R230 B1): the root sees all of it, refs
        // what is left after the root's two entries.
        fn budgets(dir: &Path, budget: usize) -> std::io::Result<Vec<(String, bool)>> {
            let want = if dir.file_name().and_then(|n| n.to_str()) == Some("refs") {
                SKILL_ENTRIES - 2
            } else {
                SKILL_ENTRIES
            };
            assert_eq!(budget, want, "{dir:?}");
            folder(dir, budget)
        }
        assert!(
            skill_fingerprint(
                &root,
                SkillReads {
                    root: not_link,
                    folder: budgets,
                    file
                }
            )
            .is_some()
        );
    }

    #[test]
    fn the_skill_root_is_looked_at_only_through_the_injected_reads() {
        // F111: whether the skill is there, and whether it is a link (and to what), come from the root fn compose
        // hands doctor. The real folder is never there, so an answer that does not match the fake's is a direct
        // fs call.
        fn link(_: &Path) -> std::io::Result<Option<PathBuf>> {
            Ok(Some(PathBuf::from("/fake/checkout/skills/umx")))
        }
        fn plain(_: &Path) -> std::io::Result<Option<PathBuf>> {
            Ok(None)
        }
        fn gone(_: &Path) -> std::io::Result<Option<PathBuf>> {
            Err(std::io::ErrorKind::NotFound.into())
        }
        fn folder(_: &Path, _: usize) -> std::io::Result<Vec<(String, bool)>> {
            Err(std::io::Error::other("a different skill"))
        }
        fn file(_: &Path, _: u64) -> Option<Vec<u8>> {
            None
        }
        let config = Config::at(std::env::temp_dir().join("umx-f111-no-such-home"), None);
        assert!(config.skill_dir.as_ref().is_some_and(|d| !d.exists()));
        let build = Build {
            version: "v".into(),
            commit: "c".into(),
            built: "b".into(),
        };
        let skill = Skill {
            fingerprint: "0123456789abcdef",
            commit: "unknown",
        };
        let row = |root| {
            let row = skill_row(&config, &build, skill, SkillReads { root, folder, file }).unwrap();
            let issue = row.issue.expect("a finding");
            (issue.code().to_owned(), issue.message().to_owned())
        };
        let (code, message) = row(link);
        assert_eq!(code, "doctor_skill_link_out_of_date");
        assert!(
            message.contains("links to /fake/checkout/skills/umx,"),
            "{message}"
        );
        assert_eq!(row(plain).0, "doctor_skill_out_of_date");
        assert_eq!(row(gone).0, "doctor_skill_missing");
    }
}
