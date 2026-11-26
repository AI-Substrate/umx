//! graph guide: what a graph is for and what its words mean (plan 079), for an agent new to it.

use super::{Ctx, Done, GraphStore, Outcome, alias_name, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use umx_core::ontology::Described;
use umx_core::standard;
use umx_core::{Command, Id, ValidName};
use umx_graph::{Graph, Tally};

/// One record that uses a name: a node, or an edge read as a sentence. `title_flagged` and `sentence_flagged` say the
/// title, or a local end's title in the sentence, is one umx's secret check flags (F234, the rule of a read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuideExample {
    Node {
        id: Id,
        title: String,
        title_flagged: bool,
    },
    /// `from_graph` and `to_graph` are graph ids, the guide's own graph for a local end (as edge list rows).
    Edge {
        id: Id,
        from_graph: Id,
        from: Id,
        to_graph: Id,
        to: Id,
        sentence: String,
        sentence_flagged: bool,
    },
}

/// A type, status or relation: declared or only in use, what it means, how many records use it and the lowest-id
/// one. `next` adds the missing description (or declares the name with one), None once it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideEntry {
    pub name: String,
    pub declared: bool,
    pub description: Option<String>,
    pub count: usize,
    pub example: Option<GuideExample>,
    pub next: Option<Command>,
    /// Relations only: `a r b` means `b r a` (D49).
    pub symmetric: bool,
    /// std gives the name (req-98): a std graph, and a std type, status or relation.
    pub std: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideField {
    pub name: String,
    pub kind: &'static str,
    pub values: Vec<String>,
    pub description: Option<String>,
    pub next: Option<Command>,
    /// std places the field on the type (req-98).
    pub std: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideType {
    pub entry: GuideEntry,
    pub fields: Vec<GuideField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guide {
    pub alias: String,
    pub title: String,
    pub purpose: Option<String>,
    /// `graph set --purpose`, None once there is one.
    pub purpose_next: Option<Command>,
    pub conventions: Option<String>,
    /// `graph set --conventions`, None once there are some.
    pub conventions_next: Option<Command>,
    pub nodes: usize,
    pub edges: usize,
    pub types: Vec<GuideType>,
    pub statuses: Vec<GuideEntry>,
    pub relations: Vec<GuideEntry>,
    /// The best spine `graph spine propose` offers, with its cover (B4); a hint, never a warning.
    pub spine: super::SpineLine,
}

/// A name, whether the ontology declares it, and its tally when records use it.
type Named<'a> = (&'a str, bool, Option<&'a Tally<'a>>);

/// The declared names in declaration order, then those only in use in name order, each with its tally.
fn names<'a>(declared: impl Iterator<Item = &'a str>, used: &'a [Tally<'a>]) -> Vec<Named<'a>> {
    let by_name: BTreeMap<&str, &Tally<'_>> = used.iter().map(|t| (t.name, t)).collect();
    let mut seen = BTreeSet::new();
    let mut out: Vec<Named<'_>> = Vec::new();
    for name in declared {
        if seen.insert(name) {
            out.push((name, true, by_name.get(name).copied()));
        }
    }
    for t in used {
        if seen.insert(t.name) {
            out.push((t.name, false, Some(t)));
        }
    }
    out
}

/// The command that gives a name its description: `graph set --description` once declared, else the `ontology add`
/// that declares it with one. None for a name that is not a valid name (a hand edit): there is nothing to run.
fn describe_next(
    graph: &ValidName,
    kind: Described,
    name: &str,
    declared: bool,
) -> Option<Command> {
    let name = ValidName::new(name)?;
    Some(if declared {
        Command::new("graph set")
            .name(graph)
            .word("--description")
            .described(kind, &name, None)
    } else {
        let verb = match kind {
            Described::Type => "ontology add type",
            Described::Status => "ontology add status",
            _ => "ontology add relation",
        };
        Command::new(verb)
            .name(&name)
            .word("--description")
            .word("<text>")
            .word("--graph")
            .name(graph)
    })
}

fn set(graph: &ValidName, flag: &'static str) -> Command {
    Command::new("graph set")
        .name(graph)
        .word(flag)
        .word("<text>")
}

