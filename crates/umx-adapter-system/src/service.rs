//! launchd.service-manager (macOS) and systemd.service-manager (Linux): the umx daemon as a per-user service
//! (req-86). Each writes one file it owns (a launchd agent plist or a systemd user unit) and drives the service
//! manager with fixed argv and no shell. Compose chooses the tool: `/bin/launchctl`, `/usr/bin/systemctl` (else
//! `/bin/systemctl`), or `UMX_SERVICE_TOOL`. It is never run through a shell, its stderr is discarded and it is
//! stopped after [`TOOL_TIMEOUT`]. A tool failure is [`ServiceError::Failed`] and stops the change: only launchctl's
//! documented "no such service" result counts as not loaded. Nothing here needs root.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use umx_core::ports::{ServiceError, ServiceManager, ServiceSpec, ServiceState, unit_safe};

/// `launchctl print`'s exit when the domain has no such service (measured on macOS 2026-11-10: 113; a bad domain
/// gives 112, which is a failure, not absence).
pub const LAUNCHCTL_NO_SUCH_SERVICE: i32 = 113;

/// How long one launchctl or systemctl call may take.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(10);

/// The bound [`run`] holds a tool to: [`TOOL_TIMEOUT`], except in this crate's unit tests. Their stub tools are
/// shell scripts that a gate runs niced (cargo-slot) on a loaded machine, where one once took more than 10 s to finish
/// a `cp` (F79). No unit test waits for the bound itself, so a longer one only lets a slow stub finish.
#[cfg(not(test))]
const RUN_TIMEOUT: Duration = TOOL_TIMEOUT;
#[cfg(test)]
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// One tool call's exit and stdout.
struct Ran {
    ok: bool,
    code: Option<i32>,
    stdout: String,
}

fn run(tool: &str, args: &[&str], timeout: Duration) -> Result<Ran, ServiceError> {
    let step = || format!("{tool} {}", args.first().copied().unwrap_or(""));
    let mut command = Command::new(tool);
    command.args(args);
    let ran =
        crate::tool::run(command, Instant::now() + timeout, None).map_err(
            |failed| match failed {
                crate::tool::Failed::Spawn(ErrorKind::NotFound) => {
                    ServiceError::Failed(format!("{tool} not found"))
                }
                crate::tool::Failed::Spawn(kind) => {
                    ServiceError::Failed(format!("{} could not start ({kind:?})", step()))
                }
                crate::tool::Failed::Timeout => ServiceError::Failed(format!(
                    "{} did not finish within {}s",
                    step(),
                    timeout.as_secs()
                )),
                _ => ServiceError::Failed(format!("{} could not complete", step())),
            },
        )?;
    Ok(Ran {
        ok: ran.status.success(),
        code: ran.status.code(),
        stdout: String::from_utf8_lossy(&ran.stdout).into_owned(),
    })
}

/// Runs a step that must succeed.
fn must(tool: &str, args: &[&str], timeout: Duration) -> Result<Ran, ServiceError> {
    let ran = run(tool, args, timeout)?;
    if ran.ok {
        Ok(ran)
    } else {
        Err(ServiceError::Failed(format!(
            "{tool} {} exited with {}",
            args[0],
            ran.code.map_or("a signal".to_string(), |c| c.to_string())
        )))
    }
}

fn check_paths(spec: &ServiceSpec) -> Result<(), ServiceError> {
    for path in [&spec.program, &spec.home, &spec.log] {
        if !unit_safe(path) {
            return Err(ServiceError::BadPath(path.clone()));
        }
    }
    Ok(())
}

/// Writes `text` to `path` through a temp file and a rename, mode 0644 (launchd refuses a group- or
/// world-writable plist). Creates the folder if needed.
fn write_unit(path: &Path, text: &str) -> Result<(), ServiceError> {
    use std::os::unix::fs::PermissionsExt;
    let failed = |what: &str, e: std::io::Error| {
        ServiceError::Failed(format!("{what} {} ({:?})", path.display(), e.kind()))
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| failed("create the folder of", e))?;
    }
    let tmp = path.with_extension("umx-tmp");
    fs::write(&tmp, text).map_err(|e| failed("write", e))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            failed("write", e)
        })
}

