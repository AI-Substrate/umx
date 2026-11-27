//! End-to-end tests of umx dispatch: daemon lifecycle, the owner-only socket, offline commands, and one
//! trigger for every catalogue code (req-38, req-40, req-41, req-45, req-06).
//! `test-driver` injects fixture budgets into the real adapters, without changing the shipped binary's caps.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use umx_protocol::{Build, Code, Envelope, Fill, Issue, Kind, Request, Status, Value, json, slots};

/// The Cypher fixture's row comparer (floats within 1e-9), shared with umx-cypher's tests and the LadybugDB check.
#[path = "../../umx-cypher/tests/rows/mod.rs"]
mod rows;

/// Security s3b N1: the [`Capture`] test, in its own file so this one stays under the seed gate's size cap.
#[path = "cli/capture.rs"]
mod capture;

/// req-101: `umx ontology adopt-std`'s tests, in their own file so this one stays under the seed gate's size cap.
#[path = "cli/adopt_std.rs"]
mod adopt_std;

/// A private UMX_HOME under /tmp (short, for the 104-byte socket path limit) and maybe a daemon in it.
struct Home {
    path: PathBuf,
    daemon: Option<Child>,
    /// Sets UMX_ROOTS to the home folder (the default); false leaves roots to the config.
    roots_env: bool,
    /// The working folder umx runs in; None is the test's own.
    cwd: Option<PathBuf>,
    /// Extra env for every command, the daemon included.
    env: Vec<(String, String)>,
    /// Runs umx in a new session with no controlling terminal, so /dev/tty cannot be opened even when the tests run
    /// from one (a person's shell); an agent's shell and CI have none anyway.
    no_tty: bool,
    binary: &'static str,
}

impl Home {
    fn new(name: &str) -> Self {
        let path = PathBuf::from(format!("/tmp/umx-t-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        Home {
            path,
            daemon: None,
            roots_env: true,
            cwd: None,
            env: Vec::new(),
            no_tty: false,
            binary: Self::fixture_binary(),
        }
    }

    fn run_dir(&self, mode: u32) -> PathBuf {
        let dir = self.path.join("run");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
        dir
    }

    fn socket(&self) -> PathBuf {
        self.path.join("run/umx.sock")
    }

    /// umx with no output flag and no UMX_OUTPUT, so every test also proves JSON is the default (req-63).
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = if self.no_tty {
            // By path: some tests point PATH at an empty folder.
            let mut c = Command::new("/usr/bin/perl");
            c.args([
                "-MPOSIX",
                "-e",
                "POSIX::setsid() or die 'setsid'; exec @ARGV or die 'exec'",
                self.binary,
            ]);
            c
        } else {
            Command::new(self.binary)
        };
        sandbox(&mut cmd, &self.path);
        cmd.args(args)
            .env("UMX_HOME", &self.path)
            .env_remove("UMX_ROOTS")
            .env_remove("UMX_OUTPUT")
            .env_remove("UMX_GRAPH")
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("TERM");
        if self.roots_env {
            cmd.env("UMX_ROOTS", &self.path);
        }
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd
    }

    fn fixture_binary() -> &'static str {
        #[cfg(feature = "test-driver")]
        {
            env!("CARGO_BIN_EXE_umx-test-driver")
        }
        #[cfg(not(feature = "test-driver"))]
        {
            env!("CARGO_BIN_EXE_umx")
        }
    }

    /// Runs umx and returns its envelope and exit code. Fails after [`Home::limit`], so a command that should have
    /// been refused (a `daemon start` that starts) fails the test instead of hanging it.
    fn umx(&self, args: &[&str]) -> (Envelope, u8) {
        self.umx_with(args, &[])
    }

    /// How long one umx command, or a daemon's ready line, may take: 60 s, 300 s for the fixture driver. 60 s, not
    /// 20: under the full suite's load a sound command (`graph new --secured`, which writes this machine's policy
    /// record) once took longer than 20 s (the project manager's D50 gate); the bound only catches hangs.
    fn limit(&self) -> Duration {
        if self.binary == env!("CARGO_BIN_EXE_umx") {
            Duration::from_secs(60)
        } else {
            LIMIT
        }
    }

    /// [`Home::umx`] with extra env.
    fn umx_with(&self, args: &[&str], env: &[(&str, &str)]) -> (Envelope, u8) {
        let mut cmd = self.command(args);
        cmd.envs(env.iter().copied());
        let mut captured = cmd.spawn_captured().unwrap();
        captured.what = format!("umx {args:?}");
        let out = captured.finish(self.limit());
        let env = Envelope::from_json(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "umx {args:?}: {e}: {}",
                String::from_utf8_lossy(&out.stdout)
            )
        });
        assert_eq!(env.check(), Ok(()), "umx {args:?}");
        (env, out.status.code().unwrap() as u8)
    }

    /// Starts the daemon and returns its ready envelope.
    fn start(&mut self) -> Envelope {
        self.start_with(&[])
    }

    /// Starts the daemon with extra `daemon start` flags.
    fn start_with(&mut self, flags: &[&str]) -> Envelope {
        let mut args = vec!["daemon", "start"];
        args.extend_from_slice(flags);
        let mut child = self.command(&args).stdout(Stdio::piped()).spawn().unwrap();
        let line = ready_line(&mut child, &format!("umx {args:?}"), self.limit());
        self.daemon = Some(child);
        let env = Envelope::from_json(line.as_bytes())
            .unwrap_or_else(|e| panic!("ready line {line:?}: {e}"));
        assert_eq!(env.check(), Ok(()));
        self.reloaded();
        env
    }

    /// Waits until the daemon has reloaded the graphs the registry remembers: it serves while it reloads,
    /// and tests read what came back.
    fn reloaded(&self) {
        let until = Instant::now() + self.limit();
        loop {
            let (status, _) = self.umx(&["daemon", "status"]);
            if status.data["registry"]["reloading"].as_u64().unwrap_or(0) == 0 {
                return;
            }
            assert!(
                Instant::now() < until,
                "the daemon never finished its reload"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        if self.socket().exists() {
            // Bounded: a stand-in daemon that never answers must not hang the teardown (the CLI waits for a reply).
            if let Ok(mut stop) = self
                .command(&["daemon", "stop"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                let deadline = Instant::now() + Duration::from_secs(20);
                while stop.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let _ = stop.kill();
                let _ = stop.wait();
            }
        }
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The longest any spawned command, or a wait for a daemon to exit, may take before the test kills it and fails
/// naming it: the fixture driver's bound (see [`Home::limit`]). Only a hang reaches it.
const LIMIT: Duration = Duration::from_secs(300);

/// A child's stdout or stderr as a file of its own, read once the child has exited (ws012 s3b B1). Writes to a file are
/// done when the child's write returns, so after it exits the file holds all it wrote, and no end of file has to
/// come. A pipe's does not when another process holds its write end: on macOS a pipe is made close-on-exec only after
/// it is made (std's `pipe()` then `fcntl`; Linux has `pipe2`), so a process this binary spawns from another thread
/// in between inherits it and passes it on. That fits the hang on the macOS runner: a daemon the service stub starts
/// during `daemon restart` can hold a concurrent `node add`'s stdout, and a read to its end waits for that daemon.
/// A file handed on the same way is harmless: no reader waits on it, and no daemon writes to a descriptor it never
/// opened. Files also need no reader thread, so a reply bigger than a pipe's buffer never blocks umx (ws004 s25).
struct Capture(PathBuf);

impl Capture {
    fn new() -> (Capture, Stdio) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("umx-t-out-{}-{n}", std::process::id()));
        let file = Capture::create(&path)
            .unwrap_or_else(|e| panic!("cannot create {} new: {e}", path.display()));
        (Capture(path), Stdio::from(file))
    }

    /// Creates `path` new, readable by this user only, and fails if anything is there: a link planted in a shared
    /// temp folder is never followed, so its target is never truncated (security s3b N1).
    fn create(path: &Path) -> std::io::Result<fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }

    fn read(self) -> Vec<u8> {
        fs::read(&self.0).unwrap()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// A spawned child whose stdout (and stderr, when asked) go to [`Capture`] files.
struct Captured {
    child: Child,
    stdout: Capture,
    stderr: Option<Capture>,
    what: String,
}

impl Captured {
    /// Waits for the child for at most `limit` (past it, kills it and fails naming the command), then reads all it
    /// wrote.
    fn finish(mut self, limit: Duration) -> Output {
        drop(self.child.stdin.take());
        let status = wait_within(&mut self.child, &self.what, limit);
        Output {
            status,
            stdout: self.stdout.read(),
            stderr: self.stderr.map(Capture::read).unwrap_or_default(),
        }
    }
}

fn wait_within(child: &mut Child, what: &str, limit: Duration) -> ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what} did not finish within {} s", limit.as_secs());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The first stdout line of a starting daemon, within `limit`; past it, kills the daemon and fails naming `what`. A
/// pipe, read only to its first line: the daemon writes it and runs on, so no end of file is waited for.
fn ready_line(child: &mut Child, what: &str, limit: Duration) -> String {
    let stdout = child.stdout.take().unwrap();
    lines_until(stdout, "\n", child, what, limit)
}

/// What `pipe` gives up to and including the first line holding `needle`, within `limit`; past it (or at an end
/// before it), kills `child` and fails naming `what`.
fn lines_until(
    pipe: impl Read + Send + 'static,
    needle: &'static str,
    child: &mut Child,
    what: &str,
    limit: Duration,
) -> String {
    let (sent, got) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut seen = String::new();
        while !seen.contains(needle) {
            match reader.read_line(&mut seen) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = sent.send(seen);
    });
    match got.recv_timeout(limit) {
        Ok(seen) if seen.contains(needle) => seen,
        other => {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{what} printed no {needle:?} within {} s: {:?}",
                limit.as_secs(),
                other.ok()
            )
        }
    }
}

/// Every child this file runs goes through these, so a hang fails at [`LIMIT`] naming the command.
trait Bounded {
    /// `Command::output`, bounded: stdin null, stdout and stderr captured in files.
    fn bounded_output(&mut self) -> std::io::Result<Output>;
    /// `Command::status`, bounded.
    fn bounded_status(&mut self) -> std::io::Result<ExitStatus>;
    /// Spawns with stdout in a file (stdin and stderr as already set), for a test that writes to stdin first or
    /// runs several at once; [`Captured::finish`] waits for it and reads it.
    fn spawn_captured(&mut self) -> std::io::Result<Captured>;
}

fn command_name(cmd: &Command) -> String {
    format!(
        "{} {:?}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args().collect::<Vec<_>>()
    )
}

impl Bounded for Command {
    fn bounded_output(&mut self) -> std::io::Result<Output> {
        let (stderr, to) = Capture::new();
        self.stdin(Stdio::null()).stderr(to);
        let mut captured = self.spawn_captured()?;
        captured.stderr = Some(stderr);
        Ok(captured.finish(LIMIT))
    }

    fn bounded_status(&mut self) -> std::io::Result<ExitStatus> {
        let what = command_name(self);
        let mut child = self.stdin(Stdio::null()).spawn()?;
        Ok(wait_within(&mut child, &what, LIMIT))
    }

    fn spawn_captured(&mut self) -> std::io::Result<Captured> {
        let (stdout, to) = Capture::new();
        let child = self.stdout(to).spawn()?;
        Ok(Captured {
            child,
            stdout,
            stderr: None,
            what: command_name(self),
        })
    }
}

trait BoundedChild {
    fn bounded_wait(&mut self) -> std::io::Result<ExitStatus>;
}

impl BoundedChild for Child {
    fn bounded_wait(&mut self) -> std::io::Result<ExitStatus> {
        let what = format!("process {}", self.id());
        Ok(wait_within(self, &what, LIMIT))
    }
}

impl Home {
    /// A byte-for-byte copy of this repo's skills/umx at `to`, as `npx skills add` installs it.
    fn skill_copy(&self, to: &Path) -> PathBuf {
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).unwrap();
            for entry in fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        copy(&repo_skill(), to);
        to.to_path_buf()
    }

    /// `~/.agents/skills/umx` as `just skill-link` makes it: a symlink to `to`.
    fn skill_link(&self, to: &Path) {
        let dir = self.path.join(".agents/skills");
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(to, dir.join("umx")).unwrap();
    }
}

/// This repo's skills/umx, the skill the umx under test was built with.
fn repo_skill() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills/umx")
        .canonicalize()
        .unwrap()
}

/// Every umx a test runs gets HOME inside its private home, so the service folder (`~/Library/LaunchAgents`,
/// `~/.config/systemd/user`) is a temp one; UMX_SERVICE_TOOL naming the stub launchctl or systemctl in `<home>/bin`,
/// which also puts the service under its sandbox label; and a PATH with `<home>/bin` first and only the system
/// folders after it. No test touches the machine's real services.
fn sandbox(cmd: &mut Command, home: &Path) {
    let tool = if cfg!(target_os = "macos") {
        "launchctl"
    } else {
        "systemctl"
    };
    cmd.env("UMX_SERVICE_TOOL", home.join("bin").join(tool))
        .env("HOME", home)
        .env("PATH", format!("{}/bin:/usr/bin:/bin", home.display()))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("FS3_CONFIG_DIR");
}

/// Stub launchctl and systemctl: they record each argv line in `<home>/bin/argv` and keep state in files beside
/// themselves: `loaded` (launchd) or `enabled` (systemd), `nopid` (loaded, nothing running), `pid` (the pid to
/// report, else 4242), `fail` (every call exits 1) and `run` (really start and stop the daemon the service file
/// names, as the service manager would). A restart records `serving` when the daemon's socket is still there.
const LAUNCHCTL: &str = r#"#!/bin/sh
d="$(dirname "$0")"
launch() {
  ready="$d/detached-$$"
  rm -f "$ready"
  /usr/bin/perl -MPOSIX -e 'POSIX::setsid() or die "setsid"; my $ready=shift @ARGV; open my $f, ">", $ready or die "ready"; close $f; exec @ARGV or die "exec"' "$ready" "$@" &
  launched=$!
  while [ ! -e "$ready" ]; do sleep 0.01; done
  rm -f "$ready"
}
echo "$*" >> "$d/argv"
[ -e "$d/fail" ] && exit 1
case "$1" in
  bootstrap) [ -e "$d/loaded" ] && exit 5; touch "$d/loaded"
    if [ -e "$d/run" ]; then
      prog=$(grep -A2 '<key>ProgramArguments</key>' "$3" | tail -1 | sed 's/.*<string>\(.*\)<\/string>.*/\1/')
      launch "$prog" daemon start --service >/dev/null 2>&1
      echo "$launched" > "$d/pid"
    fi; exit 0 ;;
  bootout) [ -e "$d/loaded" ] || exit 3; rm -f "$d/loaded"
    [ -e "$d/run" ] && kill "$(cat "$d/pid")" 2>/dev/null; exit 0 ;;
  print) [ -e "$d/inspectfail" ] && exit 1; [ -e "$d/loaded" ] || exit 113; echo "$2 = {"
    [ -e "$d/nopid" ] || echo "	pid = $(cat "$d/pid" 2>/dev/null || echo 4242)"; echo "}"; exit 0 ;;
  kickstart) [ -e "$d/loaded" ] || exit 113
    sleep 0.2; [ -S "$UMX_HOME/run/umx.sock" ] && echo "serving" >> "$d/argv"
    if [ -e "$d/run" ]; then
      prog=$(grep -A2 '<key>ProgramArguments</key>' "$HOME/Library/LaunchAgents/dev.umx.daemon.sandbox.plist" | tail -1 | sed 's/.*<string>\(.*\)<\/string>.*/\1/')
      # Like launchd, it returns while the old daemon still answers.
      # Starts nothing once the test's home is gone.
      launch /bin/sh -c '
        sleep 0.5; [ -e "$1/run" ] || exit 0; kill "$(cat "$1/pid")"; sleep 0.3; [ -e "$1/run" ] || exit 0
        "$2" daemon start --service & echo $! > "$1/pid"
      ' umx-fake-service "$d" "$prog" >/dev/null 2>&1
    fi; exit 0 ;;
esac
exit 64
"#;

const SYSTEMCTL: &str = r#"#!/bin/sh
d="$(dirname "$0")"
launch() {
  ready="$d/detached-$$"
  rm -f "$ready"
  /usr/bin/perl -MPOSIX -e 'POSIX::setsid() or die "setsid"; my $ready=shift @ARGV; open my $f, ">", $ready or die "ready"; close $f; exec @ARGV or die "exec"' "$ready" "$@" &
  launched=$!
  while [ ! -e "$ready" ]; do sleep 0.01; done
  rm -f "$ready"
}
echo "$*" >> "$d/argv"
[ -e "$d/fail" ] && exit 1
shift
case "$1" in
  daemon-reload) exit 0 ;;
  enable) touch "$d/enabled"; exit 0 ;;
  disable) rm -f "$d/enabled"; [ -e "$d/run" ] && kill "$(cat "$d/pid")" 2>/dev/null; exit 0 ;;
  restart) [ -e "$d/enabled" ] || exit 5
    sleep 0.2; [ -S "$UMX_HOME/run/umx.sock" ] && echo "serving" >> "$d/argv"
    if [ -e "$d/run" ]; then
      kill "$(cat "$d/pid" 2>/dev/null)" 2>/dev/null; sleep 0.3
      prog=$(sed -n 's/^ExecStart="\(.*\)" daemon.*/\1/p' "$HOME/.config/systemd/user/umx-sandbox.service")
      launch "$prog" daemon start --service >/dev/null 2>&1
      echo "$launched" > "$d/pid"
    fi; exit 0 ;;
  show) [ -e "$d/inspectfail" ] && exit 1
    if [ -e "$d/enabled" ]; then echo UnitFileState=enabled; else echo UnitFileState=disabled; fi
    if [ -e "$d/nopid" ] || [ ! -e "$d/enabled" ]; then printf 'ActiveState=inactive
MainPID=0
'
    else printf 'ActiveState=active
MainPID=%s
' "$(cat "$d/pid" 2>/dev/null || echo 4242)"; fi; exit 0 ;;
esac
exit 64
"#;

impl Home {
    fn stub_services(&self) {
        // UMX_HOME owner-only, as umx daemon install requires.
        fs::create_dir_all(&self.path).unwrap();
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700)).unwrap();
        let bin = self.path.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (name, text) in [("launchctl", LAUNCHCTL), ("systemctl", SYSTEMCTL)] {
            fs::write(bin.join(name), text).unwrap();
            fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Sets a stub state file (`nopid`, `fail`, `pid`), or removes the loaded state with `unload`.
    fn service_state(&self, what: &str, content: &str) {
        let bin = self.path.join("bin");
        if what == "unload" {
            let _ = fs::remove_file(bin.join("loaded"));
            let _ = fs::remove_file(bin.join("enabled"));
        } else {
            fs::write(bin.join(what), content).unwrap();
        }
    }

    fn argv(&self) -> String {
        fs::read_to_string(self.path.join("bin/argv")).unwrap_or_default()
    }

    /// The service file `umx daemon install` writes for this OS.
    fn unit_file(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.path
                .join("Library/LaunchAgents/dev.umx.daemon.sandbox.plist")
        } else {
            self.path.join(".config/systemd/user/umx-sandbox.service")
        }
    }

    /// Installs the service with the stubs and returns the umx binary it runs.
    fn install(&self) -> String {
        self.stub_services();
        let (env, exit) = self.umx(&["daemon", "install"]);
        assert_eq!(exit, 0, "{}", env.to_json());
        env.data["service"]["program"]
            .as_str()
            .unwrap_or_else(|| panic!("install did not report its program: {}", env.to_json()))
            .to_owned()
    }

    /// Rewrites the installed service file with `edit`.
    fn service_file_edit(&self, edit: impl Fn(&str) -> String) {
        let file = self.unit_file();
        let text = fs::read_to_string(&file).unwrap();
        let edited = edit(&text);
        assert_ne!(edited, text, "the edit changed nothing");
        fs::write(&file, edited).unwrap();
    }

    /// Makes the installed service run `program` instead, by editing its file.
    fn service_runs(&self, program: &str) {
        let file = self.unit_file();
        let text = fs::read_to_string(&file).unwrap();
        let this = fs::canonicalize(self.binary).unwrap();
        let this = this.to_str().unwrap();
        assert!(text.contains(this), "{text}");
        fs::write(&file, text.replace(this, program)).unwrap();
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The first issue of `code`; a missing one fails at the caller's line (#[track_caller]), so a plant's log names the
/// assertion it broke.
#[track_caller]
fn issue(env: &Envelope, code: Code) -> &Issue {
    let code = code.spec().code;
    match env
        .errors
        .iter()
        .chain(&env.warnings)
        .chain(&env.hints)
        .find(|i| i.code() == code)
    {
        Some(i) => i,
        None => panic!("no {code} in {}", env.to_json()),
    }
}

/// Sets `kind`'s default columns in `<path>/graph.json` by hand (F96), as an editor or a merge could.
fn plant_type_columns(path: &str, kind: &str, columns: &[&str]) {
    let file = Path::new(path).join("graph.json");
    let mut meta: Value = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
    let types = meta["ontology"]["types"].as_array_mut().unwrap();
    let t = types.iter_mut().find(|t| t["name"] == kind).unwrap();
    t["columns"] = json!(columns);
    fs::write(&file, meta.to_string()).unwrap();
}

/// The type `kind` as graph.json holds it (F96).
fn type_in_file(path: &str, kind: &str) -> Value {
    let file = Path::new(path).join("graph.json");
    let meta: Value = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
    let types = meta["ontology"]["types"].as_array().unwrap();
    types.iter().find(|t| t["name"] == kind).cloned().unwrap()
}

fn column_names(env: &Envelope) -> Vec<String> {
    let cols = env.data["columns"]
        .as_array()
        .unwrap_or_else(|| panic!("{}", env.to_json()));
    cols.iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect()
}

/// Replaces `<path>/<file>` with `bytes` as a git checkout does: a new file renamed over the old one.
fn checkout(path: &str, file: &str, bytes: &[u8]) {
    let (target, temp) = (
        Path::new(path).join(file),
        Path::new(path).join(format!("{file}.checkout")),
    );
    fs::write(&temp, bytes).unwrap();
    fs::rename(&temp, &target).unwrap();
}

/// The names in a graph folder that start with `.umx-tmp-` (temp files and the marker).
fn temp_files(path: &str) -> Vec<String> {
    fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".umx-tmp-"))
        .collect()
}

/// stdout parsed as one envelope line, or None when it is not JSON.
fn envelope_of(out: &std::process::Output) -> Option<Envelope> {
    let stdout = String::from_utf8_lossy(&out.stdout);
    (stdout.lines().count() == 1)
        .then(|| Envelope::from_json(stdout.as_bytes()).ok())
        .flatten()
}

/// Runs `args` with each output flag and UMX_OUTPUT combination; each prints one envelope or human text.
fn assert_output_modes(home: &Home, args: &[&str]) {
    for (flags, output, json) in [
        (&[][..], None, true),
        (&["--json"], None, true),
        (&["--human"], None, false),
        (&[], Some("human"), false),
        (&[], Some("json"), true),
        (&["--json"], Some("human"), true),
        (&["--human", "--json"], None, true),
        (&["--json", "--human"], None, false),
    ] {
        let mut cmd = home.command(&[args, flags].concat());
        if let Some(v) = output {
            cmd.env("UMX_OUTPUT", v);
        }
        let out = cmd.bounded_output().unwrap();
        let what = format!("{args:?} {flags:?} UMX_OUTPUT={output:?}");
        assert_eq!(out.status.code(), Some(0), "{what}");
        match envelope_of(&out) {
            Some(env) => {
                assert!(json, "{what}: JSON when text was asked for");
                assert_eq!(env.check(), Ok(()), "{what}");
            }
            None => {
                assert!(!json, "{what}: not one envelope");
                assert!(!out.stdout.is_empty(), "{what}: no text");
            }
        }
    }
}

