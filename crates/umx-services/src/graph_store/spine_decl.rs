//! Declared spines (ws008 TM5, req-149; time review R2, brief 5.2 and step 9): `umx graph spine set`, `confirm`,
//! `show` and `remove`. A declaration is stored in graph.json's `spines` (format [`FORMAT_SPINES`], raised only when
//! one is declared) through the checked graph.json write, and `show` runs the one placement rule ([`place`]) on it.
//! Every name in a declaration is the graph's own vocabulary or std (T1); a graph has at most one `main` spine.
//!
//! [`FORMAT_SPINES`]: umx_core::model::FORMAT_SPINES

use super::spine::{Facts, How, Unplaced, WhyNot, anchor_candidate, propose};
use super::{Ctx, Done, GraphStore, Outcome, alias_name, persist, problems};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use umx_core::codec::GraphFile;
use umx_core::ontology::Ontology;
use umx_core::problem::Candidate;
use umx_core::spine::{BadName, Clock, Cover, MAX_SPINES, PLACE_EVIDENCE, SpineDecl, Spread};
use umx_core::standard::{std_field, std_relation, std_type};
use umx_core::ui::Touched;
use umx_core::{Command, Problem, ValidName, Value, settings};

/// What `set` or `confirm` did: the declaration as stored, and whether it replaced one of the same name or matched
/// it already (nothing written).
#[derive(Clone, Debug, PartialEq)]
pub struct SpineSet {
    pub alias: String,
    pub spine: SpineDecl,
    pub replaced: bool,
    pub unchanged: bool,
    /// The graph.json format after the write.
    pub format: u32,
}

/// One declared spine as `show` projects it: placed by [`super::spine::place`], judged as propose judges a
/// candidate.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclaredSpine {
    pub decl: SpineDecl,
    pub clock: Clock,
    pub cover: Cover,
    pub spread: Spread,
    pub offered: bool,
    pub why_not: Option<WhyNot>,
    /// The command that lists the nodes it places only by rule (B4a); None when it places none so.
    pub by_rule_next: Option<Command>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpinesShown {
    pub alias: String,
    pub total: usize,
    pub spines: Vec<DeclaredSpine>,
    /// When the graph declares none: the command that proposes some.
    pub next: Option<Command>,
    /// `show --by-rule <name>`: one page of the ids that spine places only by rule (`candidate` is the spine name).
    pub by_rule: Option<Unplaced>,
    /// `show --unplaced <name>` (D71 step 12): one page of the ids that spine does not place.
    pub unplaced: Option<Unplaced>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpineRemoved {
    pub alias: String,
    pub name: String,
}

/// The shape checks a stored declaration also passes ([`umx_core::spine::stored_ok`]): names, caps (security N1)
/// and the title, kept exactly as given (a confirmed one must stay byte for byte).
fn check_shape(decl: &SpineDecl) -> Result<(), Problem> {
    decl.check().map_err(bad_name)?;
    if !decl.within_caps() {
        return Err(problems::usage_invalid(
            "a spine declaration has at most 16 order entries and 16 place entries",
            "graph",
        ));
    }
    if !decl.title_ok() {
        return Err(problems::usage_invalid(
            "a spine title is one line of 1 to 500 characters with no leading or trailing space",
            "graph",
        ));
    }
    Ok(())
}

/// The command that declares `name` where the declaration wants it (project manager review): a type, a relation, or
/// for an `order` entry no edge uses, a date field of the anchor type.
fn declare(graph: &ValidName, at: &str, name: &str, kind: &str, edges_use: bool) -> Command {
    let (Some(n), Some(k)) = (ValidName::new(name), ValidName::new(kind)) else {
        return Command::new("ontology show").word("--graph").name(graph);
    };
    let cmd = match at {
        "anchors.type" => Command::new("ontology add type").name(&n),
        "order" if !edges_use => Command::new("ontology add field")
            .name(&k)
            .name_with(&n, ':', "date"),
        _ => Command::new("ontology add relation").name(&n),
    };
    cmd.word("--graph").name(graph)
}

fn bad_name(bad: BadName) -> Problem {
    problems::usage_invalid(
        match bad {
            BadName::Name => "a spine name is a snake_case vocabulary name",
            BadName::AnchorType => "anchors.type is a snake_case type name",
            BadName::Inside => "anchors.inside is a snake_case relation name",
            BadName::Order(_) => "each order entry is a snake_case field or relation name",
            BadName::Place(_) => "each place entry is a snake_case relation name or evidence",
        },
        "graph",
    )
}

fn known_type(o: &Ontology, name: &str) -> bool {
    o.type_decl(name).is_some() || std_type(name).is_some()
}

fn known_relation(o: &Ontology, name: &str) -> bool {
    o.has_relation(name) || std_relation(name).is_some()
}

/// A field the anchor type declares, or a std field.
fn known_field(o: &Ontology, kind: &str, name: &str) -> bool {
    o.field_decl(kind, name).is_some() || std_field(name).is_some()
}

/// The first name in `d` that is neither the graph's vocabulary nor std, and where it is (see [`problems::
/// spine_name_unknown`]). Every name passed [`SpineDecl::check`] first.
fn unknown_name<'d>(o: &Ontology, d: &'d SpineDecl) -> Option<(&'static str, &'d str)> {
    let kind = d.anchors.kind.as_str();
    if !known_type(o, kind) {
        return Some(("anchors.type", kind));
    }
    if let Some(inside) = d.anchors.inside.as_deref()
        && !known_relation(o, inside)
    {
        return Some(("anchors.inside", inside));
    }
    if let Some(o_name) = d
        .order
        .iter()
        .find(|n| !known_field(o, kind, n) && !known_relation(o, n))
    {
        return Some(("order", o_name));
    }
    d.place
        .iter()
        .find(|n| n.as_str() != PLACE_EVIDENCE && !known_relation(o, n))
        .map(|n| ("place", n.as_str()))
}

