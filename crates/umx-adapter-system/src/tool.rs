use std::io::{ErrorKind, Read};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) struct Ran {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Failed {
    Spawn(ErrorKind),
    Wait,
    Timeout,
    Read,
    OutputTooLarge,
    Stop,
}

enum Pending {
    Thread(JoinHandle<()>),
    Child(Child),
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
static FINISHING: Mutex<()> = Mutex::new(());

#[cfg(test)]
std::thread_local! {
    static SPAWN_RETURN_GATE: std::cell::RefCell<Option<mpsc::Receiver<()>>> = const { std::cell::RefCell::new(None) };
    static SPAWN_PID: std::cell::RefCell<Option<mpsc::Sender<u32>>> = const { std::cell::RefCell::new(None) };
    static WORK_DEADLINE_BEFORE_RECEIVE: std::cell::RefCell<Option<Box<dyn FnOnce() -> Instant>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static WORK_DEADLINE_AFTER_SPAWN: std::cell::RefCell<Option<Box<dyn FnOnce() -> Instant>>> = const { std::cell::RefCell::new(None) };
    static HARD_DEADLINE_BEFORE_DRAIN: std::cell::RefCell<Option<Box<dyn FnOnce() -> Instant>>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn register(handle: JoinHandle<()>) {
    PENDING
        .lock()
        .expect("tool helper registry")
        .push(Pending::Thread(handle));
}

fn pending_child(child: Child) {
    PENDING
        .lock()
        .expect("tool helper registry")
        .push(Pending::Child(child));
}

pub(crate) fn finish_tools(by: Instant) -> bool {
    let _finishing = loop {
        match FINISHING.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() < by => {
                std::thread::sleep(
                    Duration::from_millis(2).min(by.saturating_duration_since(Instant::now())),
                );
            }
            Err(std::sync::TryLockError::WouldBlock) => return false,
        }
    };
    loop {
        let pending = {
            let mut list = PENDING.lock().expect("tool helper registry");
            std::mem::take(&mut *list)
        };
        let mut left = Vec::new();
        for item in pending {
            match item {
                Pending::Thread(handle) if handle.is_finished() => {
                    if handle.join().is_err() {
                        eprintln!("umx: a tool cleanup helper failed");
                    }
                }
                Pending::Child(mut child) => match crate::sys::exited_without_reap(child.id()) {
                    Ok(true) => {
                        if child.wait().is_err() {
                            eprintln!("umx: a tool could not be reaped");
                        }
                    }
                    Ok(false) => left.push(Pending::Child(child)),
                    Err(error) if error.kind() == ErrorKind::Interrupted => {
                        left.push(Pending::Child(child))
                    }
                    Err(_) => eprintln!("umx: a tool wait could not be checked"),
                },
                other => left.push(other),
            }
        }
        let empty = {
            let mut list = PENDING.lock().expect("tool helper registry");
            list.extend(left);
            list.is_empty()
        };
        if empty {
            return true;
        }
        if Instant::now() >= by {
            return false;
        }
        std::thread::sleep(
            Duration::from_millis(2).min(by.saturating_duration_since(Instant::now())),
        );
    }
}

fn cleanup(mut child: Child, by: Instant) -> Result<ExitStatus, Failed> {
    if crate::sys::kill_group(child.id()).is_err() {
        pending_child(child);
        return Err(Failed::Stop);
    }
    loop {
        match crate::sys::exited_without_reap(child.id()) {
            Ok(true) => return child.wait().map_err(|_| Failed::Wait),
            Ok(false) if Instant::now() < by => {
                std::thread::sleep(
                    Duration::from_millis(2).min(by.saturating_duration_since(Instant::now())),
                );
            }
            Ok(false) => {
                pending_child(child);
                return Err(Failed::Timeout);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted && Instant::now() < by => {}
            Err(_) => {
                pending_child(child);
                return Err(Failed::Wait);
            }
        }
    }
}

struct Owned {
    child: Option<Child>,
    hard_deadline: Instant,
}

impl Owned {
    fn finish(mut self) -> Result<ExitStatus, Failed> {
        cleanup(self.child.take().expect("owned child"), self.hard_deadline)
    }
}

/// Silent (architect, plan 188 item 6): an adapter does not print. Every failure arm of [`cleanup`] hands the child to
/// the pending registry, which [`finish_tools`] reaps before the process exits; the binary reports only a child still
/// pending then.
impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            let _ = cleanup(child, self.hard_deadline);
        }
    }
}

