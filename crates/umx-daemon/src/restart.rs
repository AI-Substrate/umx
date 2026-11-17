//! `umx daemon restart` (F46). It runs in the binary, not through the daemon, so it can use the service manager.
//! It needs a daemon answering on this UMX_HOME's socket and never starts one from nothing (D13), not even a
//! loaded service that is stopped. When the installed service's process is that daemon, the daemon is stopped
//! through its writer (every queued write applied, later ones refused) and the service manager starts it again;
//! otherwise the daemon replaces itself in place (same pid, new process image, the config read again) and the
//! service, which may run another UMX_HOME, is not touched. Every request has a deadline.

use std::time::{Duration, Instant};

use umx_core::ports::ServiceState;
use umx_protocol::{Build, Code, Envelope, Issue, Request, json};

use crate::doctor::ask_by;
use crate::{Config, mode, service};

const COMMAND: &str = "daemon restart";

/// How long the first `daemon status` may take.
const STATUS: Duration = Duration::from_secs(10);
/// How long the daemon may take to apply the writes queued before the restart and reply.
const DRAIN: Duration = Duration::from_secs(60);
/// How long the restarted daemon may take to answer.
const COME_BACK: Duration = Duration::from_secs(10);
/// How long one probe for the restarted daemon may take.
const PROBE: Duration = Duration::from_secs(2);