fn declared(spines: &[SpineDecl]) -> Vec<Candidate> {
    spines
        .iter()
        .map(|s| Candidate {
            id: s.name.clone(),
            title: Some(s.title.clone()),
            score: None,
        })
        .collect()
}

impl GraphStore {
    fn spine_graph(&self, alias: &str) -> Result<(), Problem> {
        if !self.graphs.contains_key(alias) {
            return Err(self.not_loaded(alias));
        }
        Ok(())
    }

    /// `umx graph spine set <graph> --file <decl.json>`: stores one declaration, replacing one of the same name. The
    /// shape, the names (plain identifiers, then the graph's vocabulary or std) and the one-main rule are checked
    /// before anything is written.
    pub fn graph_spine_set(&mut self, ctx: &Ctx, alias: &str, decl: &Value) -> Outcome<SpineSet> {
        let decl = SpineDecl::from_value(decl).ok_or_else(|| {
            problems::usage_invalid(
                "the --file is not a spine declaration (name, title, main, anchors {type, inside}, order and place, and no other key)",
                "graph",
            )
        })?;
        self.store_spine(ctx, alias, decl)
    }

    /// `umx graph spine confirm <graph> <candidate>`: proposes again, and stores the named candidate's declaration
    /// unchanged, checked as `set` checks one. A built-in candidate has none (`spine_builtin_candidate`).
    pub fn graph_spine_confirm(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        candidate: &str,
    ) -> Outcome<SpineSet> {
        self.spine_graph(alias)?;
        let graph = alias_name(alias);
        let l = &self.graphs[alias];
        let proposed = propose(&l.graph, l.ontology(), &graph);
        let Some((c, _)) = proposed
            .candidates
            .iter()
            .find(|(c, _)| c.name == candidate)
        else {
            let names = proposed
                .candidates
                .iter()
                .map(|(c, _)| Candidate {
                    id: c.name.clone(),
                    title: None,
                    score: None,
                })
                .collect();
            return Err(
                problems::usage_invalid("unknown spine candidate", "time").candidates(names)
            );
        };
        let Some(decl) = c.decl.clone() else {
            return Err(problems::spine_builtin_candidate(&graph, candidate));
        };
        self.store_spine(ctx, alias, decl)
    }

