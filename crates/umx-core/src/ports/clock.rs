//! Clock port: the only way core and services read time.

pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
    /// This adapter's id in the adapter matrix (blueprint d-6), such as `system.clock`. describe() and
    /// `umx daemon status` read it from the wired object (E-1).
    fn adapter_id(&self) -> &'static str;
}