/// The first row of the umx banner.
const BANNER_ROW: &str = "██    ██  ███▄  ▄███  ▀██▄  ▄██▀";

/// Runs umx with extra env and returns (stdout, stderr).
fn output_with(home: &Home, args: &[&str], env: &[(&str, &str)]) -> (String, String) {
    let mut cmd = home.command(args);
    cmd.envs(env.iter().copied());
    let out = cmd.bounded_output().unwrap();
    assert!(out.status.code().is_some(), "{args:?}");
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// Runs `umx <args>` on a real terminal (a pty from `script`) with extra env and returns what it printed.
fn on_a_pty(home: &Home, args: &[&str], env: &[(&str, &str)]) -> String {
    let umx = env!("CARGO_BIN_EXE_umx");
    let mut cmd = Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.args(["-q", "/dev/null", umx]).args(args);
    } else {
        cmd.args(["-qec", &format!("{umx} {}", args.join(" ")), "/dev/null"]);
    }
    sandbox(&mut cmd, &home.path);
    let out = cmd
        .env("UMX_HOME", &home.path)
        .env_remove("UMX_OUTPUT")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env("TERM", "xterm-256color")
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .bounded_output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

/// The JSON object in some output: from the first `{` to the last `}`.
fn json_part(s: &str) -> &str {
    let (start, end) = (s.find('{').unwrap(), s.rfind('}').unwrap());
    &s[start..=end]
}

/// Sends one raw request, as another build of the CLI would.
fn raw(socket: &Path, request: &Request) -> (Envelope, u8) {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.write_all(request.to_json().as_bytes()).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let env = Envelope::from_json(&bytes).unwrap();
    assert_eq!(env.check(), Ok(()));
    let exit = env.exit_code();
    (env, exit)
}

fn provenance_write(seat: &str, session: &str, title: &str) -> Request {
    let mut request = Request::op(
        umx_protocol::Op::NodeAdd {
            tags: Default::default(),
            on_key: None,
            related: None,
            raised_in: None,
            kind: "problem".into(),
            title: title.into(),
            status: None,
            note: None,
            sets: vec![],
            parent: None,
            evidence: vec![],
            evidence_kind: None,
            locator: None,
        },
        Some("demo".into()),
        Some("writer".into()),
        older_cli(),
    );
    request.actor = Some(umx_protocol::ActorClaim {
        tier: "pij".into(),
        harness: Some("copilot".into()),
        session: Some(session.into()),
        seat: Some(seat.into()),
        role: Some("coder".into()),
        model: Some("openai/test-model".into()),
        effort: None,
        user: Some("writer".into()),
    });
    request
}

/// A graph view's first wait for `graphs` (ws009 S6), as the view sends it (there is no CLI verb): no epoch yet, so
/// it answers at once with a reset that carries the daemon's epoch.
fn view_wait(timeout_ms: u64, graphs: &[&str]) -> Request {
    view_wait_at(None, timeout_ms, graphs)
}

/// A graph view's wait in `epoch` from seq 0: with no event yet, it sleeps until one comes or `timeout_ms` passes.
fn view_wait_at(epoch: Option<String>, timeout_ms: u64, graphs: &[&str]) -> Request {
    let op = umx_protocol::Op::UiWait {
        epoch,
        since: 0,
        timeout_ms,
        graphs: graphs.iter().map(|g| (*g).to_owned()).collect(),
        revs: Default::default(),
        supervisor: false,
    };
    Request::op(op, None, None, older_cli())
}

/// A graph view publishing the person's selection in `graph` (S7).
fn view_select(graph: &str, nodes: &[&str]) -> Request {
    let op = umx_protocol::Op::UiSelect {
        nodes: nodes.iter().map(|n| (*n).to_owned()).collect(),
    };
    Request::op(op, Some(graph.to_owned()), None, older_cli())
}

/// `<repo>/<rel>` as text.
fn in_repo(repo: &std::path::Path, rel: &str) -> String {
    repo.join(rel).display().to_string()
}

/// A canonical Graphfile listing graphs g0 to g<n> (n + 1 of them), none of which exists.
fn graphfile_of(n: usize) -> String {
    let mut out = String::from(
        "# Graphfile: the umx graphs in this repo. umx keeps it sorted; see `umx docs graphfile`.\ngraphfile = 1\n",
    );
    let mut names: Vec<String> = (0..=n).map(|i| format!("g{i}")).collect();
    names.sort();
    for name in names {
        let i: usize = name[1..].parse().unwrap();
        out += &format!(
            "\n[[graph]]\npath = \"{name}\"\nalias = \"{name}\"\nid = \"01JDCYPHER{i:016}\"\n"
        );
    }
    out
}

impl Home {
    /// Starts the daemon and makes `<home>/repo` a git repository (it holds `.git`); returns it (ws002 s17).
    fn git_repo(&mut self) -> PathBuf {
        self.start();
        let repo = self.path.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        repo
    }

    /// Starts the daemon and makes graph `demo` at `<home>/g`; returns the folder.
    fn graph(&mut self) -> String {
        self.start();
        let path = self.path.join("g").display().to_string();
        let (env, exit) = self.umx(&["graph", "new", &path, "--alias", "demo"]);
        assert_eq!(exit, 0, "{}", env.to_json());
        path
    }

    /// Starts the daemon and loads a copy of the Cypher design fixture (crates/umx-cypher/fixtures/design) as graph
    /// `design`, with nodes 01JDCYPHER...Q1 and so on (its README has the legend).
    fn design(&mut self) -> String {
        self.start();
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../umx-cypher/fixtures/design");
        let to = self.path.join("design");
        fs::create_dir_all(&to).unwrap();
        for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
            fs::copy(from.join(file), to.join(file)).unwrap();
        }
        let path = to.display().to_string();
        let (env, exit) = self.umx(&["graph", "load", &path]);
        assert_eq!(exit, 0, "{}", env.to_json());
        path
    }

    /// Loads another copy of the loaded design fixture (the same graph id) as `alias` (d-21).
    fn design_copy(&mut self, alias: &str) {
        let from = self.path.join("design");
        let to = self.path.join(alias);
        fs::create_dir_all(&to).unwrap();
        for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
            fs::copy(from.join(file), to.join(file)).unwrap();
        }
        let path = to.display().to_string();
        let (env, exit) = self.umx(&["graph", "load", &path, "--as", alias]);
        assert_eq!(exit, 0, "{}", env.to_json());
    }

    /// `umx query <text>` over the design fixture with extra flags.
    fn query(&self, text: &str, flags: &[&str]) -> (Envelope, u8) {
        let mut args = vec!["query", text];
        args.extend_from_slice(flags);
        self.umx(&args)
    }

    /// Starts the daemon and makes a secured graph `demo` at `<home>/g`.
    fn secured_graph(&mut self) -> String {
        self.start();
        let path = self.path.join("g").display().to_string();
        let (env, exit) = self.umx(&["graph", "new", &path, "--alias", "demo", "--secured"]);
        assert_eq!(exit, 0, "{}", env.to_json());
        assert_eq!(env.data["graph"]["effective_policy"], "secured");
        path
    }

    /// Writes this machine's policy record as umx would trust it (0600 in a 0700 folder).
    fn state_file(&self, bytes: &[u8]) -> PathBuf {
        let dir = self.path.join("state");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let file = dir.join("graph-policies.json");
        fs::write(&file, bytes).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        file
    }

    /// Writes this machine's graph registry as umx would trust it (0600 in a 0700 folder).
    fn registry_file(&self, bytes: &[u8]) -> PathBuf {
        let dir = self.path.join("registry");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let file = dir.join("graphs.toml");
        fs::write(&file, bytes).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        file
    }

    /// Writes a batch ops file in the home folder and returns its path.
    fn ops_file(&self, text: &str) -> String {
        fs::create_dir_all(&self.path).unwrap();
        let file = self.path.join(format!("ops-{}.jsonl", text.len()));
        fs::write(&file, text).unwrap();
        file.display().to_string()
    }

    /// `umx batch <args>` run for real, its stdout (the receipt exactly as printed) saved as `<name>.json` for a
    /// later `--refs-from`. Returns the file and the parsed receipt.
    fn receipt(&self, name: &str, args: &[&str]) -> (String, Envelope) {
        let out = self
            .command(args)
            .stdout(Stdio::piped())
            .bounded_output()
            .unwrap();
        let env = Envelope::from_json(&out.stdout).unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "umx {args:?}: {}",
            env.to_json()
        );
        let file = self.path.join(format!("{name}.json"));
        fs::write(&file, &out.stdout).unwrap();
        (file.display().to_string(), env)
    }

    /// Adds a node and returns its id.
    fn add(&self, kind: &str, title: &str) -> String {
        let (env, exit) = self.umx(&["node", "add", "--type", kind, "--title", title]);
        assert_eq!(exit, 0, "{}", env.to_json());
        env.data["node"]["id"].as_str().unwrap().to_owned()
    }

    /// UMX_HOME and its run folder made owner-only, as the daemon makes them, so doctor's mode checks pass.
    fn owner_only(&self) {
        fs::create_dir_all(&self.path).unwrap();
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700)).unwrap();
        self.run_dir(0o700);
    }

    /// A stand-in daemon on the socket: `daemon status` gets `status`, any other request `other`, or a closed
    /// connection when `other` is None.
    fn fake_daemon(&self, status: Envelope, other: Option<Envelope>) {
        self.owner_only();
        let listener = UnixListener::bind(self.socket()).unwrap();
        fs::set_permissions(self.socket(), fs::Permissions::from_mode(0o600)).unwrap();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut bytes = Vec::new();
                let _ = stream.read_to_end(&mut bytes);
                let reply = match Request::from_json(&bytes) {
                    Ok(r) if r.command == "daemon status" => Some(&status),
                    _ => other.as_ref(),
                };
                if let Some(reply) = reply {
                    let _ = stream.write_all(reply.to_json().as_bytes());
                }
            }
        });
    }

    /// What `daemon status` answers for a healthy daemon of this CLI's build.
    fn healthy_status(&self) -> Envelope {
        let build = self.umx(&["version"]).0.data["cli"].clone();
        Envelope::ok(
            "daemon status",
            json!({ "running": true, "writer": "ok", "pid": 1, "version": build }),
        )
    }
}

impl Home {
    /// Writes `$UMX_HOME/config.toml` by hand (0644, which umx trusts) and returns its path.
    fn write_config(&self, text: &str) -> PathBuf {
        fs::create_dir_all(&self.path).unwrap();
        let file = self.path.join("config.toml");
        fs::write(&file, text).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        file
    }

    /// Makes `<home>/repo` a git repository (inside the roots) and runs umx from it.
    fn repo(&mut self) -> PathBuf {
        let repo = self.path.join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        self.cwd = Some(repo.clone());
        repo
    }

    /// Runs umx and fails unless it exits 0.
    fn ok(&self, args: &[&str]) -> Envelope {
        let (env, exit) = self.umx(args);
        assert_eq!(exit, 0, "umx {args:?}: {}", env.to_json());
        env
    }

    fn restart(&mut self) -> Envelope {
        self.ok(&["daemon", "stop"]);
        if let Some(mut child) = self.daemon.take() {
            child.bounded_wait().unwrap();
        }
        self.start()
    }

    /// Defines a complete local embedder instance `nomic`.
    fn nomic(&self) {
        for (k, v) in [
            ("kind", "local"),
            ("model", "nomic-embed-text-v1.5"),
            ("dims", "256"),
        ] {
            self.ok(&["config", "set", &format!("embedders.nomic.{k}"), v]);
        }
    }

    /// Writes a machine config whose active embedder is the azure_openai instance `az` (auth cli unless `extra` says
    /// otherwise), and points PATH at `<home>/bin`, which holds no az or azd unless a test puts one there. The
    /// endpoint is a closed local port, so nothing ever leaves the machine. Returns the bin folder.
    fn az_active(&mut self, extra: &str) -> PathBuf {
        self.write_config(&format!(
            "embedder.active = \"az\"\n\
             embedders.az.kind = \"azure_openai\"\n\
             embedders.az.endpoint = \"https://127.0.0.1:9\"\n\
             embedders.az.deployment = \"emb\"\n\
             embedders.az.model = \"text-embedding-3-small\"\n\
             embedders.az.dims = 256\n{extra}"
        ));
        let bin = self.path.join("bin");
        fs::create_dir_all(&bin).unwrap();
        self.env.push(("PATH".into(), bin.display().to_string()));
        bin
    }

    /// Defines a complete azure_openai instance `az`.
    fn az(&self) {
        for (k, v) in [
            ("kind", "azure_openai"),
            ("endpoint", "https://example.openai.azure.com"),
            ("deployment", "emb"),
            ("model", "text-embedding-3-small"),
            ("dims", "256"),
        ] {
            self.ok(&["config", "set", &format!("embedders.az.{k}"), v]);
        }
    }
}

/// A fake AWS-style access key, built at run time so no key-shaped literal sits in the source.
fn fake_aws_key() -> String {
    format!("uses AKIA{}", "Q7Z3X9K2Q7Z3X9K2")
}

/// Doctor over a local instance told to run on the CPU with `threads` threads. HOME is the test's, so its models
/// folder is empty and nothing loads; doctor goes by the CPU estimate (0.9 texts a second a thread, floor 2).
fn local_on_cpu(home: &mut Home, threads: &str) -> (Envelope, u8) {
    home.start();
    for (k, v) in [
        ("kind", "local"),
        ("model", "embeddinggemma-2"),
        ("dims", "256"),
        ("execution", "cpu"),
        ("threads", threads),
    ] {
        home.ok(&["config", "set", &format!("embedders.gemma.{k}"), v]);
    }
    home.ok(&["config", "set", "embedder.active", "gemma"]);
    let fake_home = home.path.display().to_string();
    home.umx_with(&["doctor"], &[("HOME", &fake_home)])
}

/// A design-fixture query (and flags) that raises `code`.
fn query_trigger(code: Code) -> (String, &'static [&'static str]) {
    let d1 = "01JDCYPHER00000000000000D1";
    let text = match code {
        Code::QuerySyntax => "MATCH (n) WHERE n.status != 'open' RETURN n.id AS id".into(),
        Code::QueryWriteNotSupported => "CREATE (n:question)".into(),
        Code::QueryClauseNotSupported => "CALL db.labels()".into(),
        Code::QueryFeatureNotSupported => "MATCH p = (a)-[:blocks]->(b) RETURN b.id AS id".into(),
        Code::QueryAmbiguousLabels => "MATCH (n:question:criterion) RETURN n.id AS id".into(),
        Code::QueryStdQualifier => "MATCH (n:std:person) RETURN n.id AS id".into(),
        Code::QueryUnknownFunction => "MATCH (n) RETURN labels(n) AS l".into(),
        Code::QueryAliasShadowsVariable => "MATCH (q:question) RETURN q.id AS q".into(),
        Code::QueryOrderByEntity => "MATCH (n:question) RETURN n ORDER BY n".into(),
        Code::QueryWithOrderNeedsLimit => "MATCH (b) WITH b ORDER BY b.id RETURN b.id AS id".into(),
        Code::QueryUnbounded => "MATCH (a)-[:blocks*]->(t) RETURN t.id AS id".into(),
        Code::QueryBoundTooLarge => "MATCH (a)-[:blocks*1..20]->(t) RETURN t.id AS id".into(),
        Code::QuerySymmetricDirection => {
            "MATCH (a:decision)-[:conflicts_with]->(b) RETURN a.id AS a_id".into()
        }
        // 1,100 x 1,100 rows: past the cap of 1,000,000 held at once.
        Code::QueryTooLarge => {
            let list = (0..1_100)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!("UNWIND [{list}] AS x UNWIND [{list}] AS y RETURN count(*) AS n")
        }
        Code::QueryInvalid => "UNWIND 1 AS x RETURN x AS y".into(),
        Code::QueryParamMissing => "MATCH (n {id: $id}) RETURN n.id AS id".into(),
        Code::QueryUnknownVariable => "MATCH (a:question) RETURN nope.id AS x".into(),
        Code::QueryColumnReserved => "MATCH (n) RETURN n.title AS secret_suspected".into(),
        Code::QueryTruncated => {
            return ("MATCH (n) RETURN n.id AS id".into(), &["--limit", "2"]);
        }
        Code::QueryUnknownLabel => "MATCH (n:nope_type) RETURN n.id AS id".into(),
        Code::ProvenanceHidden => "MATCH (n:session) RETURN n.id AS id".into(),
        Code::QueryUnknownRelation => "MATCH (a)-[:nope]->(b) RETURN a.id AS id".into(),
        Code::QueryUnknownProperty => "MATCH (n) RETURN n.nope AS x".into(),
        Code::QuerySymmetricInDirectedStep => {
            format!("MATCH (a {{id: '{d1}'}})-->(b) RETURN b.id AS id")
        }
        other => unreachable!("{other:?} is not a query code"),
    };
    (text, &[])
}