/// Whether node `node`'s title `title` in the guide's graph is flagged.
type TitleFlag<'f> = &'f dyn Fn(Id, &str) -> bool;

fn node_example(g: &Graph, t: Option<&Tally<'_>>, flag: TitleFlag<'_>) -> Option<GuideExample> {
    let n = g.node(t?.first)?;
    Some(GuideExample::Node {
        id: n.id(),
        title: n.title().to_string(),
        title_flagged: flag(n.id(), n.title()),
    })
}

/// An edge read as a sentence. A local end reads as its title; an end in another graph reads as its qualified
/// address `umx:<graph>/<node>` and is never looked up here (review B3): its id names a node of that graph, not of
/// this one, and the guide loads nothing and reads no other graph's text.
fn edge_example(
    g: &Graph,
    own: Id,
    t: Option<&Tally<'_>>,
    flag: TitleFlag<'_>,
) -> Option<GuideExample> {
    let e = g.edge(t?.first)?;
    let end = |graph: Option<Id>, node: Id| match graph.filter(|x| *x != own) {
        Some(other) => (other, format!("umx:{other}/{node}"), false),
        None => match g.node(node) {
            Some(n) => (own, n.title().to_string(), flag(node, n.title())),
            None => (own, node.to_text(), false),
        },
    };
    let (from_graph, from, from_flagged) = end(e.from_graph(), e.from());
    let (to_graph, to, to_flagged) = end(e.to_graph(), e.to());
    Some(GuideExample::Edge {
        id: e.id(),
        from_graph,
        from: e.from(),
        to_graph,
        to: e.to(),
        sentence: format!("{from} {} {to}", e.kind().replace('_', " ")),
        sentence_flagged: from_flagged || to_flagged,
    })
}

