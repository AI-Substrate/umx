//! System adapters (blueprint 001, c-adapter-system): system.clock (wall clock), system.id-generator (ULID), and the
//! service managers launchd.service-manager and systemd.service-manager ([`service`]), and system.ui-host, which runs
//! the graph view for `umx ui` ([`ui_host`]).

pub mod agent_identity;
pub mod service;
mod sys;
mod tool;
pub mod ui_host;

/// Client helpers stay owned until they finish, including an exec still returning from the kernel.
pub fn register_tool_helper(handle: std::thread::JoinHandle<()>) {
    tool::register(handle);
}

/// Joins completed helpers without waiting beyond `by`; unfinished kernel work remains owned.
pub fn finish_tools(by: std::time::Instant) -> bool {
    tool::finish_tools(by)
}

use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;

/// This process's effective uid. std has no geteuid, but a new socket is owned by the effective uid that made it
/// (Linux sockfs and macOS both report it from fstat).
pub fn effective_uid() -> Option<u32> {
    let (socket, _peer) = UnixStream::pair().ok()?;
    File::from(OwnedFd::from(socket))
        .metadata()
        .ok()
        .map(|m| m.uid())
}

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use umx_core::Id;
use umx_core::ports::{Clock, IdGenerator};

/// system.clock
#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn adapter_id(&self) -> &'static str {
        "system.clock"
    }

    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}

/// system.id-generator: monotonic ULIDs (strictly increasing, also within one millisecond).
#[derive(Default)]
pub struct SystemIds(Mutex<ulid::Generator>);

impl SystemIds {
    pub fn new() -> SystemIds {
        SystemIds::default()
    }
}

impl IdGenerator for SystemIds {
    fn adapter_id(&self) -> &'static str {
        "system.id-generator"
    }

    fn next_id(&self) -> Id {
        let mut generator = self.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            // Fails only when 2^80 ids were minted in one millisecond; the next millisecond resets it.
            if let Ok(u) = generator.generate() {
                return Id(u.0);
            }
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    umx_contracts::clock_contract!(
        SystemClock::default;
        ck_c1_now_is_a_plausible_epoch_ms(),
        ck_c2_now_never_goes_backwards(),
    );
    umx_contracts::id_generator_contract!(
        SystemIds::new;
        id_c1_ids_are_unique(),
        id_c2_each_id_is_greater_than_the_last(),
        id_c3_threads_share_one_sequence(),
    );
}
