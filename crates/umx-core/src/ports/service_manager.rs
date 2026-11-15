//! ServiceManager port (req-86): runs the umx daemon as a per-user service that starts at login and comes back
//! after a crash. Adapters: launchd.service-manager (a launchd agent on macOS), systemd.service-manager (a systemd
//! user unit on Linux), fake.service-manager. Never needs root: everything is per user.

use alloc::string::String;
use core::fmt;

/// What the service runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    /// Absolute path of the umx binary; the service runs `<program> daemon start --service`.
    pub program: String,
    /// `$UMX_HOME`, given to the service as `UMX_HOME`.
    pub home: String,
    /// Where the service's output goes: `$UMX_HOME/daemon.log`.
    pub log: String,
}

/// Whether the service is installed and running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceState {
    NotInstalled,
    Installed {
        /// What the installed unit runs, read back from it (`None` if umx cannot read it).
        spec: Option<ServiceSpec>,
        /// The service manager knows the service (launchd: bootstrapped; systemd: enabled).
        loaded: bool,
        /// The service's process, when one is running.
        pid: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceError {
    /// A service with umx's label is already installed for another program or home. Nothing changed.
    Conflict { found: Option<ServiceSpec> },
    /// A path cannot be written into a unit file safely (a quote, a backslash, `%`, `$` or a control
    /// character). Nothing changed.
    BadPath(String),
    /// The service manager or the file system failed. The message names the step and the exit status, never
    /// file contents.
    Failed(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Conflict { .. } => {
                f.write_str("a umx service is already installed for another program or home")
            }
            ServiceError::BadPath(p) => write!(f, "{p} cannot be written into a service file"),
            ServiceError::Failed(m) => f.write_str(m),
        }
    }
}

/// Characters a path in a unit file may not hold: they would need escaping that differs between launchd's XML and
/// systemd's unit syntax, or expand (`%`, `$`).
pub fn unit_safe(path: &str) -> bool {
    !path.is_empty()
        && path.starts_with('/')
        && !path
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '\\' | '%' | '$' | '\'' | '`'))
}

pub trait ServiceManager: Send + Sync {
    /// `<adapter>.service-manager`, as in the adapter matrix (d-6).
    fn adapter_id(&self) -> &'static str;

    fn status(&self) -> Result<ServiceState, ServiceError>;

    /// Writes the unit and loads it, so the daemon starts now and at every login. Installing the same spec again
    /// rewrites and reloads it (how `just dev` upgrades). A unit for another program or home is `Conflict`, and
    /// nothing changes.
    fn install(&self, spec: &ServiceSpec) -> Result<(), ServiceError>;

    /// Unloads the service and removes its unit. Nothing installed is Ok.
    fn uninstall(&self) -> Result<(), ServiceError>;

    /// Stops the service's process and starts it again, so the installed binary runs.
    fn restart(&self) -> Result<(), ServiceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_safe_paths() {
        for ok in [
            "/usr/local/bin/umx",
            "/Users/Jo Smith/.umx",
            "/home/j/.cargo/bin/umx",
        ] {
            assert!(unit_safe(ok), "{ok}");
        }
        for bad in [
            "", "umx", "/a\"b", "/a\\b", "/a%h", "/a$HOME", "/a\nb", "/a'b", "/a`b",
        ] {
            assert!(!unit_safe(bad), "{bad:?}");
        }
    }
}