    fn store_spine(&mut self, _ctx: &Ctx, alias: &str, decl: SpineDecl) -> Outcome<SpineSet> {
        self.spine_graph(alias)?;
        self.files_unchanged(alias)?;
        let graph = alias_name(alias);
        check_shape(&decl)?;
        // D71 step 12 (pm-001): --along reads these names as the built-in spines.
        if super::reserved_spine_name(&decl.name) {
            return Err(problems::spine_name_reserved(&graph, &decl.name));
        }
        let before = self.graphs[alias].meta.clone();
        if let Some((at, name)) = unknown_name(self.graphs[alias].ontology(), &decl) {
            let edges_use = self.graphs[alias]
                .graph
                .relation_kinds()
                .iter()
                .any(|k| k.as_str() == name);
            let fix = declare(&graph, at, name, &decl.anchors.kind, edges_use);
            return Err(problems::spine_name_unknown(
                &graph, &decl.name, at, name, fix,
            ));
        }
        if decl.main
            && let Some(main) = before.spines.iter().find(|s| s.main && s.name != decl.name)
        {
            return Err(problems::spine_main_taken(&graph, &main.name));
        }
        let at = before.spines.iter().position(|s| s.name == decl.name);
        if at.is_none() && before.spines.len() >= MAX_SPINES {
            return Err(problems::usage_invalid(
                "a graph declares at most 32 spines (remove one first)",
                "graph",
            ));
        }
        if at.is_some_and(|i| before.spines[i] == decl) {
            return Ok(Done::ok(SpineSet {
                alias: graph.as_str().into(),
                spine: decl,
                replaced: false,
                unchanged: true,
                format: before.umx,
            }));
        }
        let now = self.ports.clock.now_ms();
        let files = self.ports.files.clone();
        let l = self.graphs.get_mut(alias).expect("checked above");
        l.last_used.store(now, Ordering::Relaxed);
        match at {
            Some(i) => l.meta.spines[i] = decl.clone(),
            None => l.meta.spines.push(decl.clone()),
        }
        let mut warnings = persist(
            files.as_ref(),
            alias,
            l,
            &[GraphFile::Graph],
            Touched::default(),
        )
        .inspect_err(|_| l.meta = before)?;
        let fix = Command::new("graph spine set")
            .name(&graph)
            .word("--file")
            .word("<decl.json>");
        warnings.extend(settings::secret_warnings(
            &graph,
            "spine_title",
            Some(&decl.title),
            fix,
        ));
        Ok(Done {
            value: SpineSet {
                alias: graph.as_str().into(),
                spine: decl,
                replaced: at.is_some(),
                unchanged: false,
                format: l.meta.umx,
            },
            notes: warnings,
        })
    }