impl GraphStore {
    /// One read of a loaded graph: graph.json's guide keys and one census of the graph (O(nodes + edges), never a
    /// pass per name). A graph with nothing described is not a failure: `purpose_missing` and
    /// `descriptions_missing` are warnings, and each entry's `next` fills it in.
    ///
    /// A std graph's guide reads its effective ontology (req-98) but lists a std entry only when graph.json declares
    /// it or a record uses it, marked `std`, with the hint `std_entries_shown`. A std entry graph.json does not
    /// declare has no `next` and is never counted in `descriptions_missing`: `graph set --description` takes only a
    /// declared type or field.
    pub fn graph_guide(&self, ctx: &Ctx, alias: &str) -> Outcome<Guide> {
        let Some(l) = self.graphs.get(alias) else {
            return Err(problems::graph_not_loaded(alias));
        };
        l.last_used
            .store(self.ports.clock.now_ms(), Ordering::Relaxed);
        let graph = alias_name(alias);
        let o = l.ontology();
        let stored = &l.meta.ontology;
        let is_std = o.std_version().is_some();
        let std_name = |kind: Described, name: &str| {
            is_std
                && match kind {
                    Described::Type => standard::std_type(name).is_some(),
                    Described::Status => standard::std_status(name).is_some(),
                    _ => standard::std_relation(name).is_some(),
                }
        };
        // Only std has it: graph.json does not declare it.
        let std_only = |kind: Described, name: &str| {
            std_name(kind, name)
                && match kind {
                    Described::Type => stored.type_decl(name).is_none(),
                    Described::Status => !stored.has_status(name),
                    _ => !stored.has_relation(name),
                }
        };
        // A std entry graph.json does not declare and no record uses is left out.
        let listed = |kind: Described, (name, _, tally): &Named<'_>| {
            tally.is_some() || !std_only(kind, name)
        };
        let g = &l.graph;
        let read_ctx = Ctx {
            graph: Some(alias.into()),
            ..ctx.clone()
        };
        let scope = read_ctx.read_scope(&l.graph);
        let census = self.read_census(&read_ctx, alias);
        let nodes = scope.node_count();
        let mut edges = census.relations.iter().map(|tally| tally.count).sum();
        let allows = core::cell::OnceCell::new();
        let flag = |node: Id, title: &str| self.title_flagged(&allows, l.meta.id, node, title);
        let mut missing: Vec<Command> = Vec::new();
        let mut entry = |kind: Described,
                         name: &str,
                         declared: bool,
                         description: Option<&String>,
                         tally: Option<&Tally<'_>>,
                         example: Option<GuideExample>| {
            let next = match description {
                Some(_) => None,
                None if std_only(kind, name) => None,
                None => describe_next(&graph, kind, name, declared),
            };
            if let Some(c) = &next {
                missing.push(c.clone());
            }
            GuideEntry {
                name: name.to_string(),
                declared,
                description: description.cloned(),
                count: tally.map_or(0, |t| t.count),
                example,
                next,
                symmetric: kind == Described::Relation && o.is_symmetric(name),
                std: std_name(kind, name),
            }
        };
        let mut types = Vec::new();
        for (name, declared, tally) in names(o.type_names(), &census.types)
            .into_iter()
            .filter(|n| listed(Described::Type, n))
        {
            let decl = o.type_decl(name);
            let e = entry(
                Described::Type,
                name,
                declared,
                decl.and_then(|t| t.description.as_ref()),
                tally,
                node_example(g, tally, &flag),
            );
            types.push((e, decl));
        }
        let statuses: Vec<GuideEntry> =
            names(o.statuses.iter().map(String::as_str), &census.statuses)
                .into_iter()
                .filter(|n| listed(Described::Status, n))
                .map(|(name, declared, tally)| {
                    entry(
                        Described::Status,
                        name,
                        declared,
                        o.status_descriptions.get(name),
                        tally,
                        node_example(g, tally, &flag),
                    )
                })
                .collect();
        let mut relations: Vec<GuideEntry> =
            names(o.relations.iter().map(String::as_str), &census.relations)
                .into_iter()
                .filter(|n| listed(Described::Relation, n))
                .filter(|(name, _, _)| {
                    !ctx.include_provenance || !matches!(*name, "authored_by" | "edited_by")
                })
                .map(|(name, declared, tally)| {
                    entry(
                        Described::Relation,
                        name,
                        declared,
                        o.relation_descriptions.get(name),
                        tally,
                        edge_example(g, l.meta.id, tally, &flag),
                    )
                })
                .collect();
        if ctx.include_provenance {
            let mut counts = BTreeMap::new();
            for edge in g.provenance_edges() {
                *counts.entry(edge.kind).or_insert(0) += 1;
                edges += 1;
            }
            relations.extend(counts.into_iter().map(|(name, count)| GuideEntry {
                name: name.into(),
                declared: true,
                description: Some("Read-only node attribution to its session".into()),
                count,
                example: None,
                next: None,
                symmetric: false,
                std: is_std && standard::std_relation(name).is_some(),
            }));
        }
        let types: Vec<GuideType> = types
            .into_iter()
            .map(|(entry, decl)| {
                let fields = decl.map_or_else(Vec::new, |t| {
                    t.fields
                        .iter()
                        .map(|f| {
                            let next = match (
                                &f.description,
                                ValidName::new(&t.name),
                                ValidName::new(&f.name),
                            ) {
                                // Only a field graph.json declares takes a description.
                                (None, Some(tn), Some(fname))
                                    if stored.field_decl(&t.name, &f.name).is_some() =>
                                {
                                    Some(
                                        Command::new("graph set")
                                            .name(&graph)
                                            .word("--description")
                                            .described(Described::Field, &tn, Some(&fname)),
                                    )
                                }
                                _ => None,
                            };
                            if let Some(c) = &next {
                                missing.push(c.clone());
                            }
                            GuideField {
                                name: f.name.clone(),
                                kind: f.kind.as_str(),
                                values: f.values.clone(),
                                description: f.description.clone(),
                                next,
                                std: is_std
                                    && standard::std_field(&f.name)
                                        .is_some_and(|s| s.types.contains(&t.name.as_str())),
                            }
                        })
                        .collect()
                });
                GuideType { entry, fields }
            })
            .collect();
        let mut warnings = Vec::new();
        if o.purpose.is_none() {
            warnings.push(problems::purpose_missing(&graph));
        }
        if let Some(first) = missing.first() {
            warnings.push(problems::descriptions_missing(
                &graph,
                missing.len(),
                first.clone(),
            ));
        }
        if let Some(std) = o.std_version() {
            warnings.push(problems::std_entries_shown(&graph, std));
        }
        warnings.extend(self.role_hints(alias, None));
        Ok(Done {
            value: Guide {
                alias: graph.as_str().to_string(),
                title: l.meta.title.clone(),
                purpose: o.purpose.clone(),
                purpose_next: o.purpose.is_none().then(|| set(&graph, "--purpose")),
                conventions: o.conventions.clone(),
                conventions_next: o
                    .conventions
                    .is_none()
                    .then(|| set(&graph, "--conventions")),
                nodes,
                edges,
                types,
                statuses,
                relations,
                spine: super::spine::spine_line(g, o, &graph),
            },
            notes: warnings,
        })
    }
}