fn read_unit(path: &Path) -> Result<Option<String>, ServiceError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ServiceError::Failed(format!(
            "read {} ({:?})",
            path.display(),
            e.kind()
        ))),
    }
}

fn remove_unit(path: &Path) -> Result<(), ServiceError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ServiceError::Failed(format!(
            "remove {} ({:?})",
            path.display(),
            e.kind()
        ))),
    }
}

/// `Conflict` when an installed unit runs another program or home (or cannot be read back).
fn same_or_conflict(found: Option<ServiceSpec>, spec: &ServiceSpec) -> Result<(), ServiceError> {
    match found {
        Some(f) if f.program == spec.program && f.home == spec.home => Ok(()),
        found => Err(ServiceError::Conflict { found }),
    }
}

/// launchd.service-manager: `~/Library/LaunchAgents/dev.umx.daemon.plist`, loaded into the user's GUI domain.
pub struct Launchd {
    agents: PathBuf,
    tool: String,
    uid: u32,
    label: &'static str,
    timeout: Duration,
}

impl Launchd {
    /// The label: one per user.
    pub const LABEL: &'static str = "dev.umx.daemon";
    /// The label when the tool is overridden away from the default home, so an override never rewrites the real
    /// agent.
    pub const SANDBOX_LABEL: &'static str = "dev.umx.daemon.sandbox";

    /// `agents` is the LaunchAgents folder, `tool` the launchctl to run, `uid` the user whose GUI domain it is.
    pub fn new(agents: PathBuf, tool: impl Into<String>, uid: u32, label: &'static str) -> Launchd {
        Launchd {
            agents,
            tool: tool.into(),
            uid,
            label,
            timeout: RUN_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn plist_path(&self) -> PathBuf {
        self.agents.join(format!("{}.plist", self.label))
    }

    fn target(&self) -> String {
        format!("gui/{}/{}", self.uid, self.label)
    }

    /// The service's pid while launchd has it loaded, from `launchctl print`; `None` inside when it is not running.
    /// `None` only for launchctl's "no such service" exit; any other failure is `Failed`, so a broken tool is never
    /// read as "not loaded" (and uninstall never removes the plist of a service it could not boot out).
    fn loaded(&self) -> Result<Option<Option<u32>>, ServiceError> {
        let ran = run(&self.tool, &["print", &self.target()], self.timeout)?;
        match (ran.ok, ran.code) {
            (true, _) => {}
            (false, Some(LAUNCHCTL_NO_SUCH_SERVICE)) => return Ok(None),
            (false, code) => {
                return Err(ServiceError::Failed(format!(
                    "{} print exited with {}",
                    self.tool,
                    code.map_or("a signal".to_string(), |c| c.to_string())
                )));
            }
        }
        let pid = ran.stdout.lines().find_map(|l| {
            l.trim()
                .strip_prefix("pid = ")
                .and_then(|p| p.trim().parse().ok())
        });
        Ok(Some(pid))
    }
}

/// The agent plist: runs `<program> daemon start --service` at login (RunAtLoad), again after a crash but not after
/// a clean `umx daemon stop` (KeepAlive SuccessfulExit false), with UMX_HOME set and output appended to the log.
pub fn plist(label: &str, spec: &ServiceSpec) -> String {
    let x = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by `umx daemon install`; removed by `umx daemon uninstall`. -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
		<string>daemon</string>
		<string>start</string>
		<string>--service</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>UMX_HOME</key>
		<string>{home}</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>2</integer>
	<key>ProcessType</key>
	<string>Background</string>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#,
        program = x(&spec.program),
        home = x(&spec.home),
        log = x(&spec.log),
    )
}

/// Reads back what [`plist`] wrote: the first ProgramArguments string, UMX_HOME and StandardOutPath.
fn plist_spec(text: &str) -> Option<ServiceSpec> {
    let unx = |s: &str| {
        s.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    };
    let after = |key: &str| -> Option<String> {
        let rest = &text[text.find(&format!("<key>{key}</key>"))?..];
        let start = rest.find("<string>")? + "<string>".len();
        let end = start + rest[start..].find("</string>")?;
        Some(unx(&rest[start..end]))
    };
    Some(ServiceSpec {
        program: after("ProgramArguments")?,
        home: after("UMX_HOME")?,
        log: after("StandardOutPath")?,
    })
}

impl ServiceManager for Launchd {
    fn adapter_id(&self) -> &'static str {
        "launchd.service-manager"
    }