/// The id of the edge an `edge add`, `edge delete` or `edge edit` reply carries.
fn edge_id(env: &Envelope) -> String {
    env.data["edge"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no edge in {}", env.to_json()))
        .to_owned()
}

/// Adds an edge to graph `demo` at `path`, rewrites its line in edges.jsonl with `f` (a hand edit) and loads the
/// graph again; gives the edge's id.
/// A node whose line in nodes.jsonl `f` changes by hand, then the graph reloaded; returns its id.
fn rewritten_node(home: &mut Home, path: &str, f: impl Fn(&mut Value)) -> String {
    let id = home.add("problem", "Hand edited");
    let nodes = Path::new(path).join("nodes.jsonl");
    let text: String = fs::read_to_string(&nodes)
        .unwrap()
        .lines()
        .map(|text| {
            let mut line: Value = umx_protocol::parse_json(text).unwrap();
            if line["id"] == json!(id) {
                f(&mut line);
            }
            format!("{line}\n")
        })
        .collect();
    fs::write(&nodes, text).unwrap();
    let (env, exit) = home.umx(&["graph", "load", path]);
    assert_eq!(exit, 0, "{}", env.to_json());
    id
}

fn rewritten_edge(home: &mut Home, path: &str, f: impl Fn(&mut Value)) -> String {
    let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
    let id = edge_id(&home.umx(&["edge", "add", &a, "blocks", &b]).0);
    let edges = Path::new(path).join("edges.jsonl");
    let text: String = fs::read_to_string(&edges)
        .unwrap()
        .lines()
        .map(|text| {
            let mut line: Value = umx_protocol::parse_json(text).unwrap();
            if line["id"] == json!(id) {
                f(&mut line);
            }
            format!("{line}\n")
        })
        .collect();
    fs::write(&edges, text).unwrap();
    let (env, exit) = home.umx(&["graph", "load", path]);
    assert_eq!(exit, 0, "{}", env.to_json());
    id
}

/// Adds an edge to graph `demo` at `path`, then writes an earlier version of its line (relation `supports`) above
/// it in edges.jsonl, as a git merge that kept both sides does, and loads the graph again through the real file
/// loader; gives the edge's id. The later line is the one in use; the earlier one is kept.
fn repeated_edge(home: &mut Home, path: &str) -> String {
    let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
    let id = edge_id(&home.umx(&["edge", "add", &a, "blocks", &b]).0);
    let edges = Path::new(path).join("edges.jsonl");
    let text: String = fs::read_to_string(&edges)
        .unwrap()
        .lines()
        .map(|text| {
            let line: Value = umx_protocol::parse_json(text).unwrap();
            if line["id"] == json!(id) {
                let mut old = line.clone();
                old["type"] = json!("supports");
                format!("{old}\n{line}\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    fs::write(&edges, text).unwrap();
    let (env, exit) = home.umx(&["graph", "load", path]);
    assert_eq!(exit, 0, "{}", env.to_json());
    id
}

/// Usage 220: a node citing `notes.md` in the graph folder, hashed when cited.
fn source_cited(home: &Home, path: &str) {
    fs::write(Path::new(path).join("notes.md"), "first\n").unwrap();
    home.ok(&[
        "node",
        "add",
        "--type",
        "problem",
        "--title",
        "Cited",
        "--evidence",
        "notes.md#L1",
    ]);
}

/// Every catalogue code, raised for real. A new code without a trigger here does not compile.
/// Each call gets its own home: tests that raise the same code run in parallel, and `Home::new` empties its folder.
fn trigger(code: Code) -> (Envelope, u8) {
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut home = Home::new(&format!("{}-{call}", code.spec().code));
    match code {
        Code::FieldValueOffVocabulary => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "Risk rating",
                "--set",
                "impact=severe",
            ])
        }
        Code::FieldValuesInvalid => {
            home.graph();
            home.umx(&[
                "ontology",
                "add",
                "field",
                "task",
                "priority:string",
                "--values",
                "Very High",
            ])
        }
        Code::ActorInvalid => {
            let path = home.graph();
            let result = raw(
                &home.socket(),
                &provenance_write("pij-alpha-seat", "RawSession_01", "Invalid capture"),
            );
            assert_eq!(result.0.data["actor_tier"], "unknown");
            assert!(!result.0.to_json().contains("RawSession_01"));
            for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
                assert!(
                    !fs::read_to_string(Path::new(&path).join(file))
                        .unwrap()
                        .contains("RawSession_01")
                );
            }
            result
        }
        Code::ReservedType => {
            home.graph();
            home.umx(&["node", "add", "--type", "agent", "--title", "User agent"])
        }
        Code::ReservedRelation | Code::DerivedRelation => {
            home.graph();
            let a = home.add("problem", "First");
            let b = home.add("problem", "Second");
            let relation = if code == Code::ReservedRelation {
                "session_of"
            } else {
                "authored_by"
            };
            home.umx(&["edge", "add", &a, relation, &b])
        }
        Code::StdFieldConflict => {
            home.graph();
            home.umx(&["ontology", "add", "field", "person", "job_title:int"])
        }
        Code::AdoptFieldKindClash
        | Code::AdoptReservedRelation
        | Code::AdoptMapUnknown
        | Code::AdoptMapAmbiguous
        | Code::AdoptMapMerge
        | Code::AdoptMapSymmetry
        | Code::AdoptMapDuplicateEdge
        | Code::AdoptValueKind
        | Code::StdFieldConflictStored
        | Code::AlreadyStd
        | Code::AdoptRenameSuggested
        | Code::AdoptFieldToTagRefused
        | Code::AdoptTagValueInvalid => adopt_std::adopt_trigger(&mut home, code),
        Code::TagInvalid
        | Code::TagLimit
        | Code::TagNeedsStd
        | Code::TagOffVocabulary
        | Code::TagNotCanonical
        | Code::TagUnusable
        | Code::TagFilterNotStd => tags::tags_trigger(&mut home, code),
        Code::TagFilterUntested => tag_reads::untested_trigger(&mut home),
        Code::EntitiesGrouped => entity_neighbours::grouped_trigger(&mut home),
        Code::ReservedRelationStored => {
            let path = home.graph();
            let a = home.add("problem", "First");
            let b = home.add("problem", "Second");
            let edge = home.ok(&["edge", "add", &a, "blocks", &b]);
            let id = edge.data["edge"]["id"].as_str().unwrap().to_owned();
            home.ok(&["graph", "unload", "demo"]);
            let file = Path::new(&path).join("edges.jsonl");
            let text = fs::read_to_string(&file).unwrap();
            fs::write(&file, text.replace("\"blocks\"", "\"authored_by\"")).unwrap();
            let loaded = home.umx(&["graph", "load", &path]);
            assert!(loaded.0.to_json().contains(&id), "{}", loaded.0.to_json());
            loaded
        }
        Code::SessionAgentConflict => {
            home.graph();
            let first = provenance_write("pij-alpha-seat", "h-0123456789abcdef", "First claim");
            let (made, exit) = raw(&home.socket(), &first);
            assert_eq!(exit, 0, "{}", made.to_json());
            raw(
                &home.socket(),
                &provenance_write("pij-beta-seat", "h-0123456789abcdef", "Another claim"),
            )
        }
        Code::ProvenanceKeyInvalid | Code::DuplicateProvKey => {
            let path = home.graph();
            let id = home.add("problem", "Content");
            let file = Path::new(&path).join("nodes.jsonl");
            let mut records: Vec<Value> = fs::read_to_string(&file)
                .unwrap()
                .lines()
                .map(|line| umx_protocol::parse_json(line).unwrap())
                .collect();
            if code == Code::ProvenanceKeyInvalid {
                let record = records.iter_mut().find(|n| n["id"] == id).unwrap();
                record["edited_by_session"] = json!(7);
            } else {
                let mut duplicate = records
                    .iter()
                    .find(|n| n["type"] == "agent" && n.get("prov_key").is_some())
                    .unwrap()
                    .clone();
                duplicate["id"] = json!("7ZZZZZZZZZZZZZZZZZZZZZZZZZ");
                records.push(duplicate);
            }
            fs::write(
                &file,
                records
                    .into_iter()
                    .map(|n| format!("{n}\n"))
                    .collect::<String>(),
            )
            .unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::ScopeFormatNewer => {
            home.start();
            let file = home.path.join("scopes.toml");
            fs::write(&file, "umx_scopes = 2\nfuture = true\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            home.umx(&["scope", "list"])
        }
        Code::ScopeMemberMoved => {
            home.design();
            home.design_copy("design_b");
            home.umx(&["scope", "save", "both", "--graph", "design"]);
            home.umx(&["graph", "unload", "design"]);
            home.umx(&["node", "list", "--scope", "both"])
        }
        Code::ScopeUnknown => {
            home.start();
            home.umx(&["scope", "show", "absent"])
        }
        Code::ScopeExists | Code::ScopeGraphNotLoaded | Code::ScopeStoreNotSaved => {
            home.graph();
            home.umx(&["scope", "save", "both", "--graph", "demo"]);
            if code == Code::ScopeGraphNotLoaded {
                home.umx(&["graph", "unload", "demo"]);
                home.umx(&["node", "list", "--scope", "both"])
            } else if code == Code::ScopeStoreNotSaved {
                fs::set_permissions(&home.path, fs::Permissions::from_mode(0o500)).unwrap();
                let result = home.umx(&["scope", "delete", "both"]);
                fs::set_permissions(&home.path, fs::Permissions::from_mode(0o700)).unwrap();
                result
            } else {
                home.umx(&["scope", "save", "both", "--graph", "demo"])
            }
        }
        Code::ScopeIncludesSecured => {
            home.secured_graph();
            home.umx(&["scope", "save", "vault", "--graph", "demo"])
        }
        Code::CopyDropped => {
            home.design();
            home.design_copy("design_b");
            home.umx(&["scope", "save", "both", "--graph", "design_b"])
        }
        Code::ScopeStoreInvalid | Code::ScopeLimit => {
            home.start();
            let file = home.path.join("scopes.toml");
            let bytes = if code == Code::ScopeLimit {
                vec![b' '; 60 * 1024 + 1]
            } else {
                b"invalid toml".to_vec()
            };
            fs::write(&file, bytes).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            home.umx(&["scope", "list"])
        }
        Code::GraphNoneLoaded => {
            home.start();
            home.umx(&["node", "list"])
        }
        Code::GraphAmbiguous => {
            home.graph();
            let other = home.path.join("h").display().to_string();
            home.umx(&["graph", "new", &other, "--alias", "other"]);
            home.umx(&["node", "list"])
        }
        Code::GraphMismatch => {
            home.graph();
            home.umx(&["node", "get", "--graph", "demo", "umx:other/01ABCDEF"])
        }
        Code::GraphNotLoaded => {
            home.graph();
            home.umx(&["node", "list", "--graph", "nope"])
        }
        Code::NodeIdIsGraphId => {
            home.start();
            let target = home.path.join("collision");
            fs::create_dir(&target).unwrap();
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../umx-services/tests/fixtures/node-id-graph");
            for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
                fs::copy(fixture.join(file), target.join(file)).unwrap();
            }
            home.umx(&["graph", "load", target.to_str().unwrap()])
        }
        Code::ExternalEdgesSkipped => {
            home.graph();
            let a = home.add("problem", "Author");
            home.ok(&[
                "edge",
                "add",
                &a,
                "blocks",
                "umx:00000000000000000000000009/00000000000000000000000008",
                "--graph",
                "demo",
            ]);
            home.umx(&[
                "query",
                &format!("MATCH (a {{id: '{a}'}})-[]->(b) RETURN b.id AS id"),
            ])
        }
        Code::RegistryUnreadable | Code::DoctorRegistryUnreadable => {
            home.registry_file(b"umx_registry = 9\n");
            home.start();
            if code == Code::DoctorRegistryUnreadable {
                home.umx(&["doctor"])
            } else {
                let path = home.path.join("g").display().to_string();
                home.umx(&["graph", "new", &path, "--alias", "demo"])
            }
        }
        Code::GraphIdChanged => {
            let path = home.graph();
            let other = home.path.join("other").display().to_string();
            home.umx(&["graph", "new", &other, "--alias", "other"]);
            home.stop();
            for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
                fs::copy(format!("{other}/{file}"), format!("{path}/{file}")).unwrap();
            }
            home.start();
            home.umx(&["node", "list", "--graph", "demo"])
        }
        Code::GraphLoadedOnDemand => {
            home.start_with(&["--idle-unload", "1s"]);
            let path = home.path.join("g").display().to_string();
            home.umx(&["graph", "new", &path, "--alias", "demo"]);
            home.wait_until_idle();
            home.umx(&["node", "list", "--graph", "demo"])
        }
        Code::RegistryNotSaved => {
            let path = home.graph();
            let dir = home.path.join("registry");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
            let result = home.umx(&["graph", "load", &path]);
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::DoctorGraphNotReloaded => {
            let path = home.graph();
            home.ok(&["daemon", "stop"]);
            if let Some(mut child) = home.daemon.take() {
                child.bounded_wait().unwrap();
            }
            fs::remove_dir_all(&path).unwrap();
            home.start();
            home.umx(&["doctor"])
        }
        Code::ExternalEndUnchecked => {
            home.graph();
            let a = home.add("problem", "Author");
            home.umx(&[
                "edge",
                "add",
                &a,
                "blocks",
                "umx:00000000000000000000000090/00000000000000000000000091",
                "--graph",
                "demo",
            ])
        }
        Code::LinkTargetsUnnamed => {
            let (_, _, _, _) = home.linked_graphs();
            home.umx(&["check", "--graph", "demo"])
        }
        Code::LinkIntoSecured | Code::EdgeBroken => {
            home.graph();
            let a = home.add("problem", "Author");
            let beta = home.path.join("beta").display().to_string();
            home.ok(&["graph", "new", &beta, "--alias", "beta"]);
            let b = home
                .ok(&[
                    "node", "add", "--type", "problem", "--title", "Far node", "--graph", "beta",
                ])
                .data["node"]["id"]
                .as_str()
                .unwrap()
                .to_string();
            if code == Code::LinkIntoSecured {
                home.ok(&["graph", "set", "beta", "--policy", "secured"]);
            }
            let address = format!("umx:beta/{b}");
            let result = home.umx(&["edge", "add", &a, "blocks", &address, "--graph", "demo"]);
            if code == Code::LinkIntoSecured {
                result
            } else {
                fs::write(Path::new(&beta).join("nodes.jsonl"), b"").unwrap();
                home.ok(&["graph", "load", &beta]);
                home.umx(&["check", "--graph", "demo"])
            }
        }
        Code::OwnGraphEnd => {
            home.start();
            let target = home.path.join("own");
            fs::create_dir(&target).unwrap();
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../umx-services/tests/fixtures/own-graph-end");
            for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
                fs::copy(fixture.join(file), target.join(file)).unwrap();
            }
            home.umx(&["graph", "load", target.to_str().unwrap()])
        }
        Code::GraphForgetLoaded => {
            home.graph();
            home.umx(&["graph", "forget", "demo"])
        }
        Code::GraphForgetNotSaved => {
            home.graph();
            home.ok(&["graph", "unload", "demo"]);
            let dir = home.path.join("registry");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
            let result = home.umx(&["graph", "forget", "demo"]);
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::RegistryUpdateNotSaved => {
            home.design();
            home.design_copy("design_b");
            home.design_copy("design_c");
            let dir = home.path.join("registry");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
            let result = home.umx(&["graph", "unload", "design"]);
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::GraphNotKnown => {
            home.start();
            home.umx(&["graph", "forget", "nope"])
        }
        Code::AliasRedirected => {
            home.graph();
            home.ok(&["graph", "rename", "demo", "renamed"]);
            home.umx(&["node", "list", "--graph", "demo"])
        }
        Code::FormerAliasAmbiguous => {
            home.start();
            for (folder, live) in [("one", "first"), ("two", "second")] {
                let path = home.path.join(folder).display().to_string();
                home.ok(&["graph", "new", &path, "--alias", "shared"]);
                home.ok(&["graph", "rename", "shared", live]);
            }
            home.umx(&["node", "list", "--graph", "shared"])
        }
        Code::GraphMoveRefused => {
            let path = home.graph();
            let taken = home.path.join("taken");
            fs::create_dir_all(&taken).unwrap();
            let (env, exit) = home.umx(&["graph", "move", "demo", taken.to_str().unwrap()]);
            assert!(
                Path::new(&path).join("graph.json").exists(),
                "nothing moved"
            );
            (env, exit)
        }
        Code::GraphMoveCrossRepo => {
            let repo = home.git_repo();
            home.ok(&["graph", "new", &in_repo(&repo, "a"), "--alias", "a"]);
            let other = home.path.join("other");
            fs::create_dir_all(other.join(".git")).unwrap();
            home.umx(&["graph", "move", "a", other.join("a").to_str().unwrap()])
        }
        Code::EvidenceRootStale => {
            let repo = home.git_repo();
            home.ok(&["graph", "new", &in_repo(&repo, "a"), "--alias", "a"]);
            let n = home.add("problem", "Cited");
            home.ok(&["node", "evidence", &n, "add", "root:repo/a/notes.md#L2"]);
            home.ok(&["graph", "move", "a", &in_repo(&repo, "b")]);
            home.umx(&["check", "--graph", "a"])
        }
        Code::GraphfileNotUpdated => {
            let repo = home.git_repo();
            fs::write(repo.join("Graphfile"), graphfile_of(0)).unwrap();
            home.ok(&["graph", "new", &in_repo(&repo, "a"), "--alias", "a"]);
            home.umx(&["graph", "rename", "a", "g0"])
        }
        Code::AliasAmbiguous => {
            home.start();
            for folder in ["one", "two"] {
                let path = home.path.join(folder).display().to_string();
                home.ok(&["graph", "new", &path, "--alias", "demo"]);
                home.ok(&["graph", "unload", "demo"]);
            }
            home.umx(&["graph", "load", "demo"])
        }
        Code::PrimaryRestored => {
            home.design();
            home.design_copy("design_b");
            home.design_copy("design_c");
            home.ok(&["graph", "unload", "design"]);
            home.stop();
            fs::rename(home.path.join("design_c"), home.path.join("moved")).unwrap();
            home.start();
            home.umx(&["daemon", "status"])
        }
        Code::GraphNotFound => {
            home.start();
            let none = home.path.join("none").display().to_string();
            home.umx(&["graph", "load", &none])
        }
        Code::GraphExists => {
            let path = home.graph();
            home.umx(&["graph", "new", &path, "--alias", "again"])
        }
        Code::GraphfileSkippedSecured => {
            let repo = home.git_repo();
            home.umx(&[
                "graph",
                "new",
                &in_repo(&repo, "s"),
                "--alias",
                "s",
                "--secured",
            ])
        }
        Code::GraphfileNoRepo => {
            home.start();
            let loose = home.path.join("loose").display().to_string();
            home.umx(&["graph", "new", &loose, "--alias", "loose", "--graphfile"])
        }
        Code::GraphfileNotCanonical => {
            let repo = home.git_repo();
            fs::write(repo.join("Graphfile"), "# ours\ngraphfile = 1\n").unwrap();
            home.umx(&["graph", "new", &in_repo(&repo, "a"), "--alias", "a"])
        }
        Code::GraphfileTooLarge => {
            let repo = home.git_repo();
            // Exactly the 1,000-graph cap: the file reads, one more cannot be written.
            fs::write(repo.join("Graphfile"), graphfile_of(999)).unwrap();
            home.umx(&["graph", "new", &in_repo(&repo, "a"), "--alias", "a"])
        }
        Code::GraphfileUnusable => {
            let repo = home.git_repo();
            fs::write(repo.join("Graphfile"), "graphfile = 1\nextra = 1\n").unwrap();
            home.umx(&["graph", "new", &in_repo(&repo, "a"), "--alias", "a"])
        }
        Code::GraphfileIdMismatch => {
            let repo = home.git_repo();
            home.ok(&[
                "graph",
                "new",
                &in_repo(&repo, "a"),
                "--alias",
                "a",
                "--no-graphfile",
            ]);
            home.ok(&["graph", "unload", "a"]);
            fs::write(repo.join("Graphfile"), graphfile_of(0).replace("g0", "a")).unwrap();
            home.umx(&["graph", "load", "--graphfile", repo.to_str().unwrap()])
        }
        Code::GraphfileEntrySkipped => {
            let repo = home.git_repo();
            fs::write(repo.join("Graphfile"), graphfile_of(0)).unwrap();
            home.umx(&["graph", "load", "--graphfile", repo.to_str().unwrap()])
        }
        Code::GraphfileInvalid => {
            let repo = home.git_repo();
            let twice = graphfile_of(1).replace("path = \"g1\"", "path = \"g0\"");
            fs::write(repo.join("Graphfile"), twice).unwrap();
            home.umx(&["graph", "list", "--graphfile", repo.to_str().unwrap()])
        }
        Code::GraphfileNewer => {
            let repo = home.git_repo();
            fs::write(repo.join("Graphfile"), "graphfile = 2\n").unwrap();
            home.umx(&["graph", "list", "--graphfile", repo.to_str().unwrap()])
        }
        Code::GraphfileMissing => {
            let repo = home.git_repo();
            home.umx(&["graph", "list", "--graphfile", repo.to_str().unwrap()])
        }
        Code::GraphfileRepoNone => {
            home.start();
            let here = home.path.display().to_string();
            home.umx(&["graph", "list", "--graphfile", &here])
        }
        Code::GraphfileUnknown => {
            let repo = home.git_repo();
            home.ok(&["graph", "new", &in_repo(&repo, "a"), "--alias", "a"]);
            home.umx(&["graph", "scan", repo.to_str().unwrap(), "--drop", "nope"])
        }
        Code::GraphfileLoadTooMany => {
            let repo = home.git_repo();
            fs::write(repo.join("Graphfile"), graphfile_of(256)).unwrap();
            home.umx(&["graph", "load", "--graphfile", repo.to_str().unwrap()])
        }
        Code::GraphOrphanRecords => {
            home.start();
            let orphan = home.path.join("orphan");
            fs::create_dir_all(&orphan).unwrap();
            fs::write(orphan.join("nodes.jsonl"), "{\"id\":\"x\"}\n").unwrap();
            home.umx(&[
                "graph",
                "new",
                orphan.to_str().unwrap(),
                "--alias",
                "orphan",
            ])
        }
        Code::GraphInvalid => {
            home.start();
            let bad = home.path.join("bad");
            fs::create_dir_all(&bad).unwrap();
            fs::write(bad.join("graph.json"), "{").unwrap();
            home.umx(&["graph", "load", bad.to_str().unwrap()])
        }
        Code::GraphFormatNewer => {
            let path = home.graph();
            let file = Path::new(&path).join("graph.json");
            let mut meta: umx_protocol::Value =
                umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
            // A new graph is written as format 1, and this umx reads up to 2: 1000 is newer than any.
            meta["umx"] = json!(1000);
            fs::write(&file, meta.to_string()).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::GraphTooLarge => {
            // A graph.json padded past its 1 MiB cap (as a pull could bring): refused unread.
            let path = home.graph();
            let file = Path::new(&path).join("graph.json");
            let mut text = fs::read_to_string(&file).unwrap();
            text.push_str(&" ".repeat(1 << 20));
            fs::write(&file, text).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::UnsortedFile => {
            let path = home.graph();
            home.add("problem", "One");
            home.add("problem", "Two");
            let file = Path::new(&path).join("nodes.jsonl");
            let text = fs::read_to_string(&file).unwrap();
            let lines: Vec<&str> = text.lines().rev().collect();
            fs::write(&file, format!("{}\n", lines.join("\n"))).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::DuplicateId => {
            let path = home.graph();
            home.add("problem", "x");
            let file = Path::new(&path).join("nodes.jsonl");
            let line = fs::read_to_string(&file).unwrap();
            fs::write(&file, format!("{line}{}", line.replace("\"x\"", "\"y\""))).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::LoadWarningsNotShown => {
            let path = home.graph();
            home.add("problem", "x");
            let file = Path::new(&path).join("nodes.jsonl");
            let line = fs::read_to_string(&file).unwrap();
            let id = &line[7..33];
            let lines: String = (1..=21)
                .map(|i| line.replace(id, &format!("{i:026}")))
                .collect();
            fs::write(&file, lines.repeat(2)).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::DanglingEdge => {
            let path = home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "blocks", &b]);
            let nodes = Path::new(&path).join("nodes.jsonl");
            let kept: String = fs::read_to_string(&nodes)
                .unwrap()
                .lines()
                .filter(|l| !l.contains(&b))
                .map(|l| format!("{l}\n"))
                .collect();
            fs::write(&nodes, kept).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::SymmetricRefused => {
            home.graph();
            home.umx(&["ontology", "add", "relation", "part_of", "--symmetric"])
        }
        // ws006 S3b: resource nodes through the verbs.
        Code::ResourceExists
        | Code::ResourceElsewhere
        | Code::LocatorInUse
        | Code::LocatorChanged
        | Code::NotAResource
        | Code::LocatorInvalid
        | Code::LocatorAbsolute => {
            let path = home.graph();
            let resource = |locator: &str| {
                let env = home.ok(&["node", "add", "--type", "document", "--locator", locator]);
                env.data["node"]["id"].as_str().unwrap().to_owned()
            };
            let add = |locator: &str| {
                home.umx(&["node", "add", "--type", "document", "--locator", locator])
            };
            match code {
                Code::ResourceExists => {
                    resource("docs/brief.md");
                    add("./docs//brief.md")
                }
                Code::ResourceElsewhere => {
                    resource("https://example.com/brief");
                    let beta = Path::new(&path)
                        .with_file_name("beta")
                        .display()
                        .to_string();
                    home.ok(&["graph", "new", &beta, "--alias", "beta"]);
                    home.umx(&[
                        "node",
                        "add",
                        "--type",
                        "document",
                        "--locator",
                        "https://Example.com/brief",
                        "--graph",
                        "beta",
                    ])
                }
                Code::LocatorInUse => {
                    resource("docs/brief.md");
                    let other = resource("docs/other.md");
                    home.umx(&["node", "edit", &other, "--locator", "docs/brief.md"])
                }
                Code::LocatorChanged => {
                    let brief = resource("docs/brief.md");
                    home.umx(&["node", "edit", &brief, "--locator", "docs/brief-v2.md"])
                }
                Code::NotAResource => {
                    let plain = home.add("problem", "Plain");
                    home.umx(&["resource", "show", &plain])
                }
                Code::LocatorInvalid => add("docs/../../outside.md"),
                _ => add("/etc/hosts"),
            }
        }
        Code::LocatorFieldInvalid => {
            home.graph();
            let a = home.add("problem", "Plain");
            home.umx(&["node", "edit", &a, "--set", "locator=docs/brief.md"])
        }
        Code::DuplicateLocator | Code::LocatorNotNormalised | Code::LocatorFieldKept => {
            // ws006 S3a: resource lines a merge or hand edit left, and a custom field named locator from before the
            // name was reserved; no verb writes a locator until S3b.
            let path = home.graph();
            let (a, b) = (home.add("problem", "Ours"), home.add("problem", "Theirs"));
            let nodes = Path::new(&path).join("nodes.jsonl");
            let text = fs::read_to_string(&nodes).unwrap();
            let lines: Vec<String> = text
                .lines()
                .map(|l| {
                    let mut v: umx_protocol::Value = umx_protocol::parse_json(l).unwrap();
                    let id = v["id"].as_str().unwrap().to_owned();
                    let source = match (code, id == a, id == b) {
                        (Code::DuplicateLocator, true, _) | (Code::DuplicateLocator, _, true) => {
                            Some("docs/brief.md")
                        }
                        (Code::LocatorFieldKept, true, _) => Some(""),
                        (_, true, _) => Some("./docs//brief.md"),
                        _ => None,
                    };
                    match source {
                        Some("") => v["locator"] = json!("an old custom value"),
                        Some(source) => {
                            v["locator"] =
                                json!({ "kind": "file", "format": "markdown", "source": source });
                        }
                        None => {}
                    }
                    v.to_string()
                })
                .collect();
            fs::write(&nodes, format!("{}\n", lines.join("\n"))).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::SymmetricDuplicate => {
            // Both directions of a conflicts_with edge in the file, as a merge can leave them (D49).
            let path = home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            let spare = home.add("problem", "Spare id");
            home.umx(&["edge", "add", &a, "conflicts_with", &b]);
            let edges = Path::new(&path).join("edges.jsonl");
            let text = fs::read_to_string(&edges).unwrap();
            let mut line = text
                .lines()
                .map(|line| umx_protocol::parse_json(line).unwrap())
                .find(|edge| edge["type"] == "conflicts_with")
                .unwrap();
            let (from, to) = (line["from"].clone(), line["to"].clone());
            (line["from"], line["to"], line["id"]) = (to, from, json!(spare));
            fs::write(&edges, format!("{text}{line}\n")).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::UnknownKey => {
            let path = home.graph();
            let file = Path::new(&path).join("graph.json");
            let mut meta: umx_protocol::Value =
                umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
            meta["ontology"]["descriptions"] = json!({});
            fs::write(&file, meta.to_string()).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::AliasInvalid => {
            home.start();
            let path = home.path.join("g").display().to_string();
            home.umx(&["graph", "new", &path, "--alias", "Bad-Alias"])
        }
        Code::AliasInUse => {
            home.graph();
            let other = home.path.join("h").display().to_string();
            home.umx(&["graph", "new", &other, "--alias", "demo"])
        }
        Code::PathOutsideRoots => {
            home.start();
            home.umx(&["graph", "new", "/umx-outside-roots/g", "--alias", "x"])
        }
        Code::NodeNotFound => {
            home.graph();
            home.umx(&["node", "get", "7ZZZZZZZZZ"])
        }
        Code::IdAmbiguous => {
            home.graph();
            let a = home.add("problem", "One");
            home.add("problem", "Two");
            // Ids minted within about 17 minutes share their first 6 characters.
            home.umx(&["node", "get", &a[..6]])
        }
        Code::WhoAmbiguous => {
            home.graph();
            // One session id under two harnesses: two session keys end with it.
            let session = "h-1111111111111111";
            let first = provenance_write("pij-who-one", session, "First");
            assert_eq!(raw(&home.socket(), &first).1, 0);
            let mut second = provenance_write("pij-who-two", session, "Second");
            second.actor.as_mut().unwrap().harness = Some("claude".into());
            assert_eq!(raw(&home.socket(), &second).1, 0);
            home.umx(&["who", session])
        }
        Code::WhoUnknown => {
            home.graph();
            home.umx(&["who", "agent/pij/pij-nobody-here"])
        }
        Code::NameInvalid => {
            home.graph();
            home.umx(&["node", "add", "--type", "Bad-Type", "--title", "x"])
        }
        Code::ReservedName => {
            home.graph();
            home.umx(&[
                "node", "add", "--type", "problem", "--title", "x", "--set", "status=1",
            ])
        }
        Code::EntityKeyInvalid => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "person",
                "--title",
                "x",
                "--set",
                "entity_key=Foo@X.com",
            ])
        }
        Code::EntityLinkInvalid => {
            home.graph();
            let id = home
                .ok(&["node", "add", "--type", "person", "--title", "x"])
                .data["node"]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            home.umx(&["entity", "link", &id, &id])
        }
        Code::EntityGraphTwice => {
            home.graph();
            let a = home.add("person", "Jo");
            let b = home.add("person", "Jo S");
            home.ok(&["entity", "link", &a, &b]);
            home.umx(&["entity", "show", &a])
        }
        Code::EntityKeyIgnored => {
            home.graph();
            let set = ["--set", "entity_key=email:jo@example.com"];
            let a = home.ok(&[
                &["node", "add", "--type", "person", "--title", "Jo"][..],
                &set,
            ]
            .concat());
            home.ok(&[
                &["node", "add", "--type", "person", "--title", "Jo S"][..],
                &set,
            ]
            .concat());
            home.umx(&["entity", "show", a.data["node"]["id"].as_str().unwrap()])
        }
        Code::EntityLinkOverlap => {
            home.graph();
            let (a, b, c) = (
                home.add("person", "Jo"),
                home.add("person", "Jo S"),
                home.add("person", "Joe"),
            );
            home.ok(&["entity", "link", &a, &b]);
            home.umx(&["entity", "link", &c, &a])
        }
        Code::EntityTypesMixed => {
            home.graph();
            let (a, b) = (home.add("person", "Acme"), home.add("organisation", "Acme"));
            home.ok(&["entity", "link", &a, &b]);
            home.umx(&["entity", "show", &a])
        }
        Code::EntityKeyIsTitle => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "person",
                "--title",
                "Jo Smith",
                "--set",
                "entity_key=name:jo  smith",
            ])
        }
        Code::EntityJoinDenied => {
            home.graph();
            let (p, r, q) = (
                home.add("person", "Jo"),
                home.add("person", "Jo R"),
                home.add("person", "Jo Q"),
            );
            home.ok(&["entity", "link", &p, &r]);
            home.ok(&["entity", "link", &r, &q]);
            home.ok(&["entity", "deny", &p, &q, "--reason", "two different people"]);
            home.umx(&["entity", "show", &p])
        }
        Code::EntityTypeMismatch => {
            home.graph();
            let mut ids = Vec::new();
            for kind in ["person", "organisation"] {
                let added = home.ok(&["node", "add", "--type", kind, "--title", "x"]);
                ids.push(added.data["node"]["id"].as_str().unwrap().to_owned());
            }
            home.umx(&["entity", "link", &ids[0], &ids[1]])
        }
        Code::EntityKeyNotCanonical | Code::EntityKeyUnusable => {
            let path = home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "person",
                "--title",
                "a",
                "--set",
                "entity_key=email:a@x.com",
            ]);
            let file = Path::new(&path).join("nodes.jsonl");
            let text = fs::read_to_string(&file).unwrap();
            let edited = if code == Code::EntityKeyNotCanonical {
                text.replace("email:a@x.com", "email:A@X.com")
            } else {
                text.replace("email:a@x.com", "not a key")
            };
            fs::write(&file, edited).unwrap();
            home.umx(&["graph", "unload", "demo"]);
            home.umx(&["graph", "load", &path])
        }
        Code::DuplicateKey => {
            home.graph();
            let key = |k: &str| format!("entity_key={k}");
            home.umx(&[
                "node",
                "add",
                "--type",
                "person",
                "--title",
                "a",
                "--set",
                &key("email:a@x.com"),
            ]);
            home.umx(&[
                "node",
                "add",
                "--type",
                "person",
                "--title",
                "b",
                "--set",
                &key("email:A@x.com"),
            ])
        }
        Code::OnKeyAmbiguous | Code::OnKeyFieldHeld => {
            home.graph();
            let add = |home: &Home, title: &str, on_key: bool| {
                let mut words = vec![
                    "node",
                    "add",
                    "--type",
                    "person",
                    "--title",
                    title,
                    "--set",
                    "entity_key=email:jo@example.com",
                ];
                if on_key {
                    words.extend(["--on-key", "resolve"]);
                }
                home.umx(&words)
            };
            add(&home, "Jo Smith", false);
            if code == Code::OnKeyAmbiguous {
                add(&home, "Jo S", false);
            }
            add(&home, "Joanna Smith", true)
        }
        Code::AlreadyDeclared => {
            home.graph();
            home.umx(&["ontology", "add", "relation", "conflicts_with"])
        }
        Code::OntologyInUse => {
            home.graph();
            home.add("problem", "x");
            home.umx(&["ontology", "remove", "type", "problem"])
        }
        Code::OntologyBuiltin => {
            home.graph();
            home.umx(&["ontology", "remove", "relation", "part_of"])
        }
        Code::FieldMissing => {
            home.graph();
            home.umx(&["ontology", "add", "type", "actor", "--field", "x:int"])
        }
        Code::KindInvalid => {
            home.graph();
            home.umx(&["ontology", "add", "field", "problem", "x:colour"])
        }
        Code::TypeColumnUnknown => {
            let path = home.graph();
            plant_type_columns(&path, "problem", &["title", "gone"]);
            home.umx(&["graph", "load", &path])
        }
        Code::RoleNotEmbeddable => {
            home.graph();
            home.umx(&[
                "ontology", "add", "field", "problem", "size:int", "--role", "content",
            ])
        }
        Code::FieldNotDeclared => {
            home.graph();
            home.umx(&[
                "ontology", "set", "field", "problem", "file", "--role", "data",
            ])
        }
        Code::RoleChanged => {
            // A content string field with text in one node, made data: that node's vectors are pending until remade.
            home.graph();
            home.umx(&[
                "ontology",
                "add",
                "field",
                "problem",
                "file:string",
                "--role",
                "content",
            ]);
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "Export",
                "--set",
                "file=src/a.rs",
            ]);
            home.umx(&[
                "ontology", "set", "field", "problem", "file", "--role", "data",
            ])
        }
        Code::TextFormOutdated => {
            unreachable!(
                "raised by umx-services' tests/embedding.rs (a v1 line made from its node's v1 text)"
            )
        }
        Code::TypeUnknown => {
            home.graph();
            home.umx(&["ontology", "add", "field", "problem,nope", "x:int"])
        }
        Code::FieldConflict => {
            home.graph();
            home.umx(&["ontology", "add", "field", "actor", "x:int"]);
            home.umx(&["ontology", "add", "field", "problem,actor", "x:string"])
        }
        Code::RelationFieldConflict => {
            home.graph();
            home.umx(&[
                "ontology",
                "add",
                "field",
                "--relation",
                "supports",
                "x:int",
            ]);
            home.umx(&[
                "ontology",
                "add",
                "field",
                "--relation",
                "supports",
                "x:string",
            ])
        }
        Code::OntologyErrorsTruncated => {
            home.graph();
            let many: Vec<String> = (0..=100).map(|i| format!("nope_{i}")).collect();
            home.umx(&["ontology", "add", "field", &many.join(","), "x:int"])
        }
        Code::FieldRoleConflict => {
            home.graph();
            home.umx(&[
                "ontology",
                "add",
                "field",
                "problem",
                "file:string",
                "--role",
                "data",
            ]);
            home.umx(&[
                "ontology",
                "add",
                "field",
                "problem",
                "file:string",
                "--role",
                "content",
            ])
        }
        Code::WhereInvalid => {
            home.graph();
            home.umx(&["node", "list", "--where", "x~1"])
        }
        Code::CrossGraphNotYet => {
            home.graph();
            home.umx(&[
                "edge",
                "list",
                "--from",
                "umx:other/01ABCDEF",
                "--to",
                "umx:demo/01ABCDEF",
            ])
        }
        Code::WriteFailed => {
            let path = home.graph();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
            let result = home.umx(&["node", "add", "--type", "problem", "--title", "x"]);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::WriteNotDurable => {
            // Write and search but no read on the folder: the rename lands, the folder sync after it fails.
            let path = home.graph();
            home.add("problem", "Establish this writer's session");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o300)).unwrap();
            let result = home.umx(&["node", "add", "--type", "problem", "--title", "x"]);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::GitignoreLineMissing => {
            // A .gitignore someone rewrote without umx's line.
            let path = home.graph();
            fs::write(Path::new(&path).join(".gitignore"), "target\n").unwrap();
            home.umx(&["check"])
        }
        Code::RecoveryConflict => {
            // A marker left by a cut-off write whose target has changed since (here: it is not there at all).
            home.start();
            let folder = home.path.join("cut");
            fs::create_dir_all(&folder).unwrap();
            let line = r#"{"target":"graph.json","temp":".umx-tmp-1-graph.json","old":{"size":2,"hash":"0000000000000001"},"new":{"size":3,"hash":"0000000000000002"}}"#;
            let marker = format!("{{\"hash\":\"fnv1a64-words-v1\"}}\n{line}\n");
            fs::write(folder.join(".umx-tmp-set.json"), marker).unwrap();
            home.umx(&["graph", "load", &folder.display().to_string()])
        }
        Code::GraphFilesChanged => {
            // A checkout replaced nodes.jsonl under the loaded graph; the next write is refused.
            let path = home.graph();
            checkout(&path, "nodes.jsonl", b"");
            home.umx(&["node", "add", "--type", "problem", "--title", "x"])
        }
        Code::StaleFiles => {
            // The same, and a read: it answers from memory, with the warning.
            let path = home.graph();
            checkout(&path, "nodes.jsonl", b"");
            home.umx(&["node", "list"])
        }
        Code::OffVocabularyType => {
            home.graph();
            home.umx(&["node", "add", "--type", "widget", "--title", "x"])
        }
        Code::OffVocabularyStatus => {
            home.graph();
            home.umx(&[
                "node", "add", "--type", "problem", "--title", "x", "--status", "parked",
            ])
        }
        Code::OffVocabularyRelation => {
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "causes", &b])
        }
        Code::OffVocabularyEdgeField => {
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "supports", &b, "--set", "confidence=0.7"])
        }
        Code::OffVocabularyField => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "x",
                "--set",
                "colour=red",
            ])
        }
        Code::RelatedSkipped => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "x",
                "--no-related",
            ])
        }
        Code::NearDuplicate => {
            home.graph();
            home.add("problem", "Approval delays");
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "Approval delay",
            ])
        }
        Code::NearDuplicateLine => {
            home.graph();
            let file = home.ops_file(
                "{\"op\":\"node add\",\"type\":\"problem\",\"title\":\"Approval delays\"}\n\
                 {\"op\":\"node add\",\"type\":\"problem\",\"title\":\"Approval delay\"}\n",
            );
            home.umx(&["batch", "--file", &file, "--dry-run"])
        }
        Code::FindNoMatch => {
            home.graph();
            home.add("problem", "Approval delays");
            home.umx(&["node", "find", "chaos"])
        }
        Code::EdgeExists => {
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "blocks", &b]);
            home.umx(&["edge", "add", &a, "blocks", &b])
        }
        Code::EvidenceAppended => {
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "blocks", &b]);
            home.umx(&["edge", "add", &a, "blocks", &b, "--evidence", "notes.md#L1"])
        }
        Code::EvidenceFieldKept | Code::EvidenceFieldInvalid => {
            // An older edge whose own custom `evidence` field is prose, not an evidence list (req-124): kept on load;
            // evidence given to it is refused (B1).
            let path = home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "blocks", &b, "--set", "why=x"]);
            let edges = Path::new(&path).join("edges.jsonl");
            let text = fs::read_to_string(&edges).unwrap();
            fs::write(&edges, text.replace("\"why\"", "\"evidence\"")).unwrap();
            let loaded = home.umx(&["graph", "load", &path]);
            if code == Code::EvidenceFieldKept {
                loaded
            } else {
                home.umx(&["edge", "add", &a, "blocks", &b, "--evidence", "notes.md#L1"])
            }
        }
        Code::EvidenceSourcesFieldKept => {
            // A node's own `evidence_sources`, written before umx reserved the name: kept on load, warned.
            let path = home.graph();
            let id = home.add("problem", "One");
            home.ok(&["node", "edit", &id, "--set", "cites=x"]);
            let nodes = Path::new(&path).join("nodes.jsonl");
            let text = fs::read_to_string(&nodes).unwrap();
            fs::write(&nodes, text.replace("\"cites\"", "\"evidence_sources\"")).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::EdgeExistsDiffers => {
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "blocks", &b, "--set", "why=first"]);
            home.umx(&["edge", "add", &a, "blocks", &b, "--set", "why=second"])
        }
        Code::EdgeNotFound => {
            home.graph();
            // A real id, but a node's: edges and nodes are named apart.
            let a = home.add("problem", "One");
            home.umx(&["edge", "delete", &a])
        }
        Code::EdgeWouldDuplicate => {
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "blocks", &b]);
            let other = edge_id(&home.umx(&["edge", "add", &a, "supports", &b]).0);
            home.umx(&["edge", "edit", &other, "--relation", "blocks"])
        }
        Code::EdgeDerived => {
            // A similar_to edge written by hand: umx keeps those itself.
            let path = home.graph();
            let id = rewritten_edge(&mut home, &path, |line| line["type"] = json!("similar_to"));
            home.umx(&["edge", "delete", &id])
        }
        Code::EdgeRepeated => {
            let path = home.graph();
            let id = repeated_edge(&mut home, &path);
            home.umx(&["edge", "delete", &id])
        }
        Code::EvidenceNotFound => {
            home.graph();
            let id = home.add("problem", "x");
            home.umx(&[
                "node", "evidence", &id, "remove", "a.md#L1", "--reason", "stale",
            ])
        }
        Code::EvidenceAmbiguous => {
            home.graph();
            let id = home.add("problem", "x");
            for kind in ["room", "chat"] {
                home.ok(&[
                    "node",
                    "evidence",
                    &id,
                    "add",
                    "a.md#L1",
                    "--evidence-kind",
                    kind,
                ]);
            }
            home.umx(&[
                "node", "evidence", &id, "remove", "a.md#L1", "--reason", "stale",
            ])
        }
        Code::AlreadyRemoved => {
            home.graph();
            let id = home.add("problem", "x");
            home.ok(&["node", "evidence", &id, "add", "a.md#L1"]);
            home.ok(&[
                "node", "evidence", &id, "remove", "a.md#L1", "--reason", "stale",
            ]);
            home.umx(&[
                "node", "evidence", &id, "remove", "a.md#L1", "--reason", "stale",
            ])
        }
        Code::SegmentWithoutParent => {
            home.graph();
            home.umx(&["node", "add", "--type", "segment", "--title", "Opening"])
        }
        Code::HintsTruncated => {
            // Ten segments with no parent in one batch: eight hints, then the marker.
            home.graph();
            let lines: String = (0..10)
                .map(|i| {
                    format!(
                        "{}\n",
                        json!({"op": "node add", "ref": format!("s{i}"), "type": "segment", "title": format!("Part {i}")})
                    )
                })
                .collect();
            let ops = home.ops_file(&lines);
            home.umx(&["batch", "--file", &ops, "--reason", "segments"])
        }
        Code::RedactedInFilesOnly => {
            home.graph();
            let id = home.add("problem", "x");
            home.ok(&["node", "evidence", &id, "add", "a.md#L1"]);
            home.umx(&[
                "node", "evidence", &id, "remove", "a.md#L1", "--redact", "--reason", "leaked",
            ])
        }
        Code::EvidenceRedactBlocked => {
            // The source is also in the title, which a redact never rewrites.
            home.graph();
            let id = home.add("problem", "see a.md");
            home.ok(&["node", "evidence", &id, "add", "a.md#L1"]);
            home.umx(&[
                "node", "evidence", &id, "remove", "a.md#L1", "--redact", "--reason", "leaked",
            ])
        }
        Code::EvidenceMissingSkipped => {
            home.graph();
            let id = home.add("problem", "x");
            let ops = home.ops_file(&format!(
                "{{\"op\":\"evidence remove\",\"node\":\"{id}\",\"pointer\":\"a.md#L1\",\"missing\":\"skip\"}}\n"
            ));
            home.umx(&["batch", "--file", &ops, "--reason", "stale"])
        }
        Code::NodeReservedFieldInUse => {
            // An older graph's own `evidence_removed` field: umx never takes it for its trail.
            let path = home.graph();
            let id = rewritten_node(&mut home, &path, |line| {
                line["evidence_removed"] = json!("mine")
            });
            home.ok(&["node", "evidence", &id, "add", "a.md#L1"]);
            home.umx(&[
                "node", "evidence", &id, "remove", "a.md#L1", "--reason", "stale",
            ])
        }
        Code::EdgeReservedFieldInUse => {
            // An older graph's own `updated` field: umx never takes it for its edit stamp.
            let path = home.graph();
            let id = rewritten_edge(&mut home, &path, |line| line["updated"] = json!("soon"));
            home.umx(&["edge", "edit", &id, "--relation", "supports"])
        }
        Code::EdgeReverseSymmetric => {
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            let id = edge_id(&home.umx(&["edge", "add", &a, "conflicts_with", &b]).0);
            home.umx(&["edge", "edit", &id, "--reverse"])
        }
        Code::DateNotADate => {
            // A value stored as given with field_kind_mismatch, which umx check then names.
            home.graph();
            home.ok(&["ontology", "add", "field", "problem", "seen_on:date"]);
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "x",
                "--set",
                "seen_on=2025-13",
            ]);
            home.umx(&["check"])
        }
        Code::EdgeDirectionNormalised => {
            // The later node (higher id) as from, then a retype to a symmetric relation: stored lower end first.
            home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            let id = edge_id(&home.umx(&["edge", "add", &b, "supports", &a]).0);
            home.umx(&["edge", "edit", &id, "--relation", "conflicts_with"])
        }
        Code::TimeNoOffset => {
            home.graph();
            home.ok(&[
                "ontology",
                "add",
                "type",
                "meeting",
                "--field",
                "starts_on:date",
            ]);
            home.umx(&[
                "node",
                "add",
                "--type",
                "meeting",
                "--title",
                "x",
                "--set",
                "starts_on=2026-10-30T09:40",
            ])
        }
        Code::TimeAsText => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "x",
                "--set",
                "due=2026-10-30",
            ])
        }
        Code::AnchorUndated => {
            home.graph();
            home.umx(&["node", "add", "--type", "meeting", "--title", "x"])
        }
        Code::StdNewer => {
            let path = home.graph();
            edit_graph_json(&path, |g| g["ontology"]["std"] = json!(2));
            home.umx(&["graph", "load", &path])
        }
        Code::StdEntriesShown => {
            home.graph();
            home.umx(&["ontology", "show"])
        }
        Code::RelationEndpointUnexpected => {
            home.graph();
            let anchor = home
                .ok(&["node", "add", "--type", "person", "--title", "Ada"])
                .data["node"]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "x",
                "--raised-in",
                &anchor,
            ])
        }
        Code::LintOrphans => {
            home.graph();
            home.add("problem", "Alone");
            home.umx(&["check", "--lint"])
        }
        Code::LintProposedKeys => {
            home.graph();
            let id = home.add("problem", "Keyed");
            home.ok(&["node", "edit", &id, "--set", "entity_key=sku:1"]);
            home.ok(&["node", "status", &id, "proposed", "--reason", "new"]);
            home.umx(&["check", "--lint"])
        }
        Code::LintTerms => {
            home.graph();
            f139_terms(
                &home,
                r#"{"umx_lint_terms":1,"terms":[{"term":"Zorblax"}]}"#,
                0o600,
            );
            home.add("problem", "The zorblax plan");
            home.umx(&["check", "--lint"])
        }
        Code::LintDoubleNegatives => {
            home.graph();
            home.add("problem", "Plan lacks no owner");
            home.umx(&["check", "--lint"])
        }
        Code::LintRaisedInEvidence => {
            home.graph();
            let (_, node) = f139_apart(&home);
            assert!(!node.is_empty());
            home.umx(&["check", "--lint"])
        }
        Code::LintTruncated => {
            home.graph();
            let lines: String = (0..101)
                .map(|i| {
                    format!(
                        "{}\n",
                        json!({"op": "node add", "type": "problem", "title": format!("Loose {i}")})
                    )
                })
                .collect();
            let ops = home.ops_file(&lines);
            home.ok(&["batch", "--file", &ops, "--reason", "loose"]);
            home.umx(&["check", "--lint", "--kind", "orphans"])
        }
        Code::LintTermsUnusable => {
            home.graph();
            f139_terms(&home, "not json", 0o600);
            home.umx(&["check", "--lint"])
        }
        // Usage 220: source hashes.
        Code::SourceNotHashed => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "Cited",
                "--evidence",
                "calls/none.vtt#seq=1-2",
            ])
        }
        Code::SourceChanged => {
            let path = home.graph();
            source_cited(&home, &path);
            fs::write(Path::new(&path).join("notes.md"), "edited\n").unwrap();
            home.umx(&["check"])
        }
        Code::SourceNotRefreshed => {
            home.graph();
            let acme = home.path.join("acme");
            fs::create_dir_all(&acme).unwrap();
            fs::write(acme.join("a.md"), "a\n").unwrap();
            home.ok(&["root", "add", "acme", &acme.display().to_string()]);
            let node = home.ok(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "Cited",
                "--evidence",
                "root:acme/a.md#L1",
            ]);
            let id = node.data["node"]["id"].as_str().unwrap().to_owned();
            home.ok(&["root", "remove", "acme"]);
            home.umx(&["node", "evidence", &id, "refresh", "--all"])
        }
        Code::SourceMissing => {
            let path = home.graph();
            source_cited(&home, &path);
            fs::remove_file(Path::new(&path).join("notes.md")).unwrap();
            home.umx(&["check"])
        }
        Code::RuleInvalid => {
            home.graph();
            home.umx(&["rule", "add", "invalid_rule", "--type", "risk"])
        }
        Code::RuleRegexInvalid => {
            home.graph();
            home.umx(&[
                "rule",
                "add",
                "invalid_regex",
                "--match",
                "title=(",
                "--field",
                "step",
            ])
        }
        Code::RuleVocabularyUnknown => {
            home.graph();
            home.umx(&[
                "rule", "add", "custom", "--type", "widget", "--field", "step",
            ])
        }
        Code::RuleNotFound => {
            home.graph();
            home.umx(&["rule", "show", "not_here"])
        }
        Code::RuleNameTaken => {
            home.graph();
            home.ok(&["rule", "add", "rated", "--type", "risk", "--field", "step"]);
            home.umx(&[
                "rule", "add", "rated", "--type", "person", "--field", "step",
            ])
        }
        Code::RuleMatchesCut => {
            home.graph();
            home.add("risk", "First risk");
            home.add("risk", "Second risk");
            home.ok(&["rule", "add", "rated", "--type", "risk", "--field", "step"]);
            home.umx(&["rule", "show", "rated", "--limit", "1"])
        }
        Code::RuleScanCut | Code::RuleNodeUnchecked => {
            home.graph();
            home.ok(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "Long note",
                "--note",
                &"é".repeat(50_000),
            ]);
            home.umx(&[
                "rule",
                "add",
                "bounded",
                "--match",
                r"note=\b(a{8}){8}b",
                "--field",
                "step",
                "--dry-run",
            ])
        }
        Code::RuleUnmet => {
            home.graph();
            home.ok(&[
                "rule", "add", "rated", "--type", "risk", "--field", "impact",
            ]);
            home.umx(&["node", "add", "--type", "risk", "--title", "Unrated"])
        }
        Code::RuleCheckCut => {
            let path = home.graph();
            let id = home.add("risk", "Seed");
            home.ok(&[
                "rule",
                "add",
                "bounded_check",
                "--match",
                r"note=\b(a{8}){8}b",
                "--field",
                "impact",
            ]);
            home.ok(&["graph", "unload", "demo"]);
            let file = Path::new(&path).join("nodes.jsonl");
            let original = fs::read_to_string(&file).unwrap();
            let seed = original
                .lines()
                .map(|line| umx_protocol::parse_json(line).unwrap())
                .find(|row| row["id"].as_str() == Some(id.as_str()))
                .unwrap();
            let mut rows = original;
            for i in 1..=500 {
                let mut row = seed.clone();
                row["id"] = json!(format!("{i:026}"));
                row["note"] = json!("é".repeat(2_000));
                rows.push_str(&row.to_string());
                rows.push('\n');
            }
            fs::write(file, rows).unwrap();
            home.ok(&["graph", "load", &path]);
            home.umx(&["check", "--rules", "--rule", "bounded_check"])
        }
        Code::RuleSkipped => {
            let path = home.graph();
            home.ok(&["graph", "unload", "demo"]);
            let file = Path::new(&path).join("graph.json");
            let mut meta = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
            meta["umx"] = json!(8);
            meta["rules"] =
                json!([{"name":"huge","match":{"regex":{"title":"a{1000000000}"}},"field":"step"}]);
            fs::write(&file, meta.to_string()).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::HubInvalid => {
            home.graph();
            home.umx(&["hub", "set", "meetings", "--type-hub", "meeting"])
        }
        Code::HubNotFound => {
            home.graph();
            home.umx(&["hub", "show", "not_here"])
        }
        Code::HubJoined => {
            home.graph();
            let hub = home.add("topic", "Risk register");
            home.ok(&[
                "hub",
                "set",
                "risk_register",
                "--node",
                &hub,
                "--type",
                "risk",
            ]);
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "Supplier may slip",
            ])
        }
        Code::HubUnjoined | Code::HubNodeMissing => {
            // Declared by hand, as a merge brings it: nothing has joined the risk yet.
            let path = home.graph();
            let hub = home.add("topic", "Risk register");
            home.add("risk", "Supplier may slip");
            home.ok(&["graph", "unload", "demo"]);
            let file = Path::new(&path).join("graph.json");
            let mut meta = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
            meta["umx"] = json!(9);
            let node = if code == Code::HubUnjoined {
                hub
            } else {
                "0".repeat(26)
            };
            meta["hubs"] = json!([{"name":"risk_register","node":node,"via":"part_of","match":{"type":["risk"]}}]);
            fs::write(&file, meta.to_string()).unwrap();
            home.ok(&["graph", "load", &path]);
            home.umx(&["hub", "show", "risk_register"])
        }
        Code::HubNodeRetired => {
            home.graph();
            let hub = home.add("topic", "Risk register");
            home.ok(&["hub", "set", "risk_register", "--node", &hub]);
            home.umx(&["node", "status", &hub, "ruled_out", "--reason", "replaced"])
        }
        Code::HubScanCut => {
            home.graph();
            let lines: String = (0..1001)
                .map(|i| format!("{}\n", json!({"op": "node add", "type": "risk", "title": format!("Risk number {i}")})))
                .collect();
            home.ok(&["batch", "--file", &home.ops_file(&lines)]);
            let hub = home.add("topic", "Risk register");
            home.umx(&[
                "hub",
                "set",
                "risk_register",
                "--node",
                &hub,
                "--type",
                "risk",
            ])
        }
        Code::HubEdgesRemain => {
            home.graph();
            let hub = home.add("topic", "Risk register");
            home.ok(&[
                "hub",
                "set",
                "risk_register",
                "--node",
                &hub,
                "--type",
                "risk",
            ]);
            home.add("risk", "Supplier may slip");
            home.umx(&["hub", "remove", "risk_register"])
        }
        Code::RuleSetSkipped
        | Code::RuleSetsCut
        | Code::RuleSetsFull
        | Code::RuleSetNameTaken
        | Code::RuleSetsRefreshFailed => rules::set_code_trigger(&mut home, code),
        Code::SpineNameUnknown => {
            home.graph();
            let file = home.ops_file(&tm5_decl("widget", "widget", false).to_string());
            home.umx(&["graph", "spine", "set", "demo", "--file", &file])
        }
        Code::SpineMainTaken => {
            home.graph();
            let first = home.ops_file(&tm5_decl("day", "segment", true).to_string());
            home.ok(&["graph", "spine", "set", "demo", "--file", &first]);
            let second = home.ops_file(&tm5_decl("meetings", "meeting", true).to_string());
            home.umx(&["graph", "spine", "set", "demo", "--file", &second])
        }
        Code::SpineNotFound => {
            home.graph();
            home.umx(&["graph", "spine", "show", "demo", "day"])
        }
        Code::SpineBuiltinCandidate => {
            home.graph();
            home.add("problem", "x");
            home.umx(&["graph", "spine", "confirm", "demo", "recorded"])
        }
        // D71 step 12 (plan 206).
        Code::SpineNameReserved => {
            home.graph();
            let file = home.ops_file(&tm5_decl("recorded", "segment", false).to_string());
            home.umx(&["graph", "spine", "set", "demo", "--file", &file])
        }
        Code::SpineNameReservedStored => {
            let path = home.graph();
            s12_store_spine(&home, &path, tm5_decl("status", "segment", false));
            home.umx(&["check"])
        }
        Code::AlongEndInvalid => {
            home.graph();
            home.add("problem", "x");
            home.umx(&["node", "list", "--along", "recorded", "--after", "soon"])
        }
        Code::AlongNotPlaced => {
            home.graph();
            home.add("problem", "never moved");
            home.umx(&["node", "list", "--along", "status"])
        }
        Code::AlongByRule => {
            tm5_day(&mut home);
            let file = home.ops_file(&tm5_decl("day", "segment", true).to_string());
            home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
            let risk = home
                .ok(&["node", "list", "--type", "risk", "--limit", "1"])
                .data["rows"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let near = home.add("problem", "Linked to a point only");
            home.ok(&["edge", "add", &near, "relates_to", &risk]);
            home.umx(&["node", "list", "--along", "day"])
        }
        Code::FieldKindMismatch => {
            home.graph();
            home.umx(&[
                "ontology",
                "add",
                "type",
                "risk",
                "--field",
                "likelihood:int",
            ]);
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "x",
                "--set",
                "likelihood=high",
            ])
        }
        Code::UseStatusVerb => {
            home.graph();
            let id = home.add("problem", "x");
            home.umx(&["node", "edit", &id, "--set", "status=agreed"])
        }
        Code::EvidencePointerInvalid => {
            home.graph();
            let id = home.add("problem", "x");
            home.umx(&[
                "node",
                "note",
                &id,
                "--append",
                "y",
                "--evidence",
                "notes.md",
            ])
        }
        Code::DanglingEvidence | Code::EvidenceUnchecked => {
            home.graph();
            let graph = if code == Code::DanglingEvidence {
                "demo"
            } else {
                "sec_b"
            };
            let pointer = format!("umx:{graph}/01ZZZZZZZZZZZZZZZZZZZZZZZZ#quote=nothing here");
            home.umx(&[
                "node",
                "add",
                "--type",
                "finding",
                "--title",
                "x",
                "--evidence",
                &pointer,
            ])
        }
        Code::FieldNotFound => {
            home.graph();
            let id = home.add("problem", "x");
            home.umx(&["resolve", &format!("umx:demo/{id}#nope")])
        }
        Code::RootUnknown | Code::RootUnavailable => {
            // No source root acme is set; the graph's folder is in no git repository inside the roots.
            home.graph();
            let source = if code == Code::RootUnknown {
                "root:acme/transcripts/016.ndjson#seq=1-5"
            } else {
                "root:repo/docs/x.md#L1"
            };
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "x",
                "--evidence",
                source,
            ])
        }
        Code::EvidenceOutsideRoots => {
            home.graph();
            let id = home.add("problem", "x");
            home.umx(&[
                "node",
                "note",
                &id,
                "--append",
                "y",
                "--evidence",
                "../secret.md#L1",
            ])
        }
        Code::VectorOrphan | Code::VectorLineKept => {
            // A shard line for a node this graph does not have, or one umx cannot read: kept, warned at load.
            let path = home.graph();
            let space = Path::new(&path).join("embeddings/m@4");
            fs::create_dir_all(&space).unwrap();
            let line = if code == Code::VectorOrphan {
                format!(
                    "01JC0000000000000000000AB7\ttitle\t{}\tAQIDBA==\n",
                    "0".repeat(32)
                )
            } else {
                "<<<<<<< HEAD\n".to_owned()
            };
            fs::write(space.join("b7.tsv"), line).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::VectorWrongWidth => {
            // An 8-wide vector in a 4-wide space: never used, and reported by shard, line and width.
            let path = home.graph();
            let space = Path::new(&path).join("embeddings/m@4");
            fs::create_dir_all(&space).unwrap();
            let line = format!(
                "01JC0000000000000000000AB7\ttitle\t{}\tAQIDBAUGBwg=\n",
                "0".repeat(32)
            );
            fs::write(space.join("b7.tsv"), line).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::SimilarToIncomplete => {
            // A graph recording m@4 with one committed whole-node vector and no similar/ files yet: the stored lists
            // do not match, so they are rebuilt and the load says so meanwhile.
            home.start();
            let path = home.path.join("g").display().to_string();
            home.umx(&[
                "graph",
                "new",
                &path,
                "--alias",
                "demo",
                "--embedding-model",
                "m@4",
            ]);
            let id = home.add("problem", "x");
            let shard = id[id.len() - 2..].to_ascii_lowercase();
            let space = Path::new(&path).join("embeddings/m@4");
            fs::create_dir_all(&space).unwrap();
            let line = format!("{id}\t@node.v1\t{}\tAQIDBA==\n", "0".repeat(32));
            fs::write(space.join(format!("{shard}.tsv")), line).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::FloorUncalibrated => {
            // A graph holding an m@4 vector: m@4 has no calibrated floor and the machine config sets none, so the
            // load says so, once.
            home.start();
            let path = home.path.join("g").display().to_string();
            home.umx(&[
                "graph",
                "new",
                &path,
                "--alias",
                "demo",
                "--embedding-model",
                "m@4",
            ]);
            let id = home.add("problem", "x");
            let shard = id[id.len() - 2..].to_ascii_lowercase();
            let space = Path::new(&path).join("embeddings/m@4");
            fs::create_dir_all(&space).unwrap();
            let line = format!("{id}\t@node.v1\t{}\tAQIDBA==\n", "0".repeat(32));
            fs::write(space.join(format!("{shard}.tsv")), line).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::SimilarityModelMismatch => {
            // Two graphs, each with one committed whole-node vector, in different models: `similarity pairs` over
            // both compares the first named graph's model and names the other.
            home.start();
            for (alias, model) in [("demo", "m@4"), ("other", "n@4")] {
                let path = home.path.join(alias).display().to_string();
                home.umx(&[
                    "graph",
                    "new",
                    &path,
                    "--alias",
                    alias,
                    "--embedding-model",
                    model,
                ]);
                let (made, _) = home.umx(&[
                    "node", "add", "--graph", alias, "--type", "problem", "--title", "x",
                ]);
                let id = made.data["node"]["id"].as_str().unwrap().to_owned();
                let shard = id[id.len() - 2..].to_ascii_lowercase();
                let space = Path::new(&path).join(format!("embeddings/{model}"));
                fs::create_dir_all(&space).unwrap();
                let line = format!("{id}\t@node.v1\t{}\tAQIDBA==\n", "0".repeat(32));
                fs::write(space.join(format!("{shard}.tsv")), line).unwrap();
                home.umx(&["graph", "load", &path]);
            }
            home.umx(&["similarity", "pairs", "--graph", "demo,other"])
        }
        Code::SimilaritySettingInvalid => {
            // k = 999 is out of range: the default is used, and graph.json keeps the value.
            home.start();
            let path = home.path.join("g").display().to_string();
            home.umx(&[
                "graph",
                "new",
                &path,
                "--alias",
                "demo",
                "--embedding-model",
                "m@4",
            ]);
            let file = Path::new(&path).join("graph.json");
            let mut v: Value =
                umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
            v["embedding"]["similarity"] = json!({ "k": 999 });
            fs::write(&file, v.to_string()).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::VectorsUnreadable => {
            // A folder where a shard file should be: the shard cannot be read, and the load says so.
            let path = home.graph();
            fs::create_dir_all(Path::new(&path).join("embeddings/m@4/00.tsv")).unwrap();
            home.umx(&["graph", "load", &path])
        }
        Code::EmbedderNotConfigured => {
            home.graph();
            home.umx(&["embeddings", "repair"])
        }
        Code::ReasonMissing => {
            home.graph();
            let id = home.add("problem", "x");
            home.umx(&["node", "status", &id, "agreed"])
        }
        Code::ConfigKeyUnknown => {
            home.start();
            home.umx(&["config", "set", "colour", "red"])
        }
        Code::ConfigValueInvalid => {
            home.start();
            home.umx(&["config", "set", "embedders.nomic.dims", "0"])
        }
        Code::ConfigRepoIgnored => {
            let repo = home.repo();
            fs::create_dir_all(repo.join(".umx")).unwrap();
            fs::write(repo.join(".umx/config.toml"), "roots = [\"/\"]\n").unwrap();
            home.start();
            home.umx(&["config", "check"])
        }
        Code::ConfigRepoRefused => {
            home.repo();
            home.start();
            home.umx(&["config", "set", "roots", "/", "--repo"])
        }
        Code::ConfigRepoNone => {
            fs::create_dir_all(&home.path).unwrap();
            home.cwd = Some(home.path.clone());
            home.start();
            home.umx(&["config", "set", "embedder.active", "none", "--repo"])
        }
        Code::ConfigMissing => {
            home.start();
            home.ok(&["config", "set", "embedders.nomic.kind", "local"]);
            home.umx(&["config", "check"])
        }
        Code::ConfigInstanceUnknown => {
            home.start();
            home.umx(&["config", "set", "embedder.active", "ghost"])
        }
        Code::ConfigFileInvalid => {
            home.write_config("roots = [\n");
            home.start();
            home.umx(&["config", "set", "daemon.idle_unload", "1h"])
        }
        Code::ConfigFileRefused => {
            let file = home.write_config("");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
            home.start();
            home.umx(&["config", "set", "daemon.idle_unload", "1h"])
        }
        Code::ConfigInvalid => {
            home.roots_env = false;
            home.write_config("roots = [\"relative\"]\n");
            home.start();
            home.umx(&["graph", "list"])
        }
        Code::ConfigRestartNeeded => {
            home.start();
            home.umx(&["config", "set", "daemon.idle_unload", "1h"])
        }
        Code::SecretMissing => {
            home.start();
            home.az();
            home.ok(&["config", "set", "embedders.az.auth", "api_key"]);
            home.ok(&[
                "config",
                "set",
                "embedders.az.api_key_env",
                "UMX_TEST_NO_SUCH_KEY",
            ]);
            home.umx(&["config", "check"])
        }
        Code::SecretsLineInvalid => {
            fs::create_dir_all(&home.path).unwrap();
            let file = home.path.join("secrets.env");
            fs::write(&file, "GOOD=1\nnot a line\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            home.start();
            home.umx(&["config", "check"])
        }
        Code::DaemonNotRunning => home.umx(&["graph", "list"]),
        Code::DaemonBusy => {
            // What the daemon answers past its slots; the full path is a_full_daemon_is_busy_but_answers_status.
            let busy = Envelope::fail("graph list", Issue::new(Code::DaemonBusy));
            let status = home.healthy_status();
            home.fake_daemon(status, Some(busy));
            home.umx(&["graph", "list"])
        }
        Code::DaemonAlreadyRunning => {
            home.start();
            home.umx(&["daemon", "start"])
        }
        Code::DaemonRestartRefused => {
            // An older daemon: it answers status but refuses `daemon restart` as an unknown command.
            let refused = Envelope::fail(
                "daemon restart",
                Issue::fill(
                    Code::UsageInvalid,
                    &[
                        ("cmd", Fill::command("daemon restart")),
                        ("what", Fill::word("unknown command")),
                        ("noun", Fill::word("daemon")),
                    ],
                ),
            );
            let status = home.healthy_status();
            home.fake_daemon(status, Some(refused));
            home.umx(&["daemon", "restart"])
        }
        Code::DaemonVersionMismatch => {
            home.start();
            let other = Build {
                version: "0.0.0".into(),
                commit: "other".into(),
                built: "other".into(),
            };
            raw(
                &home.socket(),
                &Request::new("daemon status", vec![], other),
            )
        }
        Code::ProtocolMismatch => {
            home.start();
            let mut old = Request::new("graph list", vec![], older_cli());
            old.umx -= 1;
            raw(&home.socket(), &old)
        }
        Code::UsageInvalid => home.umx(&[]),
        Code::SocketUnavailable => {
            home.run_dir(0o755);
            home.umx(&["daemon", "start"])
        }
        Code::DocNotFound => home.umx(&["docs", "get", "no_such_topic"]),
        Code::HelpUnknown => home.umx(&["no_such_noun", "--help"]),
        Code::PolicyDowngradeUnconfirmed => {
            let path = home.secured_graph();
            home.umx(&["graph", "unload", "demo"]);
            edit_graph_json(&path, |v| v["embedding"]["policy"] = json!("open"));
            home.umx(&["graph", "load", &path])
        }
        Code::PolicyLowered => {
            home.secured_graph();
            home.umx(&["graph", "set", "demo", "--policy", "open"])
        }
        Code::PolicyNotLowered => {
            // The secured graph is recorded; then the record's folder takes no writes, so recording the lower one fails.
            home.secured_graph();
            let state = home.path.join("state");
            fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
            let result = home.umx(&["graph", "set", "demo", "--policy", "open"]);
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::LocalStateUnreadable => {
            home.state_file(b"not json");
            home.graph();
            home.umx(&["graph", "load", &home.path.join("g").display().to_string()])
        }
        Code::LocalStateNotSaved => {
            // A trusted record in a folder this user cannot write in: the record of the new secured graph fails.
            home.state_file(b"{\"umx_state\": 1, \"graphs\": []}");
            let state = home.path.join("state");
            fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
            home.start();
            let path = home.path.join("g").display().to_string();
            let result = home.umx(&["graph", "new", &path, "--alias", "demo", "--secured"]);
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::LocalStateNotDurable => {
            // Write and search but no read on the folder: the rename lands, the folder sync after it fails.
            home.state_file(b"{\"umx_state\": 1, \"graphs\": []}");
            let state = home.path.join("state");
            fs::set_permissions(&state, fs::Permissions::from_mode(0o300)).unwrap();
            home.start();
            let path = home.path.join("g").display().to_string();
            let result = home.umx(&["graph", "new", &path, "--alias", "demo", "--secured"]);
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
            result
        }
        Code::PolicyNotRecorded => {
            home.graph();
            home.state_file(b"{\"umx_state\": 99}");
            home.umx(&["graph", "confirm-policy", "demo"])
        }
        Code::DoctorDaemonDown => home.umx(&["doctor"]),
        Code::DoctorDaemonBuild => {
            let mut status = home.healthy_status();
            status.data["version"]["built"] = json!("another-build");
            home.fake_daemon(
                status,
                Some(Envelope::ok("graph list", json!({ "rows": [] }))),
            );
            home.umx(&["doctor"])
        }
        Code::DoctorDaemonProtocol => {
            let mut status = home.healthy_status();
            status.umx += 1;
            home.fake_daemon(status, None);
            home.umx(&["doctor"])
        }
        Code::DoctorWriterFailed => {
            let mut status = home.healthy_status();
            status.data["writer"] = json!("failed");
            home.fake_daemon(
                status,
                Some(Envelope::ok("graph list", json!({ "rows": [] }))),
            );
            home.umx(&["doctor"])
        }
        Code::DoctorSocketStale => {
            home.owner_only();
            drop(UnixListener::bind(home.socket()).unwrap());
            home.umx(&["doctor"])
        }
        Code::DoctorModeOpen => {
            home.owner_only();
            home.run_dir(0o750);
            home.umx(&["doctor"])
        }
        Code::DoctorNotYours => {
            home.owner_only();
            let elsewhere = home.path.join("elsewhere");
            fs::create_dir_all(&elsewhere).unwrap();
            fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).unwrap();
            fs::remove_dir(home.path.join("run")).unwrap();
            std::os::unix::fs::symlink(&elsewhere, home.path.join("run")).unwrap();
            home.umx(&["doctor"])
        }
        Code::DoctorEmbedderNone => {
            home.start();
            home.umx(&["doctor"])
        }
        Code::DoctorEmbedderUnready => {
            // az logged out: no az or azd on PATH, so no token. ready() never calls the endpoint.
            home.start();
            home.az();
            home.ok(&["config", "set", "embedder.active", "az"]);
            let empty = home.path.join("no-tools").display().to_string();
            home.umx_with(&["doctor"], &[("PATH", &empty)])
        }
        Code::DoctorConfigClosed => {
            home.owner_only();
            let file = home.write_config("roots = [\n");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            home.umx(&["doctor"])
        }
        Code::DoctorConfigProblems => {
            home.start();
            // An instance with only its kind: config check reports the keys it lacks; graph folders stay open.
            home.ok(&["config", "set", "embedders.az.kind", "azure_openai"]);
            home.umx(&["doctor"])
        }
        Code::DoctorGraphsUnlisted => {
            // The daemon answers status but not graph list.
            let status = home.healthy_status();
            home.fake_daemon(status, None);
            home.umx(&["doctor"])
        }
        Code::DoctorDaemonConfigClosed => {
            // Started on a config that does not parse, then the file is fixed but the daemon is not restarted.
            home.owner_only();
            let file = home.write_config("roots = [\n");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            home.start();
            fs::remove_file(&file).unwrap();
            home.umx(&["doctor"])
        }
        Code::DoctorGraphUnchecked => {
            let path = home.graph();
            home.add("problem", "One");
            fs::write(Path::new(&path).join("nodes.jsonl"), "not-json\n").unwrap();
            home.umx(&["doctor"])
        }
        Code::DoctorGraphFolder => {
            let path = home.graph();
            fs::remove_dir_all(&path).unwrap();
            home.umx(&["doctor"])
        }
        Code::DoctorGraphSourcesUnchecked => {
            let path = home.graph();
            source_cited(&home, &path);
            home.umx_with(&["doctor"], &[("UMX_DOCTOR_SOURCE_BUDGET", "0")])
        }
        Code::DoctorGraphNotStd => {
            home.start();
            let path = home.path.join("g").display().to_string();
            home.ok(&["graph", "new", &path, "--alias", "demo", "--no-std"]);
            home.umx(&["doctor"])
        }
        Code::ServiceConflict => {
            home.install();
            home.service_runs("/elsewhere/umx");
            home.umx(&["daemon", "install"])
        }
        Code::ServicePathUnsafe => {
            home.stub_services();
            let odd = home.path.join("a%b").display().to_string();
            home.umx_with(&["daemon", "install"], &[("UMX_HOME", &odd)])
        }
        Code::ServiceFailed => {
            home.stub_services();
            home.service_state("fail", "");
            home.umx(&["daemon", "install"])
        }
        Code::DoctorServiceNone => {
            home.start();
            home.umx(&["doctor"])
        }
        Code::DoctorSkillMissing => {
            home.start();
            home.umx(&["doctor"])
        }
        Code::DoctorSkillOutOfDate => {
            home.start();
            let skill = home.skill_copy(&home.path.join(".agents/skills/umx"));
            fs::write(skill.join("SKILL.md"), "an older skill\n").unwrap();
            home.umx(&["doctor"])
        }
        Code::DoctorSkillLinkOutOfDate => {
            home.start();
            let skill = home.skill_copy(&home.path.join("checkout/skills/umx"));
            fs::write(skill.join("SKILL.md"), "an older skill\n").unwrap();
            home.skill_link(&skill);
            home.umx(&["doctor"])
        }
        Code::DoctorServiceNotLoaded => {
            home.install();
            home.service_state("unload", "");
            home.umx(&["doctor"])
        }
        Code::DoctorServiceDown => {
            home.install();
            home.service_state("nopid", "");
            let (env, exit) = home.umx(&["doctor"]);
            // A loaded service owns the daemon: its next is umx daemon install, never a hand start.
            assert!(
                env.warnings
                    .iter()
                    .all(|w| w.code() != "doctor_daemon_down"),
                "{}",
                env.to_json()
            );
            (env, exit)
        }
        Code::DoctorServiceBeside => {
            home.start();
            home.install();
            home.service_state("nopid", "");
            home.umx(&["doctor"])
        }
        Code::DoctorServiceBuild => {
            home.install();
            home.service_state("pid", "4242");
            let mut status = home.healthy_status();
            status.data["pid"] = json!(4242);
            status.data["version"]["built"] = json!("another-build");
            home.fake_daemon(
                status,
                Some(Envelope::ok("graph list", json!({ "rows": [] }))),
            );
            home.umx(&["doctor"])
        }
        Code::DoctorServiceMissing => {
            home.install();
            home.service_runs("/nonexistent/umx");
            home.umx(&["doctor"])
        }
        Code::DoctorServiceUnreadable => {
            home.install();
            fs::set_permissions(home.unit_file(), fs::Permissions::from_mode(0o000)).unwrap();
            home.umx(&["doctor"])
        }
        Code::BatchTooLarge => {
            let op = "{\"op\":\"node add\",\"type\":\"q\",\"title\":\"T\"}\n";
            let file = home.ops_file(&op.repeat(10_001));
            home.umx(&["batch", "--file", &file])
        }
        Code::IdInvalid | Code::IdTaken | Code::IdInFuture | Code::IdInOtherGraph => {
            home.graph();
            let id = match code {
                Code::IdInvalid => "legacy-42".to_owned(),
                Code::IdInFuture => "7ZZZZZZZZZZZZZZZZZZZZZZZZZ".to_owned(),
                _ => home.add("problem", "Source"),
            };
            let graph = if code == Code::IdInOtherGraph {
                let path = home.path.join("target").display().to_string();
                home.ok(&["graph", "new", &path, "--alias", "target"]);
                "target"
            } else {
                "demo"
            };
            let file = home.ops_file(&format!(
                "{{\"op\":\"node add\",\"id\":\"{id}\",\"type\":\"problem\",\"title\":\"Ported\"}}\n"
            ));
            home.umx(&["batch", "--file", &file, "--graph", graph])
        }
        Code::BatchLineInvalid | Code::BatchFailed => {
            let file = home.ops_file("{\"op\":\"node delete\"}\n");
            home.umx(&["batch", "--file", &file])
        }
        Code::BatchErrorsTruncated => {
            let file = home.ops_file(&"{}\n".repeat(101));
            home.umx(&["batch", "--file", &file])
        }
        Code::BatchRefUnknown => {
            home.graph();
            let file =
                home.ops_file("{\"op\":\"node edit\",\"node_ref\":\"nope\",\"title\":\"x\"}\n");
            home.umx(&["batch", "--file", &file])
        }
        Code::BatchWarningsTruncated => {
            home.graph();
            let ops: String = (0..101)
                .map(|i| {
                    format!("{{\"op\":\"node add\",\"type\":\"widget\",\"title\":\"W{i}\"}}\n")
                })
                .collect();
            let file = home.ops_file(&ops);
            // Over a pipe's buffer: read it as it comes (see `big`).
            let out = home
                .command(&["batch", "--file", &file])
                .stdout(Stdio::piped())
                .bounded_output()
                .unwrap();
            let env = Envelope::from_json(&out.stdout).unwrap();
            let exit = out.status.code().unwrap() as u8;
            (env, exit)
        }
        Code::BatchRefRepeated => {
            home.graph();
            let op = "{\"op\":\"node add\",\"ref\":\"q\",\"type\":\"question\",\"title\":\"T\"}\n";
            let file = home.ops_file(&op.repeat(2));
            home.umx(&["batch", "--file", &file])
        }
        Code::DuplicateField => {
            home.graph();
            let file = home.ops_file(
                "{\"op\":\"node add\",\"type\":\"problem\",\"title\":\"T\",\"sets\":[{\"field\":\"owner\",\"value\":\"a\"},{\"field\":\"owner\",\"value\":\"b\"}]}\n",
            );
            home.umx(&["batch", "--file", &file, "--dry-run"])
        }
        Code::BatchUseRefField => {
            home.graph();
            let file = home.ops_file(
                "{\"op\":\"node add\",\"ref\":\"q1\",\"type\":\"question\",\"title\":\"T\"}\n\
                 {\"op\":\"node note\",\"node\":\"q1\",\"append\":\"more\"}\n",
            );
            home.umx(&["batch", "--file", &file, "--dry-run"])
        }
        Code::BatchRefsFromDryRun | Code::BatchRefsOtherGraph | Code::BatchRefConflict => {
            home.graph();
            let first = home.ops_file(
                "{\"op\":\"node add\",\"ref\":\"q\",\"type\":\"question\",\"title\":\"T\"}\n",
            );
            let mut args = vec!["batch", "--file", &first];
            if code == Code::BatchRefsFromDryRun {
                args.push("--dry-run");
            }
            let (receipt, _) = home.receipt("first", &args);
            let mut args = vec!["batch", "--refs-from", &receipt, "--file"];
            let edit =
                home.ops_file("{\"op\":\"node edit\",\"node_ref\":\"q\",\"title\":\"Renamed\"}\n");
            let target = home.path.join("target").display().to_string();
            match code {
                // A line defines the ref the receipt imports: refused at the line, never shadowed.
                Code::BatchRefConflict => args.push(&first),
                // The receipt is demo's; the batch writes target.
                Code::BatchRefsOtherGraph => {
                    home.ok(&["graph", "new", &target, "--alias", "target"]);
                    args.extend([edit.as_str(), "--graph", "target"]);
                }
                _ => args.push(&edit),
            }
            home.umx(&args)
        }
        // ws003 s09: umx batch --idempotency-key.
        Code::BatchReplayed
        | Code::IdempotencyKeyReused
        | Code::IdempotencyReceiptStale
        | Code::ReceiptsUnreadable
        | Code::IdempotencyKeyExpired => {
            let path = home.graph();
            let receipts = Path::new(&path).join("receipts.jsonl");
            let one = home.ops_file(
                "{\"op\":\"node add\",\"ref\":\"q\",\"type\":\"question\",\"title\":\"T\"}\n",
            );
            let keyed = |file: &str| -> Vec<String> {
                ["batch", "--idempotency-key", "window-08", "--file", file]
                    .map(String::from)
                    .to_vec()
            };
            let args = keyed(&one);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match code {
                Code::BatchReplayed => {
                    home.ok(&args);
                }
                Code::IdempotencyKeyReused => {
                    home.ok(&args);
                    let two = home.ops_file(
                        "{\"op\":\"node add\",\"ref\":\"q\",\"type\":\"question\",\"title\":\"T2\"}\n",
                    );
                    let other = keyed(&two);
                    return home.umx(&other.iter().map(String::as_str).collect::<Vec<_>>());
                }
                // The kept receipt names an id the graph does not hold (nodes.jsonl changed under it).
                Code::IdempotencyReceiptStale => {
                    home.ok(&args);
                    let forged: String = fs::read_to_string(&receipts)
                        .unwrap()
                        .lines()
                        .map(|line| {
                            let mut v: Value = umx_protocol::parse_json(line).unwrap();
                            if v.get("refs").is_some() {
                                v["refs"] = json!(["01JC8XA4H7B3KT9M2NZ5QW6RDE"]);
                            }
                            format!("{v}\n")
                        })
                        .collect();
                    fs::write(&receipts, forged).unwrap();
                }
                Code::ReceiptsUnreadable => {
                    fs::write(&receipts, "not a receipts file\n").unwrap();
                }
                // A key whose receipt the bounds dropped: the file keeps its digest only.
                _ => {
                    let (_, probe) = home.receipt("probe", &["batch", "--file", &one, "--dry-run"]);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;
                    let file = format!(
                        "{{\"umx_receipts\":1,\"graph_id\":{}}}\n{{\"key\":\"{}\",\"at\":0,\"dropped\":{now}}}\n",
                        probe.data["graph_id"],
                        umx_core::receipts::key_digest("window-08"),
                    );
                    fs::write(&receipts, file).unwrap();
                }
            }
            home.umx(&args)
        }
        Code::BatchRefSource => {
            // A receipt whose ref names no node of the graph (here written by hand, as a stale receipt would be).
            home.graph();
            let receipt = home.path.join("stale.json");
            let env = Envelope::ok(
                "batch",
                json!({ "applied": true, "dry_run": false, "graph": "demo", "rows": [],
                        "refs": { "q": "01JC8XA4H7B3KT9M2NZ5QW6RDE" } }),
            );
            fs::write(&receipt, env.to_json()).unwrap();
            let edit =
                home.ops_file("{\"op\":\"node edit\",\"node_ref\":\"q\",\"title\":\"Renamed\"}\n");
            home.umx(&[
                "batch",
                "--refs-from",
                receipt.to_str().unwrap(),
                "--file",
                &edit,
            ])
        }
        Code::DoctorServiceThrottled => {
            let pid = home.start().data["pid"].to_string();
            home.install();
            home.service_state("pid", &pid);
            home.service_file_edit(|text| {
                text.replace("<string>Standard</string>", "<string>Background</string>")
                    .replace(
                        "Restart=on-failure",
                        "IOSchedulingClass=idle\nRestart=on-failure",
                    )
            });
            home.umx(&["doctor"])
        }
        Code::DoctorServicePath => {
            let pid = home.start().data["pid"].to_string();
            home.install();
            home.service_state("pid", &pid);
            // A service file written before umx set PATH.
            home.service_file_edit(|text| {
                let mut kept = Vec::new();
                let mut lines = text.lines();
                while let Some(line) = lines.next() {
                    if line.trim() == "<key>PATH</key>" {
                        lines.next();
                    } else if !line.starts_with("Environment=\"PATH=") {
                        kept.push(line);
                    }
                }
                kept.join("\n") + "\n"
            });
            home.umx(&["doctor"])
        }
        Code::DoctorServiceElsewhere => {
            let pid = home.start().data["pid"].to_string();
            home.install();
            home.service_state("pid", &pid);
            home.service_runs("/bin/sh");
            home.umx(&["doctor"])
        }
        Code::DoctorGraphProblems => {
            let path = home.graph();
            let (a, b) = (home.add("problem", "One"), home.add("decision", "Two"));
            home.umx(&["edge", "add", &a, "blocks", &b]);
            let nodes = Path::new(&path).join("nodes.jsonl");
            let kept: String = fs::read_to_string(&nodes)
                .unwrap()
                .lines()
                .filter(|l| !l.contains(&b))
                .map(|l| format!("{l}\n"))
                .collect();
            fs::write(&nodes, kept).unwrap();
            home.umx(&["doctor"])
        }
        Code::VectorShardUnreadable => {
            // A graph with a model (so pending is counted), a node, then a folder where its shard file belongs.
            home.start();
            let path = home.path.join("g");
            let p = path.display().to_string();
            home.umx(&[
                "graph",
                "new",
                &p,
                "--alias",
                "demo",
                "--embedding-model",
                "m@8",
            ]);
            let id = home.add("problem", "Slow builds");
            let tail = id[id.len() - 2..].to_lowercase();
            fs::create_dir_all(path.join(format!("embeddings/m@8/{tail}.tsv"))).unwrap();
            home.umx(&["node", "edit", &id, "--title", "Very slow builds"])
        }
        Code::SecuredEmbedRefused => {
            home.az_active("");
            home.secured_graph();
            home.umx(&["embeddings", "reembed", "--graph", "demo"])
        }
        Code::ReembedModelNotConfigured => {
            home.az_active("");
            home.graph();
            home.umx(&[
                "embeddings",
                "reembed",
                "--graph",
                "demo",
                "--model",
                "other-model@8",
                "--dry-run",
            ])
        }
        Code::EmbedderMismatch => {
            home.az_active("");
            home.start();
            let path = home.path.join("g").display().to_string();
            home.umx(&[
                "graph",
                "new",
                &path,
                "--alias",
                "demo",
                "--embedding-model",
                "other-model@8",
            ])
        }
        Code::EmbedderNotLocalForSecured => {
            home.az_active("");
            home.secured_graph();
            home.umx(&["graph", "load", &home.path.join("g").display().to_string()])
        }
        Code::EmbedderUnavailable => {
            home.az_active("");
            home.start();
            home.umx(&["embeddings", "probe"])
        }
        Code::SecuredShardsRemain => {
            let path = home.graph();
            open_period_shards(&path, &["ab"]);
            home.umx(&["graph", "set", "demo", "--policy", "secured"])
        }
        Code::SecuredShardsUnchecked => {
            let path = home.graph();
            open_period_shards(&path, &["ab"]);
            let embeddings = Path::new(&path).join("embeddings");
            fs::set_permissions(&embeddings, fs::Permissions::from_mode(0o000)).unwrap();
            let result = home.umx(&["graph", "set", "demo", "--policy", "secured"]);
            fs::set_permissions(&embeddings, fs::Permissions::from_mode(0o755)).unwrap();
            result
        }
        Code::DoctorLocalEmbedderSlow => local_on_cpu(&mut home, "1"),
        Code::DoctorLocalEmbedderCpu => local_on_cpu(&mut home, "4"),
        Code::ConfigImportRefused => {
            home.start();
            home.umx(&["config", "import", "--from", "flowspace3"])
        }
        Code::ConfigImportExists => {
            let dir = fs3_folder(&home, FS3_CONFIG);
            home.env
                .push(("FS3_CONFIG_DIR".into(), dir.display().to_string()));
            home.start();
            home.ok(&["config", "import", "--from", "flowspace3"]);
            home.umx(&["config", "import", "--from", "flowspace3"])
        }
        Code::ImportKeyUnmatched | Code::SimilarityFloorUncalibrated => {
            let dir = fs3_folder(&home, FS3_CONFIG);
            home.env
                .push(("FS3_CONFIG_DIR".into(), dir.display().to_string()));
            home.start();
            home.umx(&["config", "import", "--from", "flowspace3", "--dry-run"])
        }
        Code::EmbedderNotInUse => unreachable!("raised in umx-daemon's s27 tests"),
        Code::GraphfileChanged => {
            unreachable!("raised by umx-services' tests/graphfile.rs (a racing config files port)")
        }
        Code::ConfigKeyDeprecated => {
            // D62: a trusted_endpoint left from D53 is ignored with a warning, never refused.
            home.az_active("embedders.az.trusted_endpoint = \"https://127.0.0.1:9\"\n");
            home.start();
            home.umx(&["config", "check"])
        }
        Code::EmbedderRouteChanged => {
            // D62, security s20b: a named graph of az's model, then az repointed.
            home.az_active("");
            home.start();
            let path = home.path.join("g").display().to_string();
            home.ok(&[
                "graph",
                "new",
                &path,
                "--alias",
                "demo",
                "--embedding-model",
                "text-embedding-3-small@256",
            ]);
            home.ok(&["graph", "set", "demo", "--visibility", "named"]);
            home.umx(&[
                "config",
                "set",
                "embedders.az.endpoint",
                "https://127.0.0.2:9",
            ])
        }
        Code::EmbedderInvalid => {
            home.write_config(
                "embedder.active = \"nomic\"\nembedders.nomic.kind = \"local\"\n\
                 embedders.nomic.model = \"nomic-embed-text-v1.5\"\nembedders.nomic.dims = 256\n",
            );
            home.start();
            home.umx(&["embeddings", "probe"])
        }
        Code::SecretSuspected => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "Export fails",
                "--note",
                &fake_aws_key(),
            ])
        }
        Code::SecretAllowRefused => {
            // An allows file umx cannot parse is never overwritten: the allow is refused and the text stays held.
            home.graph();
            let (env, _) = home.umx(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "Export fails",
                "--note",
                &fake_aws_key(),
            ]);
            let id = env.data["node"]["id"].as_str().unwrap().to_owned();
            let dir = home.path.join("allows");
            fs::create_dir_all(&dir).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            let file = dir.join("secret-allows.json");
            fs::write(&file, "not json").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            home.umx(&["embeddings", "allow", "--node", &id, "--field", "note"])
        }
        Code::SimilarityUnavailable => {
            // With no embedder configured, a search is word-only and says so.
            home.graph();
            home.umx(&["search", "anything"])
        }
        Code::SimilarityNoEmbedder => {
            // A secured graph on a machine whose only instance is hosted: words only, and why (s32).
            home.az_active("");
            home.secured_graph();
            home.umx(&["search", "anything", "--graph", "demo"])
        }
        Code::WordPrefixCapped => {
            // 70 words start with `plank`: more than the 64 a prefix may match (req-92).
            home.graph();
            let ops: String = (0..70u8)
                .map(|i| {
                    format!(
                        "{{\"op\":\"node add\",\"type\":\"problem\",\"title\":\"Fix plank{}{}\"}}\n",
                        (b'a' + i / 26) as char,
                        (b'a' + i % 26) as char
                    )
                })
                .collect();
            let file = home.ops_file(&ops);
            home.umx(&["batch", "--file", &file]);
            home.umx(&["search", "plank"])
        }
        Code::QuerySyntax
        | Code::QueryWriteNotSupported
        | Code::QueryClauseNotSupported
        | Code::QueryFeatureNotSupported
        | Code::QueryAmbiguousLabels
        | Code::QueryStdQualifier
        | Code::QueryUnknownFunction
        | Code::QueryAliasShadowsVariable
        | Code::QueryOrderByEntity
        | Code::QueryWithOrderNeedsLimit
        | Code::QueryUnbounded
        | Code::QueryBoundTooLarge
        | Code::QuerySymmetricDirection
        | Code::QueryTooLarge
        | Code::QueryInvalid
        | Code::QueryParamMissing
        | Code::QueryUnknownVariable
        | Code::QueryColumnReserved
        | Code::QueryTruncated
        | Code::QueryUnknownLabel
        | Code::ProvenanceHidden
        | Code::QueryUnknownRelation
        | Code::QueryUnknownProperty
        | Code::QuerySymmetricInDirectedStep => {
            home.design();
            let (text, flags) = query_trigger(code);
            home.query(&text, flags)
        }
        Code::QueryLabelUnused | Code::QueryRelationUnused | Code::QueryPropertyUnused => {
            home.graph();
            let text = match code {
                Code::QueryLabelUnused => {
                    home.ok(&["ontology", "add", "type", "planned_kind"]);
                    "MATCH (n:planned_kind) RETURN n.id AS id"
                }
                Code::QueryRelationUnused => "MATCH ()-[:answers]->(n) RETURN n.id AS id",
                Code::QueryPropertyUnused => {
                    home.ok(&[
                        "ontology",
                        "add",
                        "field",
                        "problem",
                        "planned_field:string",
                    ]);
                    "MATCH (n) RETURN n.planned_field AS field"
                }
                _ => unreachable!(),
            };
            home.query(text, &[])
        }
        Code::CopiesSkipped | Code::QueryGraphTwice => {
            home.design();
            home.design_copy("design_b");
            let flags: &[&str] = if code == Code::QueryGraphTwice {
                &["--graph", "design,design_b"]
            } else {
                &[]
            };
            home.query("MATCH (n) RETURN count(*) AS nodes", flags)
        }
        Code::CopyAmbiguous => {
            home.design();
            home.design_copy("design_b");
            home.design_copy("design_c");
            home.umx(&["graph", "unload", "design"]);
            home.query("MATCH (n) RETURN count(*) AS nodes", &[])
        }
        Code::PrimaryIsSecured => {
            home.design();
            home.design_copy("design_b");
            home.umx(&["graph", "set", "design_b", "--policy", "secured"]);
            home.umx(&["graph", "primary", "design_b"])
        }
        Code::QueryBusy | Code::QueryTimedOut => {
            unreachable!("raised by umx-services' tests/cypher.rs (permits held, a ticking clock)")
        }
        Code::SimilarityIncomplete => unreachable!("raised by umx-services' hybrid tests"),
        Code::NoStrongMatch => {
            unreachable!("raised by umx-daemon's s08_tests and umx-services' hybrid_recall")
        }
        Code::SimilarityPairsTooLarge | Code::SimilarityPairsTimedOut => {
            unreachable!("raised by umx-services' pairs caps test and umx-daemon's s31_tests")
        }
        Code::ModelHashMismatch | Code::ModelDiskSpace => {
            unreachable!("raised by umx-daemon's models tests and the fetcher contract")
        }
        Code::UiNoView => {
            home.graph();
            home.umx(&["ui", "clear"])
        }
        Code::UiOpenRefused => {
            home.secured_graph();
            home.umx(&["ui", "open", "--graph", "demo"])
        }
        Code::UiOpenLeftOut => {
            home.graph();
            home.umx(&["ui", "open", "--graph", "demo,ghost"])
        }
        Code::UiWaitersBusy => {
            home.graph();
            let socket = home.socket();
            let first = raw(&socket, &view_wait(0, &["demo"])).0;
            let epoch = first.data["epoch"].as_str().map(str::to_owned);
            let views: Vec<_> = (0..4)
                .map(|_| {
                    let (socket, epoch) = (socket.clone(), epoch.clone());
                    std::thread::spawn(move || {
                        raw(&socket, &view_wait_at(epoch, 15_000, &["demo"]))
                    })
                })
                .collect();
            // A probe that gets in answers at once (timeout 0); the fifth wait while four sleep is refused.
            let until = Instant::now() + Duration::from_secs(20);
            let busy = loop {
                let probe = raw(&socket, &view_wait(0, &["demo"]));
                if probe
                    .0
                    .errors
                    .first()
                    .is_some_and(|e| e.code() == "ui_waiters_busy")
                {
                    break probe;
                }
                assert!(Instant::now() < until, "four waits never held the daemon");
                std::thread::sleep(Duration::from_millis(50));
            };
            home.umx(&["ui", "clear"]);
            for v in views {
                v.join().unwrap();
            }
            busy
        }
        Code::UiAlreadyRunning => {
            home.graph();
            let env = home.fake_view();
            let env: Vec<(&str, &str)> =
                env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let (up, exit) = home.umx_with(&["ui", "--detach", "--no-open"], &env);
            assert_eq!(exit, 0, "{}", up.to_json());
            let again = home.umx_with(&["ui", "--detach", "--no-open"], &env);
            home.ok(&["ui", "stop"]);
            again
        }
        Code::UiSelectionHidden => {
            home.graph();
            let vault = home.path.join("vault").display().to_string();
            home.umx(&["graph", "new", &vault, "--alias", "vault", "--secured"]);
            let (added, _) = home.umx(&[
                "node", "add", "--type", "problem", "--title", "Hidden", "--graph", "vault",
            ]);
            let id = added.data["node"]["id"].as_str().unwrap().to_owned();
            raw(&home.socket(), &view_wait(0, &["vault"]));
            raw(&home.socket(), &view_select("vault", &[&id]));
            home.umx(&["ui", "selection"])
        }
        Code::UiDirRefused => {
            home.start();
            home.umx_with(&["ui"], &[("UMX_UI_DIR", "/nonexistent/apps/ui")])
        }
        Code::UiNotBuilt | Code::UiNodeNotFound | Code::UiFailed => {
            home.start();
            let app = home.path.join("app");
            fs::create_dir_all(app.join("bin")).unwrap();
            fs::write(app.join("bin/serve.mjs"), "").unwrap();
            if code != Code::UiNotBuilt {
                fs::create_dir_all(app.join(".next/standalone")).unwrap();
                fs::write(app.join(".next/standalone/server.js"), "").unwrap();
            }
            let dir = app.display().to_string();
            if code == Code::UiFailed {
                // A "node" that exits 3 at once; the name also makes the stub's fake host die before it is ready.
                let node = home.path.join("node-dies");
                fs::write(&node, "#!/bin/sh\nexit 3\n").unwrap();
                fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
                let file = home.write_config(&format!("ui.node = \"{}\"\n", node.display()));
                fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            }
            home.umx_with(&["ui"], &[("UMX_UI_DIR", &dir), ("PATH", "/nonexistent")])
        }
        Code::UiStartTimedOut | Code::UiOpenFailed => {
            unreachable!(
                "raised by umx-daemon's ui tests, the UiHost contract and system.ui-host's tests"
            )
        }
        Code::ModelUnknown => home.umx(&["models", "fetch", "no-such-model"]),
        Code::ModelFetchFailed => {
            // The model's folder is a symlink: refused before any byte is fetched, so no network is reached.
            let cache = home.path.join("cache");
            let folder = cache.join(
                "umx/models/ggml-org--embeddinggemma-2-GGUF/6ea3e3a309d12a4779cfda98b8c19800d2d5da71",
            );
            fs::create_dir_all(folder.parent().unwrap()).unwrap();
            let elsewhere = home.path.join("elsewhere");
            fs::create_dir_all(&elsewhere).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &folder).unwrap();
            let cache = cache.display().to_string();
            home.umx_with(
                &["models", "fetch", "embeddinggemma-2"],
                &[("XDG_CACHE_HOME", &cache)],
            )
        }
        Code::SearchResultsCut => {
            // PRD 002 search-13: 12 nodes hold the word, --limit 5 shows 5.
            home.graph();
            for i in 0..12 {
                home.ok(&[
                    "node",
                    "add",
                    "--type",
                    "problem",
                    "--title",
                    &format!("Walrus report {i}"),
                ]);
            }
            home.umx(&["search", "walrus", "--limit", "5"])
        }
        Code::SecuredGraphsSkipped => {
            home.secured_graph();
            home.umx(&["search", "anything"])
        }
        Code::SecuredGraphsRead => {
            home.secured_graph();
            home.umx(&["search", "anything", "--all", "--include-named"])
        }
        Code::SettingUnknown => {
            home.graph();
            home.umx(&["graph", "set", "demo", "--colour", "blue"])
        }
        Code::DescriptionTooLong => {
            home.graph();
            let long = format!("status:open={}", "x".repeat(501));
            home.umx(&["graph", "set", "demo", "--description", &long])
        }
        Code::DescriptionUndeclared => {
            home.graph();
            home.umx(&[
                "graph",
                "set",
                "demo",
                "--description",
                "type:widget=A thing",
            ])
        }
        Code::PurposeMissing | Code::DescriptionsMissing => {
            home.graph();
            home.umx(&["graph", "guide", "demo"])
        }
        Code::FieldDataByDefault => {
            // PRD 002 search-04: a string field declared with no role is data.
            home.graph();
            home.umx(&["ontology", "add", "field", "deal", "summary:string"])
        }
        Code::FieldLooksLikeLabels | Code::FieldLooksLikeProse => {
            // PRD 002 search-05: a label field declared content and a prose field declared data.
            home.graph();
            home.ok(&[
                "ontology",
                "add",
                "field",
                "deal",
                "tier:string",
                "--role",
                "content",
            ]);
            home.ok(&[
                "ontology",
                "add",
                "field",
                "deal",
                "summary:string",
                "--role",
                "data",
            ]);
            for i in 0..6 {
                let tier = format!("tier={}", ["gold", "silver"][i % 2]);
                let summary =
                    format!("summary=the supplier asked for a longer review window in round {i}");
                home.ok(&[
                    "node",
                    "add",
                    "--type",
                    "deal",
                    "--title",
                    &format!("Deal {i}"),
                    "--set",
                    &tier,
                    "--set",
                    &summary,
                ]);
            }
            home.umx(&["graph", "guide", "demo"])
        }
        Code::SettingSecretSuspected => {
            home.graph();
            home.umx(&["graph", "set", "demo", "--purpose", &fake_aws_key()])
        }
        Code::Internal => {
            home.run_dir(0o700);
            let listener = UnixListener::bind(home.socket()).unwrap();
            let garbage = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let _ = s.read(&mut [0; 4096]);
                s.write_all(b"not an envelope").unwrap();
            });
            let result = home.umx(&["daemon", "stop"]);
            garbage.join().unwrap();
            result
        }
    }
}

