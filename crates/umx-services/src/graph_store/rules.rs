//! Rule management and bounded Rust-regex matching. No write-time checking or automatic edge creation.

use super::{GraphStore, alias_name, persist};
use crate::problems;
use crate::{Ctx, Done, Outcome};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use regex_automata::Input;
use regex_automata::hybrid::dfa;
use regex_automata::nfa::thompson::{
    NFA, WhichCaptures,
    pikevm::{Cache, PikeVM},
};
use umx_core::codec::GraphFile;
use umx_core::rules::{MAX_REGEX_CHARS, MAX_RULES, Rule};
use umx_core::standard::{std_field, std_relation, std_status, std_type};
use umx_core::ui::Touched;
use umx_core::{Command, GraphMeta, Id, Node, Ontology, Problem, ValidName, Value};

const MAX_COMPILED_BYTES: usize = 1 << 20;
pub(super) const MAX_GRAPH_COMPILED_BYTES: usize = 8 << 20;
const MAX_DFA_CACHE_BYTES: usize = 64 << 10;
pub(super) const MAX_NODE_MATCH_WORK: u64 = 1_000_000;
pub(super) const MAX_MATCH_WORK: u64 = 100_000_000;
const MAX_NOT_CHECKED: usize = 100;

pub struct RuleMatcher {
    rule: Rule,
    regex: BTreeMap<String, CompiledRegex>,
    pub(super) work_left: u64,
}

struct CompiledRegex {
    vm: PikeVM,
    cache: Cache,
    dfa: Option<(dfa::DFA, dfa::Cache)>,
    states: u64,
}

enum CompileError {
    Invalid(Problem),
    Limit(&'static str, usize),
}

impl CompileError {
    fn problem(self) -> Problem {
        match self {
            Self::Invalid(problem) => problem,
            Self::Limit(reason, position) => problems::rule_regex_invalid(reason, position),
        }
    }
}

fn parse_patterns(rule: &Rule) -> Result<Vec<regex_syntax::hir::Hir>, Problem> {
    rule.pattern
        .regex
        .values()
        .map(|pattern| {
            regex_syntax::Parser::new().parse(pattern).map_err(|e| {
                let offset = match e {
                    regex_syntax::Error::Parse(e) => e.span().start.offset,
                    regex_syntax::Error::Translate(e) => e.span().start.offset,
                    _ => 0,
                };
                problems::rule_regex_invalid(
                    "invalid or unsupported Rust regex syntax",
                    pattern[..offset].chars().count() + 1,
                )
            })
        })
        .collect()
}

impl RuleMatcher {
    pub fn new(rule: &Rule) -> Result<Self, Problem> {
        let mut budget = MAX_GRAPH_COMPILED_BYTES;
        Self::compile(rule, &mut budget).map_err(CompileError::problem)
    }

    fn compile(rule: &Rule, budget: &mut usize) -> Result<Self, CompileError> {
        rule.check()
            .map_err(|reason| CompileError::Invalid(problems::rule_invalid(reason)))?;
        // Parse every pattern even when another hits a limit: invalid syntax still refuses a load.
        let patterns = parse_patterns(rule).map_err(CompileError::Invalid)?;
        let mut regex = BTreeMap::new();
        let mut remaining = *budget;
        for ((field, pattern), hir) in rule.pattern.regex.iter().zip(patterns) {
            if pattern.chars().count() > MAX_REGEX_CHARS {
                return Err(CompileError::Limit(
                    "a regex has at most 256 characters",
                    257,
                ));
            }
            let nfa = NFA::compiler()
                .configure(
                    NFA::config()
                        .nfa_size_limit(Some(MAX_COMPILED_BYTES))
                        .which_captures(WhichCaptures::None),
                )
                .build_from_hir(&hir)
                .map_err(|_| {
                    CompileError::Limit("regex exceeds the 1 MiB compile size limit", 1)
                })?;
            remaining = remaining
                .checked_sub(nfa.memory_usage())
                .ok_or(CompileError::Limit(
                    "regex exceeds the graph's 8 MiB compiled-size budget",
                    1,
                ))?;
            let states = nfa.states().len() as u64;
            let dfa = dfa::DFA::builder()
                .configure(
                    dfa::DFA::config()
                        .cache_capacity(MAX_DFA_CACHE_BYTES)
                        .minimum_cache_clear_count(Some(0))
                        .unicode_word_boundary(true),
                )
                .build_from_nfa(nfa.clone())
                // Unsupported DFA assertions or a cache too small use the bounded PikeVM instead.
                .ok()
                .map(|dfa| {
                    let cache = dfa.create_cache();
                    (dfa, cache)
                });
            let vm = PikeVM::new_from_nfa(nfa)
                .map_err(|_| CompileError::Limit("regex cannot be compiled", 1))?;
            let cache = vm.create_cache();
            regex.insert(
                field.clone(),
                CompiledRegex {
                    vm,
                    cache,
                    dfa,
                    states,
                },
            );
        }
        *budget = remaining;
        Ok(Self {
            rule: rule.clone(),
            regex,
            work_left: MAX_MATCH_WORK,
        })
    }