    fn status(&self) -> Result<ServiceState, ServiceError> {
        let Some(text) = read_unit(&self.plist_path())? else {
            return Ok(ServiceState::NotInstalled);
        };
        let loaded = self.loaded()?;
        Ok(ServiceState::Installed {
            spec: plist_spec(&text),
            loaded: loaded.is_some(),
            pid: loaded.flatten(),
        })
    }

    fn install(&self, spec: &ServiceSpec) -> Result<(), ServiceError> {
        check_paths(spec)?;
        if let Some(text) = read_unit(&self.plist_path())? {
            same_or_conflict(plist_spec(&text), spec)?;
        }
        // Inspect before writing: a failing launchctl leaves the plist as it was (review F42).
        let loaded = self.loaded()?;
        write_unit(&self.plist_path(), &plist(self.label, spec))?;
        // A loaded agent keeps its old definition: unload it so bootstrap reads the new one and starts the new build.
        if loaded.is_some() {
            must(&self.tool, &["bootout", &self.target()], self.timeout)?;
        }
        let domain = format!("gui/{}", self.uid);
        let plist = self.plist_path().display().to_string();
        must(&self.tool, &["bootstrap", &domain, &plist], self.timeout).map(drop)
    }

    fn uninstall(&self) -> Result<(), ServiceError> {
        if self.loaded()?.is_some() {
            must(&self.tool, &["bootout", &self.target()], self.timeout)?;
        }
        remove_unit(&self.plist_path())
    }

    fn restart(&self) -> Result<(), ServiceError> {
        if self.loaded()?.is_none() {
            return Err(ServiceError::Failed(
                "the umx agent is not loaded".to_string(),
            ));
        }
        must(
            &self.tool,
            &["kickstart", "-k", &self.target()],
            self.timeout,
        )
        .map(drop)
    }
}

/// systemd.service-manager: `~/.config/systemd/user/umx.service`, enabled in the user's systemd.
pub struct Systemd {
    units: PathBuf,
    tool: String,
    unit: &'static str,
    timeout: Duration,
}

impl Systemd {
    /// The unit: one per user.
    pub const UNIT: &'static str = "umx.service";
    /// The unit when the tool is overridden away from the default home (see [`Launchd::SANDBOX_LABEL`]).
    pub const SANDBOX_UNIT: &'static str = "umx-sandbox.service";

