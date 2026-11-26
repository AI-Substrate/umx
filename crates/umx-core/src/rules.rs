//! Graph shape as data. Patterns read only a node's stored values; regex execution belongs to services.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::{Node, Value};

pub const MAX_RULES: usize = 64;
pub const MAX_RULE_SETS: usize = 32;
pub const MAX_RULE_SET_BYTES: u64 = 64 << 10;
pub const MAX_REGEX_CHARS: usize = 256;
pub const MAX_REGEXES: usize = 4;

pub fn name_ok(name: &str) -> bool {
    match name.split_once(':') {
        Some((set, rule)) => crate::name::is_snake_case(set) && crate::name::is_valid_name(rule),
        None => crate::name::is_valid_name(name),
    }
}

pub fn set_name_ok(name: &str) -> bool {
    name != "std" && crate::name::is_snake_case(name)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub rules_off: Vec<String>,
}

impl RuleSet {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        let set: Self = serde_json::from_slice(bytes)
            .map_err(|_| "invalid JSON, unknown keys, or a field with the wrong type")?;
        set.check()?;
        Ok(set)
    }

    pub fn check(&self) -> Result<(), &'static str> {
        if self.rules.len() > MAX_RULES {
            return Err("a rule set holds at most 64 rules");
        }
        stored_ok(&self.rules, &self.rules_off)
    }

    /// Loaded names are qualified; the separately copied source bytes remain unchanged.
    pub fn qualified(mut self, set: &crate::ValidName) -> Self {
        for rule in &mut self.rules {
            rule.name = format!("{}:{}", set.as_str(), rule.name);
        }
        self.rules.sort_by(|a, b| a.name.cmp(&b.name));
        self
    }
}

fn present<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    #[serde(default, rename = "type", skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub field: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub regex: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    #[serde(rename = "match")]
    pub pattern: Pattern,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub field: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub edge: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub from: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub to: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub to_node: Option<crate::Id>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
}

impl Rule {
    pub fn sentence(&self) -> crate::SafeText {
        let subject = match self.pattern.types.first() {
            Some(kind) => format!(
                "{} {}",
                if Self::article(kind) == "an" {
                    "An"
                } else {
                    "A"
                },
                Self::alternatives(&self.pattern.types)
            ),
            None => String::from("A matching node"),
        };
        let shape = if let Some(field) = &self.field {
            format!("the {field} field")
        } else {
            let edge = self.edge.as_deref().expect("checked shape");
            let end = if let Some(types) = &self.from {
                format!(
                    "from {} {}",
                    Self::article(&types[0]),
                    Self::alternatives(types)
                )
            } else if let Some(types) = &self.to {
                format!(
                    "to {} {}",
                    Self::article(&types[0]),
                    Self::alternatives(types)
                )
            } else {
                format!("to node {}", self.to_node.expect("checked shape"))
            };
            format!("{} {edge} edge {end}", Self::article(edge))
        };
        crate::SafeText::rule_sentence(&format!("{subject} should have {shape}."))
    }

    fn article(word: &str) -> &'static str {
        if word.starts_with(['a', 'e', 'i', 'o', 'u']) {
            "an"
        } else {
            "a"
        }
    }

    fn alternatives(types: &[String]) -> String {
        let (last, rest) = types.split_last().expect("checked type list");
        if rest.is_empty() {
            last.clone()
        } else {
            format!("{} or {last}", rest.join(", "))
        }
    }

    pub fn from_value(value: &Value) -> Option<Self> {
        Self::deserialize(value).ok()
    }

    /// The reason is always a fixed sentence, never input text.
    pub fn check(&self) -> Result<(), &'static str> {
        if !name_ok(&self.name) {
            return Err("a rule name is snake_case, optionally qualified by a set");
        }
        let p = &self.pattern;
        if p.regex.len() > MAX_REGEXES {
            return Err("a rule has at most 4 regex fields");
        }
        if p.types.is_empty()
            && p.status.is_empty()
            && p.tag.is_empty()
            && p.field.is_empty()
            && p.regex.is_empty()
        {
            return Err("match needs at least one nonempty pattern key");
        }
        if p.types
            .iter()
            .chain(&p.status)
            .any(|s| !crate::name::is_valid_name(s))
            || self
                .field
                .iter()
                .chain(&self.edge)
                .any(|s| !crate::name::is_valid_name(s))
            || p.field
                .keys()
                .chain(p.regex.keys())
                .any(|s| !crate::name::is_valid_name(s))
        {
            return Err("types, statuses, fields and relations are snake_case names");
        }
        if p.tag.iter().any(|t| {
            crate::tags::Pattern::parse(t).is_err()
                || crate::tags::Pattern::parse(t).is_ok_and(|p| p.text() != *t)
        }) {
            return Err("a tag pattern is a canonical tag or facet:*");
        }
        let selectors = usize::from(self.from.is_some())
            + usize::from(self.to.is_some())
            + usize::from(self.to_node.is_some());
        match (&self.field, &self.edge) {
            (Some(_), None) if selectors == 0 => {}
            (None, Some(_)) if selectors == 1 => {}
            _ => return Err("give a field, or an edge with exactly one of from, to or to_node"),
        }
        for types in self.from.iter().chain(&self.to) {
            if types.is_empty() || types.iter().any(|s| !crate::name::is_valid_name(s)) {
                return Err("from and to each need a nonempty list of snake_case type names");
            }
        }
        if self.description.as_ref().is_some_and(|d| {
            d.chars().count() > crate::ontology::MAX_DESCRIPTION_CHARS || d.contains(['\n', '\r'])
        }) {
            return Err("a description is one line of at most 500 characters");
        }
        Ok(())
    }
}