impl GraphStore {
    /// search-05: one hint per declared `string` field of `alias` whose values say it is in the wrong role group
    /// ([`umx_core::role_advice`]): labels declared content, prose declared data. Each hint carries counts only and
    /// names the `umx ontology set field` command; nothing is changed here. `only` keeps one `(type, field)`. One
    /// pass over the nodes, reading only the candidate fields; nothing when the graph is not loaded.
    pub fn role_hints(&self, alias: &str, only: Option<(&str, &str)>) -> Vec<umx_core::Problem> {
        let Some(l) = self.graphs.get(alias) else {
            return Vec::new();
        };
        let mut wanted: BTreeMap<&str, Vec<&umx_core::FieldDecl>> = BTreeMap::new();
        for t in &l.ontology().types {
            for f in &t.fields {
                let kept = only.is_none_or(|(ty, name)| ty == t.name && name == f.name);
                if kept && f.kind == umx_core::FieldKind::String {
                    wanted.entry(t.name.as_str()).or_default().push(f);
                }
            }
        }
        if wanted.is_empty() {
            return Vec::new();
        }
        let mut values: BTreeMap<(&str, &str), Vec<String>> = BTreeMap::new();
        for node in l.graph.nodes() {
            let Some((type_name, fields)) = wanted.get_key_value(node.kind()) else {
                continue;
            };
            for f in fields {
                if let Some(umx_core::Value::String(s)) = node.field(&f.name) {
                    values
                        .entry((type_name, f.name.as_str()))
                        .or_default()
                        .push(s);
                }
            }
        }
        let graph = alias_name(alias);
        let mut hints = Vec::new();
        for (type_name, fields) in &wanted {
            for f in fields {
                let held = values.get(&(*type_name, f.name.as_str()));
                let advice = umx_core::role_advice::role_advice(
                    f,
                    held.into_iter().flatten().map(String::as_str),
                );
                let (Some(advice), Some(t), Some(name)) =
                    (advice, ValidName::new(type_name), ValidName::new(&f.name))
                else {
                    continue;
                };
                hints.push(match advice {
                    umx_core::role_advice::RoleAdvice::BecomeData { values, distinct } => {
                        problems::field_looks_like_labels(&graph, &t, &name, values, distinct)
                    }
                    umx_core::role_advice::RoleAdvice::BecomeContent { values, long } => {
                        problems::field_looks_like_prose(&graph, &t, &name, values, long)
                    }
                });
            }
        }
        hints
    }

    /// PRD 002 search-04: `field_data_by_default` for a `string` field of `type_name` declared with no role (so data:
    /// no vector, left out of the whole-node text), naming the command that makes it content. None for a stated
    /// role, another kind or a field umx never embeds.
    pub fn data_default_hint(
        &self,
        alias: &str,
        type_name: &str,
        field: &str,
    ) -> Option<umx_core::Problem> {
        let decl = self
            .graphs
            .get(alias)?
            .meta
            .ontology
            .field_decl(type_name, field)?;
        let defaulted = decl.role.is_none()
            && decl.kind.may_be_content()
            && !umx_core::ontology::never_embedded_field(field);
        if !defaulted {
            return None;
        }
        Some(problems::field_data_by_default(
            &alias_name(alias),
            &ValidName::new(type_name)?,
            &ValidName::new(field)?,
        ))
    }
}
