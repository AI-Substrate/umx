//! Secrets port (req-72): looks up a secret by env var name, in the environment first and then in
//! `$UMX_HOME/secrets.env`. A secret's value can reach only the code that asks for it: [`Secret`] has no Display or
//! Serialize, and its Debug prints `<redacted>`.

use crate::ports::GraphDir;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// A secret value. Read it with [`Secret::expose`] at the point of use, never into a log or an envelope.
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where a secret was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretSource {
    Env,
    SecretsFile,
}

impl SecretSource {
    pub fn word(self) -> &'static str {
        match self {
            SecretSource::Env => "env",
            SecretSource::SecretsFile => "secrets.env",
        }
    }
}

/// What is wrong with the secrets file. Never its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretsIssue {
    /// The file is a symlink, not owned by this user, or readable or writable by group or others; it is ignored.
    Refused {
        file: GraphDir,
        reason: &'static str,
    },
    /// A line that is not `NAME=value` (1-based); it is ignored.
    BadLine { file: GraphDir, line: u64 },
}

pub trait Secrets: Send + Sync {
    /// Which adapter this is (`fs.secrets`, `fake.secrets`), as in the adapter matrix (d-6, E-1).
    fn adapter_id(&self) -> &'static str;
    /// The secret named `name`, env first, then the secrets file.
    fn lookup(&self, name: &str) -> Option<(SecretSource, Secret)>;
    /// What is wrong with the secrets file, in line order; empty if it is fine or absent.
    fn issues(&self) -> Vec<SecretsIssue>;
}
