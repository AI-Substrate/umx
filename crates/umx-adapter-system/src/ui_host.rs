//! system.ui-host (req-104, ws009 s2): runs the graph view's server for `umx ui` and opens the browser at it.
//!
//! The server is `<node> <app>/bin/serve.mjs --token-stdin [--port=<n>]`, started with no shell, a fixed env and in a
//! process group of its own, so the terminal's Ctrl-C reaches umx only. The token is one line on its stdin, never in
//! argv or env; stdin then stays open as a lifeline: when umx ends in any way (Ctrl-C, kill -9) the pipe closes and
//! serve.mjs stops its whole group. Stopping on purpose closes stdin too, waits [`STOP_GRACE`], then kills the group.
//! The server says it is ready with one stdout line, `{"ready":true,"port":N}`; the rest of its stdout is read and
//! dropped. Its stderr is the terminal's.
//!
//! The browser never gets the URL through argv (other users can read argv with ps; security ruling, ws009 s2):
//! macOS gives `open location "<url>"` to /usr/bin/osascript on stdin; Linux writes a 0600 redirect page with a
//! random name in umx's 0700 run folder and gives its path to xdg-open, then deletes it after [`PAGE_LIFE`] or when
//! this host is dropped; the next open sweeps any older than [`PAGE_LIFE`] that a killed umx left.
//!
//! ws009 M1b2: after the token, each further stdin line `code <code>` hands the server a launch code; it answers one
//! stdout line, `{"code":"armed"}` or `{"code":"tab"}`. A detached `umx ui` is started in a session of its own with
//! stdio on /dev/null and reports one line on a socket in the run folder ([`SystemUiHost::with_run_dir`]), named in
//! its argv after [`REPORT_FLAG`]. Process start times and command lines come from ps, by absolute path.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime};

use umx_core::ports::ui_host::{REPORT_FLAG, TOKEN_BYTES, base64url, token_ok, url_ok};
use umx_core::ports::{
    UiCodeUse, UiDetached, UiEnd, UiError, UiHost, UiLaunch, UiProcess, UiServed,
};

/// How long a server may take to stop after its stdin closes before its group is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(3);

/// How long a Linux redirect page is kept for the browser to read.
pub const PAGE_LIFE: Duration = Duration::from_secs(30);

/// How long the browser opener may take.
#[cfg_attr(test, allow(dead_code))]
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// The bound [`run_with_timeout`] holds an opener to: [`OPEN_TIMEOUT`], except in this crate's unit tests. Their
/// openers are shell scripts, and on a loaded machine one took more than 10 s to `stat` and `cp` a page (ws012 s3b),
/// as service.rs's stub tools did (F79). No unit test waits for the bound itself, so a longer one only lets a slow
/// stub finish.
#[cfg(not(test))]
const OPEN_BOUND: Duration = OPEN_TIMEOUT;
#[cfg(test)]
const OPEN_BOUND: Duration = Duration::from_secs(120);

const PAGE_PREFIX: &str = "ui-open-";

/// How long a server may take to answer a launch code.
const CODE_ANSWER: Duration = Duration::from_secs(5);

/// The most a detached child may report.
const REPORT_MAX: u64 = 64 * 1024;

/// How the browser is opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opener {
    /// macOS: this osascript, the script on stdin.
    AppleScript(PathBuf),
    /// Linux: this xdg-open, given a redirect page written in `dir` (umx's run folder, which must be 0700 and ours).
    RedirectPage { program: PathBuf, dir: PathBuf },
    /// No opener on this system; why.
    None(&'static str),
}

/// system.ui-host
pub struct SystemUiHost {
    opener: Opener,
    /// `/bin/kill`, by absolute path, for signalling a process group (std signals one process only).
    kill: PathBuf,
    /// This process's effective uid, for the run folder check.
    uid: Option<u32>,
    /// Redirect pages this host wrote, removed when it is dropped (never another process's).
    pages: Mutex<Vec<PathBuf>>,
    /// The servers this host started and has not yet seen end, by id. Dropping the host stops them.
    servers: Mutex<BTreeMap<u64, SystemUiServer>>,
    next: AtomicU64,
    /// `ps`, by absolute path.
    ps: PathBuf,
    /// umx's owner-only run folder, where a detached child's report socket goes.
    run_dir: Option<PathBuf>,
    /// `<run>/ui.lock`, flocked, once [`UiHost::own_view`] took this UMX_HOME's view; closing it lets go.
    view: Mutex<Option<fs::File>>,
}

impl SystemUiHost {
    pub fn new(opener: Opener, kill: PathBuf, uid: Option<u32>) -> SystemUiHost {
        SystemUiHost {
            opener,
            kill,
            uid,
            pages: Mutex::new(Vec::new()),
            servers: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
            ps: ["/bin/ps", "/usr/bin/ps"]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
                .unwrap_or_else(|| PathBuf::from("/bin/ps")),
            run_dir: None,
            view: Mutex::new(None),
        }
    }