    fn charge(node_left: &mut u64, scan_left: &mut u64, work: u64) -> Option<()> {
        let node = node_left.checked_sub(work)?;
        let scan = scan_left.checked_sub(work)?;
        *node_left = node;
        *scan_left = scan;
        Some(())
    }

    pub(super) fn matches_stored(&mut self, node: umx_graph::NodeRef<'_>) -> Option<bool> {
        let pattern = &self.rule.pattern;
        if (!pattern.types.is_empty() && !pattern.types.iter().any(|t| t == node.kind()))
            || (!pattern.status.is_empty() && !pattern.status.iter().any(|s| s == node.status()))
        {
            return Some(false);
        }
        if pattern.tag.is_empty() && pattern.field.is_empty() && pattern.regex.is_empty() {
            let mut node_left = MAX_NODE_MATCH_WORK;
            Self::charge(&mut node_left, &mut self.work_left, 1)?;
            return Some(true);
        }
        self.matches(&node.to_node())
    }

    /// None marks this node as not checked; a later, cheaper node may still be checked.
    pub fn matches(&mut self, node: &Node) -> Option<bool> {
        if !self.rule.pattern.matches_exact(node) {
            return Some(false);
        }
        let mut node_left = MAX_NODE_MATCH_WORK;
        Self::charge(&mut node_left, &mut self.work_left, 1)?;
        for (field, regex) in &mut self.regex {
            let text = match field.as_str() {
                "title" => Some(node.title.as_str()),
                "note" => Some(node.note.as_str()),
                _ => node.fields.get(field).and_then(Value::as_str),
            };
            let Some(text) = text else {
                return Some(false);
            };
            let bytes = (text.len() as u64).saturating_add(1);
            let dfa_match = match &mut regex.dfa {
                Some((dfa, cache)) => {
                    Self::charge(&mut node_left, &mut self.work_left, bytes)?;
                    match dfa.try_search_fwd(cache, &Input::new(text).earliest(true)) {
                        Ok(found) => Some(found.is_some()),
                        Err(_) => {
                            // A cache at its give-up threshold must not force later nodes into the VM.
                            cache.reset(dfa);
                            None
                        }
                    }
                }
                None => None,
            };
            let matched = match dfa_match {
                Some(matched) => matched,
                None => {
                    Self::charge(
                        &mut node_left,
                        &mut self.work_left,
                        bytes.saturating_mul(regex.states),
                    )?;
                    regex.vm.is_match(&mut regex.cache, text)
                }
            };
            if !matched {
                return Some(false);
            }
        }
        Some(true)
    }
}

pub(super) struct CompiledRules {
    pub(super) matchers: BTreeMap<String, RuleMatcher>,
    skipped: BTreeMap<String, &'static str>,
    remaining: usize,
}

impl CompiledRules {
    pub(super) fn notes(&self, graph: &ValidName) -> Vec<Problem> {
        self.skipped
            .iter()
            .map(|(name, reason)| problems::rule_skipped(name, reason, graph))
            .collect()
    }
}

pub(super) fn compile_rules<'a>(
    rules: impl Iterator<Item = &'a Rule>,
    budget: usize,
) -> Result<CompiledRules, Problem> {
    let mut result = CompiledRules {
        matchers: BTreeMap::new(),
        skipped: BTreeMap::new(),
        remaining: budget,
    };
    let rules: BTreeMap<_, _> = rules.map(|rule| (&rule.name, rule)).collect();
    for (name, rule) in rules {
        match RuleMatcher::compile(rule, &mut result.remaining) {
            Ok(matcher) => {
                result.matchers.insert(name.clone(), matcher);
            }
            Err(CompileError::Limit(reason, _)) => {
                result.skipped.insert(name.clone(), reason);
            }
            Err(CompileError::Invalid(problem)) => return Err(problem),
        }
    }
    Ok(result)
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleEntry {
    pub rule: Rule,
    pub source: &'static str,
    pub off: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RulesListed {
    pub graph: String,
    pub rows: Vec<RuleEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleShown {
    pub graph: String,
    pub rule: Rule,
    pub nodes: Vec<Id>,
    pub total: Option<usize>,
    pub not_checked: Vec<Id>,
    pub not_checked_total: usize,
    pub offset: usize,
    pub limit: usize,
    pub next: Option<Command>,
    pub unchanged: bool,
    pub dry_run: bool,
    pub format: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleChanged {
    pub graph: String,
    pub name: String,
    pub unchanged: bool,
    pub format: u32,
}

fn known_field(o: &Ontology, types: &[String], field: &str) -> bool {
    std_field(field).is_some()
        || o.types
            .iter()
            .filter(|t| types.is_empty() || types.contains(&t.name))
            .any(|t| o.field_decl(&t.name, field).is_some())
}

fn name_notes(o: &Ontology, rule: &Rule, graph: &ValidName) -> Vec<Problem> {
    let fail = |at: &'static str, name: &str, fix: Command| {
        problems::rule_vocabulary_unknown(&rule.name, at, name, fix.word("--graph").name(graph))
    };
    let mut notes = Vec::new();
    for kind in rule
        .pattern
        .types
        .iter()
        .chain(rule.from.iter().flatten())
        .chain(rule.to.iter().flatten())
    {
        if o.type_decl(kind).is_none() && std_type(kind).is_none() {
            notes.push(fail(
                "type",
                kind,
                Command::new("ontology add type")
                    .name(&ValidName::new(kind).expect("shape checked")),
            ));
        }
    }
    for status in &rule.pattern.status {
        if !o.has_status(status) && std_status(status).is_none() {
            notes.push(fail(
                "status",
                status,
                Command::new("ontology add status")
                    .name(&ValidName::new(status).expect("shape checked")),
            ));
        }
    }
    if let Some(edge) = &rule.edge
        && !o.has_relation(edge)
        && std_relation(edge).is_none()
    {
        notes.push(fail(
            "edge",
            edge,
            Command::new("ontology add relation")
                .name(&ValidName::new(edge).expect("shape checked")),
        ));
    }
    for field in rule
        .field
        .iter()
        .chain(rule.pattern.field.keys())
        .chain(rule.pattern.regex.keys())
        .collect::<alloc::collections::BTreeSet<_>>()
    {
        if matches!(field.as_str(), "title" | "note") && rule.pattern.regex.contains_key(field) {
            continue;
        }
        if !known_field(o, &rule.pattern.types, field) {
            let cmd = Command::new("ontology add field");
            let cmd = match rule.pattern.types.first().and_then(|t| ValidName::new(t)) {
                Some(t) => cmd.name(&t),
                None => cmd.word("--all-types"),
            };
            notes.push(fail(
                "field",
                field,
                cmd.name_with(
                    &ValidName::new(field).expect("shape checked"),
                    ':',
                    "string",
                ),
            ));
        }
    }
    notes
}

pub(super) fn validate_stored(meta: &GraphMeta) -> Result<(), Problem> {
    for rule in &meta.rules {
        parse_patterns(rule)?;
    }
    Ok(())
}

pub(super) fn stored_notes(meta: &GraphMeta, graph: &ValidName) -> Result<Vec<Problem>, Problem> {
    let o = meta.ontology.effective();
    let mut notes = compile_rules(meta.rules.iter(), MAX_GRAPH_COMPILED_BYTES)?.notes(graph);
    notes.extend(
        meta.rules
            .iter()
            .flat_map(|rule| name_notes(&o, rule, graph)),
    );
    Ok(notes)
}

pub(super) fn std_rules<'a>(
    central: &'a [Rule],
    meta: &'a GraphMeta,
) -> impl Iterator<Item = &'a Rule> {
    central
        .iter()
        .filter(|rule| !meta.rules_off.contains(&rule.name))
        .chain(&meta.rules)
}

impl GraphStore {
    pub(super) fn central(&self, meta: &GraphMeta) -> &[Rule] {
        if meta.ontology.std_version().is_some() {
            &self.central_rules
        } else {
            &[]
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn set_central_rules_for_test(&mut self, rules: Vec<Rule>) {
        self.central_rules = rules;
    }

    pub fn rule_list(&self, ctx: &Ctx) -> Outcome<RulesListed> {
        let alias = self.select(ctx, &[])?;
        let meta = &self.graphs[&alias].meta;
        let mut std = self.central(meta).to_vec();
        std.sort_by(|a, b| a.name.cmp(&b.name));
        let mut own = meta.rules.clone();
        own.sort_by(|a, b| a.name.cmp(&b.name));
        let rows = std
            .into_iter()
            .map(|rule| RuleEntry {
                off: meta.rules_off.contains(&rule.name),
                rule,
                source: "std",
            })
            .chain(own.into_iter().map(|rule| RuleEntry {
                rule,
                source: "graph",
                off: false,
            }))
            .collect();
        let mut notes = compile_rules(
            std_rules(self.central(meta), meta),
            MAX_GRAPH_COMPILED_BYTES,
        )?
        .notes(&alias_name(&alias));
        notes.extend(self.stale_note(&alias));
        Ok(Done {
            value: RulesListed { graph: alias, rows },
            notes,
        })
    }

    pub fn rule_show(
        &self,
        ctx: &Ctx,
        name: &str,
        limit: usize,
        offset: usize,
    ) -> Outcome<RuleShown> {
        let alias = self.select(ctx, &[])?;
        let l = &self.graphs[&alias];
        let std = self.central(&l.meta);
        let rule = std
            .iter()
            .chain(&l.meta.rules)
            .find(|r| r.name == name)
            .ok_or_else(|| problems::rule_not_found(name))?;
        let mut compiled = compile_rules(std_rules(std, &l.meta), MAX_GRAPH_COMPILED_BYTES)?;
        let matcher = if l.meta.rules_off.iter().any(|off| off == name) {
            match RuleMatcher::compile(rule, &mut compiled.remaining) {
                Ok(matcher) => Some(matcher),
                Err(CompileError::Limit(reason, _)) => {
                    compiled.skipped.insert(name.into(), reason);
                    None
                }
                Err(CompileError::Invalid(problem)) => return Err(problem),
            }
        } else {
            compiled.matchers.remove(name)
        };
        let mut shown = self.show_rule(&alias, rule, matcher, limit, offset)?;
        shown.notes.extend(compiled.notes(&alias_name(&alias)));
        shown.notes.extend(self.stale_note(&alias));
        Ok(shown)
    }

    fn show_rule(
        &self,
        alias: &str,
        rule: &Rule,
        mut matcher: Option<RuleMatcher>,
        limit: usize,
        offset: usize,
    ) -> Outcome<RuleShown> {
        if !(1..=1000).contains(&limit) {
            return Err(problems::rule_invalid(
                "rule show --limit is between 1 and 1000",
            ));
        }
        let mut nodes = Vec::new();
        let mut not_checked = Vec::new();
        let mut not_checked_total = 0;
        let mut complete = matcher.is_some();
        if let Some(matcher) = &mut matcher {
            let graph = &self.graphs[alias].graph;
            let mut ids: Vec<_> = graph.nodes().map(|node| node.id()).collect();
            ids.sort_unstable();
            for id in ids {
                let node = graph.node(id).expect("collected node");
                if umx_core::ontology::PROVENANCE_TYPES.contains(&node.kind()) {
                    continue;
                }
                match matcher.matches_stored(node) {
                    Some(true) => nodes.push(node.id()),
                    Some(false) => {}
                    None => {
                        complete = false;
                        not_checked_total += 1;
                        if not_checked.len() < MAX_NOT_CHECKED {
                            not_checked.push(node.id());
                        }
                    }
                }
            }
        }
        nodes.sort_unstable();
        let total = nodes.len();
        let end = offset.saturating_add(limit).min(total);
        let rule_name = rule.name.strip_prefix("std:").unwrap_or(&rule.name);
        let command = Command::new("rule show");
        let name = ValidName::new(rule_name).expect("shape checked");
        let command = if rule.name.starts_with("std:") {
            command.qualified("std:", &name)
        } else {
            command.name(&name)
        }
        .word("--graph")
        .name(&alias_name(alias));
        let next = (end < total).then(|| {
            command
                .clone()
                .word("--offset")
                .count(end as u64)
                .word("--limit")
                .count(limit as u64)
        });
        let mut notes = if next.is_some() {
            alloc::vec![problems::rule_matches_cut(next.clone().expect("present"))]
        } else {
            Vec::new()
        };
        if !complete && matcher.is_some() {
            notes.push(problems::rule_scan_cut());
            notes.push(problems::rule_node_unchecked(
                &rule.name,
                not_checked_total,
                command,
            ));
        }
        Ok(Done {
            value: RuleShown {
                graph: alias.into(),
                rule: rule.clone(),
                nodes: nodes.into_iter().skip(offset).take(limit).collect(),
                total: complete.then_some(total),
                not_checked,
                not_checked_total,
                offset,
                limit,
                next,
                unchanged: false,
                dry_run: false,
                format: self.graphs[alias].meta.umx,
            },
            notes,
        })
    }

    pub fn rule_add(&mut self, ctx: &Ctx, value: &Value, dry_run: bool) -> Outcome<RuleShown> {
        let target = value
            .get("to_node")
            .and_then(Value::as_str)
            .map(super::resolve::fold_scheme);
        if target
            .as_deref()
            .is_some_and(|node| node.starts_with("umx:") && node.contains(['#', '?']))
        {
            return Err(problems::rule_invalid(
                "to_node names a node without a selector or fragment",
            ));
        }
        let alias = if dry_run {
            self.select(ctx, &[target.as_deref()])?
        } else {
            self.select_write(ctx, &[target.as_deref()])?
        };
        let mut value = value.clone();
        if let Some(target) = target.as_deref() {
            let id = super::resolve_node_in(self.get(&alias), ctx, &alias, target)?;
            value["to_node"] = Value::String(id.to_text());
        }
        let rule = Rule::from_value(&value).ok_or_else(|| problems::rule_invalid("a rule has only name, match, field or edge with from, to or to_node, and optional description"))?;
        umx_core::rules::stored_ok(core::slice::from_ref(&rule), &[])
            .map_err(problems::rule_invalid)?;
        let before = self.graphs[&alias].meta.clone();
        let ontology = self.graphs[&alias].ontology();
        let notes = name_notes(ontology, &rule, &alias_name(&alias));
        if let Some(existing) = before.rules.iter().find(|r| r.name == rule.name)
            && existing != &rule
        {
            return Err(problems::rule_name_taken(&rule.name));
        }
        let unchanged = before.rules.contains(&rule);
        if !unchanged && before.rules.len() >= MAX_RULES {
            return Err(problems::rule_invalid(
                "a graph holds at most 64 rules of its own",
            ));
        }
        let mut compiled = compile_rules(
            std_rules(self.central(&before), &before)
                .filter(|r| r.name != rule.name)
                .chain(core::iter::once(&rule)),
            MAX_GRAPH_COMPILED_BYTES,
        )?;
        let matcher = compiled
            .matchers
            .remove(&rule.name)
            .ok_or_else(|| problems::rule_regex_invalid(compiled.skipped[&rule.name], 1))?;
        let mut shown = self.show_rule(&alias, &rule, Some(matcher), 100, 0)?;
        if dry_run && !unchanged {
            shown.value.next = None;
            shown.notes.retain(|p| p.code != "rule_matches_cut");
            for note in &mut shown.notes {
                if note.code == "rule_node_unchecked" {
                    *note = problems::rule_node_unchecked(
                        &rule.name,
                        shown.value.not_checked_total,
                        Command::new("docs get").word("rules"),
                    );
                }
            }
        }
        shown.notes.extend(compiled.notes(&alias_name(&alias)));
        shown.notes.extend(notes);
        shown.value.unchanged = unchanged;
        shown.value.dry_run = dry_run;
        if dry_run {
            shown.notes.extend(self.stale_note(&alias));
        }

        if !dry_run && !unchanged {
            let files = self.ports.files.clone();
            let l = self.graphs.get_mut(&alias).expect("selected");
            l.meta.rules.push(rule);
            l.meta.rules.sort_by(|a, b| a.name.cmp(&b.name));
            let notes = persist(
                files.as_ref(),
                &alias,
                l,
                &[GraphFile::Graph],
                Touched::default(),
            )
            .inspect_err(|_| l.meta = before)?;
            shown.notes.extend(notes);
            shown.value.format = l.meta.umx;
        }
        Ok(shown)
    }

    pub fn rule_remove(&mut self, ctx: &Ctx, name: &str) -> Outcome<RuleChanged> {
        let alias = self.select_write(ctx, &[])?;
        if name.starts_with("std:") {
            return Err(problems::rule_invalid(
                "central rules are switched off with rule off, not removed",
            ));
        }
        let before = self.graphs[&alias].meta.clone();
        let unchanged = !before.rules.iter().any(|r| r.name == name);
        let files = self.ports.files.clone();
        let l = self.graphs.get_mut(&alias).expect("selected");
        let notes = if unchanged {
            Vec::new()
        } else {
            l.meta.rules.retain(|r| r.name != name);
            persist(
                files.as_ref(),
                &alias,
                l,
                &[GraphFile::Graph],
                Touched::default(),
            )
            .inspect_err(|_| l.meta = before)?
        };
        Ok(Done {
            value: RuleChanged {
                graph: alias,
                name: name.into(),
                unchanged,
                format: l.meta.umx,
            },
            notes,
        })
    }

    pub fn rule_switch(&mut self, ctx: &Ctx, name: &str, off: bool) -> Outcome<RuleChanged> {
        let alias = self.select_write(ctx, &[])?;
        let before = self.graphs[&alias].meta.clone();
        if !self.central(&before).iter().any(|r| r.name == name) {
            return Err(problems::rule_not_found(name));
        }
        let unchanged = before.rules_off.iter().any(|n| n == name) == off;
        let files = self.ports.files.clone();
        let l = self.graphs.get_mut(&alias).expect("selected");
        let notes = if unchanged {
            Vec::new()
        } else {
            if off {
                l.meta.rules_off.push(name.into());
            } else {
                l.meta.rules_off.retain(|n| n != name);
            }
            l.meta.rules_off.sort();
            persist(
                files.as_ref(),
                &alias,
                l,
                &[GraphFile::Graph],
                Touched::default(),
            )
            .inspect_err(|_| l.meta = before)?
        };
        Ok(Done {
            value: RuleChanged {
                graph: alias,
                name: name.into(),
                unchanged,
                format: l.meta.umx,
            },
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_graph_budget_skips_only_the_rules_that_do_not_fit() {
        let mut first = Rule::from_value(&json!({
            "name":"first", "match":{"regex":{"title":"a{100}"}},"field":"step"
        }))
        .unwrap();
        let mut remaining = MAX_GRAPH_COMPILED_BYTES;
        assert!(RuleMatcher::compile(&first, &mut remaining).is_ok());
        let used = MAX_GRAPH_COMPILED_BYTES - remaining;
        let mut second = first.clone();
        second.name = "second".into();
        let compiled = compile_rules([&first, &second].into_iter(), used).unwrap();
        assert_eq!(compiled.matchers.len(), 1);
        assert!(compiled.skipped.contains_key("second"));
        assert_eq!(compiled.remaining, 0);
        first.pattern.regex.insert("title".into(), "(?".into());
        assert!(compile_rules([&second, &first].into_iter(), 0).is_err());
    }

    #[test]
    fn lazy_dfa_cache_is_bounded_and_unicode_quits_fall_back_to_pikevm() {
        let rule = Rule::from_value(&json!({
            "name":"word", "match":{"regex":{"title":r"\bCafé\b"}},"field":"step"
        }))
        .unwrap();
        let mut matcher = RuleMatcher::new(&rule).unwrap();
        let regex = matcher.regex.get_mut("title").unwrap();
        let (dfa, cache) = regex.dfa.as_mut().expect("bounded DFA fits this pattern");
        assert!(dfa.try_search_fwd(cache, &Input::new("Café")).is_err());
        assert!(cache.memory_usage() <= MAX_DFA_CACHE_BYTES);
        assert!(regex.vm.is_match(&mut regex.cache, "Café"));
        assert!(!regex.vm.is_match(&mut regex.cache, "Caféteria"));
    }
}
