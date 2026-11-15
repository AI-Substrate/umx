//! This machine's policy record (ws004 s06 A2): `$UMX_HOME/state/graph-policies.json`, read and written through the
//! LocalState port. It holds the last embedding policy confirmed on this machine for each graph, keyed by graph id
//! and by folder, so an edit of graph.json (by hand, by a pull, by an agent) can make a graph stricter at once but
//! never less strict until a person confirms it at their own terminal.
//!
//! Format: `{"umx_state": 1, "graphs": [...]}`, records sorted by graph id then folder, keys in a fixed order;
//! unknown keys are kept and written back. A newer `umx_state` is refused, never rewritten.

use crate::id::Id;
use crate::model::{EmbedderRule, Legacy, Policy, Visibility};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The newest state format this build reads and writes, as `"umx_state"`.
pub const STATE_VERSION: u32 = 1;
/// The folder under `$UMX_HOME` that holds the state file.
pub const STATE_DIR: &str = "state";
pub const STATE_FILE: &str = "graph-policies.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyState {
    pub umx_state: u32,
    #[serde(default)]
    pub graphs: Vec<PolicyRecord>,
    /// Keys this build does not know, kept through read and write.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One copy of a graph on this machine: its id, its folder (the resolved path) and the policy last confirmed for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyRecord {
    pub graph: Id,
    pub folder: String,
    /// Written as `policy` (the legacy word an older umx reads; `secured` for every pair but open + any) and, for a
    /// pair that is not a legacy one, `visibility` and `embedder`.
    #[serde(flatten)]
    pub policy: Stated,
    /// When this policy was recorded (ISO 8601 UTC).
    pub confirmed_at: String,
    /// Every downgrade a person confirmed, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub downgrades: Vec<Downgrade>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Downgrade {
    pub confirmed_at: String,
    /// The legacy words; `change` spells the pairs when either is not a legacy one.
    pub from: Legacy,
    pub to: Legacy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A graph's two settings as a record spells them. Read like graph.json: a pair the writer never writes is refused,
/// so the whole record is unusable and every graph counts as secured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Words", into = "Words")]
pub struct Stated(pub Policy);

#[derive(Serialize, Deserialize)]
struct Words {
    policy: Legacy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<Visibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    embedder: Option<EmbedderRule>,
}

impl TryFrom<Words> for Stated {
    type Error = &'static str;

    fn try_from(w: Words) -> Result<Self, Self::Error> {
        Policy::read(w.policy, w.visibility, w.embedder).map(Stated)
    }
}

impl From<Stated> for Words {
    fn from(s: Stated) -> Self {
        let (policy, visibility, embedder) = s.0.written();
        Words {
            policy,
            visibility,
            embedder,
        }
    }
}

/// Why a state file cannot be used. Its contents are never quoted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// Written by a newer umx: refused, and never rewritten by this one.
    Newer(u32),
    /// Does not parse; the reason names only a line and column.
    Invalid(String),
}

impl StateError {
    /// umx's own words for a message.
    pub fn reason(&self) -> String {
        match self {
            StateError::Newer(n) => format!(
                "it is state format {n} and this umx reads up to {STATE_VERSION} (install a newer umx)"
            ),
            StateError::Invalid(r) => r.clone(),
        }
    }
}

fn invalid(e: &serde_json::Error) -> StateError {
    StateError::Invalid(format!(
        "it does not parse at line {} column {} (fix or remove it)",
        e.line(),
        e.column()
    ))
}