    /// The run folder (0700, this user's) that [`UiHost::detach`] puts its report socket in.
    pub fn with_run_dir(mut self, dir: PathBuf) -> SystemUiHost {
        self.run_dir = Some(dir);
        self
    }

    fn owner_only(&self, dir: &Path) -> bool {
        fs::symlink_metadata(dir).is_ok_and(|m| {
            m.is_dir() && m.mode() & 0o077 == 0 && self.uid.is_none_or(|u| m.uid() == u)
        })
    }

    /// One `ps -o <field>=` value for `pid`, or None when ps finds no such process.
    fn ps_field(&self, pid: u32, field: &str) -> Option<String> {
        let out = Command::new(&self.ps)
            .args(["-ww", "-o", &format!("{field}="), "-p", &pid.to_string()])
            .env_clear()
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !text.is_empty()).then_some(text)
    }

    /// `/bin/kill`, else `/usr/bin/kill`.
    pub fn default_kill() -> PathBuf {
        ["/bin/kill", "/usr/bin/kill"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/kill"))
    }

    fn servers(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, SystemUiServer>> {
        self.servers.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn open_page(&self, program: &Path, dir: &Path, url: &str) -> Result<(), UiError> {
        let failed = |m: &str| UiError::Open(m.to_string());
        let meta = fs::symlink_metadata(dir).map_err(|_| failed("the run folder is missing"))?;
        if !meta.is_dir() || meta.mode() & 0o077 != 0 || Some(meta.uid()) != self.uid {
            return Err(failed(
                "the run folder is not an owner-only folder of this user",
            ));
        }
        sweep_pages(dir, PAGE_LIFE);
        let name = format!("{PAGE_PREFIX}{}.html", random_name()?);
        let page = dir.join(name);
        // create_new is O_CREAT|O_EXCL, which never follows a symlink at the name.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&page)
            .map_err(|e| {
                UiError::Open(format!(
                    "the redirect page could not be written ({:?})",
                    e.kind()
                ))
            })?;
        file.write_all(redirect_page(url).as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|_| failed("the redirect page could not be written"))?;
        drop(file);
        self.pages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(page.clone());
        let mut command = Command::new(program);
        command
            .arg(&page)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let ran = run_with_timeout(&mut command, None);
        let doomed = page.clone();
        std::thread::spawn(move || {
            std::thread::sleep(PAGE_LIFE);
            let _ = fs::remove_file(doomed);
        });
        ran.inspect_err(|_| {
            let _ = fs::remove_file(&page);
        })
    }
}

impl Drop for SystemUiHost {
    fn drop(&mut self) {
        // Each server's own drop closes its lifeline, waits and kills its group.
        self.servers
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        for page in self
            .pages
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = fs::remove_file(page);
        }
    }
}

/// The redirect page: a meta refresh and a plain link, no script.
pub fn redirect_page(url: &str) -> String {
    format!(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<meta name=\"referrer\" content=\"no-referrer\">\n\
         <meta http-equiv=\"refresh\" content=\"0;url={url}\">\n<title>umx ui</title>\n\
         <p><a href=\"{url}\" rel=\"noreferrer\">Open the umx graph view</a></p>\n"
    )
}

/// Removes redirect pages in `dir` older than `age` (left by a umx that was killed before it could).
fn sweep_pages(dir: &Path, age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|d| d >= age);
        if name.to_string_lossy().starts_with(PAGE_PREFIX) && old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N], UiError> {
    let mut bytes = [0u8; N];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| UiError::Token(format!("/dev/urandom ({:?})", e.kind())))?;
    Ok(bytes)
}

fn random_name() -> Result<String, UiError> {
    random_bytes::<12>()
        .map(|b| base64url(&b))
        .map_err(|_| UiError::Open("no random name could be made".to_string()))
}

/// Starts `command`, retrying for up to about a second while Linux reports the program's file busy (ETXTBSY), as in
/// the service managers.
fn spawn_retrying(command: &mut Command) -> std::io::Result<Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy && tries < 100 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

/// Runs a short helper to its end, with `input` on its stdin, and fails unless it exits 0 within [`OPEN_TIMEOUT`].
fn run_with_timeout(command: &mut Command, input: Option<&str>) -> Result<(), UiError> {
    let name = command.get_program().to_string_lossy().into_owned();
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = spawn_retrying(command).map_err(|e| match e.kind() {
        ErrorKind::NotFound => UiError::Open(format!("{name} not found")),
        k => UiError::Open(format!("{name} could not start ({k:?})")),
    })?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(UiError::Open(format!(
                    "{name} exited with {}",
                    status
                        .code()
                        .map_or("a signal".to_string(), |c| c.to_string())
                )));
            }
            Ok(None) if started.elapsed() < OPEN_BOUND => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(UiError::Open(format!("{name} did not finish")));
            }
        }
    }
}

