//! Client-side discovery only. An author claim never grants authority.

use alloc::string::String;
use core::fmt;

use crate::provenance::Actor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityError {
    /// Names the failed step, never the captured value or subprocess output.
    Unavailable(String),
    Invalid(&'static str),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(step) => write!(f, "identity unavailable: {step}"),
            Self::Invalid(field) => write!(f, "invalid identity field: {field}"),
        }
    }
}

pub trait AgentIdentity: Send + Sync {
    fn adapter_id(&self) -> &'static str;
    fn current(&self) -> Result<Option<Actor>, IdentityError>;
}