impl Pattern {
    pub fn matches_exact(&self, node: &Node) -> bool {
        (self.types.is_empty() || self.types.contains(&node.kind))
            && (self.status.is_empty() || self.status.contains(&node.status))
            && (self.tag.is_empty()
                || node.fields.get(crate::tags::TAGS).is_some_and(|v| {
                    let tags = crate::tags::stored_tags(v);
                    self.tag.iter().any(|p| {
                        crate::tags::Pattern::parse(p)
                            .is_ok_and(|p| tags.iter().any(|t| p.matches(t)))
                    })
                }))
            && self
                .field
                .iter()
                .all(|(key, value)| node.fields.get(key) == Some(value))
    }
}

pub fn stored_ok(rules: &[Rule], off: &[String]) -> Result<(), &'static str> {
    if rules.len() > MAX_RULES {
        return Err("a graph holds at most 64 rules of its own");
    }
    let mut names = BTreeSet::new();
    for rule in rules {
        rule.check()?;
        if rule.name.contains(':') || !names.insert(&rule.name) {
            return Err("stored rule names are unique and unqualified");
        }
    }
    let mut names = BTreeSet::new();
    for name in off {
        if !name_ok(name) || !names.insert(name) {
            return Err("rules_off holds unique rule names");
        }
    }
    Ok(())
}

/// Central, filename-ordered sets, then graph rules, minus switch-offs from every source.
pub fn effective<'a>(
    central: &'a [Rule],
    sets: &'a BTreeMap<String, RuleSet>,
    own: &'a [Rule],
    off: &[String],
) -> Vec<&'a Rule> {
    let off = off_names(sets, off);
    central
        .iter()
        .chain(sets.values().flat_map(|set| &set.rules))
        .chain(own)
        .filter(|r| !off.contains(r.name.as_str()))
        .collect()
}