/// `just dev`'s restart step, run on the test build into a private install dir.
fn dev_restart(home: &Home) -> std::process::Output {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/dev-restart.sh");
    let mut cmd = Command::new("bash");
    sandbox(&mut cmd, &home.path);
    cmd.args([script, home.binary])
        .arg(home.path.join("bin"))
        .arg("dev")
        .env("UMX_HOME", &home.path)
        .bounded_output()
        .unwrap()
}

impl Home {
    /// Runs umx with human output; returns stdout.
    fn human(&self, args: &[&str]) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_umx"));
        cmd.args(args)
            .arg("--human")
            .env("UMX_HOME", &self.path)
            .env("UMX_ROOTS", &self.path)
            .env_remove("UMX_OUTPUT")
            .env_remove("UMX_GRAPH")
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("TERM");
        let out = cmd.bounded_output().unwrap();
        assert!(
            out.status.success(),
            "umx {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

impl Home {
    /// Runs umx with extra env, returning the envelope and exit code.
    fn umx_env(&self, args: &[&str], env: &[(&str, &str)]) -> (Envelope, u8) {
        let mut cmd = self.command(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.bounded_output().unwrap();
        let envelope = Envelope::from_json(&out.stdout).unwrap();
        assert_eq!(envelope.check(), Ok(()), "umx {args:?}");
        (envelope, out.status.code().unwrap() as u8)
    }

    /// Runs a `next` command as printed (it starts with `umx `).
    fn run_next(&self, next: &str) -> (Envelope, u8) {
        let words: Vec<&str> = next.split(' ').skip(1).collect();
        self.umx(&words)
    }

    fn lines(&self, file: &str) -> usize {
        fs::read_to_string(self.path.join(file))
            .unwrap()
            .lines()
            .count()
    }
}

fn older_cli() -> Build {
    Build {
        version: "0.0.0".into(),
        commit: "older".into(),
        built: "older".into(),
    }
}

/// Starts the daemon with its stderr in `<home>/daemon.log`, as the installed service writes it, and makes graph
/// `demo` at `<home>/g`; returns the folder.
fn logged_graph(home: &mut Home) -> String {
    fs::create_dir_all(&home.path).unwrap();
    let log = fs::File::create(home.path.join("daemon.log")).unwrap();
    let mut child = home
        .command(&["daemon", "start"])
        .stdout(Stdio::piped())
        .stderr(log)
        .spawn()
        .unwrap();
    let limit = home.limit();
    ready_line(&mut child, "umx daemon start", limit);
    home.daemon = Some(child);
    let path = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "demo"]);
    path
}

/// Every file under `dir` (all of umx's home: graph folder, state, run folder, daemon.log) holding a needle.
fn files_holding(dir: &Path, needles: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let (path, kind) = (entry.path(), entry.file_type().unwrap());
        if kind.is_dir() {
            found.extend(files_holding(&path, needles));
        } else if kind.is_file() {
            let text = String::from_utf8_lossy(&fs::read(&path).unwrap_or_default()).into_owned();
            if needles.iter().any(|n| text.contains(n)) {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

/// Runs a printed command line through `sh -c`, as an agent pasting it would, with `umx` this build.
fn run_as_printed(home: &Home, line: &str) -> (Envelope, u8) {
    let script = format!("umx() {{ \"$UMX_BIN\" \"$@\"; }}\n{line}");
    let out = Command::new("sh")
        .args(["-c", &script])
        .env("UMX_BIN", env!("CARGO_BIN_EXE_umx"))
        .env("UMX_HOME", &home.path)
        .env("UMX_ROOTS", &home.path)
        .env_remove("UMX_OUTPUT")
        .env_remove("UMX_GRAPH")
        .bounded_output()
        .unwrap();
    let env = Envelope::from_json(&out.stdout)
        .unwrap_or_else(|e| panic!("{line}: {e}: {}", String::from_utf8_lossy(&out.stdout)));
    (env, out.status.code().unwrap() as u8)
}

fn graph_json(dir: &str) -> umx_protocol::Value {
    umx_protocol::parse_json(&fs::read_to_string(format!("{dir}/graph.json")).unwrap()).unwrap()
}

/// The entry of `name` in a guide list (`types`, `statuses` or `relations`).
fn guide_entry<'a>(env: &'a Envelope, list: &str, name: &str) -> &'a umx_protocol::Value {
    env.data[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no {list} entry {name}: {}", env.to_json()))
}

impl Home {
    /// F113: graph demo (node a) links to d1 (node b) and d2 (node c), both then set to named visibility (D53).
    /// Returns a, b, c and the d1 folder.
    fn linked_graphs(&mut self) -> (String, String, String, String) {
        self.graph();
        let a = self.add("problem", "Author");
        let mut far = Vec::new();
        for alias in ["d1", "d2"] {
            let path = self.path.join(alias).display().to_string();
            self.ok(&["graph", "new", &path, "--alias", alias]);
            let node = self
                .ok(&[
                    "node", "add", "--type", "problem", "--title", "Far node", "--graph", alias,
                ])
                .data["node"]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let address = format!("umx:{alias}/{node}");
            self.ok(&["edge", "add", &a, "blocks", &address, "--graph", "demo"]);
            far.push((node, path));
        }
        for alias in ["d1", "d2"] {
            self.ok(&["graph", "set", alias, "--visibility", "named"]);
        }
        let (c, _) = far.pop().unwrap();
        let (b, d1) = far.pop().unwrap();
        (a, b, c, d1)
    }
}

fn check_totals(env: &Envelope) -> Vec<(String, u64)> {
    let graphs = env.data["graphs"]
        .as_array()
        .unwrap_or_else(|| panic!("{}", env.to_json()));
    let mut out: Vec<(String, u64)> = graphs
        .iter()
        .map(|g| {
            (
                g["graph"].as_str().unwrap().to_owned(),
                g["total"].as_u64().unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

impl Home {
    /// F113: linked_graphs, plus a link from d2 into graph `gone`, which is then unloaded. Returns the ids doctor must
    /// never show: that edge, its d2 node and the gone node.
    fn linked_graphs_and_a_gone_one(&mut self) -> [String; 3] {
        let (_, _, c, _) = self.linked_graphs();
        let gone = self.path.join("gone").display().to_string();
        self.ok(&["graph", "new", &gone, "--alias", "gone"]);
        let g = self
            .ok(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "Gone node",
                "--graph",
                "gone",
            ])
            .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let address = format!("umx:gone/{g}");
        let edge = self
            .ok(&["edge", "add", &c, "blocks", &address, "--graph", "d2"])
            .data["edge"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.ok(&["graph", "unload", "gone"]);
        [edge, c, g]
    }
}

// ---- req-72: umx's own config ----

fn setting<'a>(show: &'a Envelope, key: &str) -> &'a umx_protocol::Value {
    show.data["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap_or_else(|| panic!("no {key} in {}", show.to_json()))
}

/// Shows that graph verbs are closed with config_invalid while the daemon is up.
fn assert_closed(home: &Home, ready: &Envelope) {
    issue(ready, Code::ConfigInvalid);
    assert_eq!(home.ok(&["daemon", "status"]).data["config"], "invalid");
    let (env, exit) = home.umx(&["graph", "list"]);
    assert_eq!(
        (exit, issue(&env, Code::ConfigInvalid).next()),
        (1, "umx config check")
    );
    let path = home.path.join("g").display().to_string();
    let (env, _) = home.umx(&["graph", "new", &path, "--alias", "demo"]);
    issue(&env, Code::ConfigInvalid);
}

fn assert_open(home: &Home, ready: &Envelope) {
    assert_eq!(ready.status, Status::Ok, "{}", ready.to_json());
    assert_eq!(home.ok(&["daemon", "status"]).data["config"], "ok");
    home.ok(&["graph", "list"]);
}

/// A stub `az` in `bin` that prints a token, as `az account get-access-token --output json` does.
fn fake_az(bin: &Path, token: &str) -> PathBuf {
    let az = bin.join("az");
    fs::write(
        &az,
        format!("#!/bin/sh\necho '{{\"accessToken\":\"{token}\",\"expires_on\":4102444800}}'\n"),
    )
    .unwrap();
    fs::set_permissions(&az, fs::Permissions::from_mode(0o755)).unwrap();
    az
}

/// Everything umx printed for a probe and the daemon status after it, for leak checks.
fn printed(home: &Home, probe: &Envelope) -> String {
    let status = home.ok(&["daemon", "status"]);
    format!("{}\n{}", probe.to_json(), status.to_json())
}

/// graph.json as JSON, edited by `f` and written back, as a pull or a hand edit would.
fn edit_graph_json(folder: &str, f: impl FnOnce(&mut Value)) {
    let file = Path::new(folder).join("graph.json");
    let mut v = umx_protocol::parse_json(&fs::read_to_string(&file).unwrap()).unwrap();
    f(&mut v);
    fs::write(&file, v.to_string()).unwrap();
}

fn listed(home: &Home, alias: &str) -> Value {
    let (env, _) = home.umx(&["graph", "list"]);
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == alias)
        .cloned()
        .unwrap_or_else(|| panic!("{alias} not listed: {}", env.to_json()))
}

fn has(env: &Envelope, code: Code) -> bool {
    env.warnings
        .iter()
        .chain(&env.errors)
        .any(|i| i.code() == code.spec().code)
}

/// A stand-in az that prints a token (logged in) or fails (logged out). It is the only az doctor can find.
fn stub_az(home: &Home, logged_in: bool) -> String {
    let bin = home.path.join("tools");
    fs::create_dir_all(&bin).unwrap();
    let body = if logged_in {
        "#!/bin/sh\necho '{\"accessToken\":\"t\",\"expires_on\":4102444800}'\n"
    } else {
        "#!/bin/sh\necho 'Please run az login' >&2\nexit 1\n"
    };
    fs::write(bin.join("az"), body).unwrap();
    fs::set_permissions(bin.join("az"), fs::Permissions::from_mode(0o755)).unwrap();
    bin.display().to_string()
}

/// The query fixture (crates/umx/tests/fixtures/query-graph), copied into a home and loaded as `fixture`.
/// Node n is 01QN...0n and edge n is 01QE...0n:
///   1 Launch in March, 2 Approval delays, 3 Legal team, 4 Budget ceiling, 5 Switch vendor, 6 Vendor lock in,
///   7 Programme, 8 Workstream north, 9 Unrelated question (no edges), 10 Approval delay backlog.
///   e1 2 blocks 1, e2 2 owned_by 3, e3 4 criterion_for 1, e4 5 answers 2, e5 6 undercuts 5, e6 1 part_of 8,
///   e7 2 part_of 8, e8 8 part_of 7, e9 5 part_of 7, e10 10 similar_to 2, e11 3 supports 4.
fn query_fixture(name: &str) -> Home {
    let mut home = Home::new(name);
    home.start();
    let dir = home.path.join("query-graph");
    fs::create_dir_all(&dir).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-graph");
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        fs::copy(src.join(file), dir.join(file)).unwrap();
    }
    let (env, exit) = home.umx(&["graph", "load", dir.to_str().unwrap()]);
    assert_eq!(exit, 0, "{}", env.to_json());
    home
}

fn n(i: u8) -> String {
    format!("01QN00000000000000000000{i:02}")
}

fn e(i: u8) -> String {
    format!("01QE00000000000000000000{i:02}")
}

/// One column of every row, as text.
fn column(env: &Envelope, name: &str) -> Vec<String> {
    env.data["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("no rows: {}", env.to_json()))
        .iter()
        .map(|r| match &r[name] {
            umx_protocol::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect()
}

/// Shard files as an open graph's embed worker leaves them in its folder (names only; content is not read).
fn open_period_shards(graph: &str, shards: &[&str]) {
    let space = Path::new(graph).join("embeddings/text-embedding-3-small@256");
    fs::create_dir_all(&space).unwrap();
    for shard in shards {
        fs::write(space.join(format!("{shard}.tsv")), "").unwrap();
    }
}

/// Unix-socket file descriptors the process `pid` holds: /proc on Linux, lsof on macOS.
fn unix_sockets(pid: u64) -> usize {
    if cfg!(target_os = "linux") {
        fs::read_dir(format!("/proc/{pid}/fd"))
            .unwrap()
            .filter_map(|e| fs::read_link(e.unwrap().path()).ok())
            .filter(|t| t.to_string_lossy().starts_with("socket:"))
            .count()
    } else {
        let out = Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-U", "-F", "f"])
            .bounded_output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.starts_with('f'))
            .count()
    }
}

impl Home {
    /// Installs the service with the stubs running a real daemon, waits until it answers and returns its pid.
    fn run_service(&self) -> Value {
        self.stub_services();
        self.service_state("run", "");
        self.install();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = self.umx(&["daemon", "status"]).0;
            if status.data["running"] == true {
                return status.data["pid"].clone();
            }
            assert!(
                Instant::now() < deadline,
                "the service daemon did not start"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// No kickstart or systemctl restart in the stub's argv after byte `mark` (install itself restarts on Linux).
    fn no_manager_restart_since(&self, mark: usize) {
        let argv = self.argv();
        let since = &argv[mark..];
        assert!(
            !since.contains("kickstart") && !since.contains("--user restart"),
            "{since}"
        );
    }

    /// `daemon status` data: pid and start stamp, to show a daemon was left alone.
    fn pid_and_start(&self) -> (Value, Value) {
        let data = self.ok(&["daemon", "status"]).data;
        (data["pid"].clone(), data["started_ms"].clone())
    }
}

/// Runs node adds into the default graph while `umx daemon restart` runs, checks the restart succeeded and that
/// every acknowledged write is in the file, and returns the restart's reply. The restart starts as soon as the first
/// write is acknowledged, so writes are always in flight around it: a fixed head start (100 ms) was too short under
/// a loaded gate, where no write had landed yet and every one then met the restart (F78).
fn restart_while_writing(home: &Home) -> Envelope {
    let (acked, (env, exit)) = std::thread::scope(|scope| {
        let (first, landed) = std::sync::mpsc::channel();
        let writer = scope.spawn(move || {
            let mut acked = Vec::new();
            for i in 0..40 {
                let title = format!("n{i}");
                let (env, exit) =
                    home.umx(&["node", "add", "--type", "problem", "--title", &title]);
                if exit == 0 {
                    acked.push(env.data["node"]["id"].as_str().unwrap().to_owned());
                    let _ = first.send(());
                }
            }
            acked
        });
        // The writer drops its sender when it ends, so this fails at once if no write was acknowledged at all.
        landed
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|e| panic!("no write was acknowledged before the restart ({e})"));
        let restarted = home.umx(&["daemon", "restart"]);
        (writer.join().unwrap(), restarted)
    });
    assert_eq!(exit, 0, "{}", env.to_json());
    assert!(!acked.is_empty(), "some writes landed before the restart");
    let nodes = fs::read_to_string(home.path.join("g/nodes.jsonl")).unwrap();
    for id in &acked {
        assert!(nodes.contains(id.as_str()), "acknowledged {id} lost");
    }
    env
}

/// A stand-in umx in `home` that leaves a process holding its stdout (as a daemon that inherited the pipe did on the
/// macOS runner, ws012 s3b), then writes a 256 KiB reply, more than a pipe holds, and exits. Returns the holder's
/// pid file.
fn held_writer_stand_in(home: &mut Home) -> PathBuf {
    fs::create_dir_all(&home.path).unwrap();
    let stand_in = home.path.join("umx-stand-in");
    let pid_file = home.path.join("holder.pid");
    let reply = home.path.join("reply.json");
    let big = "x".repeat(256 * 1024);
    fs::write(
        &reply,
        Envelope::ok("node add", json!({ "big": big })).to_json(),
    )
    .unwrap();
    fs::write(
        &stand_in,
        format!(
            "#!/bin/sh\n/bin/sleep 120 &\necho $! > '{}'\n/bin/cat '{}'\n",
            pid_file.display(),
            reply.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&stand_in, fs::Permissions::from_mode(0o755)).unwrap();
    home.binary = Box::leak(stand_in.display().to_string().into_boxed_str());
    pid_file
}

/// umx for a reply bigger than a pipe's buffer: `Home::umx` waits before it reads stdout, which would block the
/// child once the pipe fills.
fn big(home: &Home, args: &[&str]) -> Envelope {
    let out = home
        .command(args)
        .stdout(Stdio::piped())
        .bounded_output()
        .unwrap();
    let env = Envelope::from_json(&out.stdout).unwrap();
    assert_eq!(env.check(), Ok(()));
    assert_eq!(
        out.status.code(),
        Some(0),
        "umx {args:?}: {}",
        env.to_json()
    );
    env
}

/// The folder files an idempotent replay must leave byte for byte.
fn folder_bytes(path: &str) -> Vec<Option<Vec<u8>>> {
    ["graph.json", "nodes.jsonl", "edges.jsonl", "receipts.jsonl"]
        .iter()
        .map(|f| fs::read(Path::new(path).join(f)).ok())
        .collect()
}

fn batch_echo_external_privacy_case(graph_endpoint: bool) {
    let mut home = Home::new(if graph_endpoint {
        "batch-echo-alias"
    } else {
        "batch-echo-title"
    });
    home.graph();
    let local = home.add("problem", "Local");
    let path = home.path.join("far").display().to_string();
    let made = home.ok(&["graph", "new", &path, "--alias", "alias_plant_147"]);
    let far = made.data["graph"]["id"].as_str().unwrap();
    let node = home.ok(&[
        "node",
        "add",
        "--graph",
        "alias_plant_147",
        "--type",
        "problem",
        "--title",
        "Far title plant147",
    ]);
    let node_id = node.data["node"]["id"].as_str().unwrap();
    let named = if graph_endpoint {
        "umx:alias_plant_147".into()
    } else {
        format!("umx:alias_plant_147/{node_id}")
    };
    let canonical = if graph_endpoint {
        format!("umx:{far}")
    } else {
        format!("umx:{far}/{node_id}")
    };
    let hidden = format!("umx:{far}/{}", if graph_endpoint { far } else { node_id });
    let file = home.ops_file(
        &json!({
            "op":"edge add","from":local,"relation":"supports","to":named,
        })
        .to_string(),
    );
    let open = home.ok(&[
        "batch",
        "--graph",
        "demo",
        "--file",
        &file,
        "--dry-run",
        "--echo-sentences",
    ]);
    let visible = if graph_endpoint {
        "alias_plant_147"
    } else {
        "Far title plant147"
    };
    assert_eq!(
        open.data["sentences"][0]["sentence"],
        format!("Local supports {visible}")
    );
    home.ok(&["graph", "set", "alias_plant_147", "--policy", "secured"]);
    for dry in [true, false] {
        let mut args = vec![
            "batch",
            "--graph",
            "demo",
            "--file",
            &file,
            "--echo-sentences",
        ];
        if dry {
            args.push("--dry-run");
        }
        let got = home.ok(&args);
        assert_eq!(
            got.data["sentences"][0]["sentence"],
            format!("Local supports {hidden}")
        );
        assert!(!got.data["sentences"].to_string().contains(visible));
    }
    home.ok(&["graph", "unload", "alias_plant_147"]);
    let file = home.ops_file(
        &json!({
            "op":"edge add","from":local,"relation":"supports","to":canonical,
        })
        .to_string(),
    );
    for dry in [true, false] {
        let mut args = vec![
            "batch",
            "--graph",
            "demo",
            "--file",
            &file,
            "--echo-sentences",
        ];
        if dry {
            args.push("--dry-run");
        }
        let got = home.ok(&args);
        assert_eq!(
            got.data["sentences"][0]["sentence"],
            format!("Local supports {hidden}")
        );
    }
    let listed = home.ok(&["graph", "list"]);
    assert!(
        listed.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["alias"] != "alias_plant_147")
    );
}

// ---- umx query (req-27, workstream 005 s04) ----------------------------------------------------------------------

/// A query cell as the fixture's expected rows write it: a string bare, a float at full precision, a list as
/// `[a,b]`, null as `null`, a whole node or edge as its id.
fn query_cell(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Number(n) if n.is_f64() => format!("{:?}", n.as_f64().unwrap()),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(query_cell).collect::<Vec<_>>().join(",")
        ),
        Value::Object(o) if o.contains_key("address") => query_cell(&o["id"]),
        other => other.to_string(),
    }
}

/// A query's rows as text, in its columns' order (each row is an object keyed by column).
fn query_rows(env: &Envelope) -> Vec<Vec<String>> {
    let columns: Vec<&str> = env.data["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| columns.iter().map(|c| query_cell(&row[*c])).collect())
        .collect()
}

/// `n` azure_openai instances in table form, which keeps 400 under the 64 KiB config cap; the status reply lists each.
fn many_instances(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "[embedders.e{i}]\nkind=\"azure_openai\"\nendpoint=\"https://h\"\ndeployment=\"d\"\n\
                 model=\"text-embedding-3-small\"\ndims=256\n"
            )
        })
        .collect()
}

/// The resolve fixture (crates/umx/tests/fixtures/resolve-graph) copied into `<home>/<alias>` and loaded as `alias`:
/// A1 = 01RSAAAA...01 (decision "Launch in March", field problem), B1 and B2 = 01RSBBBB...01/02 (one shared 8-char
/// prefix), edge E1 = 01RSEE...01 (B1 blocks A1).
fn resolve_fixture(home: &mut Home, alias: &str) {
    let dir = home.path.join(alias);
    fs::create_dir_all(&dir).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolve-graph");
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        fs::copy(src.join(file), dir.join(file)).unwrap();
    }
    let (env, exit) = home.umx(&["graph", "load", dir.to_str().unwrap(), "--as", alias]);
    assert_eq!(exit, 0, "{}", env.to_json());
}

const A1: &str = "01RSAAAA000000000000000001";

/// flowspace3's config folder in a test home: its config.toml, and a secrets.env holding a fake key that umx may
/// never open (mode 000, so any read fails).
fn fs3_folder(home: &Home, config: &str) -> PathBuf {
    let dir = home.path.join("fs3");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), config).unwrap();
    let secrets = dir.join("secrets.env");
    fs::write(&secrets, "FS3_FAKE_KEY=fs3-key-DO-NOT-LEAK\n").unwrap();
    fs::set_permissions(&secrets, fs::Permissions::from_mode(0o000)).unwrap();
    dir
}

