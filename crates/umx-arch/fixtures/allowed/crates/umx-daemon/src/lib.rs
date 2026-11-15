//! fixture crate
pub use umx_compose::Config;
pub(crate) use umx_core::Id;
use umx_protocol::{self as wire, Envelope};
fn _private(_: wire::Request, _: Envelope) {}
