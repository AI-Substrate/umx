//! Hubs: declared places where nodes gather (graph.json `hubs`). A named hub is one node, optionally with a pattern
//! whose matching nodes umx joins to it; a type hub makes every node of a type a hub. Hubs never warn about shape.

use alloc::collections::BTreeSet;
use alloc::string::String;
use serde::{Deserialize, Serialize};

use crate::Id;
use crate::rules::{Pattern, Rule};

pub const MAX_HUBS: usize = 64;
/// The edge field umx sets on each edge it adds for a hub, naming the hub. Only umx writes it.
pub const AUTO_ADDED_BY: &str = "auto_added_by";
/// Statuses that retire a node (umx has no node delete: a node that no longer holds moves to one of these). A named
/// hub whose node is retired still gathers members; umx says so, with `umx hub remove <name>` as the next.
pub const RETIRED_STATUSES: &[&str] = &["ruled_out", "superseded"];

/// The relation a named hub's members join it by when none is given.
pub const DEFAULT_VIA: &str = "part_of";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hub {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<Id>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub via: String,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub pattern: Option<Pattern>,
}

impl Hub {
    pub fn form(&self) -> &'static str {
        if self.node.is_some() { "named" } else { "type" }
    }

    /// The pattern as a rule row, so a named hub's match is checked and compiled exactly as a rule's is.
    pub fn as_rule(&self) -> Option<Rule> {
        Some(Rule {
            name: self.name.clone(),
            pattern: self.pattern.clone()?,
            field: None,
            edge: Some(self.via.clone()),
            from: None,
            to: None,
            to_node: Some(self.node?),
            description: None,
        })
    }

    /// The reason is always a fixed sentence, never input text.
    pub fn check(&self) -> Result<(), &'static str> {
        if !crate::name::is_valid_name(&self.name) {
            return Err("a hub name is snake_case");
        }
        if !crate::name::is_valid_name(&self.via) {
            return Err("a hub's via is a snake_case relation name");
        }
        match (&self.node, &self.kind) {
            (Some(_), None) => {}
            (None, Some(kind)) if crate::name::is_valid_name(kind) => {}
            (None, Some(_)) => return Err("a type hub names a snake_case type"),
            _ => return Err("a hub names one node, or one type with --type-hub, not both"),
        }
        if self.pattern.is_some() {
            if self.kind.is_some() {
                return Err(
                    "a type hub takes no pattern: umx cannot know which of its hubs a node belongs to",
                );
            }
            self.as_rule().expect("named with a pattern").check()?;
        }
        Ok(())
    }
}

/// A pattern object (a hub's `match`), or None when it has a key or value a pattern does not take.
pub fn pattern_from_value(value: &crate::Value) -> Option<Pattern> {
    Pattern::deserialize(value).ok()
}

pub fn stored_ok(hubs: &[Hub]) -> Result<(), &'static str> {
    if hubs.len() > MAX_HUBS {
        return Err("a graph holds at most 64 hubs");
    }
    let mut names = BTreeSet::new();
    for hub in hubs {
        hub.check()?;
        if !names.insert(&hub.name) {
            return Err("hub names are unique in a graph");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hubs_read_both_forms_and_refuse_mixed_or_patterned_type_hubs() {
        for row in [
            json!({"name":"risk_register","node":"00000000000000000000000001","via":"part_of","match":{"type":["risk"]}}),
            json!({"name":"register","node":"00000000000000000000000001","via":"part_of"}),
            json!({"name":"meetings","type":"meeting","via":"raised_in"}),
        ] {
            let hub: Hub = serde_json::from_value(row.clone()).unwrap();
            assert_eq!(hub.check(), Ok(()), "{row}");
            assert_eq!(serde_json::to_value(&hub).unwrap(), row);
        }
        for row in [
            json!({"name":"meetings","type":"meeting","via":"raised_in","match":{"type":["risk"]}}),
            json!({"name":"both","type":"meeting","node":"00000000000000000000000001","via":"part_of"}),
            json!({"name":"neither","via":"part_of"}),
            json!({"name":"Bad","type":"meeting","via":"raised_in"}),
            json!({"name":"empty","node":"00000000000000000000000001","via":"part_of","match":{}}),
        ] {
            let hub: Hub = serde_json::from_value(row.clone()).unwrap();
            assert!(hub.check().is_err(), "{row}");
        }
        assert!(
            serde_json::from_value::<Hub>(json!({"name":"x","type":"t","via":"v","fill":true}))
                .is_err()
        );
        let hub: Hub =
            serde_json::from_value(json!({"name":"meetings","type":"meeting","via":"raised_in"}))
                .unwrap();
        assert_eq!(stored_ok(core::slice::from_ref(&hub)), Ok(()));
        assert!(stored_ok(&[hub.clone(), hub]).is_err());
    }
}