pub fn off_names<'a>(sets: &'a BTreeMap<String, RuleSet>, off: &'a [String]) -> BTreeSet<&'a str> {
    off.iter()
        .chain(sets.values().flat_map(|set| &set.rules_off))
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn rule_sentences_name_types_with_articles_and_natural_lists() {
        for (row, sentence) in [
            (
                json!({"name":"rated","match":{"type":["event"]},"field":"rating"}),
                "An event should have the rating field.",
            ),
            (
                json!({"name":"rated","match":{"type":["question","risk"]},"field":"rating"}),
                "A question or risk should have the rating field.",
            ),
            (
                json!({"name":"owned","match":{"type":["question","decision","action","risk"]},"edge":"owns","from":["person","organisation","team"]}),
                "A question, decision, action or risk should have an owns edge from a person, organisation or team.",
            ),
            (
                json!({"name":"raised","match":{"type":["risk"]},"edge":"raised_in","to":["event","meeting"]}),
                "A risk should have a raised_in edge to an event or meeting.",
            ),
            (
                json!({"name":"owned","match":{"type":["risk"]},"edge":"owns","from":["organisation"]}),
                "A risk should have an owns edge from an organisation.",
            ),
            (
                json!({"name":"rated","match":{"status":["open"],"tag":["origin:import"],"field":{"rating":null},"regex":{"title":"^SEC"}},"field":"rating"}),
                "A matching node should have the rating field.",
            ),
        ] {
            let rule = Rule::from_value(&row).unwrap();
            assert!(rule.check().is_ok());
            assert_eq!(rule.sentence().as_str(), sentence);
        }
    }

    #[test]
    fn rules_read_all_three_shapes_and_refuse_ambiguous_or_unknown_keys() {
        for row in [
            json!({"name":"rated","match":{"type":["risk"]},"field":"rating"}),
            json!({"name":"owned","match":{"type":["risk"]},"edge":"owns","from":["person","team"]}),
            json!({"name":"register","match":{"regex":{"title":"^SEC-[0-9]+"}},"edge":"part_of","to_node":"00000000000000000000000001"}),
            json!({"name":"four","match":{"regex":{"a":"x","b":"x","c":"x","d":"x"}},"field":"rating"}),
        ] {
            let rule = Rule::from_value(&row).unwrap();
            assert!(rule.check().is_ok());
            assert_eq!(serde_json::to_value(&rule).unwrap(), row);
        }
        for row in [
            json!({"name":"bad","match":{},"field":"rating"}),
            json!({"name":"bad","match":{"type":["risk"]},"field":"rating","edge":"owns","from":["person"]}),
            json!({"name":"bad","match":{"type":["risk"]},"edge":"owns"}),
            json!({"name":"bad","match":{"type":["risk"]},"edge":"owns","from":[]}),
            json!({"name":"bad","match":{"type":["risk"]},"edge":"owns","from":["person"],"to":["team"]}),
            json!({"name":"bad","match":{"type":["risk"]},"field":"rating","fill":false}),
            json!({"name":"bad","match":{"type":["risk"]},"field":"rating","edge":null}),
            json!({"name":"bad","match":{"type":["risk"]},"field":"rating","description":null}),
            json!({"name":"bad","match":{"regex":{"a":"x","b":"x","c":"x","d":"x","e":"x"}},"field":"rating"}),
        ] {
            assert!(
                Rule::from_value(&row).is_none_or(|r| r.check().is_err()),
                "{row}"
            );
        }
    }

    #[test]
    fn effective_rules_subtract_central_names_without_overriding_or_writing_rows() {
        let std = vec![
            Rule::from_value(
                &json!({"name":"std:owned","match":{"type":["risk"]},"field":"owner"}),
            )
            .unwrap(),
            Rule::from_value(
                &json!({"name":"std:rated","match":{"type":["risk"]},"field":"rating"}),
            )
            .unwrap(),
        ];
        let own = vec![
            Rule::from_value(&json!({"name":"rated","match":{"type":["risk"]},"field":"rating"}))
                .unwrap(),
        ];
        let sets = BTreeMap::new();
        let rules = effective(&std, &sets, &own, &["std:owned".into()]);
        assert_eq!(
            rules.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["std:rated", "rated"]
        );
        assert_eq!(effective(&[], &BTreeMap::new(), &own, &[]), [&own[0]]);
        assert_eq!(std.len(), 2);
        assert!(stored_ok(&own, &["std:owned".into()]).is_ok());
        assert!(stored_ok(&std, &[]).is_err());
        assert!(stored_ok(&own, &["owned".into(), "other:unknown".into()]).is_ok());
    }

    #[test]
    fn rule_sets_compose_in_filename_order_and_switch_off_every_source() {
        let row = |name: &str| {
            Rule::from_value(&json!({"name":name,"match":{"type":["risk"]},"field":"rating"}))
                .unwrap()
        };
        let central = vec![row("std:rated")];
        let own = vec![row("local"), row("kept")];
        let sets = BTreeMap::from([
            (
                "zeta".into(),
                RuleSet {
                    rules: vec![row("rated")],
                    rules_off: vec!["alpha:gone".into(), "local".into(), "future:unknown".into()],
                }
                .qualified(&crate::ValidName::new("zeta").unwrap()),
            ),
            (
                "alpha".into(),
                RuleSet {
                    rules: vec![row("gone"), row("kept")],
                    rules_off: vec!["std:rated".into()],
                }
                .qualified(&crate::ValidName::new("alpha").unwrap()),
            ),
        ]);
        assert_eq!(
            effective(&central, &sets, &own, &[])
                .iter()
                .map(|rule| rule.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha:kept", "zeta:rated", "kept"]
        );
        assert_eq!(
            effective(&central, &sets, &own, &["zeta:rated".into()])
                .iter()
                .map(|rule| rule.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha:kept", "kept"]
        );
        assert_eq!(sets["zeta"].rules_off[2], "future:unknown");
    }

    #[test]
    fn a_set_refuses_unknown_keys_duplicate_or_qualified_rows_and_bad_switch_names() {
        let row = json!({"name":"rated","match":{"type":["risk"]},"field":"rating"});
        let set: RuleSet = serde_json::from_value(json!({
            "rules":[row.clone()],
            "rules_off":["rated","std:owned","another:missing"]
        }))
        .unwrap();
        assert!(set.check().is_ok());
        for value in [
            json!({"rules":[row.clone(),row.clone()]}),
            json!({"rules":[{"name":"other:rated","match":{"type":["risk"]},"field":"rating"}]}),
            json!({"rules_off":["other:bad:rule"]}),
            json!({"rules_off":["rated","rated"]}),
            json!({"hubs":[]}),
            json!({"fill":true}),
            json!({"rules":[{"name":"rated","match":{"type":["risk"]},"field":"rating","fill":false}]}),
        ] {
            assert!(
                serde_json::from_value::<RuleSet>(value.clone())
                    .map_or(true, |set| set.check().is_err()),
                "{value}"
            );
        }
    }
}