    /// `units` is the user unit folder (`$XDG_CONFIG_HOME/systemd/user`), `tool` the systemctl to run.
    pub fn new(units: PathBuf, tool: impl Into<String>, unit: &'static str) -> Systemd {
        Systemd {
            units,
            tool: tool.into(),
            unit,
            timeout: RUN_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn unit_path(&self) -> PathBuf {
        self.units.join(self.unit)
    }

    fn systemctl(&self, args: &[&str]) -> Result<Ran, ServiceError> {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        must(&self.tool, &all, self.timeout)
    }

    /// (enabled, pid while active), from `systemctl --user show`.
    fn state(&self) -> Result<(bool, Option<u32>), ServiceError> {
        let ran = self.systemctl(&[
            "show",
            self.unit,
            "--property=UnitFileState,ActiveState,MainPID",
        ])?;
        let value = |key: &str| {
            ran.stdout
                .lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('=').map(str::to_owned))
        };
        let enabled = value("UnitFileState").as_deref() == Some("enabled");
        let active = value("ActiveState").as_deref() == Some("active");
        let pid = value("MainPID")
            .and_then(|p| p.parse().ok())
            .filter(|p| active && *p > 0);
        Ok((enabled, pid))
    }
}

/// The user unit: runs `<program> daemon start --service`, again after a failure but not after a clean `umx daemon
/// stop` (Restart=on-failure), started at login (WantedBy=default.target), output appended to the log.
pub fn unit(spec: &ServiceSpec) -> String {
    format!(
        "# Written by `umx daemon install`; removed by `umx daemon uninstall`.
[Unit]
Description=umx daemon

[Service]
ExecStart=\"{program}\" daemon start --service
Environment=\"UMX_HOME={home}\"
Restart=on-failure
RestartSec=2
StandardOutput=append:{log}
StandardError=append:{log}

[Install]
WantedBy=default.target
",
        program = spec.program,
        home = spec.home,
        log = spec.log,
    )
}

/// Reads back what [`unit`] wrote.
fn unit_spec(text: &str) -> Option<ServiceSpec> {
    let line = |prefix: &str| text.lines().find_map(|l| l.strip_prefix(prefix));
    Some(ServiceSpec {
        program: line("ExecStart=\"")?.split('"').next()?.to_owned(),
        home: line("Environment=\"UMX_HOME=")?
            .strip_suffix('"')?
            .to_owned(),
        log: line("StandardOutput=append:")?.to_owned(),
    })
}

impl ServiceManager for Systemd {
    fn adapter_id(&self) -> &'static str {
        "systemd.service-manager"
    }

    fn status(&self) -> Result<ServiceState, ServiceError> {
        let Some(text) = read_unit(&self.unit_path())? else {
            return Ok(ServiceState::NotInstalled);
        };
        let (loaded, pid) = self.state()?;
        Ok(ServiceState::Installed {
            spec: unit_spec(&text),
            loaded,
            pid,
        })
    }

    fn install(&self, spec: &ServiceSpec) -> Result<(), ServiceError> {
        check_paths(spec)?;
        if let Some(text) = read_unit(&self.unit_path())? {
            same_or_conflict(unit_spec(&text), spec)?;
        }
        // Inspect before writing: a failing systemctl leaves the unit as it was (review F42).
        self.state()?;
        write_unit(&self.unit_path(), &unit(spec))?;
        self.systemctl(&["daemon-reload"])?;
        self.systemctl(&["enable", self.unit])?;
        // restart starts it if it is stopped, and runs the new build if it was running.
        self.systemctl(&["restart", self.unit]).map(drop)
    }

    fn uninstall(&self) -> Result<(), ServiceError> {
        if read_unit(&self.unit_path())?.is_none() {
            return Ok(());
        }
        self.systemctl(&["disable", "--now", self.unit])?;
        remove_unit(&self.unit_path())?;
        self.systemctl(&["daemon-reload"]).map(drop)
    }

    fn restart(&self) -> Result<(), ServiceError> {
        if !self.state()?.0 {
            return Err(ServiceError::Failed(
                "the umx unit is not enabled".to_string(),
            ));
        }
        self.systemctl(&["restart", self.unit]).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh folder with a stub tool in it; the stub records each argv line in `argv` and keeps its state in
    /// files beside it. Never the real launchctl or systemctl.
    fn sandbox(stub: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "umx-svc-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        let tool = dir.join("bin/tool");
        fs::write(&tool, stub).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    const LAUNCHCTL: &str = r#"#!/bin/sh
d="$(dirname "$0")"
echo "$*" >> "$d/argv"
case "$1" in
  bootstrap) [ -e "$d/loaded" ] && exit 5; cp "$3" "$d/loaded"; exit 0 ;;
  bootout) [ -e "$d/loaded" ] || exit 3; rm -f "$d/loaded"; exit 0 ;;
  print) [ -e "$d/inspectfail" ] && exit 1; [ -e "$d/loaded" ] || exit 113; printf '%s = {\n\tstate = running\n\tpid = 4242\n}\n' "$2"; exit 0 ;;
  kickstart) [ -e "$d/loaded" ] || exit 113; exit 0 ;;
