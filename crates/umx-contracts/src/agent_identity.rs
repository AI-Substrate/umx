use umx_core::ports::{AgentIdentity, IdentityError};
use umx_core::provenance::{Actor, ActorInput, Tier};

pub const CLAUSES: &[&str] = &[
    "ai_c1_present_is_a_valid_actor",
    "ai_c2_absence_is_not_an_error",
    "ai_c3_invalid_names_a_field_only",
    "ai_c4_overlong_values_are_not_returned",
    "ai_c5_secret_values_are_not_returned",
    "ai_c6_unavailable_never_echoes_output",
];

#[derive(Clone, Copy)]
pub enum Scenario {
    Present,
    Absent,
    Invalid,
    Overlong,
    Secret,
    Unavailable,
}

pub const OUTPUT_MARKER: &str = "DO_NOT_ECHO_IDENTITY_OUTPUT";

pub fn secret() -> String {
    format!("ghp_{}", "a".repeat(36))
}

pub fn fixture_actor() -> Actor {
    Actor::try_from(ActorInput {
        tier: Tier::Harness,
        seat: None,
        role: None,
        model: None,
        effort: None,
        harness: Some("copilot".into()),
        harness_session: Some("Session_01".into()),
        user: Some("writer".into()),
    })
    .expect("valid contract actor")
}

pub fn ai_c1_present_is_a_valid_actor<P: AgentIdentity>(factory: impl Fn(Scenario) -> P) {
    let port = factory(Scenario::Present);
    assert!(port.adapter_id().ends_with(".agent-identity"));
    let actor = port
        .current()
        .expect("identity fixture")
        .expect("present actor");
    let input: ActorInput = actor.clone().into();
    assert!(Actor::try_from(input).is_ok());
}

pub fn ai_c2_absence_is_not_an_error<P: AgentIdentity>(factory: impl Fn(Scenario) -> P) {
    assert!(matches!(factory(Scenario::Absent).current(), Ok(None)));
}

pub fn ai_c3_invalid_names_a_field_only<P: AgentIdentity>(factory: impl Fn(Scenario) -> P) {
    match factory(Scenario::Invalid).current() {
        Err(IdentityError::Invalid(field)) => assert!(
            [
                "actor",
                "tier",
                "seat",
                "role",
                "model",
                "effort",
                "harness",
                "harness_session",
                "user",
                "whoami"
            ]
            .contains(&field)
        ),
        _ => panic!("expected an invalid identity field"),
    }
}

fn refused_or_sanitized(port: impl AgentIdentity, raw: &str) {
    match port.current() {
        Ok(Some(actor)) => {
            let input = actor.fields();
            for value in [
                &input.seat,
                &input.role,
                &input.model,
                &input.effort,
                &input.harness,
                &input.harness_session,
                &input.user,
            ]
            .into_iter()
            .flatten()
            {
                assert!(value != raw, "unsafe identity value was returned");
            }
        }
        Ok(None) | Err(IdentityError::Invalid(_)) => {}
        Err(IdentityError::Unavailable(_)) => panic!("unsafe-value fixture unavailable"),
    }
}

pub fn ai_c4_overlong_values_are_not_returned<P: AgentIdentity>(factory: impl Fn(Scenario) -> P) {
    refused_or_sanitized(factory(Scenario::Overlong), &"x".repeat(129));
}

pub fn ai_c5_secret_values_are_not_returned<P: AgentIdentity>(factory: impl Fn(Scenario) -> P) {
    refused_or_sanitized(factory(Scenario::Secret), &secret());
}

pub fn ai_c6_unavailable_never_echoes_output<P: AgentIdentity>(factory: impl Fn(Scenario) -> P) {
    match factory(Scenario::Unavailable).current() {
        Err(IdentityError::Unavailable(step)) => {
            assert!(!step.contains(OUTPUT_MARKER), "subprocess output leaked");
            assert!(!step.contains(&secret()), "captured secret leaked");
            assert!(step.len() <= 128 && !step.chars().any(char::is_control));
        }
        _ => panic!("expected an unavailable identity source"),
    }
}

#[macro_export]
macro_rules! agent_identity_contract {
    ($factory:expr; $($clause:ident()),+ $(,)?) => {
        mod agent_identity_contract {
            #[allow(unused_imports)]
            use super::*;
            $(
                #[test]
                fn $clause() {
                    $crate::agent_identity::$clause($factory)
                }
            )+
            #[test]
            fn every_clause_is_listed() {
                $crate::assert_listed($crate::agent_identity::CLAUSES, &[$(stringify!($clause)),+]);
            }
        }
    };
}
