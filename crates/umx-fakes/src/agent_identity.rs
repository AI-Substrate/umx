use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use umx_core::ports::{AgentIdentity, IdentityError};
use umx_core::provenance::Actor;

pub struct FakeAgentIdentity {
    result: Mutex<Result<Option<Actor>, IdentityError>>,
    calls: AtomicUsize,
}

impl FakeAgentIdentity {
    pub fn new(result: Result<Option<Actor>, IdentityError>) -> Self {
        Self {
            result: Mutex::new(result),
            calls: AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl Default for FakeAgentIdentity {
    fn default() -> Self {
        Self::new(Ok(None))
    }
}

impl AgentIdentity for FakeAgentIdentity {
    fn adapter_id(&self) -> &'static str {
        "fake.agent-identity"
    }

    fn current(&self) -> Result<Option<Actor>, IdentityError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.result.lock().expect("identity fixture lock").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_contracts::agent_identity::{Scenario, fixture_actor};

    fn fixture(scenario: Scenario) -> FakeAgentIdentity {
        FakeAgentIdentity::new(match scenario {
            Scenario::Present => Ok(Some(fixture_actor())),
            Scenario::Absent => Ok(None),
            Scenario::Invalid => Err(IdentityError::Invalid("harness")),
            Scenario::Overlong | Scenario::Secret => Err(IdentityError::Invalid("user")),
            Scenario::Unavailable => Err(IdentityError::Unavailable(
                "identity tool unavailable".into(),
            )),
        })
    }

    umx_contracts::agent_identity_contract!(
        fixture;
        ai_c1_present_is_a_valid_actor(),
        ai_c2_absence_is_not_an_error(),
        ai_c3_invalid_names_a_field_only(),
        ai_c4_overlong_values_are_not_returned(),
        ai_c5_secret_values_are_not_returned(),
        ai_c6_unavailable_never_echoes_output(),
    );
}