fn spawn_retrying(command: &mut Command, deadline: Instant) -> Result<Child, Failed> {
    let mut tries = 0;
    loop {
        if Instant::now() >= deadline {
            return Err(Failed::Timeout);
        }
        match command.spawn() {
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy && tries < 100 => {
                tries += 1;
                std::thread::sleep(
                    Duration::from_millis(10)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            other => return other.map_err(|error| Failed::Spawn(error.kind())),
        }
    }
}

pub(crate) fn run(command: Command, deadline: Instant, cap: Option<usize>) -> Result<Ran, Failed> {
    run_until(
        command,
        deadline,
        deadline + Duration::from_millis(250),
        cap,
    )
}

pub(crate) fn run_until(
    mut command: Command,
    work_deadline: Instant,
    hard_deadline: Instant,
    cap: Option<usize>,
) -> Result<Ran, Failed> {
    if Instant::now() >= work_deadline {
        return Err(Failed::Timeout);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    #[cfg(test)]
    let spawn_gate = SPAWN_RETURN_GATE.with(|gate| gate.borrow_mut().take());
    #[cfg(test)]
    let spawn_pid = SPAWN_PID.with(|slot| slot.borrow_mut().take());
    let (spawned, child_rx) = mpsc::channel();
    let handle = std::thread::Builder::new()
        .name("umx-tool-spawn".into())
        .spawn(move || {
            let result = spawn_retrying(&mut command, work_deadline).map(|child| Owned {
                child: Some(child),
                hard_deadline,
            });
            #[cfg(test)]
            {
                if let (Ok(owned), Some(sent)) = (&result, spawn_pid) {
                    let _ = sent.send(owned.child.as_ref().expect("owned child").id());
                }
                if let Some(gate) = spawn_gate {
                    let _ = gate.recv_timeout(Duration::from_secs(180));
                }
            }
            // A dropped receiver drops Owned, including a child whose exec returned late.
            let _ = spawned.send(result);
        })
        .map_err(|e| Failed::Spawn(e.kind()))?;
    register(handle);
    #[cfg(test)]
    let work_deadline = WORK_DEADLINE_BEFORE_RECEIVE
        .with(|hook| hook.borrow_mut().take())
        .map_or(work_deadline, |hook| hook());
    let owned = child_rx
        .recv_timeout(work_deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| Failed::Timeout)??;
    if Instant::now() >= work_deadline {
        return Err(Failed::Timeout);
    }
    #[cfg(test)]
    let work_deadline = WORK_DEADLINE_AFTER_SPAWN
        .with(|hook| hook.borrow_mut().take())
        .map_or(work_deadline, |hook| hook());
    run_spawned(owned, work_deadline, hard_deadline, cap)
}

fn run_spawned(
    mut owned: Owned,
    work_deadline: Instant,
    hard_deadline: Instant,
    cap: Option<usize>,
) -> Result<Ran, Failed> {
    let child = owned.child.as_mut().expect("owned child");
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("stdout is piped");
    crate::sys::set_nonblocking(stdout.as_fd()).map_err(|_| Failed::Read)?;
    let mut output = Vec::new();
    let mut eof = false;
    loop {
        if Instant::now() >= work_deadline {
            return Err(Failed::Timeout);
        }
        if !eof {
            eof = drain(&mut stdout, &mut output, cap, work_deadline)?;
        }
        if crate::sys::exited_without_reap(pid).map_err(|_| Failed::Wait)? {
            let status = owned.finish()?;
            #[cfg(test)]
            let hard_deadline = HARD_DEADLINE_BEFORE_DRAIN
                .with(|hook| hook.borrow_mut().take())
                .map_or(hard_deadline, |hook| hook());
            while !eof {
                eof = drain(&mut stdout, &mut output, cap, hard_deadline)?;
                if !eof {
                    pause(hard_deadline);
                }
            }
            return Ok(Ran {
                status,
                stdout: output,
            });
        }
        pause(work_deadline);
    }
}

fn drain(
    stdout: &mut impl Read,
    output: &mut Vec<u8>,
    cap: Option<usize>,
    by: Instant,
) -> Result<bool, Failed> {
    let mut buffer = [0; 8192];
    loop {
        if Instant::now() >= by {
            return Err(Failed::Timeout);
        }
        let limit = cap.map_or(buffer.len(), |cap| {
            buffer
                .len()
                .min(cap.saturating_sub(output.len()).saturating_add(1))
        });
        match stdout.read(&mut buffer[..limit]) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                output.extend_from_slice(&buffer[..n]);
                if cap.is_some_and(|cap| output.len() > cap) {
                    return Err(Failed::OutputTooLarge);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(Failed::Read),
        }
    }
}

fn pause(by: Instant) {
    std::thread::sleep(Duration::from_millis(2).min(by.saturating_duration_since(Instant::now())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "umx-tool-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn word(&self, name: &str) -> String {
            format!(
                "'{}'",
                self.0
                    .join(name)
                    .display()
                    .to_string()
                    .replace('\'', "'\\''")
            )
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn alive(pid: u32) -> bool {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        let state = state.trim();
        output.status.success() && !state.is_empty() && !state.starts_with('Z')
    }

    fn ready(path: &std::path::Path) {
        let by = Instant::now() + Duration::from_secs(90);
        while !path.exists() {
            assert!(
                Instant::now() < by,
                "fixture did not signal readiness: {path:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    struct Grandchild {
        pid: u32,
        group: u32,
    }
    impl Drop for Grandchild {
        fn drop(&mut self) {
            let output = Command::new("/bin/ps")
                .args(["-p", &self.pid.to_string(), "-o", "pgid="])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()
                .ok()
                == Some(self.group)
            {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", &self.pid.to_string()])
                    .status();
            }
        }
    }

    #[test]
    fn a_late_spawn_return_is_bounded_and_its_child_is_still_owned() {
        let mut command = Command::new("/bin/sleep");
        command.arg("300");
        let (sent, received) = mpsc::channel();
        SPAWN_PID.with(|slot| *slot.borrow_mut() = Some(sent));
        let (release, gate) = mpsc::channel();
        SPAWN_RETURN_GATE.with(|slot| *slot.borrow_mut() = Some(gate));
        let (expired, expiry) = mpsc::channel();
        let start = Instant::now();
        WORK_DEADLINE_BEFORE_RECEIVE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let pid = received.recv_timeout(Duration::from_secs(90)).unwrap();
                assert!(alive(pid), "positive child control before receive expiry");
                let now = Instant::now();
                assert!(
                    (start + Duration::from_secs(120)).saturating_duration_since(now)
                        > Duration::from_secs(10),
                    "readiness must leave the ordinary deadline beyond the return bound"
                );
                expired.send((pid, now)).unwrap();
                now
            }));
        });
        let result = run_until(
            command,
            start + Duration::from_secs(120),
            start + Duration::from_secs(150),
            Some(1024),
        );
        let returned_at = Instant::now();
        release.send(()).unwrap();
        let (pid, expired_at) = expiry.try_recv().expect("receive deadline was injected");
        assert!(finish_tools(Instant::now() + Duration::from_secs(30)));
        assert!(matches!(result, Err(Failed::Timeout)));
        assert!(
            returned_at.duration_since(expired_at) < Duration::from_secs(10),
            "late spawn waited for an ordinary or hard deadline"
        );
        assert!(!alive(pid));
    }

    #[test]
    fn an_exited_leader_keeps_its_group_lease_until_grandchild_stdout_is_closed() {
        let t = Temp::new();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &format!(
                "echo $$ > {}; sleep 300 & echo $! > {}; : > {}; while [ ! -e {} ]; do sleep 0.01; done; printf complete; exit 0",
                t.word("leader"), t.word("grandchild"), t.word("ready"), t.word("release"),
            )]);
        std::thread::scope(|scope| {
            let start = Instant::now();
            let worker = scope.spawn(move || {
                run_until(
                    command,
                    start + Duration::from_secs(120),
                    start + Duration::from_secs(150),
                    Some(1024),
                )
            });
            ready(&t.0.join("ready"));
            let pid = fs::read_to_string(t.0.join("grandchild"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let group = fs::read_to_string(t.0.join("leader"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let _cleanup = Grandchild { pid, group };
            assert!(
                alive(pid),
                "the presence probe has a positive child control"
            );
            fs::write(t.0.join("release"), b"go").unwrap();
            let result = worker
                .join()
                .unwrap()
                .expect("a complete reply from an exited leader");
            assert!(result.status.success());
            assert_eq!(result.stdout, b"complete");
            assert!(finish_tools(Instant::now() + Duration::from_secs(30)));
            let until = Instant::now() + Duration::from_secs(30);
            while alive(pid) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(
                !alive(pid),
                "the grandchild must not retain stdout or keep running"
            );
        });
    }

    #[test]
    fn an_escaped_descendant_cannot_hold_the_stdout_drain_past_the_deadline() {
        let t = Temp::new();
        let mut command = Command::new("/usr/bin/perl");
        command.args(["-MPOSIX=setsid", "-e", &format!(
            "my $pid = fork(); die unless defined $pid; if ($pid == 0) {{ setsid() >= 0 or die; open my $f, '>', {} or die; print $f \"$$\\n\"; close $f; open my $r, '>', {} or die; close $r; sleep 300; exit 0; }} while (!-e {}) {{ select undef, undef, undef, 0.002; }} print 'complete';",
            t.word("escaped"), t.word("ready"), t.word("ready"),
        )]);
        let ready_path = t.0.join("ready");
        let pid_path = t.0.join("escaped");
        let (sent, received) = mpsc::channel();
        HARD_DEADLINE_BEFORE_DRAIN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                ready(&ready_path);
                let pid = fs::read_to_string(pid_path)
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                assert!(
                    alive(pid),
                    "positive escaped writer control before drain expiry"
                );
                let now = Instant::now();
                sent.send((pid, now)).unwrap();
                now
            }));
        });
        let start = Instant::now();
        let result = run_until(
            command,
            start + Duration::from_secs(120),
            start + Duration::from_secs(150),
            Some(1024),
        );
        let returned_at = Instant::now();
        let (pid, expired_at) = received.try_recv().expect("drain deadline was injected");
        let _cleanup = Grandchild { pid, group: pid };
        assert!(alive(pid), "the escaped writer still holds the pipe");
        assert!(matches!(result, Err(Failed::Timeout)));
        assert!(
            returned_at.duration_since(expired_at) < Duration::from_secs(10),
            "stdout drain waited for an ordinary or hard deadline"
        );
        assert!(
            finish_tools(Instant::now() + Duration::from_secs(30)),
            "no reader helper can remain blocked on the escaped descendant"
        );
    }

    /// Plan 188 item 6: a child dropped after its hard deadline (a pij capture that overran under load) is handed to
    /// the registry, silently, and the exit-time finish still reaps it: the old "tool cleanup pending" line reported
    /// an interim state, not a failure.
    #[test]
    fn a_child_dropped_past_its_hard_deadline_is_reaped_by_finish_tools() {
        let mut command = Command::new("/bin/sleep");
        command.arg("300").process_group(0);
        let child = command.spawn().unwrap();
        let pid = child.id();
        assert!(alive(pid), "positive child control");
        drop(Owned {
            child: Some(child),
            hard_deadline: Instant::now(),
        });
        assert!(finish_tools(Instant::now() + Duration::from_secs(30)));
        assert!(!alive(pid));
    }
}
