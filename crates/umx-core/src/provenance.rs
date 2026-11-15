//! Recorded author claims, never authentication. Keys are labels, never paths, URLs or commands.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::ontology::{
    PROVENANCE_KEYS, PROVENANCE_RELATIONS, PROVENANCE_TYPES, is_derived_relation,
};
use crate::{Id, Node};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Pij,
    Harness,
    Cli,
    Unknown,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pij => "pij",
            Self::Harness => "harness",
            Self::Cli => "cli",
            Self::Unknown => "unknown",
        }
    }
}

/// Identity names are normalized, except bounded model labels and opaque harness sessions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorInput {
    pub tier: Tier,
    pub seat: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub harness: Option<String>,
    pub harness_session: Option<String>,
    pub user: Option<String>,
}

/// Construction and deserialization both validate every field. Capture and secret scanning belong to P2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ActorInput", into = "ActorInput")]
pub struct Actor(ActorInput);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidActor(pub &'static str);

impl core::fmt::Display for InvalidActor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "invalid actor field: {}", self.0)
    }
}

impl TryFrom<ActorInput> for Actor {
    type Error = InvalidActor;

    fn try_from(input: ActorInput) -> Result<Self, Self::Error> {
        for (name, value) in [
            ("seat", &input.seat),
            ("role", &input.role),
            ("effort", &input.effort),
            ("harness", &input.harness),
            ("user", &input.user),
        ] {
            if value.as_deref().is_some_and(|s| !segment(s)) {
                return Err(InvalidActor(name));
            }
            if input
                .model
                .as_deref()
                .is_some_and(|s| !model_label(s) || normalise_name(s).is_err())
            {
                return Err(InvalidActor("model"));
            }
        }
        if input
            .harness_session
            .as_deref()
            .is_some_and(|s| !session_id(s))
        {
            return Err(InvalidActor("harness_session"));
        }
        if matches!(input.tier, Tier::Pij | Tier::Harness)
            && input
                .harness
                .as_deref()
                .is_some_and(|s| matches!(s, "cli" | "unknown"))
        {
            return Err(InvalidActor("harness"));
        }
        let valid_tier = match input.tier {
            Tier::Pij => input.seat.is_some() && input.harness.is_some(),
            Tier::Harness => {
                input.seat.is_none() && input.harness.is_some() && input.user.is_some()
            }
            Tier::Cli => {
                input.seat.is_none()
                    && input.user.is_some()
                    && input.harness.is_none()
                    && input.harness_session.is_none()
            }
            Tier::Unknown => [
                &input.seat,
                &input.role,
                &input.model,
                &input.effort,
                &input.harness,
                &input.harness_session,
                &input.user,
            ]
            .iter()
            .all(|v| v.is_none()),
        };
        if !valid_tier {
            return Err(InvalidActor("tier"));
        }
        Ok(Self(input))
    }
}

impl From<Actor> for ActorInput {
    fn from(actor: Actor) -> Self {
        actor.0
    }
}

impl Actor {
    pub fn unknown() -> Self {
        Self(ActorInput {
            tier: Tier::Unknown,
            seat: None,
            role: None,
            model: None,
            effort: None,
            harness: None,
            harness_session: None,
            user: None,
        })
    }

    pub fn fields(&self) -> &ActorInput {
        &self.0
    }

    pub fn agent_key(&self) -> String {
        match self.0.tier {
            Tier::Pij => format!(
                "agent/pij/{}",
                self.0.seat.as_deref().expect("validated tier")
            ),
            Tier::Harness => format!(
                "agent/{}/{}",
                self.0.harness.as_deref().expect("validated tier"),
                self.0.user.as_deref().expect("validated tier")
            ),
            Tier::Cli => format!(
                "agent/cli/{}",
                self.0.user.as_deref().expect("validated tier")
            ),
            Tier::Unknown => "agent/unknown".into(),
        }
    }

    pub fn session_key(&self) -> String {
        match self.0.tier {
            Tier::Pij | Tier::Harness => format!(
                "session/{}/{}",
                self.0.harness.as_deref().expect("validated tier"),
                self.0.harness_session.as_deref().unwrap_or("unknown")
            ),
            Tier::Cli => format!(
                "session/cli/{}",
                self.0.user.as_deref().expect("validated tier")
            ),
            Tier::Unknown => "session/unknown/unknown".into(),
        }
    }
}