pub fn restart(config: &Config, build: &Build) -> Envelope {
    let ask = |command: &str, within: Duration| {
        ask_by(
            &config.socket,
            &Request::new(command, Vec::new(), build.clone()),
            Instant::now() + within,
        )
    };
    let not_running = || {
        Envelope::fail(
            COMMAND,
            Issue::new(Code::DaemonNotRunning).with_subject(config.socket.display().to_string()),
        )
    };
    // A busy daemon is running: say busy, never "start it" (review R129-B2).
    let busy = |reply: &Envelope| {
        reply
            .errors
            .iter()
            .find(|e| e.code() == Code::DaemonBusy.spec().code)
            .map(|e| Envelope::fail(COMMAND, e.clone()))
    };
    let Some(before) = ask("daemon status", STATUS) else {
        return not_running();
    };
    if let Some(busy) = busy(&before) {
        return busy;
    }
    let started = before.data["started_ms"].clone();
    let services = umx_compose::compose(mode(), config);
    let agent = services.service();
    // The service path only when the service's process is the daemon on this socket: its pid proves that, whatever
    // the unit file says (it may be unreadable, or name another UMX_HOME). No service manager here: in place.
    let by_service = agent.adapter_id().is_some()
        && match agent.status() {
            Err(e) => return Envelope::fail(COMMAND, service::issue(&e)),
            Ok(ServiceState::Installed {
                loaded: true,
                pid: Some(pid),
                ..
            }) => before.data["pid"].as_u64() == Some(u64::from(pid)),
            Ok(_) => false,
        };
    let how = if by_service {
        // A clean stop is not a crash, so neither launchd (SuccessfulExit false) nor systemd (on-failure) starts it
        // again by itself; the manager does, once the writer has drained.
        match ask("daemon stop", DRAIN) {
            None => return not_running(),
            Some(reply) if !reply.errors.is_empty() => {
                return Envelope::fail(COMMAND, reply.errors[0].clone());
            }
            Some(_) => {}
        }
        if let Err(e) = agent.restart() {
            return Envelope::fail(COMMAND, service::issue(&e));
        }
        "service"
    } else {
        match ask(COMMAND, DRAIN) {
            None => return not_running(),
            Some(reply) if busy(&reply).is_some() => return busy(&reply).unwrap_or(reply),
            // An older daemon does not know the request: it refuses it as unknown, or as another protocol.
            Some(reply) if !reply.errors.is_empty() => {
                return Envelope::fail(COMMAND, Issue::new(Code::DaemonRestartRefused));
            }
            Some(_) => "in_place",
        }
    };
    // Wait for a daemon that is not the one before: the pid stays the same in place, so its start stamp tells.
    let deadline = Instant::now() + COME_BACK;
    let mut last_busy = None;
    loop {
        // Each probe has its own short deadline, so one connection that is never answered cannot use up the wait
        // (F64); ask_by reads in F59 slices.
        let now = ask_by(
            &config.socket,
            &Request::new("daemon status", Vec::new(), build.clone()),
            deadline.min(Instant::now() + PROBE),
        );
        if let Some(reply) = &now {
            last_busy = busy(reply);
        }
        if let Some(now) = now.filter(|s| s.errors.is_empty())
            && now.data["started_ms"] != started
        {
            return Envelope::ok(
                COMMAND,
                json!({
                    "how": how,
                    "pid": now.data["pid"],
                    "version": now.data["version"],
                    "started_ms": now.data["started_ms"],
                }),
            );
        }
        if Instant::now() >= deadline {
            return last_busy.unwrap_or_else(not_running);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    fn build() -> Build {
        Build {
            version: "t".into(),
            commit: "t".into(),
            built: "t".into(),
        }
    }

    /// A config whose socket a stand-in daemon listens on; `answer` handles each connection.
    fn stand_in(name: &str, answer: fn(UnixStream)) -> Config {
        let home = PathBuf::from(format!("/tmp/umx-rs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("run")).unwrap();
        let config = Config::at(home, Some(true));
        let listener = UnixListener::bind(&config.socket).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer(stream);
            }
        });
        config
    }

    fn code(env: &Envelope) -> &str {
        env.errors.first().map_or("", |e| e.code())
    }

    /// Review R129-B2: a daemon past its slots refuses before reading; restart says busy, not "start the daemon".
    #[test]
    #[ignore = "flaky on CI runners (daemon_not_running instead of daemon_busy); ws012 s3b makes it deterministic and removes this"]
    fn a_daemon_that_refuses_before_reading_is_busy_not_down() {
        let config = stand_in("refused", crate::refuse);
        let env = restart(&config, &build());
        let _ = std::fs::remove_dir_all(&config.home);
        assert_eq!(code(&env), "daemon_busy", "{}", env.to_json());
    }

    /// Review R129-B2: ask_by still reads the answer when its write fails. A 4 MiB request cannot be written into a
    /// peer that answered and closed without reading.
    #[test]
    fn ask_by_reads_a_busy_answer_after_its_write_fails() {
        let config = stand_in("write-fails", crate::refuse);
        let big = Request::new("graph list", vec!["x".repeat(4 << 20)], build());
        let got = crate::doctor::ask_by(
            &config.socket,
            &big,
            Instant::now() + Duration::from_secs(10),
        );
        let _ = std::fs::remove_dir_all(&config.home);
        assert_eq!(got.as_ref().map(code), Some("daemon_busy"), "{got:?}");
    }

    /// Review R129-B2: a busy reply to the restart request itself (the whole request read) is busy, not an older
    /// daemon refusing restart.
    #[test]
    fn a_busy_reply_to_restart_is_busy_not_refused() {
        let config = stand_in("busy-after-read", |mut s| {
            let mut bytes = Vec::new();
            let _ = s.read_to_end(&mut bytes);
            let command = Request::from_json(&bytes)
                .map(|r| r.command)
                .unwrap_or_default();
            let reply = if command == "daemon status" {
                Envelope::ok(
                    command.as_str(),
                    json!({ "running": true, "pid": 1, "started_ms": 1 }),
                )
            } else {
                Envelope::fail(&command, Issue::new(Code::DaemonBusy))
            };
            let _ = s.write_all(reply.to_json().as_bytes());
        });
        let env = restart(&config, &build());
        let _ = std::fs::remove_dir_all(&config.home);
        assert_eq!(code(&env), "daemon_busy", "{}", env.to_json());
    }
}
