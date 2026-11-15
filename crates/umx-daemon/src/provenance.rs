use umx_core::Problem;
use umx_core::provenance::{
    Actor, ActorInput, InvalidActor, Tier, identity_value, is_session_hash,
};
use umx_protocol::ActorClaim;

fn checked(claim: &ActorClaim) -> Result<Actor, InvalidActor> {
    let tier = match claim.tier.as_str() {
        "pij" => Tier::Pij,
        "harness" => Tier::Harness,
        "cli" => Tier::Cli,
        "unknown" => Tier::Unknown,
        _ => return Err(InvalidActor("tier")),
    };
    let session = match claim.session.as_deref() {
        None | Some("unknown") => None,
        Some(value) if is_session_hash(value) => Some(value.into()),
        Some(_) => return Err(InvalidActor("harness_session")),
    };
    let field = |name, value: &Option<String>| {
        value
            .as_deref()
            .map(|raw| identity_value(name, raw))
            .transpose()
    };
    Actor::try_from(ActorInput {
        tier,
        harness_session: session,
        harness: field("harness", &claim.harness)?,
        seat: field("seat", &claim.seat)?,
        role: field("role", &claim.role)?,
        model: field("model", &claim.model)?,
        effort: field("effort", &claim.effort)?,
        user: field("user", &claim.user)?,
    })
}

/// No discovery and no hashing here: only the client may resolve identity.
pub(crate) fn actor(claim: Option<&ActorClaim>) -> (Actor, Option<Problem>) {
    match claim {
        None => (Actor::unknown(), None),
        Some(claim) => match checked(claim) {
            Ok(actor) => (actor, None),
            Err(error) => (
                Actor::unknown(),
                Some(umx_services::problems::actor_invalid(error.0)),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim() -> ActorClaim {
        ActorClaim {
            tier: "pij".into(),
            harness: Some("copilot".into()),
            session: Some("h-0123456789abcdef".into()),
            seat: Some("pij-calm-otter".into()),
            role: Some("coder".into()),
            model: Some("openai/GPT-5.6".into()),
            effort: None,
            user: Some("writer".into()),
        }
    }

    #[test]
    fn valid_claim_is_checked_but_never_rehashed_and_absence_is_unknown() {
        let (found, warning) = actor(Some(&claim()));
        assert!(warning.is_none());
        assert_eq!(
            found.fields().harness_session.as_deref(),
            Some("h-0123456789abcdef")
        );
        let (missing, warning) = actor(None);
        assert_eq!(missing.fields().tier, Tier::Unknown);
        assert!(warning.is_none());
    }

    #[test]
    fn raw_sessions_and_unsafe_fields_become_unknown_without_echoing_values() {
        let token = format!("ghp_{}", "a".repeat(36));
        for field in [
            "session", "seat", "role", "model", "effort", "user", "harness",
        ] {
            let mut value = serde_json::to_value(claim()).unwrap();
            value[field] = umx_protocol::json!(token);
            let claim: ActorClaim = serde_json::from_value(value).unwrap();
            let (found, warning) = actor(Some(&claim));
            assert_eq!(found.session_key(), "session/unknown/unknown");
            let warning = warning.expect("invalid actor warning");
            assert_eq!(warning.code, "actor_invalid");
            let issue = crate::verbs::issue(&warning, "node add");
            let rendered = serde_json::to_string(&issue).unwrap();
            assert!(!rendered.contains(&token));
        }
    }
}