fn segment(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

fn model_label(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > 128 {
        return false;
    }
    let mut parts = raw.split('/');
    let safe = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.starts_with('-')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    safe(parts.next().expect("split yields one part"))
        && parts.next().is_none_or(safe)
        && parts.next().is_none()
}

pub fn is_pij_seat(raw: &str) -> bool {
    if raw.len() > 64 {
        return false;
    }
    let Some(rest) = raw.strip_prefix("pij-") else {
        return false;
    };
    let mut parts = rest.split('-');
    let safe = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    safe(parts.next().expect("split yields one part"))
        && parts.next().is_some_and(safe)
        && parts.all(safe)
}

/// Shared by client capture and daemon validation. Sessions use the separate hash-only boundary.
pub fn identity_value(field: &'static str, raw: &str) -> Result<String, InvalidActor> {
    let shape = match field {
        "seat" => is_pij_seat(raw),
        "model" => model_label(raw),
        "role" | "effort" | "harness" | "user" => {
            !raw.is_empty()
                && raw.len() <= 128
                && raw
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:@-".contains(&b))
        }
        _ => false,
    };
    if !shape || crate::secrets::scan(raw).is_some() {
        return Err(InvalidActor(field));
    }
    if matches!(field, "model" | "seat") {
        if field == "model" && normalise_name(raw).is_err() {
            return Err(InvalidActor(field));
        }
        Ok(raw.into())
    } else {
        normalise_name(raw).map_err(|_| InvalidActor(field))
    }
}

pub fn login_user(user: Option<&str>, logname: Option<&str>) -> Option<String> {
    [user, logname].into_iter().flatten().find_map(|raw| {
        let value = identity_value("user", raw).ok()?;
        (value != "unknown").then_some(value)
    })
}

pub fn session_id(s: &str) -> bool {
    (8..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

/// Compose calls this before building a request. Even hash-shaped raw input is hashed again here.
pub fn hash_session_id(raw: &str) -> Result<String, InvalidKey> {
    use sha2::{Digest, Sha256};
    if !session_id(raw) {
        return Err(InvalidKey);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(raw.as_bytes());
    let mut out = String::from("h-");
    for byte in &digest[..8] {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Ok(out)
}

/// The daemon accepts this form only, never raw session identifiers.
pub fn is_session_hash(value: &str) -> bool {
    value.len() == 18
        && value.starts_with("h-")
        && value.as_bytes()[2..]
            .iter()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

/// Invalid session IDs are replaced, not truncated or case-folded. The boolean asks capture to warn.
pub fn session_key(harness: &str, id: &str) -> Result<(String, bool), InvalidKey> {
    let harness = normalise_name(harness)?;
    if harness == "cli" || harness == "unknown" {
        return Err(InvalidKey);
    }
    let valid = session_id(id);
    Ok((
        format!("session/{harness}/{}", if valid { id } else { "unknown" }),
        !valid,
    ))
}

pub fn normalise_user(raw: &str) -> Result<String, InvalidKey> {
    normalise_name(raw)
}

/// Shared canonical segments for OS users, model names and harness names.
pub fn normalise_name(raw: &str) -> Result<String, InvalidKey> {
    if raw.len() > 128 {
        return Err(InvalidKey);
    }
    let mut out = String::new();
    let mut gap = false;
    for c in raw.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c) {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    let out = out.trim_matches(['-', '.']);
    if out.is_empty() {
        return Ok("unknown".into());
    }
    if !segment(out) {
        return Err(InvalidKey);
    }
    Ok(out.into())
}

pub fn model_key(raw: &str) -> Result<String, InvalidKey> {
    Ok(format!("model/{}", normalise_name(raw)?))
}

pub fn harness_key(raw: &str) -> Result<String, InvalidKey> {
    Ok(format!("harness/{}", normalise_name(raw)?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProvKey<'a> {
    raw: &'a str,
    kind: &'a str,
}

impl<'a> ProvKey<'a> {
    pub fn parse(raw: &'a str) -> Result<Self, InvalidKey> {
        let mut parts = raw.split('/');
        let kind = parts.next().ok_or(InvalidKey)?;
        let second = parts.next().ok_or(InvalidKey)?;
        let third = parts.next();
        if parts.next().is_some() || !segment(kind) || !segment(second) {
            return Err(InvalidKey);
        }
        let valid = match (kind, second, third) {
            ("agent", "unknown", None) | ("platform", "pij", None) => true,
            ("model" | "harness", _, None) => true,
            ("agent", harness, Some(user)) if harness != "unknown" => segment(user),
            ("session", "unknown", Some("unknown")) => true,
            ("session", "cli", Some(user)) => segment(user),
            ("session", harness, Some(id)) if harness != "unknown" => {
                id == "unknown" || session_id(id)
            }
            _ => false,
        };
        if !valid {
            return Err(InvalidKey);
        }
        Ok(Self { raw, kind })
    }

    pub fn as_str(self) -> &'a str {
        self.raw
    }

    pub fn kind(self) -> &'a str {
        self.kind
    }
}

pub fn is_provenance(node: &Node) -> bool {
    is_provenance_key(
        &node.kind,
        node.fields.get("prov_key").and_then(crate::Value::as_str),
    )
}

pub fn is_provenance_key(kind: &str, key: Option<&str>) -> bool {
    key.and_then(|s| ProvKey::parse(s).ok())
        .is_some_and(|key| key.kind() == kind && PROVENANCE_TYPES.contains(&kind))
}

pub fn invalid_provenance_fields(
    fields: &BTreeMap<String, crate::Value>,
) -> impl Iterator<Item = &'static str> + '_ {
    PROVENANCE_KEYS.iter().copied().filter(move |key| {
        fields.get(*key).is_some_and(|value| {
            let Some(text) = value.as_str() else {
                return true;
            };
            if *key == "prov_key" {
                ProvKey::parse(text).is_err()
            } else {
                text.parse::<Id>().is_err()
            }
        })
    })
}

/// Codes only: services choose the catalogue wording when they wire these guards in P2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validation {
    ReservedType,
    ReservedRelation,
    DerivedRelation,
    ReservedField,
    SessionAgentConflict,
}

impl Validation {
    pub fn code(self) -> &'static str {
        match self {
            Self::ReservedType => "reserved_type",
            Self::ReservedRelation => "reserved_relation",
            Self::DerivedRelation => "derived_relation",
            Self::ReservedField => "reserved_name",
            Self::SessionAgentConflict => "session_agent_conflict",
        }
    }
}

pub fn user_node_warning(kind: &str) -> Option<Validation> {
    PROVENANCE_TYPES
        .contains(&kind)
        .then_some(Validation::ReservedType)
}

pub fn validate_user_edge(kind: &str) -> Result<(), Validation> {
    if is_derived_relation(kind) {
        Err(Validation::DerivedRelation)
    } else if PROVENANCE_RELATIONS.contains(&kind) {
        Err(Validation::ReservedRelation)
    } else {
        Ok(())
    }
}

pub fn validate_user_field(name: &str) -> Result<(), Validation> {
    if PROVENANCE_KEYS.contains(&name) {
        Err(Validation::ReservedField)
    } else {
        Ok(())
    }
}

pub fn validate_session_agent(existing: Option<Id>, proposed: Id) -> Result<(), Validation> {
    if existing.is_some_and(|id| id != proposed) {
        Err(Validation::SessionAgentConflict)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_hashes_are_canonical_and_bad_raw_ids_are_never_hashed() {
        let raw = "Abc_1234";
        let hashed = hash_session_id(raw).unwrap();
        assert!(is_session_hash(&hashed));
        assert_ne!(hashed, raw);
        assert_ne!(hash_session_id(&hashed).unwrap(), hashed);
        assert_eq!(hash_session_id(raw).unwrap(), hashed);
        for bad in [
            "",
            "short",
            "../escape",
            "a\\b",
            "abc defgh",
            &"x".repeat(129),
        ] {
            assert_eq!(hash_session_id(bad), Err(InvalidKey));
        }

        for bad in [
            raw,
            "unknown",
            "h-ABCDEF0123456789",
            "h-abcdef012345678",
            "h-abcdef01234567890",
        ] {
            assert!(!is_session_hash(bad));
        }
    }

    #[test]
    fn captured_names_are_validated_and_scanned_before_normalization() {
        assert_eq!(
            identity_value("model", "openai/GPT-5.6"),
            Ok("openai/GPT-5.6".into())
        );
        assert_eq!(identity_value("user", "A.User"), Ok("a.user".into()));
        assert_eq!(
            identity_value("seat", "pij-calm-otter"),
            Ok("pij-calm-otter".into())
        );
        let token = format!("ghp_{}", "a".repeat(36));
        for field in ["model", "user", "role", "effort", "harness"] {
            assert!(identity_value(field, &token).is_err());
        }
        assert_eq!(login_user(Some(&token), None), None);
        assert_eq!(
            login_user(Some(&token), Some("writer")),
            Some("writer".into())
        );
        for model in [
            "/absolute",
            "a/",
            "a/b/c",
            "../model",
            "a/..",
            "-a/b",
            "a/-b",
            "a\\b",
        ] {
            assert!(identity_value("model", model).is_err());
        }
        for seat in [
            "pij-one",
            "pij-A-otter",
            "-pij-a-b",
            "pij-a--b",
            "pij-a-b-",
            "other-a-b",
        ] {
            assert!(!is_pij_seat(seat));
        }
        assert!(identity_value("user", &"a".repeat(129)).is_err());
        assert!(identity_value("unknown_field", "safe").is_err());
    }

    #[test]
    fn key_grammar_and_session_fallbacks() {
        for s in [
            "agent/pij/pij-seat",
            "agent/copilot/user",
            "agent/cli/user",
            "agent/unknown",
            "session/copilot/Abc_1234",
            "session/cli/u",
            "session/unknown/unknown",
            "session/copilot/unknown",
            "model/claude-opus-5.5",
            "harness/copilot",
            "platform/pij",
        ] {
            assert_eq!(ProvKey::parse(s).unwrap().as_str(), s);
        }
        let long = format!("session/copilot/{}", "A".repeat(128));
        assert!(ProvKey::parse(&long).is_ok());
        for s in [
            "",
            "session/unknown",
            "session/copilot/a",
            "session/copilot/..",
            "agent/cli/.",
            "session/../Abc_1234",
            "session/copilot/abc/12345",
            "session/copilot/abc\\12345",
            "agent/cli/User",
            "agent/cli/",
            "platform/other",
            "model/a/b",
            "agent/cli/../x",
        ] {
            assert!(ProvKey::parse(s).is_err(), "{s}");
        }
        assert!(ProvKey::parse(&format!("model/{}", "a".repeat(65))).is_err());
        assert!(ProvKey::parse(&format!("session/copilot/{}", "A".repeat(129))).is_err());
        assert_eq!(
            session_key("copilot", "Abc_1234"),
            Ok(("session/copilot/Abc_1234".into(), false))
        );
        for id in [
            "short",
            "../escape",
            "/absolute",
            "a\\b",
            "abc defgh",
            &"a".repeat(129),
        ] {
            assert_eq!(
                session_key("copilot", id),
                Ok(("session/copilot/unknown".into(), true))
            );
        }
        assert_eq!(normalise_user("  A  User@host  "), Ok("a-user-host".into()));
        for raw in ["", "///", ".", ".."] {
            assert_eq!(normalise_user(raw), Ok("unknown".into()));
        }
        assert!(normalise_user(&"a".repeat(65)).is_err());
        assert_eq!(
            model_key("openai/gpt-5.6"),
            Ok("model/openai-gpt-5.6".into())
        );
        assert_eq!(
            model_key("Claude-Opus-5.5"),
            Ok("model/claude-opus-5.5".into())
        );
        assert_eq!(harness_key("Copilot CLI"), Ok("harness/copilot-cli".into()));
        assert_eq!(normalise_name("..-Model..--"), Ok("model".into()));
    }

    fn input(tier: Tier) -> ActorInput {
        ActorInput {
            tier,
            seat: None,
            role: None,
            model: None,
            effort: None,
            harness: None,
            harness_session: None,
            user: None,
        }
    }

    #[test]
    fn actor_tiers_validate_on_construction_and_deserialization() {
        let cases = [
            (
                input(Tier::Unknown),
                "agent/unknown",
                "session/unknown/unknown",
            ),
            (
                ActorInput {
                    user: Some("user".into()),
                    ..input(Tier::Cli)
                },
                "agent/cli/user",
                "session/cli/user",
            ),
            (
                ActorInput {
                    harness: Some("copilot".into()),
                    user: Some("user".into()),
                    harness_session: Some("Abc_1234".into()),
                    ..input(Tier::Harness)
                },
                "agent/copilot/user",
                "session/copilot/Abc_1234",
            ),
            (
                ActorInput {
                    seat: Some("pij-seat".into()),
                    role: Some("coder".into()),
                    model: Some("gpt-6.1-sol".into()),
                    effort: Some("high".into()),
                    harness: Some("copilot".into()),
                    ..input(Tier::Pij)
                },
                "agent/pij/pij-seat",
                "session/copilot/unknown",
            ),
        ];
        for (raw, agent, session) in cases {
            let actor = Actor::try_from(raw.clone()).unwrap();
            assert_eq!(actor.agent_key(), agent);
            assert_eq!(actor.session_key(), session);
            assert!(ProvKey::parse(&actor.session_key()).is_ok());
            assert_eq!(
                serde_json::from_slice::<Actor>(&serde_json::to_vec(&raw).unwrap()).unwrap(),
                actor
            );
        }
        for raw in [
            input(Tier::Pij),
            input(Tier::Harness),
            input(Tier::Cli),
            ActorInput {
                user: Some("user\nsecret".into()),
                ..input(Tier::Cli)
            },
            ActorInput {
                role: Some("x".repeat(65)),
                ..input(Tier::Unknown)
            },
            ActorInput {
                harness_session: Some("../path".into()),
                ..input(Tier::Unknown)
            },
        ] {
            assert!(Actor::try_from(raw.clone()).is_err());
            assert!(serde_json::from_slice::<Actor>(&serde_json::to_vec(&raw).unwrap()).is_err());
        }
        let valid = ActorInput {
            seat: Some("pij-seat".into()),
            harness: Some("copilot".into()),
            ..input(Tier::Pij)
        };
        for field in [
            "seat",
            "role",
            "model",
            "effort",
            "harness",
            "harness_session",
            "user",
        ] {
            for value in [
                "",
                "../escape",
                "a\\b",
                "has space",
                "x\n",
                &"x".repeat(129),
            ] {
                let mut raw = serde_json::to_value(&valid).unwrap();
                raw[field] = serde_json::json!(value);
                let raw: ActorInput = serde_json::from_value(raw).unwrap();
                assert_eq!(Actor::try_from(raw), Err(InvalidActor(field)));
            }
        }
    }

    #[test]
    fn user_write_guards_and_session_conflicts() {
        for kind in PROVENANCE_TYPES {
            assert_eq!(user_node_warning(kind), Some(Validation::ReservedType));
        }
        assert_eq!(user_node_warning("problem"), None);
        for kind in PROVENANCE_RELATIONS {
            assert_eq!(validate_user_edge(kind), Err(Validation::ReservedRelation));
        }
        for kind in ["similar_to", "authored_by", "edited_by"] {
            assert_eq!(validate_user_edge(kind), Err(Validation::DerivedRelation));
        }
        assert!(validate_user_edge("supports").is_ok());
        for name in PROVENANCE_KEYS {
            assert_eq!(validate_user_field(name), Err(Validation::ReservedField));
        }
        assert!(validate_user_field("custom").is_ok());
        assert!(validate_session_agent(None, Id(1)).is_ok());
        assert!(validate_session_agent(Some(Id(1)), Id(1)).is_ok());
        assert_eq!(
            validate_session_agent(Some(Id(1)), Id(2)),
            Err(Validation::SessionAgentConflict)
        );
    }
}