esac
exit 64
"#;

    const SYSTEMCTL: &str = r#"#!/bin/sh
d="$(dirname "$0")"
echo "$*" >> "$d/argv"
[ "$1" = "--user" ] || exit 64
shift
case "$1" in
  daemon-reload) exit 0 ;;
  enable) touch "$d/enabled"; exit 0 ;;
  disable) rm -f "$d/enabled" "$d/active"; exit 0 ;;
  restart) [ -e "$d/enabled" ] || exit 5; touch "$d/active"; exit 0 ;;
  show)
    [ -e "$d/inspectfail" ] && exit 1
    if [ -e "$d/enabled" ]; then echo UnitFileState=enabled; else echo UnitFileState=disabled; fi
    if [ -e "$d/active" ]; then printf 'ActiveState=active\nMainPID=4242\n'; else printf 'ActiveState=inactive\nMainPID=0\n'; fi
    exit 0 ;;
esac
exit 64
"#;

    fn launchd() -> Launchd {
        let dir = sandbox(LAUNCHCTL);
        Launchd::new(
            dir.join("LaunchAgents"),
            dir.join("bin/tool").display().to_string(),
            501,
            Launchd::LABEL,
        )
    }

    fn systemd() -> Systemd {
        let dir = sandbox(SYSTEMCTL);
        Systemd::new(
            dir.join("systemd/user"),
            dir.join("bin/tool").display().to_string(),
            Systemd::UNIT,
        )
    }

    fn argv(dir: &Path) -> Vec<String> {
        fs::read_to_string(dir.join("bin/argv"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    mod launchd_contract {
        use super::*;
        umx_contracts::service_manager_contract!(
            launchd;
            sm_c1_fresh_is_not_installed(),
            sm_c2_install_is_read_back_and_loaded(),
            sm_c3_installing_the_same_spec_again_is_an_upgrade(),
            sm_c4_another_program_or_home_is_a_conflict_and_changes_nothing(),
            sm_c5_uninstall_leaves_nothing_and_can_repeat(),
            sm_c6_an_unsafe_path_is_refused_and_changes_nothing(),
            sm_c7_restart_needs_an_installed_service(),
        );
    }

    mod systemd_contract {
        use super::*;
        umx_contracts::service_manager_contract!(
            systemd;
            sm_c1_fresh_is_not_installed(),
            sm_c2_install_is_read_back_and_loaded(),
            sm_c3_installing_the_same_spec_again_is_an_upgrade(),
            sm_c4_another_program_or_home_is_a_conflict_and_changes_nothing(),
            sm_c5_uninstall_leaves_nothing_and_can_repeat(),
            sm_c6_an_unsafe_path_is_refused_and_changes_nothing(),
            sm_c7_restart_needs_an_installed_service(),
        );
    }

    fn spec() -> ServiceSpec {
        umx_contracts::service_manager::spec()
    }

    #[test]
    fn the_plist_is_exactly_this() {
        let s = ServiceSpec {
            program: "/Users/jo/.cargo/bin/umx".into(),
            home: "/Users/jo/.umx".into(),
            log: "/Users/jo/.umx/daemon.log".into(),
        };
        assert_eq!(
            plist(Launchd::LABEL, &s),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by `umx daemon install`; removed by `umx daemon uninstall`. -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>dev.umx.daemon</string>
	<key>ProgramArguments</key>
	<array>
		<string>/Users/jo/.cargo/bin/umx</string>
		<string>daemon</string>
		<string>start</string>
		<string>--service</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>UMX_HOME</key>
		<string>/Users/jo/.umx</string>
	</dict>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>2</integer>
	<key>ProcessType</key>
	<string>Background</string>
	<key>StandardOutPath</key>
	<string>/Users/jo/.umx/daemon.log</string>
	<key>StandardErrorPath</key>
	<string>/Users/jo/.umx/daemon.log</string>
</dict>
</plist>
"#
        );
        let mut odd = s.clone();
        odd.program = "/a&b <c>/umx".into();
        assert!(plist(Launchd::LABEL, &odd).contains("<string>/a&amp;b &lt;c&gt;/umx</string>"));
        assert_eq!(plist_spec(&plist(Launchd::LABEL, &odd)), Some(odd));
    }

    #[test]
    fn the_unit_is_exactly_this() {
        let s = ServiceSpec {
            program: "/home/jo/.cargo/bin/umx".into(),
            home: "/home/jo/.umx".into(),
            log: "/home/jo/.umx/daemon.log".into(),
        };
        assert_eq!(
            unit(&s),
            "# Written by `umx daemon install`; removed by `umx daemon uninstall`.
[Unit]
Description=umx daemon

[Service]
ExecStart=\"/home/jo/.cargo/bin/umx\" daemon start --service
Environment=\"UMX_HOME=/home/jo/.umx\"
Restart=on-failure
RestartSec=2
StandardOutput=append:/home/jo/.umx/daemon.log
StandardError=append:/home/jo/.umx/daemon.log

[Install]
WantedBy=default.target
"
        );
        assert_eq!(unit_spec(&unit(&s)), Some(s));
    }

    #[test]
    fn launchd_runs_exactly_these_commands() {
        let port = launchd();
        let dir = port.agents.parent().unwrap().to_path_buf();
        port.install(&spec()).unwrap();
        port.install(&spec()).unwrap();
        port.restart().unwrap();
        port.uninstall().unwrap();
        let plist = dir
            .join("LaunchAgents/dev.umx.daemon.plist")
            .display()
            .to_string();
        assert_eq!(
            argv(&dir),
            [
                "print gui/501/dev.umx.daemon".to_string(),
                format!("bootstrap gui/501 {plist}"),
                "print gui/501/dev.umx.daemon".to_string(),
                "bootout gui/501/dev.umx.daemon".to_string(),
                format!("bootstrap gui/501 {plist}"),
                "print gui/501/dev.umx.daemon".to_string(),
                "kickstart -k gui/501/dev.umx.daemon".to_string(),
                "print gui/501/dev.umx.daemon".to_string(),
                "bootout gui/501/dev.umx.daemon".to_string(),
            ]
        );
        assert!(!dir.join("LaunchAgents/dev.umx.daemon.plist").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn systemd_runs_exactly_these_commands() {
        let port = systemd();
        let dir = port.units.parent().unwrap().parent().unwrap().to_path_buf();
        port.install(&spec()).unwrap();
        port.uninstall().unwrap();
        assert_eq!(
            argv(&dir),
            [
                "--user show umx.service --property=UnitFileState,ActiveState,MainPID",
                "--user daemon-reload",
                "--user enable umx.service",
                "--user restart umx.service",
                "--user disable --now umx.service",
                "--user daemon-reload",
            ]
        );
        assert!(!dir.join("systemd/user/umx.service").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    /// Review B2: a tool that fails to inspect is a failure, never "not loaded", and uninstall then changes nothing.
    #[test]
    fn a_failing_inspection_is_a_failure_and_uninstall_changes_nothing() {
        let port = launchd();
        let dir = port.agents.parent().unwrap().to_path_buf();
        port.install(&spec()).unwrap();
        fs::write(dir.join("bin/inspectfail"), "").unwrap();
        assert!(matches!(port.status(), Err(ServiceError::Failed(_))));
        assert!(matches!(port.uninstall(), Err(ServiceError::Failed(_))));
        assert!(port.plist_path().exists(), "the plist stays");
        assert!(dir.join("bin/loaded").exists(), "still loaded");
        assert!(matches!(
            port.install(&spec()),
            Err(ServiceError::Failed(_))
        ));

        let port = systemd();
        let dir = port.units.parent().unwrap().parent().unwrap().to_path_buf();
        port.install(&spec()).unwrap();
        fs::write(dir.join("bin/inspectfail"), "").unwrap();
        assert!(matches!(port.status(), Err(ServiceError::Failed(_))));
        assert!(matches!(port.restart(), Err(ServiceError::Failed(_))));
        assert!(port.unit_path().exists(), "the unit stays");
    }

    /// Review F42: install inspects before it writes, so a failing tool leaves a valid but hand-formatted service file
    /// byte for byte as it was.
    #[test]
    fn a_failing_install_leaves_the_service_file_untouched() {
        let port = launchd();
        let dir = port.agents.parent().unwrap().to_path_buf();
        port.install(&spec()).unwrap();
        // Same program and home, so not a conflict, but not the bytes install would write.
        let hand = plist(Launchd::LABEL, &spec()).replace("\t", "    ");
        fs::write(port.plist_path(), &hand).unwrap();
        fs::write(dir.join("bin/inspectfail"), "").unwrap();
        assert!(matches!(
            port.install(&spec()),
            Err(ServiceError::Failed(_))
        ));
        assert_eq!(fs::read_to_string(port.plist_path()).unwrap(), hand);

        let port = systemd();
        let dir = port.units.parent().unwrap().parent().unwrap().to_path_buf();
        port.install(&spec()).unwrap();
        let hand = format!("# mine\n{}", unit(&spec()));
        fs::write(port.unit_path(), &hand).unwrap();
        fs::write(dir.join("bin/inspectfail"), "").unwrap();
        assert!(matches!(
            port.install(&spec()),
            Err(ServiceError::Failed(_))
        ));
        assert_eq!(fs::read_to_string(port.unit_path()).unwrap(), hand);
    }

    /// Review B1: a service file umx cannot read back is installed with no spec (doctor calls it unreadable), and
    /// install refuses to overwrite it.
    #[test]
    fn an_unparseable_service_file_has_no_spec_and_is_not_overwritten() {
        let port = launchd();
        port.install(&spec()).unwrap();
        fs::write(port.plist_path(), "this is not a plist\n").unwrap();
        assert!(matches!(
            port.status(),
            Ok(ServiceState::Installed { spec: None, .. })
        ));
        assert!(matches!(
            port.install(&spec()),
            Err(ServiceError::Conflict { found: None })
        ));
        assert_eq!(
            fs::read_to_string(port.plist_path()).unwrap(),
            "this is not a plist\n"
        );

        let port = systemd();
        port.install(&spec()).unwrap();
        fs::write(port.unit_path(), "[Service]\nExecStart=/bin/true\n").unwrap();
        assert!(matches!(
            port.status(),
            Ok(ServiceState::Installed { spec: None, .. })
        ));
        assert!(matches!(
            port.install(&spec()),
            Err(ServiceError::Conflict { found: None })
        ));
    }

    #[test]
    fn the_plist_is_not_writable_by_others() {
        let port = launchd();
        port.install(&spec()).unwrap();
        let mode = fs::metadata(port.plist_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    /// F47: Linux refuses to exec a file that some process holds open for writing (ETXTBSY), as a test thread's
    /// fork briefly does right after another thread wrote a stub. The tool still starts once the handle is gone.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_tool_file_still_held_for_writing_starts_once_released() {
        let dir = sandbox(LAUNCHCTL);
        let tool = dir.join("bin/tool");
        let held = fs::OpenOptions::new().append(true).open(&tool).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            drop(held);
        });
        let ran = run(tool.to_str().unwrap(), &["print", "gui/501/x"], RUN_TIMEOUT).unwrap();
        assert_eq!(ran.code, Some(113));
        release.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_tool_is_a_failure_that_names_it() {
        let port = Launchd::new(
            std::env::temp_dir().join("umx-none"),
            "/nonexistent/launchctl",
            501,
            Launchd::LABEL,
        );
        match port.restart() {
            Err(ServiceError::Failed(m)) => assert!(m.contains("not found"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}
