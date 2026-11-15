//! IdGenerator port: the only way core and services get a new id.

use crate::id::Id;

pub trait IdGenerator: Send + Sync {
    /// A new id, greater than every id this generator returned before.
    fn next_id(&self) -> Id;
    /// This adapter's id in the adapter matrix (blueprint d-6), such as `system.id-generator`. describe() and
    /// `umx daemon status` read it from the wired object (E-1).
    fn adapter_id(&self) -> &'static str;
}
