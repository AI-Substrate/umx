//! Soft rule checks against a write's final graph or freshly decoded check input.

use super::rules::{
    CompiledRules, MAX_GRAPH_COMPILED_BYTES, MAX_MATCH_WORK, MAX_NODE_MATCH_WORK, compile_rules,
    std_rules,
};
use super::{GraphStore, alias_name};
use crate::{Done, Outcome, problems};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::problem::Candidate;
use umx_core::rules::Rule;
use umx_core::{Command, Edge, GraphMeta, Id, Ontology, Problem, RuleContext, Slot, ValidName};
use umx_graph::{EdgeRef, Graph, NodeRef};

const PAGE: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleCheckRequest {
    pub rule: Option<String>,
    pub after: Option<Id>,
    pub offset: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleFinding {
    pub rule: String,
    pub node: Id,
    pub state: &'static str,
    pub sentence: String,
    pub next: Command,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulesChecked {
    pub graph: String,
    pub rows: Option<Vec<RuleFinding>>,
    pub total: Option<usize>,
    pub unmet_nodes: usize,
    pub not_checked: usize,
    pub cut: bool,
    pub after: Option<Id>,
    pub next: Option<Command>,
    pub resume: Option<Command>,
}

pub(super) struct RuleChecks {
    rules: BTreeMap<String, Rule>,
    compiled: CompiledRules,
}

struct Counted {
    count: usize,
    nodes: Vec<Id>,
}

impl Counted {
    fn add(&mut self, node: Id) {
        self.count += 1;
        if self.nodes.len() < PAGE {
            self.nodes.push(node);
        }
    }
}

struct Scan {
    report: RulesChecked,
    unmet: BTreeMap<String, Counted>,
    unchecked: BTreeMap<String, Counted>,
}

pub(super) fn check_command(graph: &ValidName, rule: Option<&str>) -> Command {
    let command = Command::new("check").word("--rules");
    let command = match rule {
        Some(rule) => {
            let name = ValidName::new(rule.strip_prefix("std:").unwrap_or(rule))
                .expect("checked rule name");
            let command = command.word("--rule");
            if rule.starts_with("std:") {
                command.qualified("std:", &name)
            } else {
                command.name(&name)
            }
        }
        None => command,
    };
    command.word("--graph").name(graph)
}

pub(super) fn fix(rule: &Rule, node: Id, graph: &ValidName) -> Command {
    let name = |name: &str| ValidName::new(name).expect("checked rule part");
    let command = if let Some(field) = &rule.field {
        Command::new("node edit")
            .id(node)
            .word("--set")
            .name_with(&name(field), '=', "<value>")
    } else {
        let edge = name(rule.edge.as_deref().expect("checked shape"));
        let command = Command::new("edge add");
        if rule.from.is_some() {
            command.word("<node>").name(&edge).id(node)
        } else if let Some(target) = rule.to_node {
            command.id(node).name(&edge).id(target)
        } else {
            command.id(node).name(&edge).word("<node>")
        }
    };
    command.word("--graph").name(graph)
}

fn met(rule: &Rule, node: NodeRef<'_>, graph: &Graph, ontology: &Ontology) -> bool {
    if let Some(field) = &rule.field {
        return node.field(field).is_some_and(|value| !value.is_null());
    }
    let relation = rule.edge.as_deref().expect("checked shape");
    let symmetric = ontology.is_symmetric(relation);
    let incoming = rule.from.is_some();
    let types = rule.from.as_ref().or(rule.to.as_ref());
    let local = |other: umx_graph::NodeIx| {
        let other = graph.node_at(other);
        rule.to_node.map_or_else(
            || types.is_some_and(|types| types.iter().any(|kind| kind == other.kind())),
            |id| id == other.id(),
        )
    };
    let ix = node.ix();
    ((!incoming || symmetric)
        && (graph
            .edges_out(ix)
            .any(|step| step.kind.as_str() == relation && local(step.other))
            || (rule.to_node.is_none()
                && graph
                    .edges_out_external(ix)
                    .any(|(_, kind, _, _)| kind.as_str() == relation))))
        || ((incoming || symmetric)
            && (graph
                .edges_in(ix)
                .any(|step| step.kind.as_str() == relation && local(step.other))
                || (rule.to_node.is_none()
                    && graph
                        .edges_in_external(ix)
                        .any(|(_, kind, _, _)| kind.as_str() == relation))))
}

impl RuleChecks {
    pub(super) fn batch_edge_ids(
        &self,
        graph: &Graph,
        ids: &BTreeSet<Id>,
        pending: impl Iterator<Item = Id>,
    ) -> BTreeSet<Id> {
        if ids.is_empty() || self.rules.values().all(|rule| rule.edge.is_none()) {
            return BTreeSet::new();
        }
        let mut edges = BTreeSet::new();
        for id in ids {
            let Some(node) = graph.node(*id) else {
                continue;
            };
            let ix = node.ix();
            edges.extend(
                graph
                    .edges_out(ix)
                    .chain(graph.edges_in(ix))
                    .map(|step| graph.edge_at(step.edge).id())
                    .chain(
                        graph
                            .edges_out_external(ix)
                            .chain(graph.edges_in_external(ix))
                            .map(|(edge, _, _, _)| graph.edge_at(edge).id()),
                    ),
            );
        }
        edges.extend(pending);
        edges
    }

    fn batch_edge_matches<'a>(
        &self,
        graph: &Graph,
        ontology: &Ontology,
        ids: &BTreeSet<Id>,
        edges: impl Iterator<Item = EdgeRef<'a>>,
    ) -> BTreeMap<String, BTreeSet<Id>> {
        let mut by_relation = BTreeMap::<&str, Vec<(&str, &Rule)>>::new();
        for (name, rule) in &self.rules {
            if let Some(relation) = rule.edge.as_deref() {
                by_relation.entry(relation).or_default().push((name, rule));
            }
        }
        if by_relation.is_empty() || ids.is_empty() {
            return BTreeMap::new();
        }
        let mut matched = BTreeMap::<String, BTreeSet<Id>>::new();
        for edge in edges {
            let Some(rules) = by_relation.get(edge.kind()) else {
                continue;
            };
            for &(name, rule) in rules {
                let symmetric = ontology.is_symmetric(rule.edge.as_deref().expect("edge rule"));
                let incoming = rule.from.is_some();
                let far_matches = |id: Id, external: bool| {
                    if let Some(target) = rule.to_node {
                        !external && id == target
                    } else if external {
                        true
                    } else {
                        let types = rule.from.as_ref().or(rule.to.as_ref()).expect("edge shape");
                        graph
                            .node(id)
                            .is_some_and(|node| types.iter().any(|kind| kind == node.kind()))
                    }
                };
                for id in [
                    (edge.from_graph().is_none()
                        && (!incoming || symmetric)
                        && far_matches(edge.to(), edge.to_graph().is_some()))
                    .then_some(edge.from()),
                    (edge.to_graph().is_none()
                        && (incoming || symmetric)
                        && far_matches(edge.from(), edge.from_graph().is_some()))
                    .then_some(edge.to()),
                ]
                .into_iter()
                .flatten()
                .filter(|id| ids.contains(id))
                {
                    matched.entry(name.to_string()).or_default().insert(id);
                }
            }
        }
        matched
    }

    pub(super) fn new(
        central: &[Rule],
        meta: &GraphMeta,
        only: Option<&str>,
    ) -> Result<Self, Problem> {
        let mut compiled = compile_rules(std_rules(central, meta), MAX_GRAPH_COMPILED_BYTES)?;
        let rules: BTreeMap<_, _> = std_rules(central, meta)
            .filter(|rule| only.is_none_or(|name| rule.name == name))
            .map(|rule| (rule.name.clone(), rule.clone()))
            .collect();
        if let Some(name) = only
            && rules.is_empty()
        {
            return Err(problems::rule_not_found(name));
        }
        compiled.matchers.retain(|name, _| rules.contains_key(name));
        Ok(Self { rules, compiled })
    }

    fn scan(
        &mut self,
        graph: &Graph,
        ontology: &Ontology,
        alias: &ValidName,
        ids: impl Iterator<Item = Id>,
        request: Option<&RuleCheckRequest>,
        batch_edge_ids: Option<&BTreeSet<Id>>,
    ) -> Scan {
        let mut rows = Vec::new();
        let mut total = 0;
        let mut unmet_nodes = 0;
        let mut not_checked = 0;
        let mut cut = false;
        let mut last = request.and_then(|request| request.after);
        let mut unmet = BTreeMap::<String, Counted>::new();
        let mut unchecked = BTreeMap::<String, Counted>::new();
        let mut work_left = MAX_MATCH_WORK;
        // Reserve enough for every rule on one node, so a resume point never skips a partial node.
        let reserve = MAX_NODE_MATCH_WORK * self.compiled.matchers.len() as u64;
        let ids: BTreeSet<_> = if self.compiled.matchers.is_empty() {
            BTreeSet::new()
        } else {
            ids.collect()
        };
        // A prepared batch's rows are final, but its adjacency is rebuilt only on commit.
        let batch_edges = batch_edge_ids.map(|edges| {
            self.batch_edge_matches(
                graph,
                ontology,
                &ids,
                edges.iter().filter_map(|id| graph.edge(*id)),
            )
        });
        for id in ids {
            if request
                .and_then(|request| request.after)
                .is_some_and(|after| id <= after)
            {
                continue;
            }
            let Some(node) = graph.node(id) else { continue };
            if umx_core::ontology::PROVENANCE_TYPES.contains(&node.kind()) {
                continue;
            }
            if work_left < reserve {
                cut = true;
                break;
            }
            let mut node_unmet = false;
            for (name, matcher) in &mut self.compiled.matchers {
                matcher.work_left = work_left;
                let matched = matcher.matches_stored(node);
                work_left = matcher.work_left;
                let rule = &self.rules[name];
                let state = match matched {
                    Some(false) => continue,
                    Some(true)
                        if batch_edges
                            .as_ref()
                            .filter(|_| rule.edge.is_some())
                            .map_or_else(
                                || met(rule, node, graph, ontology),
                                |matches| {
                                    matches.get(name).is_some_and(|nodes| nodes.contains(&id))
                                },
                            ) =>
                    {
                        continue;
                    }
                    Some(true) => {
                        node_unmet = true;
                        unmet
                            .entry(name.clone())
                            .or_insert_with(|| Counted {
                                count: 0,
                                nodes: Vec::new(),
                            })
                            .add(id);
                        "unmet"
                    }
                    None => {
                        not_checked += 1;
                        unchecked
                            .entry(name.clone())
                            .or_insert_with(|| Counted {
                                count: 0,
                                nodes: Vec::new(),
                            })
                            .add(id);
                        "not_checked"
                    }
                };
                let offset = request.map_or(0, |request| request.offset);
                if request.is_some() && total >= offset && rows.len() < PAGE {
                    rows.push(RuleFinding {
                        rule: name.clone(),
                        node: id,
                        state,
                        sentence: rule.sentence().as_str().to_string(),
                        next: if state == "unmet" {
                            fix(rule, id, alias)
                        } else {
                            check_command(alias, Some(name))
                        },
                    });
                }
                total += 1;
            }
            unmet_nodes += usize::from(node_unmet);
            last = Some(id);
        }
        let command = check_command(alias, request.and_then(|request| request.rule.as_deref()));
        let resume = cut.then(|| {
            command
                .clone()
                .word("--after")
                .id(last.expect("one full node fits"))
        });
        let offset = request.map_or(0, |request| request.offset);
        let next = if request.is_none() {
            Some(command)
        } else if offset.saturating_add(rows.len()) < total {
            let mut command = command;
            if let Some(after) = request.and_then(|request| request.after) {
                command = command.word("--after").id(after);
            }
            Some(command.word("--offset").count((offset + rows.len()) as u64))
        } else if cut {
            resume.clone()
        } else {
            None
        };
        Scan {
            report: RulesChecked {
                graph: alias.as_str().to_string(),
                rows: request.map(|_| rows),
                total: (!cut
                    && not_checked == 0
                    && self.compiled.matchers.len() == self.rules.len())
                .then_some(total),
                unmet_nodes,
                not_checked,
                cut,
                after: last,
                next,
                resume,
            },
            unmet,
            unchecked,
        }
    }

    fn notes(&self, scan: &Scan, graph: &ValidName) -> Vec<Problem> {
        let mut notes = self.compiled.notes(graph);
        notes.extend(scan.unchecked.iter().map(|(name, counted)| {
            let mut problem = problems::rule_node_unchecked(
                name,
                counted.count,
                check_command(graph, Some(name)),
            );
            if counted.count == 1 {
                problem.subject = Some(counted.nodes[0].to_text());
            }
            problem.candidates = counted
                .nodes
                .iter()
                .map(|id| Candidate {
                    id: id.to_text(),
                    title: None,
                    score: None,
                })
                .collect();
            problem
        }));
        if scan.report.cut {
            let fix = scan.report.resume.clone().expect("cut has resume");
            notes.push(problems::rule_check_cut(fix));
        }
        notes
    }

    pub(super) fn write_notes(
        &mut self,
        graph: &Graph,
        ontology: &Ontology,
        alias: &ValidName,
        ids: impl Iterator<Item = Id>,
        batch_edge_ids: Option<&BTreeSet<Id>>,
    ) -> Vec<Problem> {
        let ids: BTreeSet<_> = ids.collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let batch = batch_edge_ids.is_some();
        let scan = self.scan(
            graph,
            ontology,
            alias,
            ids.into_iter(),
            None,
            batch_edge_ids,
        );
        let mut notes = self.notes(&scan, alias);
        let warning = |rule: &Rule, nodes: &[Id], count: usize, batch: bool| {
            let next = if batch {
                check_command(alias, Some(&rule.name))
            } else {
                fix(rule, nodes[0], alias)
            };
            let mut problem = Problem::new("rule_unmet")
                .with("sentence", Slot::Sentence(rule.sentence()))
                .with("fix", Slot::Command(next));
            problem.rule = Some(Box::new(RuleContext {
                name: rule.name.clone(),
                count: count as u64,
                nodes: nodes.iter().map(|id| id.to_text()).collect(),
            }));
            if !batch {
                problem.subject = Some(nodes[0].to_text());
            }
            problem
        };
        for (name, counted) in scan.unmet {
            let rule = &self.rules[&name];
            if batch {
                notes.push(warning(rule, &counted.nodes, counted.count, true));
            } else {
                notes.extend(
                    counted
                        .nodes
                        .iter()
                        .map(|id| warning(rule, &[*id], 1, false)),
                );
            }
        }
        notes
    }
}

impl GraphStore {
    pub(super) fn rule_checks(&self, meta: &GraphMeta) -> RuleChecks {
        RuleChecks::new(self.central(meta), meta, None)
            .expect("rules validated at load, add or in the central table")
    }

    pub(crate) fn rule_write_notes(&self, alias: &str, ids: &[Id]) -> Vec<Problem> {
        let loaded = self.get(alias);
        self.rule_checks(&loaded.meta).write_notes(
            &loaded.graph,
            loaded.ontology(),
            &alias_name(alias),
            ids.iter().copied(),
            None,
        )
    }

    pub(crate) fn rule_edge_notes(&self, alias: &str, edge: &Edge) -> Vec<Problem> {
        let ids: Vec<_> = [
            edge.from_graph.is_none().then_some(edge.from),
            edge.to_graph.is_none().then_some(edge.to),
        ]
        .into_iter()
        .flatten()
        .collect();
        self.rule_write_notes(alias, &ids)
    }

    pub(super) fn check_rules(
        &self,
        alias: &str,
        graph: &Graph,
        meta: &GraphMeta,
        request: Option<&RuleCheckRequest>,
    ) -> Outcome<RulesChecked> {
        let mut checks = RuleChecks::new(
            self.central(meta),
            meta,
            request.and_then(|request| request.rule.as_deref()),
        )?;
        let alias = alias_name(alias);
        let scan = checks.scan(
            graph,
            &meta.ontology.effective(),
            &alias,
            graph.nodes().map(|node| node.id()),
            request,
            None,
        );
        let notes = checks.notes(&scan, &alias);
        Ok(Done {
            value: scan.report,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn batch_rules_visit_only_incident_and_pending_final_edges() {
        let node = |id, kind| {
            serde_json::from_value(json!({
                "id":Id(id),"type":kind,"title":"Node","status":"open","note":"",
                "evidence":[],"status_history":[],"created":"","updated":"","created_by":""
            }))
            .unwrap()
        };
        let edge = |id, from, to| -> Edge {
            serde_json::from_value(json!({
                "id":Id(id),"type":"owns","from":Id(from),"to":Id(to),
                "created":"","created_by":""
            }))
            .unwrap()
        };
        let mut nodes = alloc::vec![
            node(1, "person"),
            node(2, "risk"),
            node(3, "person"),
            node(4, "risk")
        ];
        nodes.extend((10..1010).map(|id| node(id, "person")));
        let mut edges: Vec<_> = (10..1010).map(|id| edge(9000 + id, id, 4)).collect();
        edges.extend([edge(8000, 1, 2), edge(8001, 2, 1)]);
        let mut incoming = edge(8002, 3, 2);
        incoming.from_graph = Some(Id(50000));
        let mut outgoing = edge(8003, 2, 4);
        outgoing.to_graph = Some(Id(50000));
        edges.extend([incoming, outgoing]);
        let mut graph = Graph::from_records(nodes, edges);
        let rule = Rule::from_value(&json!({
            "name":"owned","match":{"type":["risk"]},"edge":"owns","from":["person"]
        }))
        .unwrap();
        rule.check().unwrap();
        let mut checks = RuleChecks {
            compiled: compile_rules(core::iter::once(&rule), MAX_GRAPH_COMPILED_BYTES).unwrap(),
            rules: BTreeMap::from([(rule.name.clone(), rule)]),
        };
        let ontology = Ontology::default();
        let ids = BTreeSet::from([Id(2)]);
        let mut batch = graph.batch();
        batch.remove_edge(Id(8000));
        batch.remove_edge(Id(8002));
        batch.insert_edge(edge(7000, 3, 2));
        let candidates = checks.batch_edge_ids(batch.graph(), &ids, core::iter::once(Id(7000)));
        assert_eq!(
            candidates,
            BTreeSet::from([Id(7000), Id(8000), Id(8001), Id(8002), Id(8003)])
        );
        batch.prepare().unwrap();
        let mut visited = 0;
        let matches = checks.batch_edge_matches(
            batch.graph(),
            &ontology,
            &ids,
            candidates
                .iter()
                .filter_map(|id| batch.graph().edge(*id))
                .inspect(|_| visited += 1),
        );
        assert_eq!(visited, 3, "the 1000 unrelated edges are not visited");
        assert_eq!(matches["owned"], ids);
        let notes = checks.write_notes(
            batch.graph(),
            &ontology,
            &alias_name("demo"),
            ids.iter().copied(),
            Some(&candidates),
        );
        assert!(notes.is_empty(), "the pending owner counts after prepare");
        batch.remove_edge(Id(7000));
        batch.prepare().unwrap();
        let notes = checks.write_notes(
            batch.graph(),
            &ontology,
            &alias_name("demo"),
            ids.iter().copied(),
            Some(&candidates),
        );
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].code, "rule_unmet");
        assert_eq!(notes[0].rule.as_ref().unwrap().nodes, [Id(2).to_text()]);
        batch.rollback();
    }
}