const FS3_CONFIG: &str = "[providers.azure-embed]\n\
    kind = \"azure_openai\"\n\
    endpoint = \"https://example-resource.openai.azure.com\"\n\
    deployment = \"text-embedding-3-small-01\"\n\
    api_version = \"2024-02-01\"\n\
    dimensions = 1024\n\
    api_key_env = \"FS3_FAKE_KEY\"\n\
    [embedder]\n\
    active = \"azure-embed\"\n";

/// Runs a `next` command exactly as umx printed it (no quoting in these).
fn follow(home: &Home, next: &str) -> (Envelope, u8) {
    let words: Vec<&str> = next.split_whitespace().collect();
    assert_eq!(words[0], "umx", "{next}");
    home.umx(&words[1..])
}

impl Home {
    /// Stops the daemon and waits for it to exit.
    fn stop(&mut self) {
        self.ok(&["daemon", "stop"]);
        if let Some(mut child) = self.daemon.take() {
            child.bounded_wait().unwrap();
        }
    }

    fn aliases(&self) -> Vec<String> {
        self.reloaded();
        let list = self.ok(&["graph", "list"]);
        list.data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["alias"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Waits until idle unload has emptied the daemon.
    fn wait_until_idle(&self) {
        let until = Instant::now() + Duration::from_secs(15);
        while !self.aliases().is_empty() {
            assert!(Instant::now() < until, "nothing unloaded when idle");
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn real(path: &str) -> String {
    fs::canonicalize(path).unwrap().display().to_string()
}

/// A `graph list --all` row by alias.
fn indexed(env: &Envelope, alias: &str) -> Value {
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == alias)
        .unwrap_or_else(|| panic!("no {alias} in {}", env.to_json()))
        .clone()
}

/// Each `graph list --all` row's alias and state, in order.
fn index_states(env: &Envelope) -> Vec<(String, String)> {
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["alias"].as_str().unwrap().to_owned(),
                r["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// ws008 E2: a node in each of `graphs` (made and loaded in `home`), each a person titled `title`, with `sets`; returns
/// the addresses in order.
fn e2_people(home: &mut Home, graphs: &[&str], sets: &[&[&str]]) -> Vec<String> {
    home.start();
    let mut out = Vec::new();
    for (i, graph) in graphs.iter().enumerate() {
        let path = home.path.join(graph).display().to_string();
        if home.umx(&["graph", "load", &path, "--as", graph]).1 != 0 {
            home.ok(&["graph", "new", &path, "--alias", graph]);
        }
        let mut args = vec![
            "node", "add", "--graph", graph, "--type", "person", "--title", "Jo",
        ];
        args.extend_from_slice(sets.get(i).copied().unwrap_or(&[]));
        let added = home.ok(&args);
        out.push(added.data["node"]["address"].as_str().unwrap().to_owned());
    }
    out
}

/// The `same_as` lines of `graph`'s edges.jsonl in `home` (provenance capture adds its own session edges).
fn e2_same_as(home: &Home, graph: &str) -> Vec<String> {
    fs::read_to_string(home.path.join(graph).join("edges.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(r#""type":"same_as""#))
        .map(str::to_owned)
        .collect()
}

/// `umx entity show` members' addresses, in the reply's order.
fn e2_members(env: &Envelope) -> Vec<String> {
    env.data["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["address"].as_str().unwrap().to_owned())
        .collect()
}

/// A stand-in `node` for the view's server (ws009 M1b2): it says it is ready on the port it was
/// given (else 47999), arms every launch code it is handed, and on stdin's end (its supervisor gone) stops its group,
/// as bin/serve.mjs does. It leaves its pid beside itself.
const FAKE_NODE: &str = r#"#!/bin/sh
d="$(dirname "$0")"
echo $$ > "$d/server.pid"
echo $$ >> "$d/servers.log"
port=47999
for a in "$@"; do case "$a" in --port=*) port="${a#--port=}" ;; esac; done
printf '{"ready":true,"port":%s}\n' "$port"
while IFS= read -r l; do case "$l" in "code "*) echo '{"code":"armed"}' ;; esac; done
kill -s TERM 0
"#;

impl Home {
    /// A built-looking app folder and the stand-in node on PATH, as env for `umx ui`.
    fn fake_view(&self) -> Vec<(String, String)> {
        let app = self.path.join("app");
        fs::create_dir_all(app.join("bin")).unwrap();
        fs::create_dir_all(app.join(".next/standalone")).unwrap();
        fs::write(app.join("bin/serve.mjs"), "").unwrap();
        fs::write(app.join(".next/standalone/server.js"), "").unwrap();
        let bin = self.path.join("nodebin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("node"), FAKE_NODE).unwrap();
        fs::set_permissions(bin.join("node"), fs::Permissions::from_mode(0o755)).unwrap();
        vec![
            ("UMX_UI_DIR".into(), app.display().to_string()),
            ("PATH".into(), format!("{}:/bin:/usr/bin", bin.display())),
        ]
    }

    fn server_alive(&self) -> bool {
        fs::read_to_string(self.path.join("nodebin/server.pid")).is_ok_and(|pid| {
            Command::new("/bin/kill")
                .args(["-s", "0", pid.trim()])
                .stderr(Stdio::null())
                .bounded_status()
                .is_ok_and(|s| s.success())
        })
    }

    fn ui_running(&self) -> bool {
        self.ok(&["ui", "status"]).data["running"] == true
    }

    /// Whether process `pid` still exists. A supervisor holds the view lock until it exits, after `ui status` already
    /// says not running, so the next launcher waits for this (ws012 s3b: one refused as ui_already_running).
    fn alive(pid: u64) -> bool {
        Command::new("/bin/kill")
            .args(["-s", "0", &pid.to_string()])
            .stderr(Stdio::null())
            .bounded_status()
            .is_ok_and(|s| s.success())
    }

    fn eventually(&self, what: &str, cond: impl Fn(&Home) -> bool) {
        let until = Instant::now() + Duration::from_secs(30);
        while !cond(self) {
            assert!(Instant::now() < until, "never: {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// ws008 E2b: graphs `names` made and loaded in `home`, in that order (so their graph ids rise in that order).
fn e2b_graphs(home: &mut Home, names: &[&str]) {
    home.start();
    for name in names {
        let path = home.path.join(name).display().to_string();
        home.ok(&["graph", "new", &path, "--alias", name]);
    }
}

/// ws008 E2b: a node of `kind` titled `title` in `graph`, with entity keys `keys` (none if empty); its address.
fn e2b_node(home: &Home, graph: &str, kind: &str, title: &str, keys: &str) -> String {
    let set = format!("entity_key={keys}");
    let mut args = vec![
        "node", "add", "--graph", graph, "--type", kind, "--title", title,
    ];
    if !keys.is_empty() {
        args.extend(["--set", set.as_str()]);
    }
    home.ok(&args).data["node"]["address"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Every issue of `code` in `env`.
fn issues(env: &Envelope, code: Code) -> Vec<&Issue> {
    let code = code.spec().code;
    env.errors
        .iter()
        .chain(&env.warnings)
        .filter(|i| i.code() == code)
        .collect()
}

/// Every string under a `secret_suspected` key anywhere in `v`.
fn secret_flag_texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == "secret_suspected" {
                    out.push(child.to_string());
                }
                secret_flag_texts(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| secret_flag_texts(i, out)),
        _ => {}
    }
}

/// F190: a synthetic edge in graph demo between two new nodes, citing a plain pointer and then a secret-shaped one
/// (a fake token built at run time), with a plain custom field `alpha` and a secret-shaped `zeta`: its id. The edge
/// write warns (the positive control). Every flagged value holds "zorblax", so the lint's term places land on it too.
fn f190_edge(home: &Home) -> String {
    let a = home.add("problem", "Synthetic A");
    let b = home.add("problem", "Synthetic B");
    let pointer = format!(
        "https://example.test/zorblax?token=ghp_{}#quote=late",
        "0123456789abcdefghijklmnopqrstuvwxyzAB"
    );
    let zeta = format!("zeta=zorblax uses AKIA{}", "Q7Z3X9K2Q7Z3X9K2");
    let (env, exit) = home.umx(&[
        "edge",
        "add",
        &a,
        "supports",
        &b,
        "--evidence",
        "docs/plain.md#L1",
        "--evidence",
        &pointer,
        "--set-str",
        "alpha=plain words",
        "--set-str",
        &zeta,
    ]);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::SecretSuspected);
    env.data["edge"]["id"].as_str().unwrap().to_owned()
}

/// F190: the places `check --lint` names on edge `id` for the term "Zorblax", sorted.
fn f190_lint_places(home: &Home, id: &str) -> Vec<Value> {
    f139_terms(
        home,
        &json!({"umx_lint_terms": 1, "terms": [{"term": "Zorblax"}]}).to_string(),
        0o600,
    );
    let (env, exit) = home.umx(&["check", "--graph", "demo", "--lint", "--kind", "terms"]);
    assert_eq!(exit, 0, "{}", env.to_json());
    let mut places: Vec<Value> = f139_rows(&env, "terms")
        .into_iter()
        .filter(|(at, _)| *at == json!(id))
        .map(|(_, place)| place)
        .collect();
    places.sort_by_key(|p| p.to_string());
    places
}

/// A one-member JSON object with `key` (built at run time, so no secret-shaped key sits in a `json!` literal).
fn serde_map(key: &str, value: Value) -> Value {
    let mut object = json!({});
    object
        .as_object_mut()
        .unwrap()
        .insert(key.to_owned(), value);
    object
}
/// ws008 E2c: the `umx graph list` id of `alias`.
fn e2c_gid(home: &Home, alias: &str) -> String {
    home.ok(&["graph", "list"]).data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == alias)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn e2c_id(address: &str) -> String {
    address.rsplit('/').next().unwrap().to_owned()
}

/// ws008 TM3: run a batch of `lines` in `graph` and return its reply.
fn tm3_batch(home: &Home, graph: &str, lines: &[Value]) -> Envelope {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let ops = home.ops_file(&text);
    home.ok(&["batch", "--graph", graph, "--file", &ops])
}

/// ws008 TM4: the raised_in edges stored in `graph`, as (from id, to id) pairs, `to` given as its node id.
fn tm4_raised(home: &Home, graph: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = home
        .ok(&["edge", "list", "--graph", graph, "--type", "raised_in"])
        .data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let id = |v: &Value| v.as_str().unwrap().rsplit('/').next().unwrap().to_owned();
            (id(&r["from"]), id(&r["to"]))
        })
        .collect();
    pairs.sort();
    pairs
}

/// A batch of `lines` in g1, with `--raised-in <anchor>` when given.
fn tm4_batch(home: &Home, lines: &[Value], anchor: Option<&str>) -> Envelope {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let ops = home.ops_file(&text);
    let mut args = vec!["batch", "--graph", "g1", "--file", ops.as_str()];
    if let Some(anchor) = anchor {
        args.extend(["--raised-in", anchor]);
    }
    home.ok(&args)
}

/// ws008 TM6: `node status` with `extra` args on `node`, its reply.
fn tm6_status(home: &Home, node: &str, status: &str, extra: &[&str]) -> Envelope {
    let mut args = vec!["node", "status", node, status, "--reason", "as said"];
    args.extend(extra);
    home.ok(&args)
}

/// ws008 TM6: the status_history entries of `node`.
fn tm6_moves(home: &Home, node: &str) -> Vec<Value> {
    home.ok(&["node", "get", node]).data["node"]["status_history"]
        .as_array()
        .unwrap()
        .clone()
}

/// F139: this machine's lint terms file, written as a person would: `text` at `$UMX_HOME/lint/lint-terms.json`
/// with `mode`, in an owner-only folder. Synthetic terms only.
fn f139_terms(home: &Home, text: &str, mode: u32) {
    let dir = home.path.join("lint");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("lint-terms.json");
    fs::write(&file, text).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
}

/// F139: in graph demo, a segment citing `t.ndjson` seq 1-5 and a node raised in it that cites seq 40-45: the
/// segment's id and the node's.
fn f139_apart(home: &Home) -> (String, String) {
    let segment = home
        .ok(&[
            "node",
            "add",
            "--type",
            "segment",
            "--title",
            "Morning",
            "--set",
            "starts_on=2026-11-12T09:00:00Z",
            "--evidence",
            "t.ndjson#seq=1-5",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let node = home
        .ok(&[
            "node",
            "add",
            "--type",
            "risk",
            "--title",
            "Late",
            "--evidence",
            "t.ndjson#seq=40-45",
            "--raised-in",
            &segment,
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (segment, node)
}

/// F139: the lint rows of one kind, as (id, place).
fn f139_rows(env: &Envelope, kind: &str) -> Vec<(Value, Value)> {
    env.data["lint"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == kind)
        .map(|r| (r["id"].clone(), r["place"].clone()))
        .collect()
}

fn f139_full(home: &Home, id: &str) -> Value {
    home.ok(&["node", "get", id]).data["node"]["id"].clone()
}

/// F139: a problem node titled `title` in graph demo (when more than one graph is loaded); its address.
fn f139_add(home: &Home, title: &str) -> String {
    home.ok(&[
        "node", "add", "--graph", "demo", "--type", "problem", "--title", title,
    ])
    .data["node"]["address"]
        .as_str()
        .unwrap()
        .to_owned()
}
/// ws008 TM5: a spine declaration named `name` over anchors of `kind`, ordered by starts_on then step, placing by
/// raised_in then evidence; std names only, so any graph's vocabulary holds them.
fn tm5_decl(name: &str, kind: &str, main: bool) -> Value {
    json!({
        "name": name,
        "title": format!("{kind} by starts_on"),
        "main": main,
        "anchors": { "type": kind, "inside": "part_of" },
        "order": ["starts_on", "step"],
        "place": ["raised_in", "evidence"],
    })
}

/// D71 step 12: writes `decl` into graph.json's spines by hand (as a graph declared before umx reserved the built-in
/// names would hold it), at the spines format, and loads the graph again.
fn s12_store_spine(home: &Home, path: &str, decl: Value) {
    let mut meta = tm5_graph_json(path);
    meta["umx"] = json!(umx_core::model::FORMAT_SPINES);
    meta["spines"] = json!([decl]);
    fs::write(Path::new(path).join("graph.json"), format!("{meta}\n")).unwrap();
    let (env, exit) = home.umx(&["graph", "load", path]);
    assert_eq!(exit, 0, "{}", env.to_json());
}

/// ws008 TM5: graph.json as it is on disk.
fn tm5_graph_json(path: &str) -> Value {
    umx_protocol::parse_json(&fs::read_to_string(Path::new(path).join("graph.json")).unwrap())
        .unwrap()
}

/// ws008 TM5: a graph `demo` with two segments dated a day apart and three points raised in each; the segments'
/// ids.
fn tm5_day(home: &mut Home) -> (String, [String; 2]) {
    let path = home.graph();
    let seg = |title: &str, at: &str| {
        let set = format!("starts_on={at}");
        home.ok(&[
            "node", "add", "--type", "segment", "--title", title, "--set", &set,
        ])
        .data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let segs = [
        seg("Morning", "2026-11-12T09:00:00Z"),
        seg("Afternoon", "2026-11-13T09:00:00Z"),
    ];
    for (i, s) in segs.iter().enumerate() {
        for p in 0..3 {
            let title = format!("Point {i}{p}");
            home.ok(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                &title,
                "--raised-in",
                s,
            ]);
        }
    }
    (path, segs)
}

/// ws006 S23: graph `alpha` holding a node, graph `beta` holding a node with an edge and `umx:` evidence into it.
/// Returns (alpha's folder, alpha's node, beta's node).
fn s23_pair(home: &mut Home) -> (String, String, String) {
    home.start();
    let a = home.path.join("a").display().to_string();
    let b = home.path.join("b").display().to_string();
    home.ok(&["graph", "new", &a, "--alias", "alpha"]);
    home.ok(&["graph", "new", &b, "--alias", "beta"]);
    let x = home
        .ok(&[
            "node", "add", "--graph", "alpha", "--type", "problem", "--title", "Target",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let y = home
        .ok(&[
            "node", "add", "--graph", "beta", "--type", "problem", "--title", "Source",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.ok(&[
        "edge",
        "add",
        "--graph",
        "beta",
        &y,
        "supports",
        &format!("umx:alpha/{x}"),
    ]);
    home.ok(&[
        "node",
        "evidence",
        "--graph",
        "beta",
        &y,
        "add",
        &format!("umx:alpha/{x}#quote=Target"),
    ]);
    (a, x, y)
}

fn s23_meta(folder: &str) -> Value {
    umx_protocol::parse_json(&fs::read_to_string(Path::new(folder).join("graph.json")).unwrap())
        .unwrap()
}

/// Every hint of `code` in `env`.
fn hints_of(env: &Envelope, code: Code) -> Vec<&Issue> {
    env.hints
        .iter()
        .filter(|i| i.code() == code.spec().code)
        .collect()
}

/// The role hints of `env` for `field` (PRD 002 search-05), from the hints list only.
fn role_hints<'a>(env: &'a Envelope, code: Code, field: &str) -> Vec<&'a Issue> {
    let code = code.spec().code;
    env.hints
        .iter()
        .filter(|i| i.code() == code && i.message().starts_with(&format!("Field {field} of type ")))
        .collect()
}

/// The roles `ontology show` reports for the fields of `type_name`, by field name.
fn roles(home: &Home, type_name: &str) -> std::collections::BTreeMap<String, String> {
    let env = home.ok(&["ontology", "show"]);
    env.data["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == type_name)
        .unwrap()["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["name"].as_str().unwrap().to_owned(),
                f["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Runs the command a hint's next prints (`umx ...`), as printed.
fn run_next(home: &Home, issue: &Issue) -> Envelope {
    let next = issue.next().to_owned();
    let args: Vec<&str> = next.strip_prefix("umx ").unwrap().split(' ').collect();
    home.ok(&args)
}

/// The row of `env`'s search rows whose title is `title`.
#[track_caller]
fn hit<'a>(env: &'a Envelope, title: &str) -> &'a Value {
    match env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == title)
    {
        Some(row) => row,
        None => panic!("no hit {title} in {}", env.to_json()),
    }
}

/// The id of a node added with `title` (and any extra args).
fn add_node(home: &Home, title: &str, extra: &[&str]) -> String {
    let mut args = vec!["node", "add", "--type", "problem", "--title", title];
    args.extend_from_slice(extra);
    home.ok(&args).data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The hint with `code` in `env`'s hints list; never in its warnings.
fn hint_of(env: &Envelope, code: Code) -> &Issue {
    let code = code.spec().code;
    assert!(
        !env.warnings.iter().any(|w| w.code() == code),
        "{code} is a hint, not a warning: {}",
        env.to_json()
    );
    env.hints
        .iter()
        .find(|h| h.code() == code)
        .unwrap_or_else(|| panic!("no hint {code} in {}", env.to_json()))
}

/// ws008 T2a (req-98): `demo` (std, as `graph new` makes it) and `plain` (`--no-std`), both loaded; gives plain's
/// folder.
fn std_and_plain(home: &mut Home) -> String {
    home.graph();
    let plain = home.path.join("plain").display().to_string();
    let env = home.ok(&["graph", "new", &plain, "--alias", "plain", "--no-std"]);
    assert_eq!(
        env.data["ontology"]["std"],
        json!(null),
        "{}",
        env.to_json()
    );
    plain
}

fn named<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {list}"))
}

fn strings(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect()
}

/// ws008 T2b: `args` is refused with `code`, whose next is a umx command; nothing about the refusal is a warning.
#[track_caller]
fn refused_with(home: &Home, args: &[&str], code: Code) -> Envelope {
    let (env, exit) = home.umx(args);
    assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
    let found = issue(&env, code);
    assert!(
        found.next().starts_with("umx "),
        "{args:?}: {}",
        env.to_json()
    );
    env
}

/// A batch of `lines` (JSON objects, one per line) through `umx batch`, into graph `graph`.
fn batch_of(home: &Home, graph: &str, lines: &[Value]) -> (Envelope, u8) {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let ops = home.ops_file(&text);
    home.umx(&["batch", "--file", ops.as_str(), "--graph", graph])
}

/// ws008 T2c (req-100): the names of the fields `type_name` has in `ontology show`, in order.
fn shown_fields(home: &Home, graph: &str, type_name: &str) -> Vec<(String, Value)> {
    let env = home.ok(&["ontology", "show", "--graph", graph]);
    named(&env.data["types"], type_name)["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap().to_owned(), f["std"].clone()))
        .collect()
}

/// `args` with `--graph <graph>` after them.
fn on<'a>(graph: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    args.iter().copied().chain(["--graph", graph]).collect()
}

// The tests, by topic: each file is a module of this one, so they share its `Home` and helpers.
#[path = "cli/batch.rs"]
mod batch;
#[path = "cli/catalogue.rs"]
mod catalogue;
#[path = "cli/config.rs"]
mod config;
#[path = "cli/daemon.rs"]
mod daemon;
#[path = "cli/doctor_std.rs"]
mod doctor_std;
#[path = "cli/entity.rs"]
mod entity;
#[path = "cli/entity_neighbours.rs"]
mod entity_neighbours;
#[path = "cli/evidence.rs"]
mod evidence;
#[path = "cli/graph.rs"]
mod graph;
#[path = "cli/node_edge.rs"]
mod node_edge;
#[path = "cli/ontology_std.rs"]
mod ontology_std;
#[path = "cli/provenance.rs"]
mod provenance;
#[path = "cli/rules.rs"]
mod rules;
#[path = "cli/search_query.rs"]
mod search_query;
#[path = "cli/secrets.rs"]
mod secrets;
#[path = "cli/tag_reads.rs"]
mod tag_reads;
#[path = "cli/tags.rs"]
mod tags;
#[path = "cli/ui.rs"]
mod ui;