/// The ready line, exactly `{"ready":true,"port":N}`.
pub fn ready_port(line: &str) -> Option<u16> {
    let port = line
        .trim_end()
        .strip_prefix("{\"ready\":true,\"port\":")?
        .strip_suffix('}')?;
    port.parse().ok().filter(|p| *p > 0)
}

/// The answer to a launch code, exactly `{"code":"armed"}` or `{"code":"tab"}`.
pub fn code_answer(line: &str) -> Option<UiCodeUse> {
    match line.trim_end() {
        "{\"code\":\"armed\"}" => Some(UiCodeUse::Armed),
        "{\"code\":\"tab\"}" => Some(UiCodeUse::Tab),
        _ => None,
    }
}

enum Said {
    Ready(u16),
    Code(UiCodeUse),
    /// Stdout ended before the ready line.
    Closed,
}

struct SystemUiServer {
    child: Child,
    /// The lifeline: dropped to tell the server to stop.
    stdin: Option<ChildStdin>,
    kill: PathBuf,
    done: bool,
    /// What the server says after its ready line: answers to launch codes.
    heard: Option<mpsc::Receiver<Said>>,
}

impl SystemUiServer {
    /// Signals the whole group. Only while the leader is not yet reaped (its pid, which is the group id, cannot be
    /// reused before that), or right after reaping it.
    fn kill_group(&self) {
        let group = format!("-{}", self.child.id());
        let _ = Command::new(&self.kill)
            .args(["-s", "KILL", "--", &group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// Closes the lifeline, gives the server [`STOP_GRACE`] to stop its group, then kills the group, and reaps.
    fn finish(&mut self, grace: Duration) {
        if self.done {
            return;
        }
        self.stdin.take();
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if started.elapsed() < grace => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    self.kill_group();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        // Anything left in the group (a server that ignored its lifeline, a child of one that died) goes too.
        self.kill_group();
        self.done = true;
    }
}

impl SystemUiServer {
    fn running(&mut self) -> bool {
        if self.done {
            return false;
        }
        match self.child.try_wait() {
            Ok(None) => true,
            _ => {
                self.finish(Duration::ZERO);
                false
            }
        }
    }
}

impl Drop for SystemUiServer {
    fn drop(&mut self) {
        self.finish(STOP_GRACE);
    }
}

impl UiHost for SystemUiHost {
    fn adapter_id(&self) -> &'static str {
        "system.ui-host"
    }

    fn token(&self) -> Result<String, UiError> {
        random_bytes::<TOKEN_BYTES>().map(|b| base64url(&b))
    }

    fn serve(&self, launch: &UiLaunch) -> Result<UiServed, UiError> {
        let script = Path::new(&launch.app_dir).join("bin").join("serve.mjs");
        let mut command = Command::new(&launch.node);
        command
            .arg(&script)
            .arg("--token-stdin")
            .args(launch.port.map(|p| format!("--port={p}")))
            .current_dir(&launch.app_dir)
            .env_clear()
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0);
        let mut child = spawn_retrying(&mut command).map_err(|e| match e.kind() {
            ErrorKind::NotFound => UiError::Start(format!("{} not found", launch.node)),
            k => UiError::Start(format!("{} could not start ({k:?})", launch.node)),
        })?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut server = SystemUiServer {
            stdin: child.stdin.take(),
            child,
            kill: self.kill.clone(),
            done: false,
            heard: None,
        };
        let line = format!("{}\n", launch.token);
        if let Some(stdin) = server.stdin.as_mut() {
            // A server that died at once shows up below as an exit.
            let _ = stdin
                .write_all(line.as_bytes())
                .and_then(|()| stdin.flush());
        }
        let (said, heard) = mpsc::channel();
        std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout);
            let mut text = String::new();
            loop {
                text.clear();
                match lines.read_line(&mut text) {
                    Ok(0) | Err(_) => {
                        let _ = said.send(Said::Closed);
                        return;
                    }
                    Ok(_) => {
                        if let Some(port) = ready_port(&text) {
                            let _ = said.send(Said::Ready(port));
                            // Then only answers to launch codes count; the rest is read and dropped, so the server
                            // never blocks on a full pipe.
                            loop {
                                text.clear();
                                match lines.read_line(&mut text) {
                                    Ok(0) | Err(_) => return,
                                    Ok(_) => {
                                        if let Some(answer) = code_answer(&text) {
                                            let _ = said.send(Said::Code(answer));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        let deadline = Instant::now() + Duration::from_millis(launch.ready_ms);
        loop {
            match heard.recv_timeout(Duration::from_millis(20)) {
                Ok(Said::Ready(port)) => {
                    let id = self.next.fetch_add(1, Ordering::SeqCst);
                    server.heard = Some(heard);
                    self.servers().insert(id, server);
                    return Ok(UiServed { id, port });
                }
                Ok(Said::Code(_)) => {}
                Ok(Said::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Its stdout is shut; give the leader a moment to exit, so its code can be said.
                    let mut code = None;
                    for _ in 0..50 {
                        if let Ok(Some(status)) = server.child.try_wait() {
                            code = status.code();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    server.finish(Duration::ZERO);
                    return Err(UiError::Exited(code));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Ok(Some(status)) = server.child.try_wait() {
                server.finish(Duration::ZERO);
                return Err(UiError::Exited(status.code()));
            }
            if Instant::now() >= deadline {
                server.finish(STOP_GRACE);
                return Err(UiError::TimedOut);
            }
        }
    }

    fn running(&self, server: &UiServed) -> bool {
        let mut servers = self.servers();
        let alive = servers
            .get_mut(&server.id)
            .is_some_and(SystemUiServer::running);
        if !alive {
            servers.remove(&server.id);
        }
        alive
    }

    fn stop(&self, server: &UiServed) {
        // Out of the map first, so the lock is not held through the grace period.
        let taken = self.servers().remove(&server.id);
        drop(taken);
    }

    fn open(&self, url: &str) -> Result<(), UiError> {
        if !url_ok(url) {
            return Err(UiError::Open("not a loopback URL umx may open".to_string()));
        }
        match &self.opener {
            // url_ok leaves no quote or backslash, so the URL cannot leave the string literal.
            Opener::AppleScript(osascript) => run_with_timeout(
                Command::new(osascript)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
                Some(format!("open location \"{url}\"\n").as_str()),
            ),
            Opener::RedirectPage { program, dir } => self.open_page(program, dir, url),
            Opener::None(why) => Err(UiError::Open((*why).to_string())),
        }
    }

    fn detach(&self, exe: &str, args: &[String], ready_ms: u64) -> Result<UiDetached, UiError> {
        let start = |m: &str| UiError::Start(m.to_string());
        let dir = self
            .run_dir
            .as_ref()
            .ok_or_else(|| start("no run folder to report in"))?;
        if !self.owner_only(dir) {
            return Err(start(
                "the run folder is not an owner-only folder of this user",
            ));
        }
        let name = random_bytes::<4>()
            .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>())
            .map_err(|_| start("no random name could be made"))?;
        // Short: a Unix socket path is at most 104 bytes on macOS, and this one sits beside the daemon's.
        let socket = dir.join(format!("ui-{name}.sock"));
        if socket.as_os_str().len() >= 104 {
            return Err(start(
                "the run folder's path is too long for a socket; set UMX_HOME to a shorter path",
            ));
        }
        let listener = UnixListener::bind(&socket)
            .map_err(|e| UiError::Start(format!("the report socket ({:?})", e.kind())))?;
        let _gone = Removed(socket.clone());
        listener
            .set_nonblocking(true)
            .map_err(|e| UiError::Start(format!("the report socket ({:?})", e.kind())))?;
        let mut command = Command::new(exe);
        command
            .args(args)
            .arg(REPORT_FLAG)
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: setsid takes no arguments and is async-signal-safe, as pre_exec requires.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = spawn_retrying(&mut command).map_err(|e| match e.kind() {
            ErrorKind::NotFound => UiError::Start(format!("{exe} not found")),
            k => UiError::Start(format!("{exe} could not start ({k:?})")),
        })?;
        let deadline = Instant::now() + Duration::from_millis(ready_ms);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let said = read_report(stream)
                        .map_err(|e| UiError::Start(format!("the report ({:?})", e.kind())))?;
                    // Never waited for: it runs on in its own session.
                    return Ok(UiDetached {
                        pid: child.id(),
                        said,
                    });
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => {
                    return Err(UiError::Start(format!(
                        "the report socket ({:?})",
                        e.kind()
                    )));
                }
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(UiError::Exited(status.code()));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(UiError::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn report(&self, to: &str, line: &[u8]) -> Result<(), UiError> {
        let failed = |e: std::io::Error| UiError::Start(format!("the report ({:?})", e.kind()));
        let mut stream = UnixStream::connect(to).map_err(failed)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(failed)?;
        stream.write_all(line).map_err(failed)?;
        stream.shutdown(std::net::Shutdown::Write).map_err(failed)
    }

    fn process(&self, pid: u32) -> Option<UiProcess> {
        Some(UiProcess {
            started: self.ps_field(pid, "lstart")?,
            argv: self.ps_field(pid, "command")?,
        })
    }

    fn signal(&self, pid: u32) -> Result<(), UiError> {
        let target = libc::pid_t::try_from(pid)
            .ok()
            .filter(|p| *p > 1 && *p != std::process::id() as libc::pid_t)
            .ok_or_else(|| UiError::Start(format!("pid {pid} is not one umx signals")))?;
        // SAFETY: kill takes plain integers; TERM to one process, never a group.
        if unsafe { libc::kill(target, libc::SIGTERM) } == 0 {
            Ok(())
        } else {
            Err(UiError::Start(format!(
                "pid {pid} ({:?})",
                std::io::Error::last_os_error().kind()
            )))
        }
    }

    fn ending(&self, server: &UiServed, why: UiEnd) -> Result<(), UiError> {
        let mut servers = self.servers();
        let running = servers
            .get_mut(&server.id)
            .is_some_and(SystemUiServer::running);
        let Some(served) = servers.get_mut(&server.id).filter(|_| running) else {
            return Err(UiError::Exited(None));
        };
        let line = format!("end {}\n", why.as_str());
        served
            .stdin
            .as_mut()
            .ok_or(UiError::Exited(None))?
            .write_all(line.as_bytes())
            .and_then(|()| served.stdin.as_mut().map_or(Ok(()), |s| s.flush()))
            .map_err(|_| UiError::Exited(None))
    }

    fn own_view(&self) -> Result<bool, UiError> {
        use std::os::fd::AsRawFd;
        let start = |m: &str| UiError::Start(m.to_string());
        let mut view = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if view.is_some() {
            return Ok(true);
        }
        let dir = self
            .run_dir
            .as_ref()
            .ok_or_else(|| start("no run folder to hold the view in"))?;
        if !self.owner_only(dir) {
            return Err(start(
                "the run folder is not an owner-only folder of this user",
            ));
        }
        // Never through a symlink; std opens it close-on-exec, so no process this one starts holds the lock.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("ui.lock"))
            .map_err(|e| UiError::Start(format!("ui.lock ({:?})", e.kind())))?;
        let ours = file.metadata().is_ok_and(|m| {
            m.is_file() && m.mode() & 0o077 == 0 && self.uid.is_none_or(|u| m.uid() == u)
        });
        if !ours {
            return Err(start(
                "ui.lock is not an owner-only file of this user (remove it)",
            ));
        }
        // SAFETY: flock takes the open fd and plain flags; the fd lives as long as `file`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            *view = Some(file);
            return Ok(true);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EWOULDBLOCK) => Ok(false),
            _ => Err(start("ui.lock could not be locked")),
        }
    }

    fn launch_code(&self, server: &UiServed, code: &str) -> Result<UiCodeUse, UiError> {
        if !token_ok(code) {
            return Err(UiError::Token(
                "a launch code is made like a token".to_string(),
            ));
        }
        let mut servers = self.servers();
        let running = servers
            .get_mut(&server.id)
            .is_some_and(SystemUiServer::running);
        let Some(served) = servers.get_mut(&server.id).filter(|_| running) else {
            servers.remove(&server.id);
            return Err(UiError::Exited(None));
        };
        let heard = served.heard.as_ref().expect("a ready server is heard");
        while heard.try_recv().is_ok() {}
        let line = format!("code {code}\n");
        let wrote = served.stdin.as_mut().is_some_and(|stdin| {
            stdin
                .write_all(line.as_bytes())
                .and_then(|()| stdin.flush())
                .is_ok()
        });
        if !wrote {
            return Err(UiError::Exited(None));
        }
        match heard.recv_timeout(CODE_ANSWER) {
            Ok(Said::Code(answer)) => Ok(answer),
            _ => Err(UiError::TimedOut),
        }
    }
}

/// Removes a path when dropped.
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The detached child's one line, read to its end (at most [`REPORT_MAX`] bytes, 5 s).
fn read_report(stream: UnixStream) -> std::io::Result<Vec<u8>> {
    stream.set_nonblocking(false)?;
    // macOS refuses the option (EINVAL) once the child has closed its end; its line is then already all here.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut said = Vec::new();
    stream.take(REPORT_MAX).read_to_end(&mut said)?;
    while said.last() == Some(&b'\n') {
        said.pop();
    }
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use umx_contracts::ui_host::{Server, UiStage};

    fn temp(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "umx-ui-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    /// The stand-in serve.mjs, run by /bin/sh as "node". It records what it got beside itself, reads the token
    /// line, leaves a child in its group, and behaves as `mode` says. On stdin's end it stops its group, as
    /// serve.mjs does.
    const SERVE: &str = r#"d="$(dirname "$0")"
echo $$ > "$d/pid"
echo "$0 $*" > "$d/argv"
env > "$d/env"
IFS= read -r t
printf '%s' "$t" > "$d/token"
sleep 600 &
echo $! > "$d/child"
port=4747
for a in "$@"; do case "$a" in --port=*) port="${a#--port=}" ;; esac; done
case "$(cat "$d/mode")" in
  silent) cat > /dev/null; kill -s TERM 0 ;;
  dies) exit 3 ;;
  ends) echo noise; printf '{"ready":true,"port":%s}\n' "$port"; sleep 0.2; kill -s TERM 0 ;;
  *) echo "a line before"; printf '{"ready":true,"port":%s}\n' "$port"
     while IFS= read -r l; do
       case "$l" in
         "code "*) echo "${l#code }" >> "$d/codes"
                   if [ -e "$d/tab" ]; then echo '{"code":"tab"}'; else echo '{"code":"armed"}'; fi ;;
         "end "*) printf '%s' "${l#end }" > "$d/end" ;;
       esac
     done
     kill -s TERM 0 ;;
esac
"#;

    /// Records the opener's argv and stdin.
    const OPENER: &str = "#!/bin/sh\nd=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$d/opened-argv\"\ncat >> \"$d/opened\"\n";

    struct Stage {
        dir: PathBuf,
        children: Mutex<Vec<Child>>,
        rival: Mutex<Option<SystemUiHost>>,
    }

    impl Stage {
        fn bin(&self) -> PathBuf {
            self.dir.join("app/bin")
        }
        fn read(&self, name: &str) -> Option<String> {
            fs::read_to_string(self.bin().join(name)).ok()
        }
        fn pid_alive(&self, name: &str) -> bool {
            self.read(name).is_some_and(|pid| {
                Command::new("/bin/kill")
                    .args(["-s", "0", pid.trim()])
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
            })
        }
    }

    impl Drop for Stage {
        fn drop(&mut self) {
            for child in self.children.get_mut().unwrap().iter_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// Run by [`UiStage::child`] for `Child::Reports`, never by itself: in its working folder, reports the line in
    /// `report-line` to the socket named in `report-to`, through the adapter, as a detached `umx ui` does.
    #[test]
    #[ignore = "a helper run as a detached child"]
    fn report_child() {
        let (Ok(to), Ok(line)) = (
            fs::read_to_string("report-to"),
            fs::read_to_string("report-line"),
        ) else {
            return;
        };
        let host = SystemUiHost::new(Opener::None("none"), SystemUiHost::default_kill(), None);
        host.report(&to, line.as_bytes()).unwrap();
    }

    impl UiStage for Stage {
        fn launch(&self, server: Server, port: Option<u16>, token: &str) -> UiLaunch {
            let mode = match server {
                Server::Ready => "ready",
                Server::Silent => "silent",
                Server::Dies => "dies",
                Server::Ends => "ends",
            };
            for f in ["pid", "child", "token", "argv", "env", "end", "codes"] {
                let _ = fs::remove_file(self.bin().join(f));
            }
            fs::write(self.bin().join("mode"), mode).unwrap();
            UiLaunch {
                node: "/bin/sh".to_string(),
                app_dir: self.dir.join("app").display().to_string(),
                port,
                env: vec![("PATH".to_string(), "/bin:/usr/bin".to_string())],
                token: token.to_string(),
                // A deadline, not a wait: the shell stage can be slow to start on a loaded machine
                // (a 5 s bound timed out in the gate).
                ready_ms: 60_000,
            }
        }
        fn token_seen(&self) -> Option<String> {
            self.read("token")
        }
        fn argv_and_env_seen(&self) -> String {
            format!(
                "{}\n{}",
                self.read("argv").unwrap_or_default(),
                self.read("env").unwrap_or_default()
            )
        }
        fn alive(&self) -> bool {
            self.pid_alive("pid") || self.pid_alive("child")
        }
        fn tab(&self, live: bool) {
            if live {
                fs::write(self.bin().join("tab"), "").unwrap();
            } else {
                let _ = fs::remove_file(self.bin().join("tab"));
            }
        }
        fn codes_seen(&self) -> Vec<String> {
            self.read("codes")
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
        fn child(&self, child: umx_contracts::ui_host::Child) -> (String, Vec<String>) {
            use umx_contracts::ui_host::Child;
            let sh = |script: &str, rest: &[&str]| {
                let mut args = vec!["-c".to_string(), script.to_string()];
                args.extend(rest.iter().map(|a| a.to_string()));
                ("/bin/sh".to_string(), args)
            };
            match child {
                // $0 the test binary, $1 a folder of its own, then the host's flag ($2) and socket ($3).
                Child::Reports(line) => {
                    let at = self.dir.join("child");
                    fs::create_dir_all(&at).unwrap();
                    fs::write(at.join("report-line"), line).unwrap();
                    sh(
                        "cd \"$1\" && printf '%s' \"$3\" > report-to && exec \"$0\" --exact ui_host::tests::report_child --ignored --quiet",
                        &[
                            &std::env::current_exe().unwrap().display().to_string(),
                            &at.display().to_string(),
                        ],
                    )
                }
                Child::Exits => sh("exit 0", &[]),
                Child::Silent => sh("exec sleep 600", &[]),
            }
        }
        fn live_process(&self) -> u32 {
            let child = Command::new("/bin/sleep")
                .arg("600")
                .stdin(Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id();
            self.children.lock().unwrap().push(child);
            pid
        }
        fn ending_seen(&self) -> Option<String> {
            self.read("end")
        }
        fn other_owner(&self, holds: bool) {
            let mut rival = self.rival.lock().unwrap();
            if holds {
                let other = SystemUiHost::new(
                    Opener::None("none"),
                    SystemUiHost::default_kill(),
                    crate::effective_uid(),
                )
                .with_run_dir(self.dir.clone());
                assert_eq!(other.own_view(), Ok(true));
                *rival = Some(other);
            } else {
                rival.take();
            }
        }
        fn ended(&self, pid: u32) -> bool {
            self.children
                .lock()
                .unwrap()
                .iter_mut()
                .find(|c| c.id() == pid)
                .is_none_or(|c| matches!(c.try_wait(), Ok(Some(_))))
        }
        fn opened(&self) -> Vec<String> {
            self.read("../../opened")
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    l.strip_prefix("open location \"")
                        .and_then(|l| l.strip_suffix('"'))
                })
                .map(str::to_string)
                .collect()
        }
    }

    fn stage() -> (SystemUiHost, Stage) {
        let dir = temp("stage");
        fs::create_dir_all(dir.join("app/bin")).unwrap();
        fs::write(dir.join("app/bin/serve.mjs"), SERVE).unwrap();
        let opener = dir.join("opener");
        fs::write(&opener, OPENER).unwrap();
        fs::set_permissions(&opener, fs::Permissions::from_mode(0o755)).unwrap();
        let host = SystemUiHost::new(
            Opener::AppleScript(opener),
            SystemUiHost::default_kill(),
            crate::effective_uid(),
        )
        .with_run_dir(dir.clone());
        (
            host,
            Stage {
                dir,
                children: Mutex::new(Vec::new()),
                rival: Mutex::new(None),
            },
        )
    }

    umx_contracts::ui_host_contract!(
        stage;
        uh_c1_tokens_are_base64url_and_never_repeat(),
        uh_c2_the_token_reaches_stdin_only_and_the_port_is_reported(),
        uh_c3_stop_or_dropping_the_host_leaves_no_process(),
        uh_c4_a_server_never_ready_times_out_and_leaves_no_process(),
        uh_c5_a_server_that_dies_before_ready_is_an_exit_and_leaves_no_process(),
        uh_c6_a_server_that_ends_is_seen_as_not_running(),
        uh_c7_open_hands_over_only_a_loopback_url(),
        uh_c8_a_launch_code_reaches_stdin_only_and_is_dropped_while_a_tab_is_connected(),
        uh_c9_a_process_is_reported_until_it_is_signalled(),
        uh_c10_a_detached_child_reports_one_line_or_its_end_is_an_error(),
        uh_c11_the_view_is_owned_by_one_host_at_a_time(),
        uh_c12_why_the_view_ends_reaches_the_server(),
    );

    /// The architect: a process started while the view is held (the node server, say) never keeps the lock
    /// once its host lets go; and a symlink at ui.lock is refused, not followed.
    #[test]
    fn the_view_lock_is_never_inherited_and_never_followed_through_a_symlink() {
        let (host, stage) = stage();
        assert_eq!(host.own_view(), Ok(true));
        let mut child = Command::new("/bin/sleep")
            .arg("600")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        drop(host);
        let next = SystemUiHost::new(
            Opener::None("none"),
            SystemUiHost::default_kill(),
            crate::effective_uid(),
        )
        .with_run_dir(stage.dir.clone());
        // A moment for any fork elsewhere in this test process to reach its exec; the sleep child lives 600 s.
        let free = (0..100).any(|_| {
            let got = next.own_view() == Ok(true);
            if !got {
                std::thread::sleep(Duration::from_millis(20));
            }
            got
        });
        assert!(free, "the child kept the lock");
        let _ = child.kill();
        let _ = child.wait();
        drop(next);
        fs::remove_file(stage.dir.join("ui.lock")).unwrap();
        let elsewhere = stage.dir.join("elsewhere");
        fs::write(&elsewhere, "").unwrap();
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, stage.dir.join("ui.lock")).unwrap();
        let host = SystemUiHost::new(
            Opener::None("none"),
            SystemUiHost::default_kill(),
            crate::effective_uid(),
        )
        .with_run_dir(stage.dir.clone());
        assert!(matches!(host.own_view(), Err(UiError::Start(_))));
    }

    /// ws009 M1b2 (architect): this very process is reported, with the command line ps shows for it.
    #[test]
    fn our_own_process_is_reported_with_its_start_and_command_line() {
        let (host, _stage) = stage();
        let me = host.process(std::process::id()).expect("this process");
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert!(me.argv.contains(&name), "{me:?}");
        assert_eq!(me.started.split_whitespace().count(), 5, "{me:?}");
    }

    #[test]
    fn only_the_exact_code_answers_count() {
        assert_eq!(
            code_answer("{\"code\":\"armed\"}\n"),
            Some(UiCodeUse::Armed)
        );
        assert_eq!(code_answer("{\"code\":\"tab\"}"), Some(UiCodeUse::Tab));
        for bad in ["{\"code\":\"open\"}", " {\"code\":\"tab\"}", "code"] {
            assert_eq!(code_answer(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_opener_gets_the_url_on_stdin_never_in_argv() {
        let (host, stage) = stage();
        let url = "http://127.0.0.1:4747/#t=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        host.open(url).unwrap();
        let argv = fs::read_to_string(stage.dir.join("opened-argv")).unwrap();
        assert_eq!(argv.trim(), "", "the opener got arguments");
        assert_eq!(stage.opened(), vec![url.to_string()]);
    }

    #[test]
    fn a_redirect_page_is_owner_only_in_an_owner_only_folder_and_goes_away() {
        let dir = temp("run");
        let opener = dir.join("xdg-open");
        fs::write(
            &opener,
            "#!/bin/sh\nstat -f %Lp \"$1\" 2>/dev/null > \"$1.mode\" || stat -c %a \"$1\" > \"$1.mode\"\ncp \"$1\" \"$1.seen\"\n",
        )
        .unwrap();
        fs::set_permissions(&opener, fs::Permissions::from_mode(0o755)).unwrap();
        let host = SystemUiHost::new(
            Opener::RedirectPage {
                program: opener,
                dir: dir.clone(),
            },
            SystemUiHost::default_kill(),
            crate::effective_uid(),
        );
        let url = "http://127.0.0.1:4747/#t=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        host.open(url).unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let page = names
            .iter()
            .find(|n| n.starts_with(PAGE_PREFIX) && n.ends_with(".html"))
            .expect("a page");
        assert_eq!(page.len(), PAGE_PREFIX.len() + 16 + 5, "a random name");
        let seen = fs::read_to_string(dir.join(format!("{page}.seen"))).unwrap();
        assert_eq!(seen, redirect_page(url));
        assert!(!seen.contains("<script"));
        assert_eq!(
            fs::read_to_string(dir.join(format!("{page}.mode")))
                .unwrap()
                .trim(),
            "600"
        );
        // Another host (say the daemon's, or doctor's) going away leaves the page for the browser.
        drop(SystemUiHost::new(
            Opener::RedirectPage {
                program: dir.join("xdg-open"),
                dir: dir.clone(),
            },
            SystemUiHost::default_kill(),
            crate::effective_uid(),
        ));
        assert!(dir.join(page).exists(), "another host removed the page");
        drop(host);
        assert!(!dir.join(page).exists(), "the page outlived the host");
        // A folder others can read is refused, and nothing is written in it.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let host = SystemUiHost::new(
            Opener::RedirectPage {
                program: dir.join("xdg-open"),
                dir: dir.clone(),
            },
            SystemUiHost::default_kill(),
            crate::effective_uid(),
        );
        assert!(matches!(host.open(url), Err(UiError::Open(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_opener_says_why_and_runs_nothing() {
        let host = SystemUiHost::new(
            Opener::None("no browser opener"),
            SystemUiHost::default_kill(),
            None,
        );
        assert_eq!(
            host.open("http://127.0.0.1:1/"),
            Err(UiError::Open("no browser opener".to_string()))
        );
    }

    #[test]
    fn only_the_exact_ready_line_counts() {
        assert_eq!(ready_port("{\"ready\":true,\"port\":4747}\n"), Some(4747));
        for bad in [
            "{\"ready\":true,\"port\":0}",
            "{\"ready\":false,\"port\":1}",
            " {\"ready\":true,\"port\":1}",
            "{\"ready\":true,\"port\":70000}",
            "ready",
        ] {
            assert_eq!(ready_port(bad), None, "{bad}");
        }
    }
}
