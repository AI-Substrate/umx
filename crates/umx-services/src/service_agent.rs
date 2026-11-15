//! The daemon as a per-user service (req-86, d-12): holds only the ServiceManager port. `None` where umx has no
//! service manager (an OS other than macOS and Linux, n-13).

use alloc::string::ToString;
use alloc::sync::Arc;
use umx_core::ports::{ServiceError, ServiceManager, ServiceSpec, ServiceState};

pub struct ServiceAgent {
    manager: Option<Arc<dyn ServiceManager>>,
    /// Why there is no manager, said by every call (compose knows: the OS, or an unreadable uid).
    missing: &'static str,
}

impl ServiceAgent {
    pub fn new(manager: Option<Arc<dyn ServiceManager>>) -> ServiceAgent {
        ServiceAgent {
            manager,
            missing: "umx has no service manager on this system",
        }
    }

    /// No manager, and why.
    pub fn unavailable(why: &'static str) -> ServiceAgent {
        ServiceAgent {
            manager: None,
            missing: why,
        }
    }

    pub fn adapter_id(&self) -> Option<&'static str> {
        self.manager.as_ref().map(|m| m.adapter_id())
    }

    fn manager(&self) -> Result<&Arc<dyn ServiceManager>, ServiceError> {
        self.manager
            .as_ref()
            .ok_or_else(|| ServiceError::Failed(self.missing.to_string()))
    }

    pub fn status(&self) -> Result<ServiceState, ServiceError> {
        self.manager()?.status()
    }

    pub fn install(&self, spec: &ServiceSpec) -> Result<(), ServiceError> {
        self.manager()?.install(spec)
    }

    pub fn uninstall(&self) -> Result<(), ServiceError> {
        self.manager()?.uninstall()
    }

    pub fn restart(&self) -> Result<(), ServiceError> {
        self.manager()?.restart()
    }
}