    /// `umx graph spine show <graph> [<name>]`: each declared spine (or the one named) placed on the graph as it is
    /// now, with cover and spread as propose reports them. Reads only. `by_rule` or `unplaced` (one, never both)
    /// adds one page of the ids the named spine places only by rule, or does not place.
    #[allow(clippy::too_many_arguments)]
    pub fn graph_spine_show(
        &self,
        _ctx: &Ctx,
        alias: &str,
        name: Option<&str>,
        by_rule: bool,
        unplaced: bool,
        limit: usize,
        offset: Option<usize>,
    ) -> Outcome<SpinesShown> {
        if by_rule && unplaced {
            return Err(problems::usage_invalid(
                "give --by-rule or --unplaced, not both",
                "time",
            ));
        }
        if (by_rule || unplaced) && name.is_none() {
            return Err(problems::usage_invalid(
                "--by-rule and --unplaced list one spine's nodes: name the spine",
                "time",
            ));
        }
        self.spine_graph(alias)?;
        let l = &self.graphs[alias];
        l.last_used
            .store(self.ports.clock.now_ms(), Ordering::Relaxed);
        let graph = alias_name(alias);
        let chosen: Vec<&SpineDecl> = match name {
            Some(n) => match l.meta.spines.iter().find(|s| s.name == n) {
                Some(s) => alloc::vec![s],
                None => {
                    return Err(problems::spine_not_found(
                        &graph,
                        n,
                        declared(&l.meta.spines),
                    ));
                }
            },
            None => l.meta.spines.iter().collect(),
        };
        let f = Facts::read(&l.graph);
        let g = &l.graph;
        let mut listed = None;
        let spines = chosen
            .into_iter()
            .map(|decl| {
                let (c, placed, _) = anchor_candidate(&f, decl.clone());
                if by_rule || unplaced {
                    let (how, total) = if by_rule {
                        (How::Rule, c.cover.by_rule)
                    } else {
                        (How::None, c.cover.not_placed)
                    };
                    let offset = offset.unwrap_or(0);
                    let ids = g
                        .nodes()
                        .filter(|n| !g.is_provenance(n.ix()) && placed.how[n.ix().index()] == how)
                        .map(|n| n.id())
                        .skip(offset)
                        .take(limit)
                        .collect();
                    listed = Some(Unplaced {
                        candidate: decl.name.clone(),
                        total,
                        offset,
                        ids,
                    });
                }
                let by_rule_next = (c.cover.by_rule > 0)
                    .then(|| ValidName::new(&decl.name))
                    .flatten()
                    .map(|n| {
                        Command::new("graph spine show")
                            .name(&graph)
                            .word("--by-rule")
                            .name(&n)
                    });
                DeclaredSpine {
                    decl: decl.clone(),
                    clock: c.clock,
                    cover: c.cover,
                    spread: c.spread,
                    offered: c.offered,
                    why_not: c.why_not,
                    by_rule_next,
                }
            })
            .collect();
        Ok(Done {
            value: SpinesShown {
                alias: graph.as_str().into(),
                total: f.total,
                spines,
                next: l
                    .meta
                    .spines
                    .is_empty()
                    .then(|| Command::new("graph spine propose").name(&graph)),
                by_rule: listed.clone().filter(|_| by_rule),
                unplaced: listed.filter(|_| unplaced),
            },
            notes: self.stale_note(alias).into_iter().collect(),
        })
    }

    /// `umx graph spine remove <graph> <name>`: drops one declaration. The format is not lowered (a write never
    /// lowers it).
    pub fn graph_spine_remove(
        &mut self,
        _ctx: &Ctx,
        alias: &str,
        name: &str,
    ) -> Outcome<SpineRemoved> {
        self.spine_graph(alias)?;
        self.files_unchanged(alias)?;
        let graph = alias_name(alias);
        let before = self.graphs[alias].meta.clone();
        let Some(at) = before.spines.iter().position(|s| s.name == name) else {
            return Err(problems::spine_not_found(
                &graph,
                name,
                declared(&before.spines),
            ));
        };
        let now = self.ports.clock.now_ms();
        let files = self.ports.files.clone();
        let l = self.graphs.get_mut(alias).expect("checked above");
        l.last_used.store(now, Ordering::Relaxed);
        let removed = l.meta.spines.remove(at);
        let warnings = persist(
            files.as_ref(),
            alias,
            l,
            &[GraphFile::Graph],
            Touched::default(),
        )
        .inspect_err(|_| l.meta = before)?;
        Ok(Done {
            value: SpineRemoved {
                alias: graph.as_str().into(),
                name: removed.name.to_string(),
            },
            notes: warnings,
        })
    }
}
