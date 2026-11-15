//! `umx daemon install|uninstall|service` and the service part of `umx daemon status` (req-86). They run in the
//! binary, not through the daemon, so they work with it down. The service runs this umx binary (its canonical path) with this
//! UMX_HOME, as `<umx> daemon start --service`.

use std::path::Path;

use umx_core::ports::{ServiceError, ServiceSpec, ServiceState};
use umx_protocol::{Code, Envelope, Fill, Issue, Value, json};
use umx_services::Services;

use crate::{Config, mode, owner_only_dir};

/// What this command would install: this binary, this UMX_HOME, and its daemon.log.
pub(crate) fn this_spec(config: &Config) -> Result<ServiceSpec, Issue> {
    let failed =
        |reason: &'static str| Issue::fill(Code::ServiceFailed, &[("reason", Fill::word(reason))]);
    owner_only_dir(&config.home, config.uid)
        .map_err(|_| failed("UMX_HOME is not an owner-only folder (chmod 700 it)"))?;
    let home =
        std::fs::canonicalize(&config.home).map_err(|_| failed("UMX_HOME cannot be resolved"))?;
    let program = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|_| failed("the path of this umx binary cannot be resolved"))?;
    let text = |p: &Path| {
        p.to_str()
            .map(str::to_owned)
            .ok_or_else(|| Issue::fill(Code::ServicePathUnsafe, &[("path", Fill::path(p))]))
    };
    Ok(ServiceSpec {
        program: text(&program)?,
        home: text(&home)?,
        log: text(&home.join("daemon.log"))?,
    })
}

pub(crate) fn issue(e: &ServiceError) -> Issue {
    match e {
        ServiceError::Conflict { found } => {
            let path = |p: Option<&str>| {
                p.map_or(Fill::word("an unreadable path"), |p| {
                    Fill::path(Path::new(p))
                })
            };
            Issue::fill(
                Code::ServiceConflict,
                &[
                    ("program", path(found.as_ref().map(|f| f.program.as_str()))),
                    ("home", path(found.as_ref().map(|f| f.home.as_str()))),
                ],
            )
        }
        ServiceError::BadPath(p) => Issue::fill(
            Code::ServicePathUnsafe,
            &[("path", Fill::path(Path::new(p)))],
        ),
        ServiceError::Failed(m) => Issue::fill(Code::ServiceFailed, &[("reason", Fill::text(m))]),
    }
}

/// `umx daemon install`: writes the service and loads it, so the daemon starts now and at every login.
pub fn install(config: &Config) -> Envelope {
    const CMD: &str = "daemon install";
    let spec = match this_spec(config) {
        Ok(spec) => spec,
        Err(issue) => return Envelope::fail(CMD, issue),
    };
    let services = umx_compose::compose(mode(), config);
    match services.service().install(&spec) {
        Ok(()) => Envelope::ok(CMD, json!({ "service": state(config, &services) })),
        Err(e) => Envelope::fail(CMD, issue(&e)),
    }
}

/// `umx daemon uninstall`: stops the service and removes its file. Nothing installed is not an error. The log and
/// everything else in UMX_HOME stay.
pub fn uninstall(config: &Config) -> Envelope {
    const CMD: &str = "daemon uninstall";
    let services = umx_compose::compose(mode(), config);
    match services.service().uninstall() {
        Ok(()) => Envelope::ok(CMD, json!({ "service": state(config, &services) })),
        Err(e) => Envelope::fail(CMD, issue(&e)),
    }
}

/// The `service` object of `umx daemon status`.
pub fn service_status(config: &Config) -> Value {
    state(config, &umx_compose::compose(mode(), config))
}

/// `umx daemon service` (F58): the same `service` object as `umx daemon status`, built by the same function, without
/// asking the daemon, so a slow or stopped daemon never hides an installed service. A service manager that cannot be
/// read fails (service_failed) instead of showing `installed: null`.
pub fn service(config: &Config) -> Envelope {
    const CMD: &str = "daemon service";
    match read(config, &umx_compose::compose(mode(), config)) {
        (view, None) => Envelope::ok(CMD, json!({ "service": view })),
        (_, Some(e)) => Envelope::fail(CMD, issue(&e)),
    }
}

fn state(config: &Config, services: &Services) -> Value {
    read(config, services).0
}

/// The service's state, with the adapter and the tool it runs, so an override (UMX_SERVICE_TOOL) is visible, and the
/// service manager's error if it could not be read (the state then says `installed: null` with the error).
fn read(config: &Config, services: &Services) -> (Value, Option<ServiceError>) {
    let adapter = services.service().adapter_id();
    let tool = &config.service_tool;
    match services.service().status() {
        Ok(ServiceState::NotInstalled) => (
            json!({ "installed": false, "adapter": adapter, "tool": tool }),
            None,
        ),
        Ok(ServiceState::Installed { spec, loaded, pid }) => (
            json!({
                "installed": true,
                "loaded": loaded,
                "pid": pid,
                "program": spec.as_ref().map(|s| s.program.clone()),
                "home": spec.as_ref().map(|s| s.home.clone()),
                "adapter": adapter,
                "tool": tool,
            }),
            None,
        ),
        Err(e) => (
            json!({ "installed": null, "error": e.to_string(), "adapter": adapter, "tool": tool }),
            Some(e),
        ),
    }
}