impl Default for PolicyState {
    fn default() -> Self {
        PolicyState {
            umx_state: STATE_VERSION,
            graphs: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl PolicyState {
    pub fn decode(bytes: &[u8]) -> Result<PolicyState, StateError> {
        #[derive(Deserialize)]
        struct Head {
            umx_state: u32,
        }
        let head: Head = serde_json::from_slice(bytes).map_err(|e| invalid(&e))?;
        if head.umx_state > STATE_VERSION {
            return Err(StateError::Newer(head.umx_state));
        }
        if head.umx_state == 0 {
            return Err(StateError::Invalid("its umx_state is 0".into()));
        }
        let mut state: PolicyState = serde_json::from_slice(bytes).map_err(|e| invalid(&e))?;
        // D53's trust confirmations (D62 dropped them): never read, and gone at the next write.
        state.extra.remove("trusted");
        Ok(state)
    }

    /// Sorted by graph id then folder, so the same records always give the same bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut sorted = self.clone();
        sorted.umx_state = STATE_VERSION;
        sorted
            .graphs
            .sort_by(|a, b| (a.graph, &a.folder).cmp(&(b.graph, &b.folder)));
        let mut out = serde_json::to_vec_pretty(&sorted).expect("state always serialises");
        out.push(b'\n');
        out
    }

    /// The records for a graph: those with its id (any copy of it) or its folder (whatever id the file now says).
    fn matching(&self, graph: Id, folder: &str) -> impl Iterator<Item = &PolicyRecord> {
        let folder = String::from(folder);
        self.graphs
            .iter()
            .filter(move |r| r.graph == graph || r.folder == folder)
    }

    /// The strictest pair this machine holds for the graph (the join of its records), or None if it holds none.
    pub fn recorded(&self, graph: Id, folder: &str) -> Option<Policy> {
        self.matching(graph, folder)
            .map(|r| r.policy.0)
            .reduce(Policy::join)
    }

    /// When a person last confirmed a downgrade of this graph, if any.
    pub fn last_downgrade(&self, graph: Id, folder: &str) -> Option<String> {
        self.matching(graph, folder)
            .flat_map(|r| r.downgrades.iter().map(|d| d.confirmed_at.clone()))
            .max()
    }

    /// Records `policy` for this copy (id and folder), joined with what it holds: never looser on either setting.
    /// Returns whether anything changed.
    pub fn raise(&mut self, graph: Id, folder: &str, policy: Policy, at: &str) -> bool {
        match self
            .graphs
            .iter_mut()
            .find(|r| r.graph == graph && r.folder == folder)
        {
            Some(r) if r.policy.0.join(policy) == r.policy.0 => false,
            Some(r) => {
                r.policy = Stated(r.policy.0.join(policy));
                r.confirmed_at = at.into();
                true
            }
            None => {
                self.graphs.push(PolicyRecord {
                    graph,
                    folder: folder.into(),
                    policy: Stated(policy),
                    confirmed_at: at.into(),
                    downgrades: Vec::new(),
                    extra: BTreeMap::new(),
                });
                true
            }
        }
    }

    /// A downgrade a person confirmed: every record of the graph (by id or folder) stricter than `to` on either
    /// setting becomes `to`, and each records the change. Returns how many records changed.
    pub fn lower(&mut self, graph: Id, folder: &str, to: Policy, at: &str) -> usize {
        let mut changed = 0;
        for r in self
            .graphs
            .iter_mut()
            .filter(|r| (r.graph == graph || r.folder == folder) && r.policy.0.exceeds(to))
        {
            let from = r.policy.0;
            let legacy = |p: Policy| p.written().0;
            r.downgrades.push(Downgrade {
                confirmed_at: at.into(),
                from: legacy(from),
                to: legacy(to),
                change: (from.written().1.is_some() || to.written().1.is_some())
                    .then(|| format!("{} to {}", from.name(), to.name())),
                extra: BTreeMap::new(),
            });
            r.policy = Stated(to);
            r.confirmed_at = at.into();
            changed += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: &str = "2026-11-10T00:00:00Z";
    const LATER: &str = "2026-11-11T00:00:00Z";

    #[test]
    fn records_round_trip_sorted_with_unknown_keys_kept() {
        let text = r#"{"umx_state":1,"graphs":[
            {"graph":"01JC8X0000000000000000000B","folder":"/b","policy":"secured","confirmed_at":"t","later":1},
            {"graph":"01JC8X0000000000000000000A","folder":"/z","policy":"open","confirmed_at":"t",
             "downgrades":[{"confirmed_at":"t","from":"secured","to":"open","why":"x"}]},
            {"graph":"01JC8X0000000000000000000A","folder":"/a","policy":"secured","confirmed_at":"t"}],
            "note":"kept"}"#;
        let state = PolicyState::decode(text.as_bytes()).unwrap();
        let bytes = state.encode();
        let again = PolicyState::decode(&bytes).unwrap();
        assert_eq!(again.encode(), bytes, "encode is stable");
        let folders: Vec<&str> = again.graphs.iter().map(|r| r.folder.as_str()).collect();
        assert_eq!(
            folders,
            ["/a", "/z", "/b"],
            "sorted by graph id then folder"
        );
        let out = String::from_utf8(bytes).unwrap();
        for kept in [r#""later": 1"#, r#""why": "x""#, r#""note": "kept""#] {
            assert!(out.contains(kept), "{kept} in {out}");
        }
        let at = |k: &str| out.find(k).unwrap();
        assert!(at("\"graph\"") < at("\"folder\"") && at("\"policy\"") < at("\"confirmed_at\""));
    }

    #[test]
    fn a_newer_or_broken_state_is_an_error_that_quotes_nothing() {
        assert_eq!(
            PolicyState::decode(br#"{"umx_state":2,"graphs":"whatever"}"#),
            Err(StateError::Newer(2))
        );
        for bad in [
            &b"{\"umx_state\":1,\"graphs\":[{\"graph\":\"01JC8X0000000000000000000A\"}]}"[..],
            b"{\"umx_state\":1,\"graphs\":[{\"graph\":\"01JC8X0000000000000000000A\",\"folder\":\"/a\",\"policy\":\"secret-word\",\"confirmed_at\":\"t\"}]}",
            b"not json",
            b"{\"umx_state\":0}",
            b"",
        ] {
            let err = PolicyState::decode(bad).unwrap_err();
            assert!(matches!(err, StateError::Invalid(_)), "{err:?}");
            assert!(!err.reason().contains("secret-word"), "{}", err.reason());
        }
    }

    #[test]
    fn a_record_by_id_or_by_folder_counts() {
        let (a, b) = (Id(1), Id(2));
        let mut s = PolicyState::default();
        assert!(s.raise(a, "/one", Policy::SECURED, AT));
        assert!(!s.raise(a, "/one", Policy::SECURED, LATER), "no change");
        // Another copy of the same graph, and another id in the same folder, are both held secured.
        assert_eq!(s.recorded(a, "/copy"), Some(Policy::SECURED));
        assert_eq!(s.recorded(b, "/one"), Some(Policy::SECURED));
        assert_eq!(s.recorded(b, "/elsewhere"), None);
    }

    #[test]
    fn lowering_records_each_downgrade_and_raising_hides_it() {
        let a = Id(1);
        let mut s = PolicyState::default();
        s.raise(a, "/one", Policy::SECURED, AT);
        s.raise(a, "/copy", Policy::SECURED, AT);
        assert_eq!(s.last_downgrade(a, "/one"), None);
        assert_eq!(s.lower(a, "/one", Policy::OPEN, LATER), 2, "every copy");
        assert_eq!(s.recorded(a, "/one"), Some(Policy::OPEN));
        assert_eq!(s.last_downgrade(a, "/one").as_deref(), Some(LATER));
        assert_eq!(
            s.lower(a, "/one", Policy::OPEN, LATER),
            0,
            "nothing stricter left"
        );
        s.raise(a, "/one", Policy::SECURED, "2026-11-12T00:00:00Z");
        assert_eq!(s.recorded(a, "/copy"), Some(Policy::SECURED));
        // The history stays in the file; whether to show it is the caller's (only while the graph is open).
        assert_eq!(s.graphs[0].downgrades.len(), 1);
    }

    #[test]
    fn pairs_join_per_setting_and_trust_binds_to_its_target() {
        use crate::model::{EmbedderRule, Visibility};
        let a = Id(1);
        let named_trusted = Policy {
            visibility: Visibility::Named,
            embedder: EmbedderRule::Trusted,
        };
        let open_local = Policy {
            visibility: Visibility::Open,
            embedder: EmbedderRule::Local,
        };
        let mut s = PolicyState::default();
        assert!(s.raise(a, "/one", named_trusted, AT));
        // Neither pair is stricter than the other: the record joins them, so neither setting loosens.
        assert!(s.raise(a, "/one", open_local, AT));
        assert_eq!(s.recorded(a, "/one"), Some(Policy::SECURED));
        assert!(Policy::SECURED.exceeds(named_trusted) && !named_trusted.exceeds(Policy::SECURED));
        assert_eq!(s.lower(a, "/one", named_trusted, LATER), 1);
        assert_eq!(s.recorded(a, "/one"), Some(named_trusted));
        // A custom pair keeps `policy: secured` for an older umx, and reads back as itself.
        let bytes = s.encode();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains(r#""policy": "secured""#), "{text}");
        assert!(
            text.contains(r#""change": "secured to named+trusted""#),
            "{text}"
        );
        assert_eq!(PolicyState::decode(&bytes).unwrap(), s);
        // A record spelling a pair the writer never writes makes the whole state unusable (every graph secured).
        let bad = text
            .replace(r#""embedder": "trusted""#, r#""embedder": "any""#)
            .replace(r#""visibility": "named""#, r#""visibility": "open""#);
        assert!(PolicyState::decode(bad.as_bytes()).is_err());
        assert_eq!(PolicyState::decode(&s.encode()).unwrap(), s);
    }

    #[test]
    fn a_record_from_d53_with_trust_confirmations_loads_and_drops_them_at_the_next_write() {
        let old = r#"{
  "umx_state": 1,
  "graphs": [],
  "trusted": [{ "instance": "az", "target": "0123", "confirmed_at": "2026-11-11T00:00:00Z" }],
  "kept": true
}"#;
        let s = PolicyState::decode(old.as_bytes()).unwrap();
        let text = String::from_utf8(s.encode()).unwrap();
        assert!(!text.contains("trusted") && !text.contains("az"), "{text}");
        assert!(
            text.contains("\"kept\": true"),
            "other unknown keys are still kept: {text}"
        );
    }
}
