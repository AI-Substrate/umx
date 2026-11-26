//! Graph, node, edge and ontology verbs: typed operations in, envelopes out. Services return values and
//! problems; this module renders problems through umx-protocol's catalogue and shapes `data` (workshop 007
//! sections 4 to 7).

use crate::internal;
use std::collections::BTreeSet;
use umx_core::config::FileLayer;
use umx_core::ports::{GraphDir, ModelKey};
use umx_core::problem::Candidate;
use umx_core::{
    Command, GraphEmbedding, Id, IdText, Node, Policy, Problem, Slot, ValidName, value,
};
use umx_protocol::{
    Code, Envelope, Fill, Issue, KeyAction, Kind, Op, Request, SetArg, SetValue, UiCmd, Value,
    json, slots,
};
use umx_services::{
    BatchLine, BatchOp, BatchRequest, ConfigServices, Ctx, Direction, Done, EdgeChanged,
    EdgeEditRequest, EdgeEvidence, EdgeListRequest, EdgeRow, GraphKey, GraphfileMode, Guide,
    GuideEntry, GuideExample, HistoryRow, Idempotency, NeighboursRequest, NodeAddRequest, NodeCell,
    NodeEditRequest, NodeListRequest, NodeNoteRequest, NodeStatusRequest, PathRequest, Resolved,
    ScanRequest, Services, SpineLine, SpineProposal, SpineSet, SpinesShown, Target, WalkRequest,
};

const NODE_COLUMNS: [&str; 5] = ["id", "type", "title", "status", "updated"];

/// F137: whether `--columns` is `all` alone (the CLI refuses `all` beside other names).
fn columns_all(columns: Option<&[String]>) -> bool {
    matches!(columns, Some([c]) if c == "all")
}

/// F137: the columns `--columns all` adds for nodes: every custom field any of them has, sorted. A field named like
/// a column umx computes (a reserved node field, provenance keys included, or `copy`) is never one: the computed
/// column is authoritative, as F74's B1 for edges, even where a row leaves it out on purpose.
fn node_custom_columns<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let custom: BTreeSet<&str> = names
        .into_iter()
        // `evidence_sources` is reserved for `umx query` only: no list computes it, so a node's own stays a column.
        .filter(|c| {
            *c != "copy"
                && (*c == "evidence_sources" || !umx_core::name::RESERVED_NODE_FIELDS.contains(c))
        })
        .collect();
    custom.into_iter().map(String::from).collect()
}

/// Every column edge list computes for a row (F74, B1). These are authoritative: one a row leaves out on
/// purpose (a hidden end's title, a visible end's state) is null, never a same-named custom field of the edge.
const EDGE_COMPUTED_COLUMNS: [&str; 18] = [
    "id",
    "type",
    "from",
    "to",
    "from_title",
    "to_title",
    "sentence",
    "from_graph",
    "to_graph",
    "from_state",
    "to_state",
    "from_candidates",
    "to_candidates",
    "graph",
    "graph_id",
    "address",
    "copy",
    "derived",
];
const DEFAULT_LIMIT: usize = 100;

/// F188, F190: the key naming what umx's own secret scan flags on a node or edge object (names only), left out when
/// none. One const with the name a query column may not take (F241, cypher-expert R3).
const SECRET_SUSPECTED: &str = umx_core::secrets::SECRET_SUSPECTED;

/// Sets `value[SECRET_SUSPECTED]` to the computed `flags` (F188). The computed key always wins (R324-1): a stored
/// field of that name never passes through. Left out when there are none, unless the caller asked for the column
/// (`explicit`), which then holds `[]`.
fn with_flags(mut value: Value, flags: &[String], explicit: bool) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.remove(SECRET_SUSPECTED);
    }
    if explicit || !flags.is_empty() {
        value[SECRET_SUSPECTED] = json!(flags);
    }
    value
}

/// Whether `columns` asks for the flags by name.
fn asks_flags(columns: Option<&[String]>) -> bool {
    columns.is_some_and(|cols| cols.iter().any(|c| c == SECRET_SUSPECTED))
}

fn cwd_dir(cwd: &Option<String>) -> Option<GraphDir> {
    cwd.as_deref().and_then(|c| GraphDir::parse(c).ok())
}

fn rule_shown_json(v: &umx_services::RuleShown) -> Value {
    json!({"graph": v.graph, "rule": v.rule, "nodes": v.nodes, "total": v.total,
        "not_checked": v.not_checked, "not_checked_total": v.not_checked_total,
        "offset": v.offset, "limit": v.limit, "next": v.next.as_ref().map(|c| c.to_string()),
        "unchanged": v.unchanged, "dry_run": v.dry_run, "format": v.format})
}

fn hub_json(h: &umx_core::hubs::Hub) -> Value {
    let mut v = json!(h);
    v["form"] = json!(h.form());
    v
}

fn hub_rows_json(rows: &[umx_services::HubRow]) -> Value {
    json!(
        rows.iter()
            .map(|r| {
                let mut row = hub_json(&r.hub);
                row["hub_nodes"] = json!(r.hub_nodes);
                row["members"] = json!(r.members);
                row["missing"] = json!(r.missing);
                row["add"] = json!(r.add.to_string());
                row
            })
            .collect::<Vec<_>>()
    )
}

fn hub_joins_json(joins: &[umx_services::HubJoin]) -> Value {
    json!(
        joins
            .iter()
            .map(|j| json!({"hub": j.hub, "edge": j.edge, "node": j.node, "to": j.to}))
            .collect::<Vec<_>>()
    )
}

fn hub_filled_json(v: &umx_services::HubFilled) -> Value {
    json!({"graph": v.graph, "hub": hub_json(&v.hub), "joined": hub_joins_json(&v.joined),
        "joined_total": v.joined_total, "not_checked": v.not_checked,
        "not_checked_total": v.not_checked_total, "after": v.after, "unchanged": v.unchanged,
        "format": v.format})
}

fn rule_changed_json(v: &umx_services::RuleChanged) -> Value {
    json!({"graph": v.graph, "name": v.name, "unchanged": v.unchanged, "format": v.format})
}

fn layer(repo: bool) -> FileLayer {
    if repo {
        FileLayer::Repo
    } else {
        FileLayer::Machine
    }
}

/// The config verbs (req-72). Set and unset run on the writer thread; show and check do not.
pub fn config(cs: &ConfigServices, request: &Request) -> Envelope {
    let Some(op) = &request.op else {
        return Envelope::fail(&request.command, internal("no_operation"));
    };
    let command = op.command();
    if let Some(problem) = scope_usage(request) {
        return Envelope::fail(command, issue(&problem, command));
    }
    match op {
        Op::ConfigShow { cwd } => reply(command, cs.show(cwd_dir(cwd).as_ref()), |v| {
            let settings: Vec<Value> = v
                .settings
                .iter()
                .map(|s| {
                    let mut row = json!({
                        "key": s.key, "value": s.value, "source": s.source, "applies": s.applies,
                        "doc": s.doc,
                    });
                    let obj = row.as_object_mut().expect("an object");
                    if let Some(f) = &s.file {
                        obj.insert("file".into(), json!(f.as_str()));
                    }
                    if let Some(e) = s.env {
                        obj.insert("env".into(), json!(e));
                    }
                    if let Some(f) = s.flag {
                        obj.insert("flag".into(), json!(f));
                    }
                    if let Some(r) = &s.running {
                        obj.insert("running".into(), json!(r));
                    }
                    if let Some(secret) = s.secret {
                        obj.insert("secret".into(), json!(secret));
                    }
                    row
                })
                .collect();
            json!({
                "files": {
                    "machine": v.machine_file.as_str(),
                    "repo": v.repo_file.as_ref().map(GraphDir::as_str),
                    "secrets": v.secrets_file.as_str(),
                },
                "settings": settings,
                "problems": v.problems,
            })
        }),
        Op::ConfigSet {
            key,
            value,
            repo,
            cwd,
        } => reply(
            command,
            cs.set(cwd_dir(cwd).as_ref(), key, value, layer(*repo)),
            |v| {
                json!({
                    "key": v.key, "value": v.value, "was": v.was, "replaced": v.replaced, "layer": v.layer,
                    "file": v.file.as_str(), "applies": v.applies,
                })
            },
        ),
        Op::ConfigUnset { key, repo, cwd } => reply(
            command,
            cs.unset(cwd_dir(cwd).as_ref(), key, layer(*repo)),
            |v| json!({ "key": v.key, "removed": v.removed, "layer": v.layer, "file": v.file.as_str() }),
        ),
        Op::ConfigCheck { cwd } => {
            let found = cs.check(cwd_dir(cwd).as_ref());
            let issues: Vec<(Kind, Issue)> = found
                .iter()
                .map(|p| {
                    let kind = Code::from_code(p.code).map_or(Kind::Error, |c| c.spec().kind);
                    (kind, issue(p, command))
                })
                .collect();
            let mut env = Envelope::ok(command, json!({ "problems": found.len() }));
            let mut errors = Vec::new();
            for (kind, i) in issues {
                match kind {
                    Kind::Warning => env = env.warn(i),
                    Kind::Hint => env = env.hint(i),
                    Kind::Error => errors.push(i),
                }
            }
            if let Some((first, rest)) = errors.split_first() {
                let (warnings, hints) = (env.warnings, env.hints);
                env = Envelope::fail(command, first.clone());
                env.errors.extend(rest.iter().cloned());
                env.warnings = warnings;
                env.hints = hints;
            }
            env
        }
        Op::ConfigImport {
            from,
            name,
            model,
            dims,
            force,
            dry_run,
        } => {
            let ask = umx_services::problems::ImportAsk {
                name: name.as_deref(),
                model: model.as_deref(),
                dims: *dims,
                force: *force,
                dry_run: *dry_run,
            };
            reply(command, cs.import(from, &ask), config_imported)
        }
        Op::RootAdd { name, folder } => reply(command, cs.root_add(name, folder), |v| {
            json!({
                "root": {
                    "name": v.name, "folder": v.folder, "canonical": v.canonical.as_str(),
                    "source": format!("root:{}/<path>", v.name),
                },
                "was": v.was, "file": v.file.as_str(),
            })
        }),
        Op::RootList => reply(command, cs.root_list(), |rows| {
            let rows: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "name": r.name, "folder": r.folder,
                        "canonical": r.canonical.as_ref().map(GraphDir::as_str), "status": r.status,
                    })
                })
                .collect();
            json!({
                "roots": rows,
                "built_in": [{ "name": umx_core::config::REPO_ROOT, "folder": "the git folder holding the graph" }],
            })
        }),
        Op::RootRemove { name } => reply(
            command,
            cs.root_remove(name),
            |v| json!({ "name": v.name, "removed": true, "file": v.file.as_str() }),
        ),
        _ => Envelope::fail(command, internal("not_a_config_operation")),
    }
}

/// Runs a write on the writer thread.
pub fn run(services: &mut Services, request: &Request) -> Envelope {
    let (actor, warning) = crate::provenance::actor(request.actor.as_ref());
    let tier = actor.fields().tier.as_str();
    let ctx = context(request, actor);
    let redirected = ctx.redirected.clone();
    let mut env = with_redirects(run_in_ctx(services, request, ctx), &redirected, request);
    if let Some(data) = env.data.as_object_mut() {
        data.insert("actor_tier".into(), json!(tier));
    }
    if env.errors.is_empty()
        && let Some(warning) = warning
    {
        env = env.note(issue(&warning, &request.command));
    }
    env
}

/// ws006 S23 (architect (f)): a verb's positional graph alias is read through `canonical`, as every other alias is,
/// so a former alias names its graph (with the `alias_redirected` hint). Some(op) with the live alias when it
/// changed.
fn with_live_alias(services: &Services, ctx: &Ctx, op: &Op) -> Result<Option<Op>, Problem> {
    let mut op = op.clone();
    let Some(alias) = positional_alias(&mut op) else {
        return Ok(None);
    };
    let live = services.canonical(ctx, alias)?;
    if live == *alias {
        return Ok(None);
    }
    *alias = live;
    Ok(Some(op))
}

/// ws008 T2b (req-99): `std:<name>` on a name-taking flag or word (`--type std:person`, `--relation
/// std:works_for`, `ontology add field std:person ...`) is `<name>` when that is a std name of its kind; names are
/// stored plain. `None` when the op names none: a `std:` that is not std stays as given and fails the name rules, and
/// `ontology remove` keeps it, so it is refused as a std entry.
fn std_plain(op: &Op) -> Option<Op> {
    use umx_core::standard::{NameKind, QUALIFIER, plain};
    let named = |s: &str| s.starts_with(QUALIFIER);
    let some = |s: &Option<String>| s.as_deref().is_some_and(named);
    let any = match op {
        Op::NodeAdd { kind, .. } => named(kind),
        Op::EdgeAdd { relation, .. } | Op::OntologyAddRelationField { relation, .. } => {
            named(relation)
        }
        Op::OntologyAddType { name, .. } | Op::OntologySetType { name, .. } => named(name),
        Op::OntologySetField { type_name, .. } => named(type_name),
        Op::NodeEdit { kind, .. } | Op::NodeList { kind, .. } | Op::NodeFind { kind, .. } => {
            some(kind)
        }
        Op::EdgeEdit { relation, .. }
        | Op::Neighbours { relation, .. }
        | Op::Walk { relation, .. }
        | Op::Path { relation, .. } => some(relation),
        Op::EdgeList { kind, .. } => some(kind),
        Op::OntologyAddField { types, .. } => types.iter().any(|t| named(t)),
        Op::OntologyAddRelation { name, from, to, .. } => {
            named(name) || from.iter().chain(to).any(|t| named(t))
        }
        Op::OntologyAdoptStd { map, .. } => map
            .iter()
            .any(|m| m.split_once('=').is_some_and(|(_, to)| named(to))),
        _ => false,
    };
    if !any {
        return None;
    }
    let fix = |kind: NameKind, s: &mut String| *s = plain(kind, s).to_string();
    let fix_opt = |kind: NameKind, s: &mut Option<String>| {
        if let Some(s) = s {
            fix(kind, s);
        }
    };
    let mut op = op.clone();
    match &mut op {
        Op::NodeAdd { kind, .. }
        | Op::OntologyAddType { name: kind, .. }
        | Op::OntologySetType { name: kind, .. }
        | Op::OntologySetField {
            type_name: kind, ..
        } => fix(NameKind::Type, kind),
        Op::EdgeAdd { relation, .. } | Op::OntologyAddRelationField { relation, .. } => {
            fix(NameKind::Relation, relation)
        }
        Op::NodeEdit { kind, .. } | Op::NodeList { kind, .. } | Op::NodeFind { kind, .. } => {
            fix_opt(NameKind::Type, kind)
        }
        Op::EdgeEdit { relation, .. }
        | Op::Neighbours { relation, .. }
        | Op::Walk { relation, .. }
        | Op::Path { relation, .. }
        | Op::EdgeList { kind: relation, .. } => fix_opt(NameKind::Relation, relation),
        Op::OntologyAddField { types, .. } => types.iter_mut().for_each(|t| fix(NameKind::Type, t)),
        Op::OntologyAddRelation { name, from, to, .. } => {
            fix(NameKind::Relation, name);
            from.iter_mut()
                .chain(to.iter_mut())
                .for_each(|t| fix(NameKind::Type, t));
        }
        // A target is a type or a relation: std's name of either kind, stored plain.
        Op::OntologyAdoptStd { map, .. } => {
            for m in map.iter_mut() {
                if let Some((from, to)) = m.split_once('=') {
                    let plain_to = match plain(NameKind::Type, to) {
                        same if same == to => plain(NameKind::Relation, to),
                        other => other,
                    };
                    *m = format!("{from}={plain_to}");
                }
            }
        }
        _ => {}
    }
    Some(op)
}

/// The graph alias a verb takes by position, if it takes one.
fn positional_alias(op: &mut Op) -> Option<&mut String> {
    match op {
        Op::GraphUnload { alias }
        | Op::GraphSet { alias, .. }
        | Op::GraphConfirmPolicy { alias, .. }
        | Op::GraphGuide { alias, .. }
        | Op::GraphPrimary { alias }
        | Op::GraphSpinePropose { alias, .. }
        | Op::GraphSpineSet { alias, .. }
        | Op::GraphSpineConfirm { alias, .. }
        | Op::GraphSpineShow { alias, .. }
        | Op::GraphSpineRemove { alias, .. } => Some(alias),
        _ => None,
    }
}

fn run_in_ctx(services: &mut Services, request: &Request, ctx: Ctx) -> Envelope {
    let Some(op) = &request.op else {
        return Envelope::fail(&request.command, internal("no_operation"));
    };
    let command = op.command();
    let live = match with_live_alias(services, &ctx, op) {
        Ok(live) => live,
        Err(p) => return Envelope::fail(command, issue(&p, command)),
    };
    let op = live.as_ref().unwrap_or(op);
    let plain = std_plain(op);
    let op = plain.as_ref().unwrap_or(op);
    if let Some(problem) = scope_usage(request) {
        return Envelope::fail(command, issue(&problem, command));
    }
    match op {
        Op::ScopeSave { name, replace } => reply(
            command,
            services.scope_save(&ctx, name, *replace),
            scope_json,
        ),
        Op::RuleAdd { rule, dry_run } => reply(
            command,
            services.rule_add(&ctx, rule, *dry_run),
            rule_shown_json,
        ),
        Op::RuleRemove { name } => {
            reply(command, services.rule_remove(&ctx, name), rule_changed_json)
        }
        Op::RuleOff { name } => reply(
            command,
            services.rule_switch(&ctx, name, true),
            rule_changed_json,
        ),
        Op::RuleOn { name } => reply(
            command,
            services.rule_switch(&ctx, name, false),
            rule_changed_json,
        ),
        Op::HubSet {
            name,
            node,
            type_hub,
            via,
            pattern,
        } => reply(
            command,
            services.hub_set(
                &ctx,
                umx_services::HubSetRequest {
                    name: name.clone(),
                    node: node.clone(),
                    type_hub: type_hub.clone(),
                    via: via.clone(),
                    pattern: pattern.clone(),
                },
            ),
            hub_filled_json,
        ),
        Op::HubFill { name, after } => reply(
            command,
            services.hub_fill(&ctx, name, after.as_deref()),
            hub_filled_json,
        ),
        Op::HubRemove { name } => reply(command, services.hub_remove(&ctx, name), |v| {
            json!({"graph": v.graph, "name": v.name, "unchanged": v.unchanged,
                "remaining": v.remaining, "format": v.format})
        }),
        Op::ScopeDelete { name } => reply(
            command,
            services.scope_delete(name),
            |name| json!({ "name": name, "deleted": true }),
        ),
        Op::GraphNew {
            path,
            alias,
            title,
            empty,
            embedding_model,
            secured,
            no_graphfile,
            graphfile,
            no_std,
        } => {
            let mode = if *no_graphfile {
                GraphfileMode::Skip
            } else if *graphfile {
                GraphfileMode::Include
            } else {
                GraphfileMode::Add
            };
            let key = match embedding_model.as_deref().map(ModelKey::parse) {
                None => None,
                Some(Ok(key)) => Some(key),
                Some(Err(_)) => {
                    return Envelope::fail(
                        command,
                        issue(
                            &umx_services::problems::usage_invalid(
                                "--embedding-model must be a model key as umx docs get embeddings shows it (the model in lower-case letters and digits and . _ - then dims 1 to 8192 and an optional +pN)",
                                "graph",
                            ),
                            command,
                        ),
                    );
                }
            };
            let embedding = if *secured {
                Some(GraphEmbedding::new(key, Policy::SECURED))
            } else {
                key.map(|k| GraphEmbedding::new(Some(k), Policy::OPEN))
            };
            reply(
                command,
                services.graph_new_with(
                    &ctx,
                    path,
                    alias,
                    title.as_deref(),
                    *empty,
                    embedding,
                    mode,
                    !*no_std,
                ),
                |v| {
                    json!({
                        "graph": {
                            "id": v.graph.id.to_text(), "alias": v.graph.alias, "path": v.graph.path, "loaded": true,
                            "embedding": embedding_json(v.graph.embedding.as_ref()),
                            "effective_policy": v.graph.policy.effective.name(),
                            "effective_visibility": v.graph.policy.effective.visibility.as_str(),
                            "effective_embedder": v.graph.policy.effective.embedder.as_str(),
                            "downgraded": v.graph.policy.downgraded_at,
                            "vectors_pending": v.graph.vectors_pending, "vectors_held": v.graph.vectors_held,
                        },
                        "ontology": {
                            "types": v.types, "statuses": v.statuses, "relations": v.relations, "std": v.std,
                        },
                        "next": v.next.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                        // The line added and its undo (security, s17); null when none was added.
                        "graphfile": v.graphfile.as_ref().map(|g| json!({
                            "file": g.file.as_str(), "path": g.path, "alias": g.alias, "undo": g.undo.as_str(),
                        })),
                    })
                },
            )
        }
        Op::GraphLoad {
            path,
            graphfile: true,
            set,
            ..
        } => reply(
            command,
            services.graph_load_graphfile(&ctx, path, set.as_deref()),
            graphfile_load_json,
        ),
        Op::GraphLoad {
            path,
            as_alias,
            by_alias,
            graphfile: false,
            ..
        } => reply(
            command,
            services.graph_load_by(&ctx, path, as_alias.as_deref(), by_alias.as_deref()),
            |v| {
                let mut out = json!({
                    "graph": {
                        "id": v.graph.id.to_text(), "alias": v.graph.alias, "path": v.graph.path,
                        "nodes": v.graph.nodes, "edges": v.graph.edges, "kept_lines": v.graph.kept_lines,
                        "embedding": embedding_json(v.graph.embedding.as_ref()),
                        "effective_policy": v.graph.policy.effective.name(),
                            "effective_visibility": v.graph.policy.effective.visibility.as_str(),
                            "effective_embedder": v.graph.policy.effective.embedder.as_str(),
                        "downgraded": v.graph.policy.downgraded_at,
                        "vectors_pending": v.graph.vectors_pending, "vectors_held": v.graph.vectors_held,
                    },
                    "off_vocabulary": v.off_vocabulary,
                });
                // ws006 S23: a load that found the graph moved by hand.
                if let Some(from) = &v.moved_from {
                    out["moved_from"] = json!(from.as_str());
                    out["stale"] = json!(
                        v.stale
                            .iter()
                            .map(|(id, place)| json!({"id": id.to_text(), "place": place}))
                            .collect::<Vec<_>>()
                    );
                    out["stale_total"] = json!(v.stale_total);
                    out["stale_recorded"] = json!(false);
                }
                out
            },
        ),
        Op::NodeAdd {
            kind,
            title,
            status,
            note,
            sets,
            parent,
            evidence,
            evidence_kind,
            locator,
            raised_in,
            on_key,
            related: _,
            tags,
        } => {
            let req = NodeAddRequest {
                kind: kind.clone(),
                title: title.clone(),
                status: status.clone(),
                note: note.clone(),
                sets: core_sets(sets),
                parent: parent.clone(),
                evidence: evidence.iter().map(|p| p.as_str().into()).collect(),
                evidence_kind: evidence_kind.clone(),
                locator: locator.clone(),
                raised_in: raised_in.clone(),
                on_key_resolve: on_key.is_some(),
                tags: tags.clone(),
            };
            reply(command, services.node_add(&ctx, req), |v| {
                // F234: a sentence holding a flagged title (the new node's, the parent's or the anchor's) is named.
                let edge = |e: &umx_services::EdgeAdded| {
                    with_flags(
                        json!({ "id": e.edge.id.to_text(), "sentence": e.sentence }),
                        &flag_if(e.sentence_flagged, "sentence"),
                        false,
                    )
                };
                // req-123: filled after the write by the related check; null until it runs or when it was skipped.
                let mut data = json!({
                    "node": node_json(&v.alias, &v.node),
                    "parent_edge": v.parent_edge.as_ref().map(edge),
                    "related": null,
                    "related_floor": null,
                });
                // ws008 TM4: only with --raised-in, so a reply without it is unchanged.
                if let Some(e) = &v.raised_in_edge {
                    data["raised_in_edge"] = edge(e);
                }
                // PRD 003 hubs: only when umx added the node to a hub, so other replies are unchanged.
                if !v.hub_edges.is_empty() {
                    data["hub_edges"] = hub_joins_json(&v.hub_edges);
                }
                // ws003 s08: only with --on-key, so a reply without it is unchanged.
                if on_key.is_some() {
                    data["resolved"] = json!(v.resolved);
                    data["changed"] = json!(v.changed);
                }
                data
            })
        }
        Op::EdgeAdd {
            from,
            relation,
            to,
            sets,
            evidence,
            evidence_kind,
        } => reply(
            command,
            services.edge_add_with(
                &ctx,
                from,
                relation,
                to,
                &core_sets(sets),
                &EdgeEvidence {
                    pointers: evidence.clone(),
                    kind: evidence_kind.clone(),
                },
            ),
            |v| {
                with_flags(
                    json!({ "edge": edge_json(&v.alias, &v.edge), "sentence": v.sentence }),
                    &flag_if(v.sentence_flagged, "sentence"),
                    false,
                )
            },
        ),
        Op::EdgeDelete { edge } => reply(command, services.edge_delete(&ctx, edge), |v| {
            edge_changed(v, true)
        }),
        Op::EdgeEdit {
            edge,
            relation,
            reverse,
            sets,
            unsets,
        } => reply(
            command,
            services.edge_edit(
                &ctx,
                edge,
                &EdgeEditRequest {
                    relation: relation.clone(),
                    reverse: *reverse,
                    sets: core_sets(sets),
                    unsets: unsets.clone(),
                },
            ),
            |v| edge_changed(v, false),
        ),
        Op::OntologyAddType {
            name,
            fields,
            description,
        } => reply(
            command,
            services.ontology_add_type(&ctx, name, fields, description.as_deref()),
            |v| json!({ "added": v.added, "name": v.name, "unchanged": v.unchanged }),
        ),
        Op::OntologyAddStatus { name, description } => reply(
            command,
            services.ontology_add_status(&ctx, name, description.as_deref()),
            |v| json!({ "added": v.added, "name": v.name, "unchanged": v.unchanged }),
        ),
        Op::OntologyAddTag { tag } => reply(
            command,
            services.ontology_add_tag(&ctx, tag),
            |v| json!({ "added": v.added, "name": v.name, "unchanged": v.unchanged }),
        ),
        Op::OntologySetType { name, columns } => reply(
            command,
            services.ontology_set_type(&ctx, name, columns.as_deref()),
            |v| json!({ "type": v.name, "columns": columns, "unchanged": v.unchanged }),
        ),
        Op::OntologyAddRelation {
            name,
            symmetric,
            description,
            from,
            to,
        } => reply(
            command,
            services.ontology_add_relation_with_ends(
                &ctx,
                name,
                *symmetric,
                description.as_deref(),
                from,
                to,
            ),
            |v| json!({ "added": v.added, "name": v.name, "unchanged": v.unchanged }),
        ),
        // One type: the original form, which declares a new type too (F71 keeps it as it was).
        Op::OntologyAddField {
            types,
            field,
            all_types: false,
            description,
            role,
            values,
        } if types.len() == 1 => reply(
            command,
            services.ontology_add_field_with_values(
                &ctx,
                &types[0],
                field,
                description.as_deref(),
                role.as_deref(),
                values,
            ),
            |v| json!({ "added": v.added, "name": v.name, "unchanged": v.unchanged }),
        ),
        Op::OntologyAddField {
            types,
            field,
            all_types,
            description,
            role,
            values,
        } => match services.ontology_add_field_many_with_values(
            &ctx,
            types,
            *all_types,
            field,
            (description.as_deref(), role.as_deref(), values),
        ) {
            Ok(done) => reply(
                command,
                Ok(done),
                |v| json!({ "added": "field", "name": v.name, "types": v.types, "unchanged": v.unchanged }),
            ),
            Err(problems) => batch_errors(command, &problems),
        },
        Op::OntologySetField {
            type_name,
            field,
            role,
            values,
        } => reply(
            command,
            services.ontology_set_field_with_values(
                &ctx,
                type_name,
                field,
                role.as_deref(),
                values.as_deref(),
            ),
            |v| {
                json!({
                    "type": v.type_name, "field": v.field, "role": v.role.as_str(),
                    "changed": v.changed, "pending": v.affected.len(),
                    "values": v.values,
                })
            },
        ),
        Op::OntologyRemoveType { name } => reply(
            command,
            services.ontology_remove_type(&ctx, name),
            removed_json,
        ),
        Op::OntologyRemoveStatus { name } => reply(
            command,
            services.ontology_remove_status(&ctx, name),
            removed_json,
        ),
        Op::OntologyRemoveRelation { name } => reply(
            command,
            services.ontology_remove_relation(&ctx, name),
            removed_json,
        ),
        Op::OntologyAdoptStd {
            map,
            dry_run,
            field_to_tag,
        } => match services.ontology_adopt_std(&ctx, map, *dry_run, field_to_tag) {
            Ok(done) => reply(command, Ok(done), adopted_json),
            Err(problems) => batch_errors(command, &problems),
        },
        Op::OntologyRemoveRelationField { relation, field } => reply(
            command,
            services.ontology_remove_relation_field(&ctx, relation, field),
            |v| json!({ "removed": v.removed, "relation": relation, "name": v.name, "unchanged": v.unchanged }),
        ),
        Op::OntologyRemoveField { type_name, field } => reply(
            command,
            services.ontology_remove_field(&ctx, type_name, field),
            |v| json!({ "removed": v.removed, "type": type_name, "name": v.name, "unchanged": v.unchanged }),
        ),
        Op::OntologyAddRelationField { relation, field } => reply(
            command,
            services.ontology_add_relation_field(&ctx, relation, field),
            |v| json!({ "added": v.added, "relation": relation, "name": v.name, "unchanged": v.unchanged }),
        ),
        Op::GraphSet { alias, settings } => {
            let given: Vec<(String, String)> = settings
                .iter()
                .map(|s| (s.name.clone(), s.value.clone()))
                .collect();
            reply(
                command,
                services.graph_set(&ctx, alias, &given),
                |v| json!({ "alias": v.alias, "changed": v.changed, "title": v.title }),
            )
        }
        Op::GraphConfirmPolicy { alias } => {
            reply(command, services.confirm_policy(&ctx, alias), |v| {
                json!({
                    "alias": v.alias, "was": v.was.name(), "policy": v.policy.name(), "changed": v.changed,
                })
            })
        }
        Op::GraphUnload { alias } => reply(
            command,
            services.graph_unload(&ctx, alias),
            |v| json!({ "alias": v.alias, "unloaded": true }),
        ),
        // ws008 TM5: declared spines, written to graph.json.
        Op::GraphSpineSet { alias, decl } => reply(
            command,
            services.graph_spine_set(&ctx, alias, decl),
            spine_set_json,
        ),
        Op::GraphSpineConfirm { alias, candidate } => reply(
            command,
            services.graph_spine_confirm(&ctx, alias, candidate),
            spine_set_json,
        ),
        Op::GraphSpineRemove { alias, name } => reply(
            command,
            services.graph_spine_remove(&ctx, alias, name),
            |v| json!({ "graph": v.alias, "removed": v.name }),
        ),
        // F98: names only the copy asked for (security).
        Op::GraphPrimary { alias } => reply(
            command,
            services.graph_primary(&ctx, alias),
            |v| json!({ "alias": v.alias, "graph_id": v.graph.to_text(), "unchanged": v.unchanged }),
        ),
        // ws006 S23 (req-56).
        Op::GraphRename { from, to } => {
            reply(command, services.graph_rename(&ctx, from, to), |v| {
                json!({
                    "graph": v.graph, "graph_id": v.graph_id.to_text(), "from": v.from,
                    "former_aliases": v.former_aliases, "dropped": v.dropped, "graphfile": v.graphfile,
                })
            })
        }
        // Names the moved graph's own stale pointers only; other graphs citing the folder are not checked.
        Op::GraphMove { alias, to } => reply(command, services.graph_move(&ctx, alias, to), |v| {
            json!({
                "graph": v.graph, "graph_id": v.graph_id.to_text(), "from": v.from.as_str(), "to": v.to.as_str(),
                "former_paths": v.former_paths, "graphfile": v.graphfile,
                "stale": v.stale.iter().map(|(id, place)| json!({"id": id.to_text(), "place": place})).collect::<Vec<_>>(),
                "stale_total": v.stale_total, "other_graphs_checked": false,
            })
        }),
        Op::GraphForget { alias } => reply(
            command,
            services.graph_forget(&ctx, alias),
            |v| json!({ "alias": v.alias, "forgotten": true, "rows": v.rows }),
        ),
        Op::NodeEdit {
            node,
            title,
            kind,
            note,
            sets,
            unsets,
            locator,
            tag,
            untag,
        } => {
            let req = NodeEditRequest {
                title: title.clone(),
                kind: kind.clone(),
                note: note.clone(),
                sets: core_sets(sets),
                unsets: unsets.clone(),
                locator: locator.clone(),
                tag: tag.clone(),
                untag: untag.clone(),
            };
            reply(command, services.node_edit(&ctx, node, req), |v| {
                let mut data =
                    json!({ "node": node_json(&v.alias, &v.node), "changed": v.changed });
                // PRD 003 hubs: only when umx added the node to a hub, so other replies are unchanged.
                if !v.hub_edges.is_empty() {
                    data["hub_edges"] = hub_joins_json(&v.hub_edges);
                }
                data
            })
        }
        Op::EntityKey { node, action, key } => reply(
            command,
            services.entity_key(&ctx, node, *action == KeyAction::Add, key),
            |v| json!({ "node": node_json(&v.alias, &v.node), "changed": v.changed }),
        ),
        Op::EntityLink { a, b, dry_run } => {
            reply(command, services.entity_link(&ctx, a, b, *dry_run), |v| {
                with_flags(
                    json!({
                        "edge": v.edge.as_ref().map(|e| edge_json(&v.alias, e)), "graph": v.alias,
                        "sentence": v.sentence, "unchanged": v.unchanged, "dry_run": v.dry_run,
                    }),
                    &flag_if(v.sentence_flagged, "sentence"),
                    false,
                )
            })
        }
        Op::EntityDeny { a, b, reason } => reply(
            command,
            services.entity_deny(&ctx, a, b, reason.as_deref()),
            |v| {
                with_flags(
                    json!({
                        "edge": edge_json(&v.alias, &v.edge), "sentence": v.sentence, "unchanged": v.unchanged,
                        "replaced": v.replaced.map(|id| umx_services::address(&v.alias, id)),
                    }),
                    &flag_if(v.sentence_flagged, "sentence"),
                    false,
                )
            },
        ),
        Op::EntityUndeny { a, b } => {
            reply(command, services.entity_undeny(&ctx, a, b), |v| {
                match &v.removed {
                    Some((alias, edge)) => {
                        json!({ "edge": edge_json(alias, edge), "deleted": true, "unchanged": false })
                    }
                    None => json!({ "edge": null, "deleted": false, "unchanged": true }),
                }
            })
        }
        Op::EntityUnlink { a, b } => {
            reply(command, services.entity_unlink(&ctx, a, b), |v| {
                match &v.removed {
                    Some((alias, edge)) => {
                        json!({ "edge": edge_json(alias, edge), "deleted": true, "unchanged": false })
                    }
                    None => json!({ "edge": null, "deleted": false, "unchanged": true }),
                }
            })
        }
        Op::NodeNote {
            node,
            append,
            evidence,
            evidence_kind,
        } => match legacy_note_request(append, evidence, evidence_kind.as_deref()) {
            Ok(req) => reply(command, services.node_note_request(&ctx, node, req), |v| {
                json!({
                    "node": node_json(&v.alias, &v.node), "appended": true, "evidence_added": v.evidence_added,
                })
            }),
            Err(problem) => Envelope::fail(command, issue(&problem, command)),
        },
        Op::NodeStatus {
            node,
            status,
            reason,
            label,
            place,
            evidence,
            evidence_kind,
        } => {
            let req = NodeStatusRequest {
                status: status.clone(),
                reason: reason.clone(),
                label: label.clone(),
                anchor: place.clone(),
                evidence: evidence.clone(),
                evidence_kind: evidence_kind.clone(),
            };
            reply(command, services.node_status_with(&ctx, node, req), |v| {
                let c = &v.change;
                let mut data = json!({
                    "id": v.id.to_text(), "address": umx_services::address(&v.alias, v.id), "graph": v.alias,
                    "from": c.from, "to": c.to, "label": c.label, "reason": c.reason, "at": c.at, "by": c.by,
                });
                // ws008 TM6: only when given, so a reply without them is unchanged.
                if let Some(place) = &c.place {
                    data["in"] = json!(place);
                }
                if let Some(evidence) = &c.evidence {
                    data["evidence"] = evidence.clone();
                }
                data
            })
        }
        Op::SimilarityRebuild => similarity_rebuilt(command, services.similarity_rebuild(&ctx)),
        Op::EmbeddingsRepair => reply(command, services.embeddings_repair(&ctx), |v| {
            json!({
                "graph": v.alias, "queued": v.queued, "tidied": v.tidied, "shards_written": v.shards_written,
                "orphans": v.orphans, "kept_lines": v.kept_lines,
            })
        }),
        Op::EmbeddingsReembed { dry_run, model } => {
            let model = match reembed_model(command, model) {
                Ok(m) => m,
                Err(env) => return *env,
            };
            reply(
                command,
                services.embeddings_reembed(&ctx, *dry_run, model.as_ref()),
                reembed_json,
            )
        }
        Op::Batch {
            lines,
            dry_run,
            reason,
            echo_sentences,
            refs,
            refs_graph,
            raised_in,
            related: _,
            idempotency_key,
        } => batch(
            services,
            &ctx,
            command,
            lines,
            *dry_run,
            (reason.clone(), raised_in.clone()),
            *echo_sentences,
            (refs, refs_graph),
            // ws003 s09 (architect C4, C5): the daemon digests the canonical input, which needs the wire types; the
            // services check the key and digest it themselves.
            idempotency_key.clone().map(|key| Idempotency {
                key,
                input: umx_core::receipts::input_digest(&op.batch_input().unwrap_or_default()),
            }),
        ),
        Op::NodeEvidence {
            node,
            action,
            pointer,
            evidence_kind,
            reason,
            at,
            removed,
            redact,
            all,
        } => node_evidence(
            services,
            &ctx,
            command,
            node,
            *action,
            pointer.as_deref(),
            evidence_kind.as_deref(),
            reason.as_deref(),
            (*at, *removed, *redact, *all),
        ),
        Op::ResourceRefresh { node } => {
            reply(command, services.resource_refresh(&ctx, node), |v| {
                json!({
                    "resource": node_json(&v.alias, &v.node),
                    "changed": v.changed,
                    "hashed": v.hashed,
                })
            })
        }
        Op::EmbeddingsAllow { node, field } => {
            reply(command, services.embeddings_allow(&ctx, node, field), |v| {
                json!({
                    "graph": v.alias, "node": v.node.to_text(), "address": umx_services::address(&v.alias, v.node),
                    "field": v.field, "allowed": v.allowed, "rule": v.rule, "why": v.why,
                })
            })
        }
        _ => read(services, request),
    }
}

/// The wire's batch op as the services' (one to one; the services check every field).
fn core_evidence(
    pointers: Vec<umx_protocol::EvidencePointer>,
) -> Vec<umx_core::evidence::EvidencePointer> {
    use umx_core::evidence::EvidenceKind as Core;
    use umx_protocol::EvidenceKind as Wire;
    pointers
        .into_iter()
        .map(|p| umx_core::evidence::EvidencePointer {
            pointer: p.pointer,
            kind: p.kind.map(|kind| match kind {
                Wire::Room => Core::Room,
                Wire::Chat => Core::Chat,
                Wire::Screen => Core::Screen,
                Wire::Document => Core::Document,
                Wire::Analyst => Core::Analyst,
            }),
        })
        .collect()
}

fn legacy_note_request(
    append: &str,
    evidence: &[String],
    kind: Option<&str>,
) -> Result<NodeNoteRequest, Problem> {
    let kind = kind
        .map(|kind| {
            umx_protocol::EvidenceKind::parse(kind).ok_or_else(|| {
                umx_services::problems::usage_invalid(
                    "--evidence-kind is room or chat or screen or document or analyst",
                    "node",
                )
            })
        })
        .transpose()?;
    Ok(NodeNoteRequest {
        append: append.into(),
        evidence: core_evidence(
            evidence
                .iter()
                .map(|pointer| umx_protocol::EvidencePointer {
                    pointer: pointer.clone(),
                    kind,
                })
                .collect(),
        ),
    })
}

fn batch_op(op: &umx_protocol::BatchOp) -> BatchOp {
    match op.clone() {
        umx_protocol::BatchOp::NodeAdd {
            id,
            created,
            ref_name,
            kind,
            title,
            status,
            note,
            sets,
            parent,
            parent_ref,
            evidence,
            evidence_kind,
            raised_in,
            on_key,
            tags,
        } => BatchOp::NodeAdd {
            id,
            created,
            ref_name,
            req: NodeAddRequest {
                kind,
                title,
                status,
                note,
                sets: core_sets(&sets),
                parent,
                evidence: core_evidence(evidence.into_iter().map(|e| e.0).collect()),
                evidence_kind,
                locator: None,
                raised_in,
                on_key_resolve: on_key.is_some(),
                tags,
            },
            parent_ref,
        },
        umx_protocol::BatchOp::EdgeAdd {
            id,
            from,
            from_ref,
            relation,
            to,
            to_ref,
            sets,
            evidence,
            evidence_kind,
        } => BatchOp::EdgeAdd {
            id,
            from,
            from_ref,
            relation,
            to,
            to_ref,
            sets: core_sets(&sets),
            evidence: EdgeEvidence {
                pointers: evidence,
                kind: evidence_kind,
            },
        },
        umx_protocol::BatchOp::NodeEdit {
            node,
            node_ref,
            title,
            kind,
            note,
            sets,
            unsets,
            tag,
            untag,
        } => BatchOp::NodeEdit {
            node,
            node_ref,
            req: NodeEditRequest {
                title,
                kind,
                note,
                sets: core_sets(&sets),
                unsets,
                locator: None,
                tag,
                untag,
            },
        },
        umx_protocol::BatchOp::NodeNote {
            node,
            node_ref,
            append,
            evidence,
        } => BatchOp::NodeNote {
            node,
            node_ref,
            req: NodeNoteRequest {
                append,
                evidence: core_evidence(evidence),
            },
        },
        umx_protocol::BatchOp::NodeStatus {
            node,
            node_ref,
            status,
            reason,
            label,
            place,
            evidence,
            evidence_kind,
        } => BatchOp::NodeStatus {
            node,
            node_ref,
            req: NodeStatusRequest {
                status,
                reason,
                label,
                anchor: place,
                evidence,
                evidence_kind,
            },
        },
        umx_protocol::BatchOp::EdgeDelete { edge } => BatchOp::EdgeDelete { edge },
        umx_protocol::BatchOp::EdgeEdit {
            edge,
            relation,
            reverse,
            sets,
            unsets,
        } => BatchOp::EdgeEdit {
            edge,
            req: EdgeEditRequest {
                relation,
                reverse,
                sets: core_sets(&sets),
                unsets,
            },
        },
        umx_protocol::BatchOp::EvidenceAdd {
            node,
            node_ref,
            pointer,
            evidence_kind,
        } => BatchOp::EvidenceAdd {
            node,
            node_ref,
            pointer,
            evidence_kind,
        },
        umx_protocol::BatchOp::EvidenceRemove {
            node,
            node_ref,
            pointer,
            reason,
            at,
            missing,
        } => BatchOp::EvidenceRemove {
            node,
            node_ref,
            pointer,
            reason,
            at,
            skip_missing: missing == Some(umx_protocol::Missing::Skip),
        },
    }
}

fn evidence_head(alias: &str, id: Id) -> Value {
    json!({ "graph": alias, "id": id.to_text(), "address": umx_services::address(alias, id) })
}

/// `umx node evidence <node> list [--removed]`: a read.
fn evidence_list(
    services: &Services,
    ctx: &Ctx,
    command: &str,
    node: &str,
    removed: bool,
) -> Envelope {
    reply(
        command,
        services.node_evidence_list(ctx, node, removed),
        |v| {
            let mut out = evidence_head(&v.alias, v.id);
            out["removed"] = json!(v.removed);
            out["rows"] = v.rows.iter().map(evidence_row).collect();
            out
        },
    )
}

/// One evidence row as the envelope shows it: position, text in canonical form, and the stored object.
fn evidence_row(r: &umx_services::EvidenceRow) -> Value {
    json!({ "at": r.at, "pointer": r.text, "evidence": r.pointer })
}

/// The daemon checks what the CLI checked (R103-2): `--redact` (F183) goes with remove only, and scrubs every pointer
/// with the source, so `--at` (one of several) means nothing to it.
fn redact_misuse(
    action: umx_protocol::EvidenceAction,
    at: Option<usize>,
    redact: bool,
) -> Option<Problem> {
    let why = match (redact, action, at) {
        (false, ..) | (true, umx_protocol::EvidenceAction::Remove, None) => return None,
        (true, umx_protocol::EvidenceAction::Remove, Some(_)) => {
            "--redact scrubs every pointer with that source; drop --at"
        }
        (true, ..) => "--redact goes with remove only",
    };
    Some(umx_services::problems::usage_invalid(why, "evidence"))
}

/// `umx node evidence <node> list|add|remove|refresh` (req-126; refresh usage 220). A list is a read; add, remove and
/// refresh reach here on the writer.
/// `remove --redact` (F183) scrubs every pointer in the graph with that source and replies with counts only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn node_evidence(
    services: &mut Services,
    ctx: &Ctx,
    command: &str,
    node: &str,
    action: umx_protocol::EvidenceAction,
    pointer: Option<&str>,
    evidence_kind: Option<&str>,
    reason: Option<&str>,
    (at, removed, redact, all): (Option<usize>, bool, bool, bool),
) -> Envelope {
    use umx_protocol::EvidenceAction;
    if let Some(problem) = redact_misuse(action, at, redact) {
        return Envelope::fail(command, issue(&problem, command));
    }
    if all && action != EvidenceAction::Refresh {
        let problem =
            umx_services::problems::usage_invalid("--all goes with refresh only", "evidence");
        return Envelope::fail(command, issue(&problem, command));
    }
    let p = match (action, pointer) {
        (EvidenceAction::List, _) => {
            return evidence_list(services, ctx, command, node, removed);
        }
        (EvidenceAction::Refresh, _) => {
            let pick = match (pointer, at, all) {
                (Some(p), None, false) => Some(umx_services::RefreshPick::Pointer(p)),
                (None, Some(a), false) => Some(umx_services::RefreshPick::At(a)),
                (None, None, true) => Some(umx_services::RefreshPick::All),
                (None, None, false) => None,
                _ => {
                    let problem = umx_services::problems::usage_invalid(
                        "refresh takes one of a pointer, --at <n> or --all",
                        "evidence",
                    );
                    return Envelope::fail(command, issue(&problem, command));
                }
            };
            return reply(
                command,
                services.node_evidence_refresh(ctx, node, pick),
                |v| {
                    let mut out = evidence_head(&v.alias, v.id);
                    out["rows"] = v
                        .rows
                        .iter()
                        .map(|r| {
                            let mut row = evidence_row(&r.row);
                            row["changed"] = json!(r.changed);
                            row["read"] = json!(r.read);
                            row
                        })
                        .collect();
                    out["changed"] = json!(v.rows.iter().filter(|r| r.changed).count());
                    out["hashed"] = json!(v.hashed);
                    out
                },
            );
        }
        (_, Some(p)) => p,
        (_, None) => {
            let problem = umx_services::problems::usage_invalid(
                "name the pointer after add or remove",
                "evidence",
            );
            return Envelope::fail(command, issue(&problem, command));
        }
    };
    if action == EvidenceAction::Add {
        return reply(
            command,
            services.node_evidence_add(ctx, node, p, evidence_kind),
            |v| {
                let mut out = evidence_head(&v.alias, v.id);
                out["added"] = json!(v.added);
                out["row"] = evidence_row(&v.row);
                out
            },
        );
    }
    let done = if redact {
        services.node_evidence_redact(ctx, node, p, reason)
    } else {
        services.node_evidence_remove(ctx, node, p, reason, at)
    };
    reply(command, done, |v| {
        let mut out = evidence_head(&v.alias, v.id);
        out["removed"] = v.removed.as_ref().map_or(Value::Null, evidence_row);
        // F183: counts only, never the pointer or the ids it touched.
        if let Some(r) = v.redacted {
            out["redacted"] = json!({ "pointers": r.pointers, "nodes": r.nodes, "edges": r.edges });
        }
        out
    })
}

/// The batch caps on what arrived, as the CLI checks them on the file (review R103-2): the daemon trusts nothing the
/// CLI checked. A line's size is its op as compact JSON; over the op count or the total is `batch_too_large`, an
/// over-long line `batch_line_invalid` at its line. Checked before anything is planned or applied.
fn batch_caps(lines: &[umx_protocol::BatchLine]) -> Result<(), Vec<Problem>> {
    use umx_core::batch::{MAX_BATCH_BYTES, MAX_BATCH_ERRORS, MAX_BATCH_LINE, MAX_BATCH_OPS};
    use umx_services::problems;
    if lines.len() > MAX_BATCH_OPS {
        return Err(vec![problems::batch_too_large(
            lines.len() as u64,
            "ops",
            MAX_BATCH_OPS as u64,
        )]);
    }
    let (mut total, mut long) = (0u64, Vec::new());
    for l in lines {
        let bytes = serde_json::to_vec(&l.op).map_or(usize::MAX, |b| b.len());
        total = total.saturating_add(bytes as u64 + 1);
        if bytes > MAX_BATCH_LINE {
            long.push(l.line);
        }
    }
    if total > MAX_BATCH_BYTES {
        return Err(vec![problems::batch_too_large(
            total,
            "bytes",
            MAX_BATCH_BYTES,
        )]);
    }
    if long.is_empty() {
        return Ok(());
    }
    let mut all = vec![problems::batch_failed(long.len())];
    all.extend(
        long.iter()
            .take(MAX_BATCH_ERRORS)
            .map(|&line| problems::batch_line_invalid(line, "the line is over 1 MiB")),
    );
    if long.len() > MAX_BATCH_ERRORS {
        all.push(problems::batch_errors_truncated(
            long.len(),
            MAX_BATCH_ERRORS,
        ));
    }
    Err(all)
}

/// Every problem of a refused batch as errors, the first deciding the exit code; a hint (plan 205's
/// `batch_use_ref_field`) goes to `hints`, leaving status and the exit code to the errors.
pub(crate) fn batch_errors(command: &str, problems: &[Problem]) -> Envelope {
    let (hints, errors): (Vec<&Problem>, Vec<&Problem>) = problems
        .iter()
        .partition(|p| umx_services::problems::is_hint(p.code));
    let mut all = errors.into_iter().map(|p| issue(p, command));
    let first = all.next().unwrap_or_else(|| internal("batch_no_error"));
    let mut env = Envelope::fail(command, first);
    env.errors.extend(all);
    hints
        .into_iter()
        .fold(env, |env, h| env.hint(issue(h, command)))
}

/// `umx batch` (req-88): the result in 007's list shape, one row per op line; a failed batch lists `batch_failed`,
/// then each failed line's error with its line. `imports` are `--refs-from`'s refs and graph id, mapped as given:
/// the services check them.
#[allow(clippy::too_many_arguments)]
fn batch(
    services: &mut Services,
    ctx: &Ctx,
    command: &str,
    lines: &[umx_protocol::BatchLine],
    dry_run: bool,
    (reason, raised_in): (Option<String>, Option<String>),
    echo_sentences: bool,
    imports: (&std::collections::BTreeMap<String, String>, &Option<String>),
    idempotency: Option<Idempotency>,
) -> Envelope {
    // R236-N1: with --reason, every dry run the reply names keeps it (as a placeholder).
    let had_reason = reason.is_some();
    let shown = |p: &Problem| {
        if had_reason {
            umx_services::problems::with_batch_reason(p.clone())
        } else {
            p.clone()
        }
    };
    let shown_all = |ps: &[Problem]| ps.iter().map(shown).collect::<Vec<_>>();
    if let Err(problems) = batch_caps(lines) {
        return batch_errors(command, &shown_all(&problems));
    }
    let req = BatchRequest {
        lines: lines
            .iter()
            .map(|l| BatchLine {
                line: l.line,
                op: batch_op(&l.op),
            })
            .collect(),
        dry_run,
        reason,
        echo_sentences,
        refs: imports.0.clone(),
        refs_graph: imports.1.clone(),
        raised_in,
        idempotency,
    };
    match services.batch(ctx, req) {
        Ok(done) => {
            let v = &done.value;
            let id = |id: Option<Id>| id.map(Id::to_text);
            let rows: Vec<Value> = v
                .rows
                .iter()
                .map(|r| {
                    let mut row = json!({
                        "line": r.line, "op": r.op, "ref": r.ref_name, "id": id(r.id),
                        "address": r.id.map(|i| umx_services::address(&v.alias, i)),
                        "parent_edge": id(r.parent_edge), "changed": r.changed, "existed": r.existed,
                        "raised_in_edge": id(r.raised_in_edge), "raised_in": r.raised_in,
                        "related": null,
                    });
                    // PRD 003 hubs: only on a line whose node umx added to a hub.
                    if !r.hub_edges.is_empty() {
                        row["hub_edges"] = json!(r.hub_edges.iter().map(|e| e.to_text()).collect::<Vec<_>>());
                    }
                    row
                })
                .collect();
            let refs: serde_json::Map<String, Value> = v
                .refs
                .iter()
                .map(|(name, i)| (name.clone(), json!(id(*i))))
                .collect();
            let mut data = list(
                &[
                    "line",
                    "op",
                    "ref",
                    "id",
                    "address",
                    "parent_edge",
                    "changed",
                    "existed",
                    "raised_in_edge",
                    "raised_in",
                    "related",
                ],
                rows,
                None,
            );
            data["graph"] = json!(v.alias);
            data["graph_id"] = json!(v.graph_id.to_text());
            data["dry_run"] = json!(v.dry_run);
            data["applied"] = json!(!v.dry_run);
            data["related_floor"] = Value::Null;
            data["refs"] = Value::Object(refs);
            // ws003 s09: what a receipt keeps, on every reply, so a replay's receipt equals its first run's.
            data["counts"] = json!(v.counts);
            if let Some(k) = &v.keyed {
                data["idempotency_key"] = json!(k.key);
                data["replayed"] = json!(k.replayed);
                data["recorded_at"] = json!(k.recorded_at.map(umx_core::receipts::time_text));
            }
            if let Some(sentences) = &v.sentences {
                // F234: a sentence holding a flagged title is named `field:sentence` on its own record.
                data["sentences"] = json!(sentences.iter().map(|s| {
                    with_flags(json!({
                        "line": s.line, "op": s.op, "id": id(s.id), "sentence": s.sentence,
                    }), &flag_if(s.sentence_flagged, "sentence"), false)
                }).collect::<Vec<_>>());
                data["sentences_omitted"] = json!(v.sentences_omitted);
            }
            done.notes
                .iter()
                .fold(Envelope::ok(command, data), |env, w| {
                    env.note(issue(&shown(w), command))
                })
        }
        Err(problems) => batch_errors(command, &shown_all(&problems)),
    }
}

/// `--model` as a key, or the usage error to print.
fn reembed_model(command: &str, model: &Option<String>) -> Result<Option<ModelKey>, Box<Envelope>> {
    match model.as_deref().map(ModelKey::parse) {
        None => Ok(None),
        Some(Ok(key)) => Ok(Some(key)),
        Some(Err(_)) => Err(Box::new(Envelope::fail(
            command,
            issue(
                &umx_services::problems::usage_invalid(
                    "--model must be a model key as umx docs get embeddings shows it (the model in lower-case letters and digits and . _ - then dims and an optional +pN)",
                    "embeddings",
                ),
                command,
            ),
        ))),
    }
}

fn reembed_json(v: &umx_services::Reembed) -> Value {
    json!({
        "graph": v.alias, "from": v.from.as_ref().map(|k| k.to_string()), "to": v.to.to_string(),
        "locality": if v.local { "local" } else { "hosted" }, "nodes": v.nodes, "texts": v.texts,
        "estimated_tokens": v.estimated_tokens, "whole_node_texts_changed": v.whole_node_texts_changed,
        "field_vectors_dropped": v.field_vectors_dropped, "dry_run": v.dry_run, "queued": v.queued,
    })
}

/// The read half of `umx embeddings reembed` (architect F1): run under the shared lock, so reading every shard of
/// the target model never holds the writer. Err is the envelope to print.
pub fn reembed_plan(
    services: &Services,
    request: &Request,
) -> Result<umx_services::ReembedPlan, Box<Envelope>> {
    let Some(Op::EmbeddingsReembed { model, .. }) = &request.op else {
        return Err(Box::new(Envelope::fail(
            &request.command,
            internal("not_a_reembed"),
        )));
    };
    let command = "embeddings reembed";
    let model = reembed_model(command, model)?;
    services
        .reembed_plan(&ctx(request), model.as_ref())
        .map_err(|p| Box::new(Envelope::fail(command, issue(&p, command))))
}

/// The writer half of `umx embeddings reembed`: checks again, records the key and queues the planned nodes.
pub fn reembed_apply(services: &mut Services, plan: umx_services::ReembedPlan) -> Envelope {
    reply(
        "embeddings reembed",
        services.reembed_apply(plan),
        reembed_json,
    )
}

/// Runs a read under the shared lock.
/// The first step of `umx search`, under the shared lock: which graphs, and which get a vector leg. The daemon then
/// embeds the query holding no lock, and finishes with [`search_finish`].
pub fn search_plan(
    services: &Services,
    request: &Request,
    allows: &dyn Fn(&umx_core::ports::GraphDir, &str) -> bool,
) -> Result<umx_services::SearchPlan, Box<Envelope>> {
    if let Some(problem) = scope_usage(request) {
        return Err(Box::new(Envelope::fail(
            "search",
            issue(&problem, "search"),
        )));
    }
    let Some(Op::Search {
        text,
        within,
        limit,
        along,
        also,
        tags,
        no_tags,
    }) = &request.op
    else {
        return Err(Box::new(Envelope::fail(
            &request.command,
            internal("not_a_search"),
        )));
    };
    // PRD 002 search-09: checked again on arrival; the refusal names a phrasing by position, never its text.
    if let Some(what) = umx_protocol::also_problem(text, also) {
        return Err(Box::new(Envelope::fail(
            "search",
            issue(
                &umx_services::problems::usage_invalid(what, "search"),
                "search",
            ),
        )));
    }
    let ctx = ctx(request);
    // --along reads one graph: refused before any graph is chosen (D71 step 12, architect).
    if along.is_some()
        && let Err(p) = umx_services::along_one_graph(&ctx)
    {
        return Err(Box::new(Envelope::fail("search", issue(&p, "search"))));
    }
    services
        .search_plan_allowing(&ctx, text, within.as_deref(), limit.unwrap_or(10), allows)
        .and_then(|plan| plan.along(&ctx, along_request(along)))
        .and_then(|plan| plan.tags(services, &ctx, tags, no_tags))
        .map(|plan| plan.also(also))
        .map_err(|p| Box::new(Envelope::fail("search", issue(&p, "search"))))
}

/// The last step of `umx search`, under the shared lock again. When `--limit` cut the list, the hint
/// `search_results_cut` gives the command that shows every match found (PRD 002 search-13).
pub fn search_finish(
    services: &Services,
    request: &Request,
    plan: umx_services::SearchPlan,
    queries: umx_services::QueryVectors,
) -> Envelope {
    let outcome = services.search_finish(plan, queries);
    let cut = match &outcome {
        Ok(done) if done.value.found > done.value.hits.len() => {
            Some((done.value.hits.len(), done.value.found))
        }
        _ => None,
    };
    let weak = matches!(&outcome, Ok(done) if done.value.no_strong_match);
    let env = reply("search", outcome, |r| search_json(services, r));
    let env = match cut {
        Some((shown, found)) => env.note(issue(
            &umx_services::problems::search_results_cut(shown, found, search_rest(request, found)),
            "search",
        )),
        None => env,
    };
    if weak {
        env.note(issue(
            &umx_services::problems::no_strong_match(search_reworded(request)),
            "search",
        ))
    } else {
        env
    }
}

/// search-10: `no_strong_match`'s next: the search as the caller sent it (the words a placeholder, one `--also
/// <words>` per phrasing sent, every selector and its `--limit`), with one more `--also <words>` while fewer than 5
/// phrasings were sent, so the caller rewords or adds a phrasing; never a phrasing's text.
fn search_reworded(request: &Request) -> Command {
    let mut request = request.clone();
    if let Some(Op::Search { also, .. }) = &mut request.op
        && also.len() < umx_protocol::MAX_ALSO
    {
        also.push(String::new());
    }
    search_again(&request)
}

/// search-13: the search as the caller sent it with `--limit <found>`: [`redirected_retry`] with no redirects (the
/// words as a placeholder, and one `--also <words>` per phrasing sent, search-09; `--within`, `--along` and the
/// graphs as named: `--graph`, `--scope` or `--all`), then `--include-named` and `--provenance` when given, so
/// following it ranks the same matches and shows them all.
fn search_rest(request: &Request, found: usize) -> Command {
    let mut request = request.clone();
    if let Some(Op::Search { limit, .. }) = &mut request.op {
        *limit = Some(found);
    }
    search_again(&request)
}

/// A search's request as a runnable command: [`redirected_retry`] with no redirects, then `--include-named` and
/// `--provenance` when given.
fn search_again(request: &Request) -> Command {
    let c =
        redirected_retry(request, &[]).unwrap_or_else(|| Command::new("search").word("<words>"));
    let c = if request.all && request.include_named {
        c.word("--include-named")
    } else {
        c
    };
    if request.include_provenance {
        c.word("--provenance")
    } else {
        c
    }
}

/// A hybrid search's rows: each hit's graph and policy, the node, which legs found it, the exact field, and each
/// leg's rank, score and field; with the floor each vector space was cut at, whether it is calibrated, where it came
/// from (`calibrated`, `config` or `none`) and its calibration run (ws004 s26). A hit whose title umx's secret check
/// flags carries `secret_suspected: ["field:title"]` (F228), and `field:word_terms` too when it has word terms: a
/// matched term is the node's whole word, which a prefix query spells out, and the terms do not say which field they
/// came from, so all of them are flagged (R398-1). Each row's `text` is a cut of the field named in `text_field`
/// (PRD 002 search-07): graph data, listed untrusted, and flagged `field:text` when that field's value is. `links`
/// holds up to 3 of the hit's stored links in its own graph (relation, direction, the other node's id and title) and
/// `links_more` how many more (PRD 002 search-08); each title is listed untrusted, and a flagged one flags
/// `field:links`.
fn search_json(services: &Services, result: &umx_services::HybridResult) -> Value {
    let titles: Vec<_> = result
        .hits
        .iter()
        .map(|h| (GraphKey::Alias(&h.alias), h.node.id, h.node.title.as_str()))
        .collect();
    let rows: Vec<Value> = result
        .hits
        .iter()
        .zip(title_flags(services, &titles))
        .map(|(h, mut flags)| {
            if !flags.is_empty() && h.word.as_ref().is_some_and(|w| !w.terms.is_empty()) {
                flags.push(umx_core::place::named_place("word_terms"));
            }
            // search-07: the cut of the field that found the hit. Flagged when that field's whole value is, by the
            // rule node get uses (a secret cut in half is still flagged); --human then hides it (F224).
            let text = h.text();
            if let Some((field, _)) = &text
                && !services
                    .secret_flags(
                        &[(h.alias.as_str(), &h.node)],
                        Some(std::slice::from_ref(field)),
                    )
                    .concat()
                    .is_empty()
            {
                flags.push(umx_core::place::named_place("text"));
            }
            // search-08: up to 3 stored links in the hit's own graph and a count of the rest. Each other end's title is
            // graph data, flagged by the rule every read applies to a title it shows (F228); one flagged title flags
            // `field:links`, which --human hides whole.
            let links = services.hit_links(&h.alias, h.node.id).unwrap_or_default();
            let ends: Vec<_> = links
                .shown
                .iter()
                .map(|l| (GraphKey::Alias(&h.alias), l.id, l.title.as_str()))
                .collect();
            if services.titles_flagged(&ends).into_iter().any(|on| on) {
                flags.push(umx_core::place::named_place("links"));
            }
            let mut row = json!({
                "graph": h.alias,
                "address": umx_services::address(&h.alias, h.node.id),
                "policy": h.policy.name(),
                "id": h.node.id.to_text(),
                "type": h.node.kind,
                "title": h.node.title,
                "status": h.node.status,
                "legs": h.legs(),
                "phrasings": h.phrasings,
                "exact": h.exact,
                "word_rank": h.word.as_ref().map(|w| w.rank),
                "bm25": h.word.as_ref().map(|w| w.score),
                "word_field": h.word.as_ref().map(|w| w.field.as_str()),
                "coverage": h.word.as_ref().map(|w| w.coverage),
                "word_terms": h.word.as_ref().map(|w| {
                    w.terms
                        .iter()
                        .map(|t| json!({ "query": t.query, "term": t.term, "form": t.form }))
                        .collect::<Vec<_>>()
                }),
                "vector_rank": h.vector.as_ref().map(|v| v.rank),
                "cosine": h.vector.as_ref().map(|v| v.cosine),
                "vector_field": h.vector.as_ref().map(|v| v.field.as_str()),
                "fused": h.fused,
                "text_field": text.as_ref().map(|(field, _)| field),
                "text": text.as_ref().map(|(_, cut)| cut),
                "links": links
                    .shown
                    .iter()
                    .map(|l| json!({
                        "relation": l.relation,
                        "direction": l.way.as_str(),
                        "id": l.id.to_text(),
                        "title": l.title,
                    }))
                    .collect::<Vec<_>>(),
                "links_more": links.more,
            });
            if result.copies {
                row["copy"] = json!(h.alias);
            }
            // search-10: how strongly it matches, only when the search labelled its hits.
            if result.labelled {
                row["strength"] = json!(h.strength.map(|s| s.word()));
            }
            // search-09: which phrasings found it, only when --also gave more than one.
            if result.phrasings < 2
                && let Some(row) = row.as_object_mut()
            {
                row.remove("phrasings");
            }
            with_flags(row, &flags, false)
        })
        .collect();
    let mut columns = vec![
        "graph",
        "address",
        "policy",
        "id",
        "type",
        "title",
        "status",
        "legs",
        "exact",
        "word_rank",
        "bm25",
        "word_field",
        "coverage",
        "word_terms",
        "vector_rank",
        "cosine",
        "vector_field",
        "fused",
        "text_field",
        "text",
        "links",
        "links_more",
    ];
    if result.phrasings > 1 {
        columns.insert(8, "phrasings");
    }
    if result.labelled {
        let at = columns.iter().position(|c| *c == "legs").expect("legs") + 1;
        columns.insert(at, "strength");
    }
    if any_flagged(&rows) {
        columns.push(SECRET_SUSPECTED);
    }
    let mut data = list(&columns, rows, None);
    data["word_leg"] = json!(result.word_leg);
    data["floors"] = result
        .floors
        .iter()
        .map(|(key, floor)| {
            let mut f = json!({
                "model": key.to_string(), "floor": floor.value, "calibrated": floor.calibrated(),
                "floor_source": floor.source.word(), "floor_run": floor.run,
            });
            // search-10: the run of the strength cuts that labelled this space's hits, when it has them.
            if let Some((_, run)) = result.strength_runs.iter().find(|(k, _)| k == key) {
                f["strength_run"] = json!(run);
            }
            f
        })
        .collect();
    with_along(&mut data, result.along.as_ref());
    data
}

/// ws006 S23: one `alias_redirected` hint for each former alias the request named and the live alias used for it.
fn with_redirects(
    mut env: Envelope,
    redirected: &umx_services::Redirected,
    request: &Request,
) -> Envelope {
    let pairs = redirected.borrow();
    let fix = redirected_retry(request, &pairs);
    let ours: Vec<_> = pairs
        .iter()
        .map(|(former, live)| {
            issue(
                &umx_services::problems::alias_redirected(former, live, fix.clone()),
                &env.command,
            )
        })
        .collect();
    // One hint per redirect, the request's own first (review R18): an evidence pointer through the same former
    // alias says the same, and repeats must not take the bounded hint places.
    let earlier = core::mem::replace(&mut env.hints, ours);
    env.hints.extend(earlier);
    let mut seen = Vec::new();
    env.hints.retain(|h| {
        h.code() != "alias_redirected" || {
            let fresh = !seen.contains(&h.message().to_string());
            seen.push(h.message().to_string());
            fresh
        }
    });
    env
}

/// The command an `alias_redirected` hint gives next: the request as it was, with each former alias it gave read as
/// the live one (review R19): the positional alias, each name of `--graph` (the others kept), a `--scope` or
/// `--all` kept, and a redirected `UMX_GRAPH` as `--graph`. A retry shows addresses as placeholders, so none of them
/// carries a former alias.
fn redirected_retry(request: &Request, pairs: &[(String, String)]) -> Option<Command> {
    let live = |a: &str| {
        pairs
            .iter()
            .find(|(former, _)| former == a)
            .map_or(a, |(_, live)| live.as_str())
            .to_string()
    };
    let mut op = request.op.clone()?;
    if let Some(alias) = positional_alias(&mut op) {
        *alias = live(alias);
    }
    let c = retry(&op);
    let graphs = |c: Command, list: &str| {
        let mut seen = Vec::new();
        let names: Option<Vec<ValidName>> = list
            .split(',')
            .map(|a| live(a.trim()))
            .filter(|a| {
                let fresh = !seen.contains(a);
                seen.push(a.clone());
                fresh
            })
            .map(|a| ValidName::alias(&a))
            .collect();
        match names {
            Some(names) if (1..=64).contains(&names.len()) => c.word("--graph").names(&names, ","),
            _ => c.word("--graph").word("<graphs>"),
        }
    };
    Some(if let Some(list) = &request.graph {
        graphs(c, list)
    } else if let Some(scope) = &request.scope {
        match ValidName::new(scope) {
            Some(scope) => c.word("--scope").name(&scope),
            None => c.word("--scope").word("<scope>"),
        }
    } else if request.all {
        c.word("--all")
    } else {
        match &request.default_graph {
            Some(d) if pairs.iter().any(|(former, _)| former == d) => graphs(c, d),
            _ => c,
        }
    })
}

/// Runs a read on a reader thread.
pub fn read(services: &Services, request: &Request) -> Envelope {
    let ctx = ctx(request);
    let redirected = ctx.redirected.clone();
    with_redirects(read_in(services, request, ctx), &redirected, request)
}

fn read_in(services: &Services, request: &Request, ctx: Ctx) -> Envelope {
    let Some(op) = &request.op else {
        return Envelope::fail(&request.command, internal("no_operation"));
    };
    let command = op.command();
    let live = match with_live_alias(services, &ctx, op) {
        Ok(live) => live,
        Err(p) => return Envelope::fail(command, issue(&p, command)),
    };
    let op = live.as_ref().unwrap_or(op);
    if let Some(problem) = scope_usage(request) {
        return Envelope::fail(command, issue(&problem, command));
    }
    match op {
        Op::HubList => reply(
            command,
            services.hub_list(&ctx),
            |v| json!({"graph": v.graph, "total": v.rows.len(), "rows": hub_rows_json(&v.rows)}),
        ),
        Op::HubShow {
            name,
            limit,
            offset,
        } => reply(
            command,
            services.hub_show(
                &ctx,
                name,
                limit.unwrap_or(DEFAULT_LIMIT),
                offset.unwrap_or(0),
            ),
            |v| {
                json!({"graph": v.graph, "hub": hub_json(&v.hub),
                    "nodes": v.nodes.iter().map(|n| json!({"node": n.node, "members": n.members,
                        "members_total": n.members_total})).collect::<Vec<_>>(),
                    "nodes_total": v.nodes_total, "offset": v.offset, "limit": v.limit,
                    "next": v.next.as_ref().map(|c| c.to_string()), "candidates": v.candidates,
                    "candidates_complete": v.candidates_complete, "add": v.add.to_string()})
            },
        ),
        Op::RuleList => reply(command, services.rule_list(&ctx), |v| {
            json!({"graph": v.graph, "total": v.rows.len(), "rows": v.rows.iter().map(|r|
                json!({"rule": r.rule, "source": r.source, "off": r.off,
                    "sentence": r.rule.sentence().as_str()})).collect::<Vec<_>>()})
        }),
        Op::RuleShow {
            name,
            limit,
            offset,
        } => reply(
            command,
            services.rule_show(
                &ctx,
                name,
                limit.unwrap_or(DEFAULT_LIMIT),
                offset.unwrap_or(0),
            ),
            rule_shown_json,
        ),
        Op::ScopeList => reply(command, services.scope_list(), |scopes| {
            list(
                &["name", "members"],
                scopes.iter().map(scope_json).collect(),
                None,
            )
        }),
        Op::ScopeShow { name } => reply(command, services.scope_show(name), scope_json),
        Op::EntityShow { node } => reply(command, services.entity_show(&ctx, node), entity_json),
        Op::Who { who, limit } => reply(
            command,
            services.who(&ctx, who, limit.unwrap_or(DEFAULT_LIMIT)),
            who_json,
        ),
        Op::NodeWho { node } => reply(command, services.node_who(&ctx, node), node_who_json),
        Op::EmbeddingsReembed { dry_run: true, .. } => match reembed_plan(services, request) {
            Ok(plan) => Envelope::ok(command, reembed_json(&plan.cost)),
            Err(env) => *env,
        },
        Op::GraphList {
            graphfile: Some(folder),
            ..
        } => match services.graph_list_graphfile(folder) {
            Ok(listing) => Envelope::ok(command, graphfile_list_json(&listing)),
            Err(p) => Envelope::fail(command, issue(&p, command)),
        },
        Op::GraphScan {
            path,
            add,
            rewrite,
            drop,
            depth,
            whole_repo,
        } => {
            let req = ScanRequest {
                add: *add,
                rewrite: *rewrite,
                drop: drop.clone(),
                depth: depth.map(|d| d as usize),
                whole_repo: *whole_repo,
            };
            reply(command, services.graph_scan(&ctx, path, &req), scan_json)
        }
        Op::GraphList {
            all: true,
            graphfile: None,
        } => graph_index(services, None, request),
        Op::GraphList {
            all: false,
            graphfile: None,
        } => {
            let rows: Vec<Value> = services
                .graph_list_in(&ctx)
                .into_iter()
                .map(|g| {
                    json!({
                        "alias": g.alias, "id": g.id.to_text(), "path": g.path, "nodes": g.nodes, "edges": g.edges,
                        "kept_lines": g.kept_lines, "last_used": g.last_used, "unloads_in_s": g.unloads_in_s,
                        "embedding": embedding_json(g.embedding.as_ref()),
                        "effective_policy": g.policy.effective.name(),
                            "effective_visibility": g.policy.effective.visibility.as_str(),
                            "effective_embedder": g.policy.effective.embedder.as_str(), "downgraded": g.policy.downgraded_at,
                        "vectors_pending": g.vectors_pending, "vectors_held": g.vectors_held,
                    })
                })
                .collect();
            let columns = [
                "alias",
                "id",
                "path",
                "nodes",
                "edges",
                "kept_lines",
                "last_used",
                "unloads_in_s",
                "embedding",
                "effective_policy",
                "effective_visibility",
                "effective_embedder",
                "downgraded",
                "vectors_pending",
                "vectors_held",
            ];
            Envelope::ok(command, list(&columns, rows, None))
        }
        Op::ResourceShow { node } => reply(
            command,
            services.resource_show(&ctx, node),
            |v| json!({ "resource": node_json(&v.alias, &v.node) }),
        ),
        Op::NodeGet { node, columns } => {
            reply(command, services.node_get(&ctx, node), |(alias, n)| {
                // F137: `--columns all` is node list's default columns plus the node's custom fields, sorted.
                let columns: Option<Vec<String>> = if columns_all(columns.as_deref()) {
                    let mut all: Vec<String> = NODE_COLUMNS.map(String::from).to_vec();
                    all.extend(node_custom_columns(n.fields.keys().map(String::as_str)));
                    Some(all)
                } else {
                    columns.clone()
                };
                // F188: what umx's own secret scan flags among the values shown, by name; the virtual graph node's
                // title too (R324-2).
                let flags = services.secret_flags(&[(alias.as_str(), n)], columns.as_deref());
                let value = if services.is_virtual_graph_node(alias, n.id) {
                    virtual_node_json(alias, n, columns.as_deref())
                } else {
                    match &columns {
                        Some(cols) => pick(
                            alias,
                            n.id,
                            |name| n.field(name),
                            || n.locator.clone(),
                            cols,
                        ),
                        None => node_json(alias, n),
                    }
                };
                let value = with_flags(value, &flags[0], asks_flags(columns.as_deref()));
                json!({ "node": value })
            })
        }
        Op::NodeList {
            kind,
            status,
            wheres,
            within,
            columns,
            limit,
            offset,
            along,
            tags,
            no_tags,
        } => {
            let req = NodeListRequest {
                kind: kind.clone(),
                status: status.clone(),
                wheres: wheres.clone(),
                within: within.clone(),
                limit: limit.unwrap_or(DEFAULT_LIMIT),
                offset: offset.unwrap_or(0),
                along: along_request(along),
                tags: tags.clone(),
                no_tags: no_tags.clone(),
                several: false,
            };
            // F137: `--columns all` starts from the default columns, whatever the type's own (F96).
            let all = columns_all(columns.as_deref());
            // F96: `--type` with no `--columns` shows the type's own default columns when it has some.
            let cols: Vec<String> = if all {
                NODE_COLUMNS.map(String::from).to_vec()
            } else {
                columns
                    .clone()
                    .or_else(|| {
                        kind.as_deref()
                            .and_then(|k| services.node_type_columns(&ctx, k, within.as_deref()))
                    })
                    .unwrap_or_else(|| NODE_COLUMNS.map(String::from).to_vec())
            };
            reply(command, services.node_list(&ctx, req), |page| {
                let mut cols = cols;
                if let Some(graphs) = &page.graphs {
                    for column in ["graph", "address"]
                        .into_iter()
                        .chain(graphs.copies.then_some("copy"))
                    {
                        if !cols.iter().any(|c| c == column) {
                            cols.push(column.to_string());
                        }
                    }
                }
                // F137: then every custom field on the page, sorted; a row without one has null (pick).
                if all {
                    cols.extend(node_custom_columns(
                        page.rows.iter().flat_map(|n| n.custom_field_names()),
                    ));
                }
                let aliases: Vec<&str> = (0..page.rows.len())
                    .map(|i| {
                        page.graphs
                            .as_ref()
                            .map_or(page.alias.as_str(), |g| g.rows[i].as_str())
                    })
                    .collect();
                // F188: each row's flags cover the columns it returns; the column shows only when a row has some.
                let shown: Vec<(&str, umx_services::ListedNode<'_>)> = aliases
                    .iter()
                    .copied()
                    .zip(page.rows.iter().copied())
                    .collect();
                let flags = services.listed_secret_flags(&shown, &cols);
                let explicit = asks_flags(Some(&cols));
                let mut rows: Vec<Value> = page
                    .rows
                    .iter()
                    .zip(&aliases)
                    .zip(&flags)
                    .map(|((n, alias), flags)| {
                        let mut row =
                            pick(alias, n.id(), |name| n.field(name), || n.locator(), &cols);
                        if page.graphs.as_ref().is_some_and(|g| g.copies) {
                            row["copy"] = json!(alias);
                        }
                        with_flags(row, flags, explicit)
                    })
                    .collect();
                spine_cells(services, &cols, &aliases, &page.rows, &mut rows);
                let mut names: Vec<&str> = cols.iter().map(String::as_str).collect();
                if !explicit && flags.iter().any(|f| !f.is_empty()) {
                    names.push(SECRET_SUSPECTED);
                }
                let mut data = list(&names, rows, Some((page.total, page.truncated)));
                with_along(&mut data, page.along.as_ref());
                data
            })
        }
        Op::NodeFind {
            text,
            kind,
            limit,
            within,
            along,
            tags,
            no_tags,
        } => reply(
            command,
            services.node_find_tagged(
                &ctx,
                text,
                kind.as_deref(),
                within.as_deref(),
                limit.unwrap_or(10),
                along_request(along).as_ref(),
                tags,
                no_tags,
            ),
            |page| {
                // F228: a row whose title umx's secret check flags names it, as node get would.
                let titles: Vec<_> = page
                    .rows
                    .iter()
                    .map(|(n, _)| (GraphKey::Alias(&page.alias), n.id, n.title.as_str()))
                    .collect();
                let rows: Vec<Value> = page
                    .rows
                    .iter()
                    .zip(title_flags(services, &titles))
                    .map(|((n, score), flags)| {
                        let row = json!({ "id": n.id.to_text(), "type": n.kind, "title": n.title, "status": n.status, "score": score });
                        with_flags(row, &flags, false)
                    })
                    .collect();
                let mut columns = vec!["id", "type", "title", "status", "score"];
                if any_flagged(&rows) {
                    columns.push(SECRET_SUSPECTED);
                }
                let mut data = list(&columns, rows, None);
                with_along(&mut data, page.along.as_ref());
                data
            },
        ),
        Op::EdgeList {
            from,
            to,
            kind,
            columns: asked,
            limit,
            offset,
            along,
            wheres,
        } => {
            let req = EdgeListRequest {
                from: from.clone(),
                to: to.clone(),
                kind: kind.clone(),
                limit: limit.unwrap_or(DEFAULT_LIMIT),
                offset: offset.unwrap_or(0),
                along: along_request(along),
                wheres: wheres.clone(),
            };
            reply(command, services.edge_list(&ctx, req), |page| {
                let mut rows: Vec<Value> = page
                    .rows
                    .iter()
                    .map(|r| {
                        let mut row = edge_row(r);
                        if let Some(graphs) = &page.graphs
                            && graphs.copies
                        {
                            row["copy"] = json!(r.alias);
                        }
                        row
                    })
                    .collect();
                let mut columns: Vec<String> = [
                    "id",
                    "type",
                    "from",
                    "to",
                    "from_title",
                    "to_title",
                    "sentence",
                    "evidence_count",
                    "evidence_kinds",
                ]
                .map(String::from)
                .to_vec();
                if page.rows.iter().any(|row| row.edge.id.is_none()) {
                    columns.push("derived".into());
                }
                if page.rows.iter().any(|row| row.edge.has_external_end()) {
                    columns.extend(
                        ["from_graph", "to_graph", "from_state", "to_state"].map(String::from),
                    );
                }
                if page
                    .rows
                    .iter()
                    .any(|row| !row.from_candidates.is_empty() || !row.to_candidates.is_empty())
                {
                    columns.extend(["from_candidates", "to_candidates"].map(String::from));
                }
                let graph_columns = page.graphs.as_ref().map(|graphs| {
                    ["graph", "address"]
                        .into_iter()
                        .chain(graphs.copies.then_some("copy"))
                });
                // F190: the edge fields a row returns, which are all its flags cover (a list row shows no evidence),
                // each under its own name.
                let mut shown_fields: Vec<(String, String)> = Vec::new();
                match asked.as_deref() {
                    None => columns.extend(graph_columns.into_iter().flatten().map(String::from)),
                    // F74: `--columns all` is the default columns plus every custom field on the page, sorted. A
                    // custom field named like a computed column is never shown (B1): the computed one is
                    // authoritative, even where the row leaves it out on purpose. Nor is a stored `secret_suspected`
                    // (R324-1): the computed flags are.
                    Some([all]) if all == "all" => {
                        columns.extend(graph_columns.into_iter().flatten().map(String::from));
                        let custom: BTreeSet<&String> = page
                            .rows
                            .iter()
                            .flat_map(|r| r.edge.fields.keys())
                            .filter(|c| {
                                !EDGE_COMPUTED_COLUMNS.contains(&c.as_str())
                                    && *c != SECRET_SUSPECTED
                            })
                            .collect();
                        for (row, r) in rows.iter_mut().zip(&page.rows) {
                            for name in &custom {
                                row[name.as_str()] = r
                                    .edge
                                    .fields
                                    .get(name.as_str())
                                    .cloned()
                                    .unwrap_or(Value::Null);
                            }
                        }
                        shown_fields.extend(custom.iter().map(|c| ((*c).clone(), (*c).clone())));
                        columns.extend(custom.into_iter().cloned());
                    }
                    // As node list: exactly the named columns (a column the edge lacks is null), plus the
                    // graph columns when several graphs are listed.
                    Some(names) => {
                        columns = names.to_vec();
                        for column in graph_columns.into_iter().flatten() {
                            if !columns.iter().any(|c| c == column) {
                                columns.push(column.to_owned());
                            }
                        }
                        rows = rows
                            .iter()
                            .zip(&page.rows)
                            .map(|(row, r)| pick_edge(row, &r.edge, &columns))
                            .collect();
                        shown_fields.extend(
                            columns
                                .iter()
                                .filter(|c| {
                                    !EDGE_COMPUTED_COLUMNS.contains(&c.as_str())
                                        && !matches!(c.as_str(), "created" | "created_by")
                                })
                                .map(|c| (c.clone(), c.clone())),
                        );
                    }
                }
                let explicit = asks_flags(asked.as_deref());
                // F228: a far end's flagged title, in the title and sentence columns the row returns.
                let titles = edge_title_flags(services, &page.rows, |c| {
                    columns.iter().any(|shown| shown == c)
                });
                let mut flagged = false;
                for ((row, r), mut flags) in rows.iter_mut().zip(&page.rows).zip(titles) {
                    if !shown_fields.is_empty() {
                        flags.extend(umx_services::edge_secret_flags(
                            &r.edge,
                            &shown_fields,
                            false,
                        ));
                    }
                    if explicit || !flags.is_empty() {
                        flagged |= !flags.is_empty();
                        *row = with_flags(core::mem::take(row), &flags, explicit);
                    }
                }
                let mut names: Vec<&str> = columns.iter().map(String::as_str).collect();
                if !explicit && flagged {
                    names.push(SECRET_SUSPECTED);
                }
                let mut data = list(&names, rows, Some((page.total, page.truncated)));
                with_along(&mut data, page.along.as_ref());
                data
            })
        }
        Op::GraphGuide { alias } => reply(command, services.graph_guide(&ctx, alias), guide_json),
        Op::GraphSpinePropose {
            alias,
            unplaced,
            by_rule,
            limit,
            offset,
        } => reply(
            command,
            services.graph_spine_propose(
                &ctx,
                alias,
                unplaced.as_deref(),
                by_rule.as_deref(),
                limit.unwrap_or(DEFAULT_LIMIT),
                *offset,
            ),
            spine_json,
        ),
        Op::GraphSpineShow {
            alias,
            name,
            by_rule,
            unplaced,
            limit,
            offset,
        } => reply(
            command,
            services.graph_spine_show(
                &ctx,
                alias,
                name.as_deref(),
                *by_rule,
                *unplaced,
                limit.unwrap_or(DEFAULT_LIMIT),
                *offset,
            ),
            spines_shown_json,
        ),
        Op::OntologyShow { std: true } => {
            let mut v = ontology_json(&umx_core::Ontology::std_base());
            v["hubs"] = json!([]);
            Envelope::ok(command, v)
        }
        Op::OntologyShow { std: false } => {
            reply(command, services.ontology_show(&ctx), |(alias, o)| {
                let mut v = ontology_json(o);
                // PRD 003 rules-34: the hub declarations the view draws declared hubs from; [] when none.
                v["hubs"] = json!(
                    services
                        .hubs_of(alias)
                        .iter()
                        .map(hub_json)
                        .collect::<Vec<_>>()
                );
                v
            })
        }
        Op::Search {
            text,
            within,
            limit,
            along,
            also,
            tags,
            no_tags,
        } => match umx_protocol::also_problem(text, also) {
            Some(what) => Envelope::fail(
                command,
                issue(
                    &umx_services::problems::usage_invalid(what, "search"),
                    command,
                ),
            ),
            None => reply(
                command,
                services.search_tagged(
                    &ctx,
                    text,
                    also,
                    within.as_deref(),
                    limit.unwrap_or(10),
                    along_request(along),
                    (tags, no_tags),
                ),
                |r| search_json(services, r),
            ),
        },
        Op::Neighbours {
            node,
            relation,
            direction,
            limit,
            offset,
            tags,
            no_tags,
            entities,
        } => {
            let req = NeighboursRequest {
                node: node.clone(),
                relation: relation.clone(),
                direction: direction_of(*direction, Direction::Both),
                limit: limit.unwrap_or(DEFAULT_LIMIT),
                offset: offset.unwrap_or(0),
                tags: tags.clone(),
                no_tags: no_tags.clone(),
                // ws008 E3 (req-127): on unless the request says otherwise.
                entities: match entities.unwrap_or(umx_protocol::Entities::On) {
                    umx_protocol::Entities::On => umx_services::Entities::On,
                    umx_protocol::Entities::Off => umx_services::Entities::Off,
                    umx_protocol::Entities::Confirmed => umx_services::Entities::Confirmed,
                },
            };
            let similar = similar_to(relation);
            reply(command, services.neighbours(&ctx, req), |page| {
                let titles: Vec<_> = page
                    .rows
                    .iter()
                    .map(|r| {
                        (
                            GraphKey::Id(r.node_graph),
                            r.node.id,
                            shown_title(r.state, &r.node),
                        )
                    })
                    .collect();
                let rows: Vec<Value> = page
                    .rows
                    .iter()
                    .zip(title_flags(services, &titles))
                    .map(|(r, flags)| {
                        let mut row = if r.state.is_some() {
                            serde_json::Map::new()
                        } else {
                            node_row(&r.node)
                        };
                        row.insert("id".into(), json!(r.node.id.to_text()));
                        row.insert("address".into(), json!(r.address));
                        row.insert("graph".into(), json!(r.node_graph.to_text()));
                        row.insert("edge_graph".into(), json!(r.graph.to_text()));
                        row.insert("from".into(), json!(r.from.to_text()));
                        row.insert("to".into(), json!(r.to.to_text()));
                        row.insert("from_graph".into(), json!(r.from_graph.to_text()));
                        row.insert("to_graph".into(), json!(r.to_graph.to_text()));
                        if let Some(state) = r.state {
                            row.insert("state".into(), json!(state));
                            row.insert("graph_id".into(), json!(r.node_graph.to_text()));
                            row.insert("node_id".into(), json!(r.node.id.to_text()));
                        }
                        if !r.candidates.is_empty() {
                            row.insert("candidates".into(), json!(r.candidates));
                        }
                        row.insert("way".into(), json!(r.edge.way.as_str()));
                        row.insert("relation".into(), json!(r.edge.relation));
                        edge_cells(&mut row, &r.edge, similar);
                        // ws008 E3: only a reply that groups carries these; an ungrouped row is today's.
                        if let Some(status) = r.match_status {
                            row.insert("match_status".into(), json!(status));
                            row.insert("via".into(), json!(r.via));
                        }
                        with_flags(Value::Object(row), &flags, false)
                    })
                    .collect();
                let mut columns = vec![
                    "way", "relation", "edge", "score", "derived", "id", "type", "title", "status",
                ];
                if page
                    .rows
                    .iter()
                    .any(|row| row.from_graph != row.graph || row.to_graph != row.graph)
                {
                    columns.extend(["graph", "from_graph", "to_graph", "state"]);
                }
                if page.rows.iter().any(|row| !row.candidates.is_empty()) {
                    columns.push("candidates");
                }
                if page.rows.iter().any(|row| row.match_status.is_some()) {
                    columns.extend(["match_status", "via"]);
                }
                let mut names = with_score(&columns, similar, ctx.include_provenance);
                if any_flagged(&rows) {
                    names.push(SECRET_SUSPECTED);
                }
                let mut v = list(&names, rows, Some((page.total, page.truncated)));
                similarity_meta(&mut v, services, &page.alias, similar);
                if let Some(covered) = &page.covered {
                    v["graphs"] = json!(covered);
                }
                v
            })
        }
        Op::Walk {
            node,
            depth,
            relation,
            direction,
            within,
            limit,
            offset,
            tags,
            no_tags,
        } => {
            let req = WalkRequest {
                node: node.clone(),
                depth: depth.unwrap_or(1),
                relation: relation.clone(),
                direction: direction_of(*direction, Direction::Out),
                within: within.clone(),
                limit: limit.unwrap_or(DEFAULT_LIMIT),
                offset: offset.unwrap_or(0),
                tags: tags.clone(),
                no_tags: no_tags.clone(),
            };
            let similar = similar_to(relation);
            reply(command, services.walk(&ctx, req), |page| {
                let titles: Vec<_> = page
                    .rows
                    .iter()
                    .map(|r| {
                        (
                            GraphKey::Id(r.graph_id),
                            r.node.id,
                            shown_title(r.state, &r.node),
                        )
                    })
                    .collect();
                let rows: Vec<Value> = page
                    .rows
                    .iter()
                    .zip(title_flags(services, &titles))
                    .map(|(r, flags)| {
                        let mut row = if r.state.is_some() {
                            serde_json::Map::new()
                        } else {
                            node_row(&r.node)
                        };
                        row.insert("id".into(), json!(r.node.id.to_text()));
                        row.insert("graph".into(), json!(r.graph));
                        row.insert("graph_id".into(), json!(r.graph_id.to_text()));
                        row.insert("address".into(), json!(r.address));
                        row.insert("from_graph".into(), json!(r.from_graph.to_text()));
                        row.insert("edge_graph".into(), json!(r.edge_graph.to_text()));
                        if let Some(state) = r.state {
                            row.insert("state".into(), json!(state));
                            row.insert("node_id".into(), json!(r.node.id.to_text()));
                        }
                        if !r.candidates.is_empty() {
                            row.insert("candidates".into(), json!(r.candidates));
                        }
                        row.insert("depth".into(), json!(r.depth));
                        row.insert("from".into(), json!(r.from.to_text()));
                        row.insert("relation".into(), json!(r.edge.relation));
                        row.insert("way".into(), json!(r.edge.way.as_str()));
                        edge_cells(&mut row, &r.edge, similar);
                        with_flags(Value::Object(row), &flags, false)
                    })
                    .collect();
                let columns = [
                    "depth", "id", "type", "title", "status", "from", "relation", "way", "edge",
                    "score", "derived", "graph", "address", "state",
                ];
                let mut names = with_score(&columns, similar, ctx.include_provenance);
                if any_flagged(&rows) {
                    names.push(SECRET_SUSPECTED);
                }
                let mut v = list(&names, rows, Some((page.total, page.truncated)));
                similarity_meta(&mut v, services, &page.alias, similar);
                v
            })
        }
        Op::Path {
            from,
            to,
            max_depth,
            relation,
            direction,
        } => {
            let similar = similar_to(relation);
            let req = PathRequest {
                from: from.clone(),
                to: to.clone(),
                // Along similar_to a path goes at most 2 steps (workshop 008 section 9); more is refused if asked for.
                max_depth: max_depth.unwrap_or(if similar { 2 } else { 6 }),
                relation: relation.clone(),
                direction: direction_of(*direction, Direction::Both),
            };
            reply(command, services.path(&ctx, req), |page| {
                let titles: Vec<_> = page
                    .rows
                    .iter()
                    .map(|r| {
                        (
                            GraphKey::Id(r.graph_id),
                            r.node.id,
                            shown_title(r.state, &r.node),
                        )
                    })
                    .collect();
                let rows: Vec<Value> = page
                    .rows
                    .iter()
                    .zip(title_flags(services, &titles))
                    .map(|(r, flags)| {
                        let mut row = if r.state.is_some() {
                            serde_json::Map::new()
                        } else {
                            node_row(&r.node)
                        };
                        row.insert("id".into(), json!(r.node.id.to_text()));
                        row.insert("graph".into(), json!(r.graph));
                        row.insert("graph_id".into(), json!(r.graph_id.to_text()));
                        row.insert("address".into(), json!(r.address));
                        if let Some(state) = r.state {
                            row.insert("state".into(), json!(state));
                            row.insert("node_id".into(), json!(r.node.id.to_text()));
                        }
                        if !r.candidates.is_empty() {
                            row.insert("candidates".into(), json!(r.candidates));
                        }
                        row.insert("step".into(), json!(r.step));
                        let edge = r.edge.as_ref();
                        row.insert("relation".into(), json!(edge.map(|e| &e.relation)));
                        row.insert("way".into(), json!(edge.map(|e| e.way.as_str())));
                        match edge {
                            Some(e) => edge_cells(&mut row, e, similar),
                            None => {
                                row.insert("edge".into(), Value::Null);
                                if similar {
                                    row.insert("score".into(), Value::Null);
                                    row.insert("derived".into(), Value::Null);
                                }
                            }
                        }
                        with_flags(Value::Object(row), &flags, false)
                    })
                    .collect();
                let columns = [
                    "step", "id", "type", "title", "status", "relation", "way", "edge", "score",
                    "derived", "graph", "address", "state",
                ];
                let mut names = with_score(&columns, similar, ctx.include_provenance);
                if any_flagged(&rows) {
                    names.push(SECRET_SUSPECTED);
                }
                let mut v = list(&names, rows, Some((page.total, page.truncated)));
                similarity_meta(&mut v, services, &page.alias, similar);
                v
            })
        }
        Op::Check {
            rules,
            rule,
            after,
            lint,
            kind,
            offset,
            source_budget,
            ..
        } => {
            let request = match lint_request(
                *lint,
                kind.as_deref(),
                if *rules && !*lint { None } else { *offset },
            ) {
                Ok(request) => request,
                Err(problem) => return Envelope::fail(command, issue(&problem, command)),
            };
            if !rules && (rule.is_some() || after.is_some()) {
                return Envelope::fail(
                    command,
                    issue(
                        &umx_services::problems::usage_invalid(
                            "--rule and --after go with --rules",
                            "check",
                        ),
                        command,
                    ),
                );
            }
            let after = match after.as_deref().map(str::parse::<Id>).transpose() {
                Ok(after) => after,
                Err(_) => {
                    return Envelope::fail(
                        command,
                        issue(
                            &umx_services::problems::usage_invalid(
                                "--after is a full node id",
                                "check",
                            ),
                            command,
                        ),
                    );
                }
            };
            let rules = rules.then(|| umx_services::RuleCheckRequest {
                rule: rule.clone(),
                after,
                offset: offset.unwrap_or(0),
            });
            reply(
                command,
                services.check_with_rules(&ctx, request.as_ref(), *source_budget, rules.as_ref()),
                check_json,
            )
        }
        Op::NodeEvidence {
            node,
            action,
            at,
            removed,
            redact,
            ..
        } => match redact_misuse(*action, *at, *redact) {
            Some(problem) => Envelope::fail(command, issue(&problem, command)),
            None => evidence_list(services, &ctx, command, node, *removed),
        },
        Op::NodeHistory { node, limit } => reply(
            command,
            services.node_history(&ctx, node, limit.unwrap_or(DEFAULT_LIMIT)),
            |page| {
                let rows = page.rows.iter().map(history_row).collect();
                list(
                    &["at", "by", "kind", "from", "to", "label", "reason"],
                    rows,
                    Some((page.total, page.truncated)),
                )
            },
        ),
        Op::Query {
            text,
            limit,
            offset,
            params,
            along,
        } => {
            let req = umx_services::CypherRequest {
                text: text.clone(),
                limit: *limit,
                offset: *offset,
                params: core_sets(params),
                along: along_request(along),
            };
            reply(command, services.query(&ctx, &req), query_json)
        }
        Op::Resolve { address } => resolved(command, services.resolve(&ctx, address)),
        Op::EdgeShow { edge } => reply(command, services.edge_show(&ctx, edge), |r| {
            edge_shown(services, r)
        }),
        _ => Envelope::fail(command, internal("write_on_read_path")),
    }
}

/// `graph list --all` (req-116): every loaded graph as `graph list` shows it, then every graph this machine's index
/// knows that is not loaded. `config` finds each open graph's repository; without it `repo` is null.
pub fn graph_index(
    services: &Services,
    config: Option<&ConfigServices>,
    request: &Request,
) -> Envelope {
    let command = request.command.as_str();
    let done = services.graph_index_in(&ctx(request), config);
    let rows: Vec<Value> = done
        .value
        .iter()
        .map(|r| {
            json!({
                "alias": r.alias, "id": r.id.to_text(), "path": r.path, "state": r.state,
                "folder": r.folder.as_str(), "last_loaded": r.last_loaded, "nodes": r.nodes, "edges": r.edges,
                "model": r.model, "policy": r.policy.name(), "repo": r.repo, "reload": r.reload.as_str(),
                "last_used": r.loaded.as_ref().map(|g| &g.last_used),
                "unloads_in_s": r.loaded.as_ref().and_then(|g| g.unloads_in_s),
            })
        })
        .collect();
    let columns = [
        "alias",
        "id",
        "path",
        "state",
        "folder",
        "last_loaded",
        "nodes",
        "edges",
        "model",
        "policy",
        "repo",
        "reload",
        "last_used",
        "unloads_in_s",
    ];
    let mut env = Envelope::ok(command, list(&columns, rows, None));
    for w in &done.notes {
        env = env.note(issue(w, command));
    }
    env
}

pub(crate) fn ctx(request: &Request) -> Ctx {
    context(request, crate::provenance::actor(request.actor.as_ref()).0)
}

fn context(request: &Request, actor: umx_core::provenance::Actor) -> Ctx {
    let mut ctx = Ctx::new(request.by.clone().unwrap_or_else(|| "unknown".to_owned()));
    ctx.actor = actor;
    ctx.include_provenance = request.include_provenance;
    ctx.graph = request.graph.clone();
    ctx.scope = request.scope.clone().map(umx_services::Scope::Saved);
    if request.all {
        ctx.scope = Some(umx_services::Scope::All {
            include_named: request.include_named,
        });
    }
    ctx.default_graph = request.default_graph.clone();
    ctx.retry = request.op.as_ref().map(retry);
    ctx
}

pub(crate) fn scope_usage(request: &Request) -> Option<Problem> {
    if request.include_provenance && !request.op.as_ref().is_some_and(Op::is_read) {
        return Some(umx_services::problems::usage_invalid(
            "--provenance is only supported by read commands",
            "start",
        ));
    }
    let read_all = matches!(
        request.op,
        Some(Op::Query { .. } | Op::Search { .. } | Op::SimilarityPairs { .. })
    );
    let all_error = if request.include_named && !request.all {
        Some("--include-named requires --all")
    } else if request.all && !read_all {
        Some("--all is only supported by search, query and similarity pairs")
    } else if request.all && (request.graph.is_some() || request.scope.is_some()) {
        Some("--all cannot be combined with --graph or --scope")
    } else {
        None
    };
    if let Some(what) = all_error {
        let noun = request
            .op
            .as_ref()
            .map_or("search", Op::command)
            .split(' ')
            .next()
            .expect("typed commands have a noun");
        // `similarity` has no topic of its own; the embeddings topic documents it.
        let noun = if noun == "similarity" {
            "embeddings"
        } else {
            noun
        };
        return Some(umx_services::problems::usage_invalid(what, noun));
    }
    let allowed = matches!(
        request.op,
        Some(
            Op::NodeList { .. }
                | Op::EdgeList { .. }
                | Op::Query { .. }
                | Op::Search { .. }
                | Op::SimilarityPairs { .. }
                | Op::Walk { .. }
                | Op::Neighbours { .. }
                | Op::Path { .. }
                | Op::Check { .. }
                | Op::EntityShow { .. }
                | Op::Who { .. }
        )
    );
    let graph_list = request.graph.as_deref().is_some_and(|g| g.contains(','));
    let what = if graph_list && !allowed && !matches!(request.op, Some(Op::ScopeSave { .. })) {
        "this command takes one graph (use --graph with one name)"
    } else if request.scope.is_none() {
        return None;
    } else if request.graph.is_some() {
        "--graph and --scope cannot be used together"
    } else if !allowed {
        "this command takes one graph (use --graph with one name)"
    } else {
        return None;
    };
    Some(umx_services::problems::usage_invalid(what, "scope"))
}

/// `umx who` (ws007 s04): one row per content node or edge, with the key it matched.
fn who_json(v: &umx_services::WhoListed) -> Value {
    let rows = v
        .rows
        .iter()
        .map(|r| {
            json!({
                "graph": r.alias,
                "address": umx_services::address(&r.alias, r.id),
                "record": r.record.as_str(),
                "type": r.kind,
                "title": r.title,
                "how": r.how,
                "sessions": r.sessions,
                "secret_suspected": r.secret_suspected,
            })
        })
        .collect();
    let mut out = list(
        &[
            "graph",
            "address",
            "record",
            "type",
            "title",
            "how",
            "sessions",
            SECRET_SUSPECTED,
        ],
        rows,
        Some((Some(v.total), v.truncated)),
    );
    out["key"] = json!(v.key);
    out
}

/// `umx node who` (ws007 s04): the authored row, then the last edited row. Each address names a provenance node,
/// which reads only with `--provenance`.
fn node_who_json(v: &umx_services::NodeWho) -> Value {
    let node = umx_services::address(&v.alias, v.node);
    let key = |n: &Option<umx_services::WhoNode>| n.as_ref().map(|n| n.key.clone());
    let at = |n: &Option<umx_services::WhoNode>| {
        n.as_ref().map(|n| umx_services::address(&v.alias, n.id))
    };
    let rows = v
        .stamps
        .iter()
        .map(|s| {
            json!({
                "node": node,
                "stamp": s.stamp,
                "state": s.state.as_str(),
                "session": key(&s.session),
                "session_address": at(&s.session),
                "agent": key(&s.agent),
                "agent_address": at(&s.agent),
                "role": s.role,
                "model": s.model,
                "model_address": at(&s.model_node),
                "harness": s.harness,
                "harness_address": at(&s.harness_node),
            })
        })
        .collect();
    list(
        &[
            "node",
            "stamp",
            "state",
            "session",
            "session_address",
            "agent",
            "agent_address",
            "role",
            "model",
            "model_address",
            "harness",
            "harness_address",
        ],
        rows,
        None,
    )
}

/// `entity show` (ws008 E2): the covered graphs, then each member with how it matched, then the proposed partners.
fn entity_json(v: &umx_services::EntityShown) -> Value {
    let matched = |m: &umx_services::EntityMatch| match m {
        umx_services::EntityMatch::Asked => json!({ "by": "asked" }),
        umx_services::EntityMatch::Key(key) => json!({ "by": "entity_key", "key": key }),
        umx_services::EntityMatch::Link { alias, edge } => {
            json!({ "by": "same_as", "edge": umx_services::address(alias, *edge), "graph": alias })
        }
    };
    let members: Vec<Value> = v
        .members
        .iter()
        .map(|m| {
            with_flags(
                json!({
                    "address": umx_services::address(&m.alias, m.id), "graph": m.alias, "id": m.id.to_text(),
                    "type": m.kind, "title": m.title, "entity_key": m.keys,
                    "matched": m.matched.iter().map(matched).collect::<Vec<_>>(),
                }),
                &m.secret_suspected,
                false,
            )
        })
        .collect();
    let proposed: Vec<Value> = v
        .proposed
        .iter()
        .map(|p| {
            with_flags(
                json!({
                    "address": umx_services::address(&p.alias, p.id), "graph": p.alias, "id": p.id.to_text(),
                    "type": p.kind, "title": p.title,
                    "with": umx_services::address(&p.with.0, p.with.1),
                    "edge": umx_services::address(&p.edge.0, p.edge.1), "edge_graph": p.edge.0,
                }),
                &p.secret_suspected,
                false,
            )
        })
        .collect();
    let (lead_alias, lead_graph, lead_id) = &v.lead;
    with_flags(
        json!({
            "node": umx_services::address(&v.alias, v.node), "graphs": v.graphs,
            "entity": umx_services::address(lead_alias, *lead_id),
            "entity_id": format!("umx:{}/{}", lead_graph.to_text(), lead_id.to_text()),
            "keys": v.keys,
            "members": members, "proposed": proposed,
        }),
        &v.keys_secret_suspected,
        false,
    )
}

fn scope_json(scope: &umx_services::ScopeInfo) -> Value {
    json!({
        "name": scope.name,
        "members": scope.members.iter().map(|m| json!({
            "id": m.id.to_text(), "alias": m.alias, "folder": m.folder,
            "loaded": m.loaded, "secured": m.secured,
        })).collect::<Vec<_>>(),
    })
}

/// A `--role` value as a fixed word: `data`, `content`, else the placeholder.
fn role_word(role: &str) -> &'static str {
    match role {
        "data" => "data",
        "content" => "content",
        _ => "<role>",
    }
}

/// The command rebuilt from its parsed form, for the `graph_ambiguous` next: verb path, flags, valid names, ids and
/// numbers kept; free text (titles, notes, values, expressions, paths) and anything that fails its check become
/// placeholders. Every field of every operation is bound by name, with no `..`, so a field added to `Op` does not
/// compile until it is handled here (B2).
/// `ontology adopt-std`'s reply (ws008 T3): the plan, or what was written.
fn adopted_json(v: &umx_services::StdAdopted) -> Value {
    let names = |n: &umx_core::adopt::Names| json!({ "types": n.types, "relations": n.relations, "statuses": n.statuses });
    let fields = |fs: &[umx_core::adopt::FieldAt]| -> Vec<Value> {
        fs.iter()
            .map(|f| match f.relation {
                true => json!({ "relation": f.on, "field": f.field }),
                false => json!({ "type": f.on, "field": f.field }),
            })
            .collect()
    };
    let mut folded = names(&v.folded);
    folded["fields"] = json!(fields(&v.folded_fields));
    let mut out = json!({
        "graph": v.alias,
        "std": v.std,
        "dry_run": v.dry_run,
        "unchanged": v.unchanged,
        "folded": folded,
        "extended": fields(&v.extended),
        "custom": names(&v.custom),
        "renamed": v.renamed.iter().map(|r| json!({
            "what": r.what.as_str(), "from": r.from, "to": r.to, "nodes": r.nodes, "edges": r.edges,
        })).collect::<Vec<_>>(),
    });
    // ws008 t2b: only when --field-to-tag was given, so other replies stay as they were.
    if !v.field_to_tag.is_empty() {
        out["field_to_tag"] = json!(v.field_to_tag.iter().map(|f| json!({
            "field": f.field, "nodes": f.nodes, "tags": f.tags, "kept": f.kept, "collapsed": f.collapsed,
            "declaration_removed": f.declaration_removed,
        })).collect::<Vec<_>>());
    }
    out
}

/// `ontology remove`'s reply (F76).
fn removed_json(v: &umx_services::OntologyRemoved) -> Value {
    json!({ "removed": v.removed, "name": v.name, "unchanged": v.unchanged })
}

/// A tag in stored form (one tag or `<facet>:*`) as itself, through [`Command::tags`]; any other (a comma list, a tag
/// not yet normalised, one that looks like a secret) as the placeholder `<tag>` (ws008 t2b).
fn tag_word(c: Command, t: &str) -> Command {
    // A tag that looks like a secret, read as given or upper cased (tags are stored lowercased), is never echoed.
    let suspect = |s: &str| umx_core::secrets::field_suspect(umx_core::tags::TAGS, s).is_some();
    if suspect(t) || suspect(&t.to_ascii_uppercase()) {
        return c.word("<tag>");
    }
    match c.clone().tags(&[t.to_string()]) {
        Some(c) => c,
        None => c.word("<tag>"),
    }
}

/// `flag <tag>` for each of `tags` ([`tag_word`]).
fn tag_words(mut c: Command, flag: &'static str, tags: &[String]) -> Command {
    for t in tags {
        c = tag_word(c.word(flag), t);
    }
    c
}

fn retry(op: &Op) -> Command {
    let name = |c: Command, n: &str, placeholder: &'static str| match ValidName::new(n) {
        Some(v) => c.name(&v),
        None => c.word(placeholder),
    };
    let alias = |c: Command, a: &str| match ValidName::alias(a) {
        Some(v) => c.name(&v),
        None => c.word("<alias>"),
    };
    let node = |c: Command, r: &str, placeholder: &'static str| match IdText::new(r) {
        Some(t) => c.id_text(&t),
        None => c.word(placeholder),
    };
    let opt = |c: Command,
               flag: &'static str,
               v: &Option<String>,
               f: &dyn Fn(Command, &str) -> Command| match v {
        Some(v) => f(c.word(flag), v),
        None => c,
    };
    let text =
        |c: Command, flag: &'static str, v: &Option<String>, placeholder: &'static str| match v {
            Some(_) => c.word(flag).word(placeholder),
            None => c,
        };
    let number = |c: Command, flag: &'static str, n: &Option<usize>| match n {
        Some(n) => c.word(flag).count(*n as u64),
        None => c,
    };
    let columns = |c: Command, cols: &Option<Vec<String>>| match cols {
        Some(cols) => {
            let names: Option<Vec<ValidName>> = cols.iter().map(|c| ValidName::new(c)).collect();
            match names {
                // A comma list renders only as Fill::names: 1 to 64 names (F73).
                Some(names) if (1..=64).contains(&names.len()) => {
                    c.word("--columns").names(&names, ",")
                }
                _ => c.word("--columns").word("<columns>"),
            }
        }
        None => c,
    };
    let sets = |mut c: Command, sets: &[SetArg]| {
        for SetArg { field, value } in sets {
            let flag = match value {
                SetValue::Raw(_) => "--set",
                SetValue::Str(_) => "--set-str",
                SetValue::Json(_) => "--set-json",
            };
            c = match ValidName::new(field) {
                Some(f) => c.word(flag).name_with(&f, '=', "<value>"),
                None => c.word(flag).word("<field>=<value>"),
            };
        }
        c
    };
    let field_spec =
        |c: Command, spec: &str| match spec.split_once(':').and_then(|(a, _)| ValidName::new(a)) {
            Some(a) => c.name_with(&a, ':', "<kind>"),
            None => c.word("<name>:<kind>"),
        };
    // --along (D71 step 12): a declared spine's or built-in's name as checked words; an end is id text, else a
    // placeholder (a date or an address is the caller's text).
    let along_words = |c: Command, along: &Option<umx_protocol::Along>| {
        let Some(a) = along else {
            return c;
        };
        let c = c.word("--along");
        let c = match a.spine.strip_prefix("date_field:").map(ValidName::new) {
            Some(Some(f)) => c.qualified("date_field:", &f),
            Some(None) => c.word("<spine>"),
            None => name(c, &a.spine, "<spine>"),
        };
        let c = opt(c, "--after", &a.after, &|c, v| node(c, v, "<end>"));
        opt(c, "--before", &a.before, &|c, v| node(c, v, "<end>"))
    };
    match op {
        Op::ScopeList => Command::new("scope list"),
        Op::ScopeSave { name: n, replace } => {
            let c = name(Command::new("scope save"), n, "<name>")
                .word("--graph")
                .word("<graphs>");
            if *replace { c.word("--replace") } else { c }
        }
        Op::ScopeShow { name: n } => name(Command::new("scope show"), n, "<name>"),
        Op::ScopeDelete { name: n } => name(Command::new("scope delete"), n, "<name>"),
        // A folder is a path the agent typed: only its placeholder.
        Op::RootAdd { name: n, .. } => name(Command::new("root add"), n, "<name>").word("<folder>"),
        Op::RootList => Command::new("root list"),
        Op::RootRemove { name: n } => name(Command::new("root remove"), n, "<name>"),
        Op::Resolve { address: _ } => Command::new("resolve").word("<address>"),
        Op::EdgeDelete { edge } => node(Command::new("edge delete"), edge, "<edge>"),
        Op::EdgeEdit {
            edge,
            relation,
            reverse,
            sets: s,
            unsets,
        } => {
            let c = node(Command::new("edge edit"), edge, "<edge>");
            let c = opt(c, "--relation", relation, &|c, v| name(c, v, "<relation>"));
            let mut c = sets(if *reverse { c.word("--reverse") } else { c }, s);
            for u in unsets {
                c = name(c.word("--unset"), u, "<field>");
            }
            c
        }
        Op::EdgeShow { edge: _ } => Command::new("edge show").word("<edge>"),
        Op::GraphNew {
            path: _,
            alias: a,
            title,
            empty,
            embedding_model,
            secured,
            no_graphfile,
            graphfile,
            no_std,
        } => {
            let c = alias(Command::new("graph new").word("<path>").word("--alias"), a);
            let c = text(c, "--title", title, "<title>");
            let c = if *empty { c.word("--empty") } else { c };
            let c = if *secured { c.word("--secured") } else { c };
            let c = if *no_std { c.word("--no-std") } else { c };
            let c = if *no_graphfile {
                c.word("--no-graphfile")
            } else if *graphfile {
                c.word("--graphfile")
            } else {
                c
            };
            match embedding_model.as_deref().map(ModelKey::parse) {
                Some(Ok(k)) => c.word("--embedding-model").model_key(&k),
                Some(Err(_)) => c.word("--embedding-model").word("<model_key>"),
                None => c,
            }
        }
        Op::GraphLoad {
            graphfile: true,
            set,
            ..
        } => opt(
            Command::new("graph load").word("--graphfile"),
            "--set",
            set,
            &|c, v| name(c, v, "<set>"),
        ),
        Op::GraphLoad { as_alias, .. } => opt(
            Command::new("graph load").word("<path>"),
            "--as",
            as_alias,
            &alias,
        ),
        Op::GraphList {
            graphfile: Some(_), ..
        } => Command::new("graph list").word("--graphfile"),
        Op::GraphList { all: false, .. } => Command::new("graph list"),
        Op::GraphList { all: true, .. } => Command::new("graph list").word("--all"),
        Op::GraphScan {
            add, rewrite, drop, ..
        } => {
            let c = Command::new("graph scan").word("<folder>");
            let c = if *add { c.word("--add") } else { c };
            let c = if *rewrite { c.word("--rewrite") } else { c };
            opt(c, "--drop", drop, &alias)
        }
        Op::GraphForget { alias: a } => alias(Command::new("graph forget"), a),
        Op::NodeAdd {
            kind,
            title: _,
            status,
            note,
            sets: s,
            parent,
            evidence,
            evidence_kind,
            locator,
            raised_in,
            on_key,
            related,
            tags,
        } => {
            let c = name(Command::new("node add").word("--type"), kind, "<type>")
                .word("--title")
                .word("<title>");
            // A locator is the caller's text: only a placeholder.
            let c = text(c, "--locator", locator, "<locator>");
            let c = opt(c, "--status", status, &|c, v| name(c, v, "<status>"));
            let c = sets(text(c, "--note", note, "<text>"), s);
            let mut c = opt(c, "--parent", parent, &|c, v| node(c, v, "<node>"));
            for _ in evidence {
                c = c.word("--evidence").word("<pointer>");
            }
            let c = opt(c, "--evidence-kind", evidence_kind, &|c, v| {
                name(c, v, "<kind>")
            });
            let c = opt(c, "--raised-in", raised_in, &|c, v| node(c, v, "<anchor>"));
            let c = match on_key {
                Some(umx_protocol::OnKey::Resolve) => c.word("--on-key").word("resolve"),
                None => c,
            };
            related_flag(tag_words(c, "--tag", tags), *related)
        }
        Op::NodeGet {
            node: n,
            columns: cols,
        } => columns(node(Command::new("node get"), n, "<node>"), cols),
        Op::ResourceShow { node: n } => node(Command::new("resource show"), n, "<node>"),
        Op::ResourceRefresh { node: n } => node(Command::new("resource refresh"), n, "<node>"),
        Op::NodeList {
            kind,
            status,
            wheres,
            within,
            columns: cols,
            limit,
            offset,
            along,
            tags,
            no_tags,
        } => {
            let mut c = opt(Command::new("node list"), "--type", kind, &|c, v| {
                name(c, v, "<type>")
            });
            c = opt(c, "--status", status, &|c, v| name(c, v, "<status>"));
            for _ in wheres {
                c = c.word("--where").word("<expr>");
            }
            c = opt(c, "--within", within, &|c, v| node(c, v, "<node>"));
            c = tag_words(tag_words(c, "--tag", tags), "--no-tag", no_tags);
            let c = along_words(c, along);
            number(
                number(columns(c, cols), "--limit", limit),
                "--offset",
                offset,
            )
        }
        Op::NodeFind {
            text: _,
            kind,
            limit,
            within,
            along,
            tags,
            no_tags,
        } => {
            let c = opt(
                Command::new("node find").word("<text>"),
                "--type",
                kind,
                &|c, v| name(c, v, "<type>"),
            );
            let c = tag_words(tag_words(c, "--tag", tags), "--no-tag", no_tags);
            let c = number(c, "--limit", limit);
            along_words(
                opt(c, "--within", within, &|c, v| node(c, v, "<node>")),
                along,
            )
        }
        // The pointers are never repeated in a command (they are paths and text the agent typed).
        Op::EdgeAdd {
            from,
            relation,
            to,
            sets: s,
            ..
        } => {
            let c = node(Command::new("edge add"), from, "<from>");
            sets(node(name(c, relation, "<relation>"), to, "<to>"), s)
        }
        Op::EdgeList {
            from,
            to,
            kind,
            columns: cols,
            limit,
            offset,
            along,
            ..
        } => {
            let c = opt(Command::new("edge list"), "--from", from, &|c, v| {
                node(c, v, "<node>")
            });
            let c = opt(c, "--to", to, &|c, v| node(c, v, "<node>"));
            let c = opt(c, "--type", kind, &|c, v| name(c, v, "<relation>"));
            let c = along_words(c, along);
            number(
                number(columns(c, cols), "--limit", limit),
                "--offset",
                offset,
            )
        }
        Op::OntologyShow { std } => {
            let c = Command::new("ontology show");
            if *std { c.word("--std") } else { c }
        }
        Op::OntologyAddType {
            name: n,
            fields,
            description,
        } => {
            let mut c = name(Command::new("ontology add type"), n, "<name>");
            for f in fields {
                c = field_spec(c.word("--field"), f);
            }
            text(c, "--description", description, "<text>")
        }
        Op::OntologyAddStatus {
            name: n,
            description,
        } => text(
            name(Command::new("ontology add status"), n, "<name>"),
            "--description",
            description,
            "<text>",
        ),
        Op::OntologyAddTag { tag } => tag_word(Command::new("ontology add tag"), tag),
        Op::OntologySetType {
            name: n,
            columns: cols,
        } => match cols {
            None => name(Command::new("ontology set type"), n, "<type>").word("--reset"),
            // B1: the list renders through Fill::names (F73), so the next keeps its commas and runs as printed.
            Some(_) => columns(name(Command::new("ontology set type"), n, "<type>"), cols),
        },
        Op::OntologyAddRelation {
            name: n,
            symmetric,
            description,
            from,
            to,
        } => {
            let mut c = name(Command::new("ontology add relation"), n, "<name>");
            for (flag, types) in [("--from", from), ("--to", to)] {
                for t in types {
                    c = name(c.word(flag), t, "<type>");
                }
            }
            let c = if *symmetric { c.word("--symmetric") } else { c };
            text(c, "--description", description, "<text>")
        }
        Op::OntologyAddField {
            types,
            field,
            all_types,
            description,
            role,
            values,
        } => {
            let c = Command::new("ontology add field");
            let c = match (types.as_slice(), *all_types) {
                (_, true) => c.word("--all-types"),
                ([one], false) => name(c, one, "<type>"),
                (more, false) => {
                    match more
                        .iter()
                        .map(|t| ValidName::new(t))
                        .collect::<Option<Vec<_>>>()
                    {
                        Some(names) if (1..=64).contains(&names.len()) => c.names(&names, ","),
                        _ => c.word("<types>"),
                    }
                }
            };
            let c = text(field_spec(c, field), "--description", description, "<text>");
            let c = match role {
                Some(r) => c.word("--role").word(role_word(r)),
                None => c,
            };
            if values.is_empty() {
                c
            } else {
                c.field_values(values)
            }
        }
        Op::OntologyAddRelationField { relation, field } => field_spec(
            name(
                Command::new("ontology add field").word("--relation"),
                relation,
                "<relation>",
            ),
            field,
        ),
        Op::GraphGuide { alias: a } => alias(Command::new("graph guide"), a),
        Op::GraphSpinePropose { alias: a, .. } => alias(Command::new("graph spine propose"), a),
        Op::GraphSpineSet { alias: a, decl: _ } => alias(Command::new("graph spine set"), a)
            .word("--file")
            .word("<decl.json>"),
        Op::GraphSpineConfirm {
            alias: a,
            candidate,
        } => {
            let c = alias(Command::new("graph spine confirm"), a);
            match candidate.strip_prefix("type:").and_then(ValidName::new) {
                Some(t) => c.qualified("type:", &t),
                None => c.word("<candidate>"),
            }
        }
        Op::GraphSpineShow {
            alias: a,
            name: n,
            by_rule,
            unplaced,
            limit,
            offset,
        } => {
            let c = alias(Command::new("graph spine show"), a);
            let c = match (n, by_rule, unplaced) {
                (Some(n), true, _) => name(c.word("--by-rule"), n, "<name>"),
                (Some(n), _, true) => name(c.word("--unplaced"), n, "<name>"),
                (Some(n), false, false) => name(c, n, "<name>"),
                (None, _, _) => c,
            };
            number(number(c, "--limit", limit), "--offset", offset)
        }
        Op::GraphSpineRemove { alias: a, name: n } => {
            name(alias(Command::new("graph spine remove"), a), n, "<name>")
        }
        Op::RuleList => Command::new("rule list"),
        Op::RuleAdd { dry_run, .. } => {
            let c = Command::new("rule add").word("<name>");
            if *dry_run { c.word("--dry-run") } else { c }
        }
        Op::RuleRemove { name: n } => name(Command::new("rule remove"), n, "<name>"),
        Op::RuleOff { .. } => Command::new("rule off").word("<std:name>"),
        Op::RuleOn { .. } => Command::new("rule on").word("<std:name>"),
        Op::HubList => Command::new("hub list"),
        Op::HubSet { name: n, .. } => name(Command::new("hub set"), n, "<name>"),
        Op::HubShow { name: n, .. } => name(Command::new("hub show"), n, "<name>"),
        Op::HubFill { name: n, .. } => name(Command::new("hub fill"), n, "<name>"),
        Op::HubRemove { name: n } => name(Command::new("hub remove"), n, "<name>"),
        Op::RuleShow {
            name: n,
            limit,
            offset,
        } => number(
            number(
                name(Command::new("rule show"), n, "<name>"),
                "--limit",
                limit,
            ),
            "--offset",
            offset,
        ),
        Op::OntologyRemoveType { name: n } => {
            name(Command::new("ontology remove").word("type"), n, "<name>")
        }
        Op::OntologyRemoveStatus { name: n } => {
            name(Command::new("ontology remove").word("status"), n, "<name>")
        }
        Op::OntologyRemoveRelation { name: n } => name(
            Command::new("ontology remove").word("relation"),
            n,
            "<name>",
        ),
        Op::OntologyAdoptStd {
            map,
            dry_run,
            field_to_tag,
        } => {
            let mut c = Command::new("ontology adopt-std");
            for f in field_to_tag {
                c = name(c.word("--field-to-tag"), f, "<field>");
            }
            for m in map {
                c = match m
                    .split_once('=')
                    .and_then(|(a, b)| Some((ValidName::new(a)?, ValidName::new(b)?)))
                {
                    Some((a, b)) => c.word("--map").names(&[a, b], "="),
                    None => c.word("--map").word("<from>=<to>"),
                };
            }
            if *dry_run { c.word("--dry-run") } else { c }
        }
        Op::OntologyRemoveRelationField { relation, field } => name(
            name(
                Command::new("ontology remove field").word("--relation"),
                relation,
                "<relation>",
            ),
            field,
            "<name>",
        ),
        Op::OntologyRemoveField { type_name, field } => name(
            name(Command::new("ontology remove field"), type_name, "<type>"),
            field,
            "<name>",
        ),
        Op::OntologySetField {
            type_name,
            field,
            role,
            values,
        } => {
            let mut c = name(
                name(Command::new("ontology set field"), type_name, "<type>"),
                field,
                "<field>",
            );
            if let Some(role) = role {
                c = c.word("--role").word(role_word(role));
            }
            if let Some(values) = values {
                c = c.field_values(values);
            }
            c
        }
        Op::GraphUnload { alias: a } => alias(Command::new("graph unload"), a),
        Op::GraphPrimary { alias: a } => alias(Command::new("graph primary"), a),
        Op::GraphRename { from, to } => alias(alias(Command::new("graph rename"), from), to),
        // A path is free text: a placeholder.
        Op::GraphMove { alias: a, .. } => alias(Command::new("graph move"), a).word("<path>"),
        // Setting values are free text: only placeholders.
        Op::GraphSet { alias: a, .. } => alias(Command::new("graph set"), a)
            .word("--<setting>")
            .word("<value>"),
        Op::NodeEdit {
            node: n,
            title,
            kind,
            note,
            sets: s,
            unsets,
            locator,
            tag,
            untag,
        } => {
            let c = node(Command::new("node edit"), n, "<node>");
            let c = text(c, "--locator", locator, "<locator>");
            let c = text(c, "--title", title, "<title>");
            let c = opt(c, "--type", kind, &|c, v| name(c, v, "<type>"));
            let mut c = sets(text(c, "--note", note, "<text>"), s);
            for u in unsets {
                c = name(c.word("--unset"), u, "<field>");
            }
            tag_words(tag_words(c, "--tag", tag), "--untag", untag)
        }
        // A key is the caller's text (an email, say): only a placeholder.
        Op::EntityKey {
            node: n, action, ..
        } => node(Command::new("entity key"), n, "<node>")
            .word(match action {
                KeyAction::Add => "add",
                KeyAction::Remove => "remove",
            })
            .word("<key>"),
        // R1: a dry run's retry stays a dry run, so following the next never writes.
        Op::EntityLink { a, b, dry_run } => {
            let link = node(node(Command::new("entity link"), a, "<a>"), b, "<b>");
            if *dry_run {
                link.word("--dry-run")
            } else {
                link
            }
        }
        Op::EntityUnlink { a, b } => node(node(Command::new("entity unlink"), a, "<a>"), b, "<b>"),
        Op::EntityDeny { a, b, .. } => node(node(Command::new("entity deny"), a, "<a>"), b, "<b>")
            .word("--reason")
            .word("<text>"),
        Op::EntityUndeny { a, b } => node(node(Command::new("entity undeny"), a, "<a>"), b, "<b>"),
        Op::EntityShow { node: n } => node(Command::new("entity show"), n, "<node>"),
        Op::Who { limit, .. } => number(Command::new("who").word("<who>"), "--limit", limit),
        Op::NodeWho { node: n } => node(Command::new("node who"), n, "<node>"),
        Op::NodeNote {
            node: n,
            append: _,
            evidence,
            evidence_kind,
        } => {
            let mut c = node(Command::new("node note"), n, "<node>")
                .word("--append")
                .word("<text>");
            for _ in evidence {
                c = c.word("--evidence").word("<pointer>");
            }
            opt(c, "--evidence-kind", evidence_kind, &|c, v| {
                name(c, v, "<kind>")
            })
        }
        // The pointer and reason are the caller's text: placeholders, never echoed.
        Op::NodeEvidence {
            node: n,
            action,
            pointer: given,
            evidence_kind,
            reason: _,
            at,
            removed,
            redact,
            all,
        } => {
            let c = node(Command::new("node evidence"), n, "<node>");
            match action {
                umx_protocol::EvidenceAction::List => {
                    let c = c.word("list");
                    if *removed { c.word("--removed") } else { c }
                }
                umx_protocol::EvidenceAction::Add => opt(
                    c.word("add").word("<pointer>"),
                    "--evidence-kind",
                    evidence_kind,
                    &|c, v| name(c, v, "<kind>"),
                ),
                umx_protocol::EvidenceAction::Remove => {
                    let c = c
                        .word("remove")
                        .word("<pointer>")
                        .word("--reason")
                        .word("<reason>");
                    let c = if *redact { c.word("--redact") } else { c };
                    number(c, "--at", at)
                }
                umx_protocol::EvidenceAction::Refresh => {
                    let c = c.word("refresh");
                    let c = if given.is_some() {
                        c.word("<pointer>")
                    } else {
                        c
                    };
                    let c = if *all { c.word("--all") } else { c };
                    number(c, "--at", at)
                }
            }
        }
        Op::NodeStatus {
            node: n,
            status,
            reason,
            label,
            place,
            evidence,
            evidence_kind,
        } => {
            let c = name(
                node(Command::new("node status"), n, "<node>"),
                status,
                "<status>",
            );
            let c = text(c, "--reason", reason, "<why>");
            let c = opt(c, "--label", label, &|c, v| name(c, v, "<label>"));
            // ws008 TM6: the anchor as a node; the pointer is the caller's text, so only a placeholder.
            let c = opt(c, "--in", place, &|c, v| node(c, v, "<anchor>"));
            let c = text(c, "--evidence", evidence, "<pointer>");
            opt(c, "--evidence-kind", evidence_kind, &|c, v| {
                name(c, v, "<kind>")
            })
        }
        Op::Check {
            rules,
            rule,
            after,
            lint,
            kind,
            offset,
            ..
        } => {
            let c = Command::new("check");
            let c = if *rules { c.word("--rules") } else { c };
            let c = opt(c, "--rule", rule, &|c, value| {
                let plain = value.strip_prefix("std:").unwrap_or(value);
                match ValidName::new(plain) {
                    Some(name) if value.starts_with("std:") => c.qualified("std:", &name),
                    Some(name) => c.name(&name),
                    None => c.word("<rule>"),
                }
            });
            let c = opt(c, "--after", after, &|c, value| node(c, value, "<id>"));
            let c = if *lint { c.word("--lint") } else { c };
            let c = match kind.as_deref().and_then(umx_services::LintKind::parse) {
                Some(k) => c.word("--kind").word(k.as_str()),
                None => c,
            };
            number(c, "--offset", offset)
        }
        Op::Search {
            text: _,
            within,
            limit,
            along,
            also,
            tags,
            no_tags,
        } => {
            // search-09: one `--also <words>` per phrasing sent, so the retry ranks the same matches; never the text.
            let c = also
                .iter()
                .fold(Command::new("search").word("<words>"), |c, _| {
                    c.word("--also").word("<words>")
                });
            let c = opt(c, "--within", within, &|c, v| node(c, v, "<node>"));
            let c = tag_words(tag_words(c, "--tag", tags), "--no-tag", no_tags);
            along_words(number(c, "--limit", limit), along)
        }
        Op::EmbeddingsRepair => Command::new("embeddings repair"),
        Op::EmbeddingsAllow { node: n, field } => {
            let c = node(Command::new("embeddings allow").word("--node"), n, "<node>");
            name(c.word("--field"), field, "<field>")
        }
        Op::SimilarityRebuild => Command::new("similarity rebuild"),
        Op::SimilarityPairs {
            k,
            min,
            model,
            same_graph,
            limit,
            offset,
            rows,
        } => {
            let c = number(Command::new("similarity pairs"), "--k", k);
            let c = opt(c, "--min", min, &|c, _| c.word("<score>"));
            let c = opt(c, "--model", model, &|c, _| c.word("<model>"));
            let c = if *same_graph {
                c.word("--same-graph")
            } else {
                c
            };
            let c = number(number(c, "--limit", limit), "--offset", offset);
            match rows {
                umx_protocol::PairRows::Full => c,
                umx_protocol::PairRows::Ids => c.word("--rows").word("ids"),
            }
        }
        Op::Neighbours {
            node: n,
            relation,
            direction,
            limit,
            offset,
            tags,
            no_tags,
            entities,
        } => {
            let c = node(Command::new("neighbours"), n, "<node>");
            let c = opt(c, "--relation", relation, &|c, v| name(c, v, "<relation>"));
            let c = direction_word(c, direction);
            let c = tag_words(tag_words(c, "--tag", tags), "--no-tag", no_tags);
            let c = match entities {
                Some(e) => c.word("--entities").word(e.as_str()),
                None => c,
            };
            number(number(c, "--limit", limit), "--offset", offset)
        }
        Op::Walk {
            node: n,
            depth,
            relation,
            direction,
            within,
            limit,
            offset,
            tags,
            no_tags,
        } => {
            let c = number(node(Command::new("walk"), n, "<node>"), "--depth", depth);
            let c = opt(c, "--relation", relation, &|c, v| name(c, v, "<relation>"));
            let c = direction_word(c, direction);
            let c = opt(c, "--within", within, &|c, v| node(c, v, "<node>"));
            let c = tag_words(tag_words(c, "--tag", tags), "--no-tag", no_tags);
            number(number(c, "--limit", limit), "--offset", offset)
        }
        Op::Path {
            from,
            to,
            max_depth,
            relation,
            direction,
        } => {
            let c = node(node(Command::new("path"), from, "<from>"), to, "<to>");
            let c = number(c, "--max-depth", max_depth);
            let c = opt(c, "--relation", relation, &|c, v| name(c, v, "<relation>"));
            direction_word(c, direction)
        }
        Op::EmbeddingsReembed { dry_run, model } => {
            let c = Command::new("embeddings reembed");
            let c = if *dry_run { c.word("--dry-run") } else { c };
            match model.as_deref().map(ModelKey::parse) {
                Some(Ok(k)) => c.word("--model").model_key(&k),
                Some(Err(_)) => c.word("--model").word("<model_key>"),
                None => c,
            }
        }
        Op::NodeHistory { node: n, limit } => number(
            node(Command::new("node history"), n, "<node>"),
            "--limit",
            limit,
        ),
        Op::ConfigShow { .. } => Command::new("config show"),
        Op::ConfigCheck { .. } => Command::new("config check"),
        Op::ConfigSet { .. } => Command::new("config set").word("<key>").word("<value>"),
        Op::ConfigUnset { .. } => Command::new("config unset").word("<key>"),
        Op::ConfigImport { .. } => Command::new("config import")
            .word("--from")
            .word("flowspace3"),
        Op::EmbeddingsStatus => Command::new("embeddings status"),
        Op::EmbeddingsProbe { embedder } => opt(
            Command::new("embeddings probe"),
            "--embedder",
            embedder,
            &|c, v| match ValidName::alias(v) {
                Some(n) => c.name(&n),
                None => c.word("<name>"),
            },
        ),
        // The ops are batch content: never quoted, only the file placeholder.
        // The query and its values are free text: placeholders.
        Op::Query {
            text: _,
            limit,
            offset,
            params,
            along,
        } => {
            let mut c = along_words(Command::new("query").word("<cypher>"), along);
            for p in params {
                let flag = match p.value {
                    SetValue::Raw(_) => "--param",
                    SetValue::Str(_) => "--param-str",
                    SetValue::Json(_) => "--param-json",
                };
                c = c.word(flag).word("<name=value>");
            }
            number(number(c, "--limit", limit), "--offset", offset)
        }
        Op::Batch {
            dry_run,
            reason,
            echo_sentences,
            refs,
            related,
            idempotency_key,
            ..
        } => {
            let c = related_flag(
                Command::new("batch").word("--file").word("<file>"),
                *related,
            );
            let c = if refs.is_empty() {
                c
            } else {
                c.word("--refs-from").word("<receipt>")
            };
            let c = if *dry_run { c.word("--dry-run") } else { c };
            let c = if *echo_sentences {
                c.word("--echo-sentences")
            } else {
                c
            };
            // A retry keeps its key, so it stays at most once (ws003 s09); the key itself is a placeholder.
            let c = if idempotency_key.is_some() {
                c.word("--idempotency-key").word("<key>")
            } else {
                c
            };
            if reason.is_some() {
                c.word("--reason").word("<reason>")
            } else {
                c
            }
        }
        // Never shown: it names no node and no other graph, so it cannot be ambiguous. The typed alias is not kept.
        Op::GraphConfirmPolicy { alias: a } => alias(Command::new("graph confirm-policy"), a),
        Op::UiCommand { command } => {
            let noted = |c: Command, note: &Option<String>| text(c, "--note", note, "<note>");
            let nodes = |mut c: Command, nodes: &[String]| {
                for n in nodes {
                    c = node(c, n, "<node>");
                }
                c
            };
            match command {
                UiCmd::Focus {
                    node: n,
                    hops,
                    note,
                } => noted(
                    node(Command::new("ui focus"), n, "<node>")
                        .word("--hops")
                        .count(u64::from(*hops)),
                    note,
                ),
                UiCmd::Highlight {
                    nodes: n,
                    add,
                    note,
                } => {
                    let c = nodes(Command::new("ui highlight"), n);
                    noted(if *add { c.word("--add") } else { c }, note)
                }
                UiCmd::Filter { show, hide, note } => {
                    let mut c = Command::new("ui filter");
                    for _ in show {
                        c = c.word("--show").word("<kind>:<name>");
                    }
                    for _ in hide {
                        c = c.word("--hide").word("<kind>:<name>");
                    }
                    noted(c, note)
                }
                UiCmd::Fit { nodes: n, note } => noted(nodes(Command::new("ui fit"), n), note),
                UiCmd::Clear { note } => noted(Command::new("ui clear"), note),
                // Never ambiguous (it names graphs, not nodes); shown with placeholders only.
                UiCmd::Open {
                    only, note, raise, ..
                } => {
                    let c = Command::new("ui open").word("--graph").word("<graphs>");
                    let c = if *only { c.word("--only") } else { c };
                    noted(if *raise { c.word("--raise") } else { c }, note)
                }
            }
        }
        Op::UiSelection => Command::new("ui selection"),
        // Only the view sends these, naming its graphs, so neither is ever ambiguous.
        Op::UiWait { .. } => Command::new("ui wait"),
        Op::UiSelect { .. } => Command::new("ui select"),
    }
}

/// What `umx config import` wrote (or would write), as `data`: the machine keys and values (none is a secret) and the
/// next steps. Whether the other tool uses a key is said; its name and value never are.
fn config_imported(v: &umx_services::ConfigImported) -> Value {
    let set: Vec<Value> = v
        .entries
        .iter()
        .map(|(k, value)| json!({ "key": k, "value": value }))
        .collect();
    let e = &v.instance;
    let mut next = Vec::new();
    if v.dry_run {
        next.push(format!("{} (writes these keys)", v.write.as_str()));
    }
    next.push(format!("umx embeddings probe --embedder {e}"));
    next.push(format!(
        "umx config set embedder.active {e} (tell each graph's owner first), then umx daemon restart"
    ));
    json!({
        "from": v.from,
        "source": v.source.as_str(),
        "file": v.file.as_str(),
        "embedder": e,
        "provider": v.provider,
        "set": set,
        "key": v.key.as_ref().map(ToString::to_string),
        "dry_run": v.dry_run,
        "written": !v.dry_run,
        "replaced": v.replaced,
        "auth": "cli",
        "auth_note": if v.source_uses_key {
            "flowspace3 signs in to this provider with a key; umx copies no key and signs in with az login (auth cli)"
        } else {
            "umx signs in with az login (auth cli)"
        },
        "model_from": if v.model_inferred { "the deployment name" } else { "the config" },
        "active_unchanged": "embedder.active is not changed",
        "trust": format!(
            "{e} is in the machine config, so it is trusted (D62): graphs whose embedder setting is trusted may send their text to it once it is in use; graphs that must stay local never use it"
        ),
        "next": next,
    })
}

/// `umx embeddings status`: per loaded graph, its key, vectors pending, held and done, the instance that embeds it,
/// and `vectors_digest` (ws004 s34b): a cache key that moves when its stored vectors' space, nodes or text hashes do;
/// null for a graph with no vector space or not open (an unnamed read never shows it). `memo` keeps each digest
/// while its graph's stamp holds, so a repeated call hashes nothing. Nothing is sent and no credential is read; a
/// live check is `umx embeddings probe --graph <alias>`.
pub fn embeddings_status(
    services: &umx_services::Services,
    usable: &dyn Fn(&GraphDir, &str) -> bool,
    memo: &mut umx_services::DigestMemo,
) -> Envelope {
    let rows = services.embeddings_status(usable);
    // Only graphs listed now are kept: an unloaded one's entry goes.
    memo.retain(|alias, _| rows.iter().any(|r| &r.alias == alias));
    let graphs: Vec<Value> = rows
        .iter()
        .map(|r| {
            let digest = r
                .vectors_stamp
                .as_ref()
                .and_then(|stamp| services.vectors_digest_kept(&r.alias, stamp, memo));
            // Counts umx could not read (vectors_unreadable at load) are unknown, never current (R177-1).
            let state = match (&r.instance, r.pending, r.held) {
                (None, _, _) => "no_embedder",
                (Some(_), None, _) | (Some(_), _, None) => "unknown",
                (Some(_), Some(p), _) if p > 0 => "embedding",
                (Some(_), _, Some(h)) if h > 0 => "held",
                (Some(_), Some(_), Some(_)) => "current",
            };
            json!({
                "graph": r.alias,
                "key": r.key.as_ref().map(ToString::to_string),
                "policy": r.policy.effective.name(),
                "visibility": r.policy.effective.visibility.as_str(),
                "embedder_setting": r.policy.effective.embedder.as_str(),
                "nodes": r.nodes,
                "vectors_pending": r.pending,
                "vectors_held": r.held,
                "vectors_done": r.done,
                "embedder": r.instance,
                "locality": r.locality.map(|l| match l {
                    umx_core::ports::Locality::Local => "local",
                    umx_core::ports::Locality::Hosted => "hosted",
                }),
                "state": state,
                "vectors_digest": digest.as_ref().map(umx_core::similar::digest_hex),
                "check": format!("umx embeddings probe --graph {}", r.alias),
            })
        })
        .collect();
    let env = Envelope::ok(
        "embeddings status",
        json!({
            "graphs": graphs,
            "reachable": "not checked: status sends nothing; run the check command of a graph to probe its embedder",
        }),
    );
    // search-05: a field in the wrong role group is pointed out where its vectors are counted (open graphs only).
    rows.iter()
        .flat_map(|r| services.role_hints_unnamed(&r.alias))
        .fold(env, |env, p| env.note(issue(&p, "embeddings status")))
}

/// What `umx embeddings probe` found, as `data`. Never the vector or the text: only its width and length.
pub fn probed(
    instance: &str,
    graph: Option<&str>,
    p: &umx_services::embedding::Probed,
    latency_ms: u128,
) -> Value {
    let norm = p.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    json!({
        "embedder": instance,
        "graph": graph,
        "adapter": p.adapter,
        "key": p.key.to_string(),
        "model": p.key.model(),
        "dims": p.vector.len(),
        "norm": (norm * 1e6).round() / 1e6,
        "latency_ms": latency_ms,
        "locality": match p.locality {
            umx_core::ports::Locality::Local => "local",
            umx_core::ports::Locality::Hosted => "hosted",
        },
        "credential": p.credential.as_ref().map(|c| json!({ "kind": c.kind, "from": c.from })),
        "ready": true,
        "embedded": "umx's own fixed probe sentence, never graph text",
    })
}

fn core_sets(sets: &[SetArg]) -> Vec<(String, value::SetValue)> {
    sets.iter()
        .map(|s| {
            let v = match &s.value {
                SetValue::Raw(v) => value::SetValue::Raw(v.clone()),
                SetValue::Str(v) => value::SetValue::Str(v.clone()),
                SetValue::Json(v) => value::SetValue::Json(v.clone()),
            };
            (s.field.clone(), v)
        })
        .collect()
}

/// A graph's embedding section as the envelope shows it: key, text form, policy and, for a pair that is not open or
/// secured, its two settings (D53); keys umx does not know are not
/// shown (they are graph text; `umx check` lists their names). `null` when the graph has no section.
fn embedding_json(e: Option<&GraphEmbedding>) -> Value {
    e.map_or(Value::Null, |e| {
        // As the file says it (D53): the legacy word, and the two settings only for a pair that is not open or secured.
        let (word, visibility, embedder) = e.policy.written();
        let mut out = json!({
            "key": e.key.as_ref().map(|k| k.to_string()),
            "text_form": e.text_form,
            "policy": if word == umx_core::Legacy::Secured { "secured" } else { "open" },
        });
        if let (Some(v), Some(r)) = (visibility, embedder) {
            out["visibility"] = json!(v.as_str());
            out["embedder"] = json!(r.as_str());
        }
        out
    })
}

/// `umx similarity rebuild`'s answer: the graph's space, settings (floor as written in the files), edge count and
/// files written, with where the floor came from (ws004 s26).
pub(crate) fn similarity_rebuilt(
    command: &str,
    outcome: Result<Done<umx_services::SimilarityRebuilt>, Problem>,
) -> Envelope {
    reply(command, outcome, |v| {
        json!({
            "graph": v.alias, "model": v.key.to_string(), "k": v.k,
            "floor": umx_core::similar::format_units(v.floor).parse::<f64>().unwrap_or(0.0),
            "floor_calibrated": v.floor_calibrated, "floor_source": v.floor_source.word(),
            "floor_run": v.floor_run, "edges": v.edges, "files_written": v.files_written,
        })
    })
}

/// What `umx similarity pairs` asks, after the scope flags are checked (ws004 s31).
pub(crate) fn pairs_ask(request: &Request) -> Result<umx_services::PairsAsk, Box<Envelope>> {
    let command = "similarity pairs";
    if let Some(problem) = scope_usage(request) {
        return Err(Box::new(Envelope::fail(command, issue(&problem, command))));
    }
    let Some(Op::SimilarityPairs {
        k,
        min,
        model,
        same_graph,
        limit,
        offset,
        rows,
    }) = &request.op
    else {
        return Err(Box::new(Envelope::fail(
            command,
            internal("not_similarity_pairs"),
        )));
    };
    Ok(umx_services::PairsAsk {
        k: *k,
        min: min.clone(),
        model: model.clone(),
        same_graph: *same_graph,
        limit: *limit,
        offset: *offset,
        // The wire's word to the services' own (services never depends on umx-protocol).
        fields: match rows {
            umx_protocol::PairRows::Full => umx_services::PairRows::Full,
            umx_protocol::PairRows::Ids => umx_services::PairRows::Ids,
        },
    })
}

/// `umx similarity pairs`' reply: the list shape, one row per (node, one of its top k), best first, and how the run
/// was set up (the key and why, k, the floor and where it came from, the graphs compared and those left out). The
/// rows are written as JSON straight from the result ([`pairs_rows_json`]), never as a `Value` per row, so 250,000
/// rows cost about their own bytes (ws004 s34a). `--rows ids` rows carry only [`PAIRS_ID_COLUMNS`] (ws004 s34b).
pub(crate) fn pairs_reply(
    command: &str,
    outcome: Result<Done<umx_services::PairsResult>, Problem>,
) -> Envelope {
    let rows = outcome
        .as_ref()
        .ok()
        .map(|done| pairs_rows_json(&done.value));
    let env = reply(command, outcome, |v| {
        let mut columns: Vec<&str> = match v.fields {
            umx_services::PairRows::Full => PAIRS_COLUMNS.to_vec(),
            umx_services::PairRows::Ids => PAIRS_ID_COLUMNS.to_vec(),
        };
        // F228: named when some row on the page names a flagged title.
        if v.fields == umx_services::PairRows::Full
            && v.pairs.iter().any(|p| {
                v.names[p.node as usize].title_flagged || v.names[p.other as usize].title_flagged
            })
        {
            columns.push(SECRET_SUSPECTED);
        }
        let mut data = list(&columns, Vec::new(), Some((Some(v.total), v.truncated)));
        data["model"] = json!(v.model.as_ref().map(ToString::to_string));
        data["model_source"] = json!(v.model_source.word());
        data["k"] = json!(v.k);
        data["min"] = json!({
            "value": pair_score(v.min.units), "source": v.min.source, "calibrated": v.min.calibrated, "run": v.min.run,
        });
        data["graphs"] = json!(v.graphs);
        // Each compared graph's vectors_digest, taken with its rows in the copy's hold (ws004 s36, F206).
        data["compared"] = v
            .graphs
            .iter()
            .zip(&v.digests)
            .map(|(g, d)| json!({"graph": g, "vectors_digest": umx_core::similar::digest_hex(d)}))
            .collect();
        data["skipped_model"] = v
            .skipped_model
            .iter()
            .map(|s| {
                json!({
                    "graph": s.graph, "model": s.model.as_ref().map(ToString::to_string), "reason": s.reason,
                })
            })
            .collect();
        data["same_graph"] = json!(v.same_graph);
        data["nodes"] = json!(v.nodes);
        data
    });
    match rows {
        Some(rows) => env.with_rows_json(rows),
        None => env,
    }
}

/// A score as the reply gives it: ten-thousandths to a number at 4 decimals.
fn pair_score(units: i32) -> f64 {
    umx_core::similar::format_units(units)
        .parse::<f64>()
        .unwrap_or(0.0)
}

/// The columns of a full `umx similarity pairs` row.
const PAIRS_COLUMNS: [&str; 14] = [
    "graph",
    "address",
    "policy",
    "id",
    "type",
    "title",
    "other_graph",
    "other_address",
    "other_policy",
    "other_id",
    "other_type",
    "other_title",
    "score",
    "rank",
];

/// The columns of a `--rows ids` row (ws004 s34b): the five the graph view reads.
const PAIRS_ID_COLUMNS: [&str; 5] = ["graph", "id", "other_graph", "other_id", "score"];

/// One node's pieces of row text, each written once however many rows name it. `ids` and `other_ids` are shared by
/// both row forms, so a `--rows ids` row's five fields are the full row's own bytes (ws004 s34b). `flagged`: umx's
/// secret check flags its title (F228); only a full row shows a title, so only a full row names it.
struct RowParts {
    flagged: bool,
    /// `"graph":<g>,"id":<id>`
    ids: String,
    /// `"other_graph":<g>,"other_id":<id>`
    other_ids: String,
    /// The rest of a full row; None for `--rows ids`, whose finish resolved no type or title.
    full: Option<FullParts>,
}

/// The pieces only a full row has, in serde_json's sorted key order around the shared ones.
struct FullParts {
    /// `{"address":<a>,`
    lead: String,
    /// `,"other_address":<a>,`
    other_lead: String,
    /// `,"other_policy":<p>,"other_title":<t>,"other_type":<k>,`
    other_tail: String,
    /// `"policy":<p>,`
    policy: String,
    /// `,"title":<t>,"type":<k>}`
    tail: String,
}

/// The rows of `umx similarity pairs` as one JSON array, byte for byte what serde_json writes for a `Value` object
/// per row (keys in its sorted order). Each node's parts are written once however many rows name it; the text is
/// sized before it is written, so it is never copied to grow. `--rows ids` rows are built from the same `ids` and
/// `other_ids` parts and score text as full rows, so their five fields are the full row's by construction.
fn pairs_rows_json(v: &umx_services::PairsResult) -> String {
    let text = |s: &str| serde_json::to_string(s).expect("a string serialises");
    let full = v.fields == umx_services::PairRows::Full;
    let parts: Vec<RowParts> = v
        .names
        .iter()
        .map(|n| {
            let graph = &v.graphs[n.graph as usize];
            let (graph_text, id) = (text(graph), text(&n.id.to_text()));
            RowParts {
                flagged: n.title_flagged,
                ids: format!("\"graph\":{graph_text},\"id\":{id}"),
                other_ids: format!("\"other_graph\":{graph_text},\"other_id\":{id}"),
                full: full.then(|| {
                    let address = text(&umx_services::address(graph, n.id));
                    let policy = text(v.policies[n.graph as usize]);
                    let (title, kind) = (text(&n.title), text(&n.kind));
                    FullParts {
                        lead: format!("{{\"address\":{address},"),
                        other_lead: format!(",\"other_address\":{address},"),
                        other_tail: format!(
                            ",\"other_policy\":{policy},\"other_title\":{title},\"other_type\":{kind},"
                        ),
                        policy: format!("\"policy\":{policy},"),
                        tail: format!(",\"title\":{title},\"type\":{kind}}}"),
                    }
                }),
            }
        })
        .collect();
    let both = |p: &umx_services::Pair| (&parts[p.node as usize], &parts[p.other as usize]);
    // `,"score":-0.1234` and `{}` or `"rank":10` are at most 30 bytes; a flagged row's
    // `,"secret_suspected":["field:title","field:other_title"]` is 56.
    let size = 2 + v
        .pairs
        .iter()
        .map(|p| {
            let (a, b) = both(p);
            let rest = match (&a.full, &b.full) {
                (Some(af), Some(bf)) => {
                    af.lead.len()
                        + bf.other_lead.len()
                        + bf.other_tail.len()
                        + af.policy.len()
                        + af.tail.len()
                        + if a.flagged || b.flagged { 56 } else { 0 }
                }
                _ => 0,
            };
            1 + a.ids.len() + b.other_ids.len() + 30 + rest
        })
        .sum::<usize>();
    let mut out = String::with_capacity(size);
    out.push('[');
    for (i, p) in v.pairs.iter().enumerate() {
        let (a, b) = both(p);
        if i > 0 {
            out.push(',');
        }
        let score = serde_json::to_string(&pair_score(p.score)).expect("a number serialises");
        match (&a.full, &b.full) {
            (Some(af), Some(bf)) => {
                out.push_str(&af.lead);
                out.push_str(&a.ids);
                out.push_str(&bf.other_lead);
                out.push_str(&b.other_ids);
                out.push_str(&bf.other_tail);
                out.push_str(&af.policy);
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("\"rank\":{}", p.rank));
                out.push_str(",\"score\":");
                out.push_str(&score);
                // `secret_suspected` sorts between `score` and `title`, where serde_json would put it.
                if a.flagged || b.flagged {
                    out.push_str(",\"secret_suspected\":[");
                    let flags = [(a.flagged, "field:title"), (b.flagged, "field:other_title")];
                    let mut first = true;
                    for (_, flag) in flags.iter().filter(|(on, _)| *on) {
                        if !first {
                            out.push(',');
                        }
                        first = false;
                        out.push('"');
                        out.push_str(flag);
                        out.push('"');
                    }
                    out.push(']');
                }
                out.push_str(&af.tail);
            }
            _ => {
                out.push('{');
                out.push_str(&a.ids);
                out.push(',');
                out.push_str(&b.other_ids);
                out.push_str(",\"score\":");
                out.push_str(&score);
                out.push('}');
            }
        }
    }
    out.push(']');
    out
}

/// `ontology show`'s data: the ontology umx acts on, with each field's role as umx acts on it (D56) and whether it
/// is the kind's default, every symmetric relation, `relation_fields` always present, and what std gives (req-98):
/// `std` (the version, or null), `std: true|false` on each type, field and relation field, the std names among
/// `statuses` and `relations` (`std_statuses`, `std_relations`; those two stay lists of names), and for a std
/// graph `std_any_fields`, the std fields any node (or edge) has without a declaration.
fn ontology_json(o: &umx_core::Ontology) -> Value {
    use umx_core::standard::{self, Scope};
    let is_std = o.std_version().is_some();
    let mut v = serde_json::to_value(o).expect("an ontology serialises");
    for (t, decl) in o.types.iter().enumerate() {
        v["types"][t]["std"] = json!(is_std && standard::std_type(&decl.name).is_some());
        for (f, field) in decl.fields.iter().enumerate() {
            let at = &mut v["types"][t]["fields"][f];
            at["role"] = json!(field.effective_role().as_str());
            at["role_default"] = json!(field.role.is_none());
            at["std"] = json!(
                is_std
                    && standard::std_field(&field.name)
                        .is_some_and(|s| s.types.contains(&decl.name.as_str()))
            );
        }
    }
    // Every symmetric relation of the graph (D49), the built-in ones included, not only the marked ones.
    v["symmetric"] = json!(o.symmetric_kinds());
    // F92: always present (graph.json leaves it out when empty), so an agent sees where edge fields go.
    let mut relation_fields = json!(o.relation_fields);
    for (relation, fields) in &o.relation_fields {
        for (f, field) in fields.iter().enumerate() {
            relation_fields[relation][f]["std"] = json!(
                is_std
                    && standard::std_field(&field.name)
                        .is_some_and(|s| s.relations.contains(&relation.as_str()))
            );
        }
    }
    v["relation_fields"] = relation_fields;
    v["std"] = json!(o.std);
    let std_names = |names: &[String], known: &dyn Fn(&str) -> bool| -> Vec<String> {
        names
            .iter()
            .filter(|n| is_std && known(n))
            .cloned()
            .collect()
    };
    v["std_statuses"] = json!(std_names(&o.statuses, &|n| standard::std_status(n).is_some()));
    v["std_relations"] = json!(std_names(&o.relations, &|n| standard::std_relation(n).is_some()));
    if is_std {
        v["std_any_fields"] = standard::fields()
            .filter(|f| f.scope != Scope::Placed && !f.flags.provenance)
            .map(|f| {
                json!({
                    "name": f.name,
                    "kind": f.kind.as_str(),
                    "role": f.role.as_str(),
                    "on": if f.scope == Scope::AnyNode { "node" } else { "node_or_edge" },
                })
            })
            .collect();
    }
    v
}

pub(crate) fn reply<T>(
    command: &str,
    outcome: Result<Done<T>, Problem>,
    data: impl FnOnce(&T) -> Value,
) -> Envelope {
    match outcome {
        Ok(done) => done
            .notes
            .iter()
            .fold(Envelope::ok(command, data(&done.value)), |env, w| {
                env.note(issue(w, command))
            }),
        Err(p) => Envelope::fail(command, issue(&p, command)),
    }
}

/// Renders a problem through its catalogue row. `<cmd>` is the command's label (`command`); a code without a
/// row is a bug and becomes `internal`.
pub fn issue(p: &Problem, command: &str) -> Issue {
    let Some(code) = Code::from_code(p.code) else {
        return internal("uncatalogued_code");
    };
    let spec = code.spec();
    let fills: Vec<(&str, Fill)> = slots(spec.message)
        .chain(slots(spec.next))
        .map(|slot| {
            if slot == "cmd" {
                return (slot, Fill::command(command));
            }
            let fill = match p.slots.iter().find(|(name, _)| *name == slot) {
                Some((_, Slot::Count(n))) => Fill::count(*n),
                Some((_, Slot::Word(w))) => Fill::text(w),
                Some((_, Slot::Sentence(sentence))) => Fill::rule_sentence(sentence.as_str()),
                Some((_, Slot::Command(c))) => Fill::checked_command(c.as_str()),
                Some((_, Slot::ModelKey(k))) => {
                    let text = k.to_string();
                    Fill::model_key(&text).unwrap_or_else(|| Fill::text(&text))
                }
                Some((_, Slot::RepoHint(url))) => {
                    Fill::repo_hint(url.as_ref().map_or("", |u| u.as_str()))
                }
                Some((_, Slot::File(file))) => {
                    let path = file.path();
                    Fill::graph_file(&path).unwrap_or_else(|| Fill::text(&path))
                }
                Some((_, other)) => Fill::text(&other.to_string()),
                // Left visible so the trigger test catches a missing slot.
                None => Fill::text(&format!("<{slot}>")),
            };
            (slot, fill)
        })
        .collect();
    let mut issue = Issue::fill(code, &fills);
    if let Some(subject) = &p.subject {
        issue = issue.with_subject(subject.clone());
    }
    if let Some(line) = p.line {
        issue = issue.with_line(line);
    }
    if let Some(rule) = &p.rule {
        issue = issue.with_rule(umx_protocol::RuleContext {
            name: rule.name.clone(),
            count: rule.count,
            nodes: rule.nodes.clone(),
        });
    }
    if !p.candidates.is_empty() {
        issue = issue.with_candidates(p.candidates.iter().map(candidate).collect());
    }
    issue
}

fn candidate(c: &Candidate) -> Value {
    serde_json::to_value(c).expect("a candidate serialises")
}

/// A node's columns in a query row: `id`, `type`, `title`, `status`.
fn node_row(n: &NodeCell) -> serde_json::Map<String, Value> {
    let mut row = serde_json::Map::new();
    row.insert("id".into(), json!(n.id.to_text()));
    row.insert("type".into(), json!(n.kind));
    row.insert("title".into(), json!(n.title));
    row.insert("status".into(), json!(n.status));
    row
}

/// The wire's `--direction`, or the verb's default.
fn direction_of(d: Option<umx_protocol::Direction>, default: Direction) -> Direction {
    match d {
        Some(umx_protocol::Direction::Out) => Direction::Out,
        Some(umx_protocol::Direction::In) => Direction::In,
        Some(umx_protocol::Direction::Both) => Direction::Both,
        None => default,
    }
}

/// `--direction <d>` when the command gave one.
fn direction_word(c: Command, d: &Option<umx_protocol::Direction>) -> Command {
    match d {
        Some(d) => c.word("--direction").word(d.as_str()),
        None => c,
    }
}

/// Whether a request names the similar_to relation (its rows then carry `score` and `derived`).
fn similar_to(relation: &Option<String>) -> bool {
    relation.as_deref() == Some(umx_core::ontology::SIMILAR_TO)
}

fn with_score<'a>(columns: &[&'a str], similar: bool, provenance: bool) -> Vec<&'a str> {
    columns
        .iter()
        .copied()
        .filter(|c| (*c != "score" || similar) && (*c != "derived" || similar || provenance))
        .collect()
}

/// An edge's cells: `edge` (its id; null for a derived similar_to edge, which is not a record), and with similar_to
/// named, `score` (4 decimals, as the files write it) and `derived`.
fn edge_cells(row: &mut serde_json::Map<String, Value>, e: &umx_services::EdgeCell, similar: bool) {
    row.insert("edge".into(), json!(e.id.map(|id| id.to_text())));
    if similar {
        let score = e.score.map(|s| {
            umx_core::similar::format_units(s)
                .parse::<f64>()
                .unwrap_or(0.0)
        });
        row.insert("score".into(), json!(score));
    }
    if similar || e.derived {
        row.insert("derived".into(), json!(e.derived));
    }
}

/// A similar_to result carries the model, k and floor its lists were made with (workshop 008 section 9).
fn similarity_meta(v: &mut Value, services: &Services, alias: &str, similar: bool) {
    if !similar {
        return;
    }
    v["similarity"] = match services.similarity_info(alias) {
        Some((key, k, floor)) => {
            let (calibrated, source, run) = services.similar_floor(alias, &key, floor);
            json!({
                "model": key.to_string(), "k": k,
                "floor": umx_core::similar::format_units(floor).parse::<f64>().unwrap_or(0.0),
                "floor_calibrated": calibrated, "floor_source": source.word(), "floor_run": run,
            })
        }
        None => Value::Null,
    };
}

/// `umx query`'s rows: the 007 list shape, each row an object keyed by column (the column order is `columns`). A row
/// with a flagged value names its columns (`field:<column>`, F241), and the list names the flags column, as every
/// list does (F188).
fn query_json(rows: &umx_services::QueryRows) -> Value {
    let objects: Vec<Value> = rows
        .rows
        .iter()
        .zip(&rows.flags)
        .map(|(row, flags)| {
            let row = Value::Object(
                rows.columns
                    .iter()
                    .zip(row)
                    .map(|(c, cell)| (c.clone(), cell_json(cell, rows.copies)))
                    .collect(),
            );
            with_flags(row, flags, false)
        })
        .collect();
    let mut columns: Vec<&str> = rows.columns.iter().map(String::as_str).collect();
    if any_flagged(&objects) {
        columns.push(SECRET_SUSPECTED);
    }
    let mut v = list(&columns, objects, Some((Some(rows.total), rows.truncated)));
    v["work"] = json!(rows.work);
    // A query that named similar_to: the lists each graph in scope holds (workshop 008 section 9), each floor marked
    // calibrated or not, with its source and calibration run (embeddings-expert B1, ws004 s26).
    if let Some(lists) = &rows.similarity {
        v["similarity"] = lists
            .iter()
            .map(|s| {
                json!({
                    "graph": s.graph, "model": s.key.to_string(), "k": s.k,
                    "floor": umx_core::similar::format_units(s.floor).parse::<f64>().unwrap_or(0.0),
                    "floor_calibrated": s.floor_calibrated, "floor_source": s.floor_source.word(),
                    "floor_run": s.floor_run,
                })
            })
            .collect();
    }
    with_along(&mut v, rows.along.as_ref());
    v
}

/// One query value as JSON: a node or edge in umx's shape (cypher-expert section 1.2), anything else plain. With
/// copies of one graph named (d-21) a node or edge also says its `copy`.
fn cell_json(cell: &umx_services::cypher::Cell, copies: bool) -> Value {
    use umx_services::cypher::Cell;
    match cell {
        Cell::Null => Value::Null,
        Cell::Bool(b) => json!(b),
        Cell::Int(i) => json!(i),
        Cell::Float(f) => json!(f),
        Cell::Str(s) => json!(s),
        Cell::List(items) => Value::Array(items.iter().map(|c| cell_json(c, copies)).collect()),
        Cell::Map(entries) => Value::Object(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), cell_json(v, copies)))
                .collect(),
        ),
        Cell::Node(n) => {
            if n.is_graph() {
                return json!({
                    "id": n.id.to_text(), "address": n.address, "type": "graph",
                    "title": n.title, "alias": n.graph,
                });
            }
            let mut v = json!({
                "id": n.id.to_text(), "address": n.address, "graph": n.graph, "type": n.kind, "title": n.title,
                "status": n.status,
            });
            if copies {
                v["copy"] = json!(n.graph);
            }
            // F241: the node's own flag may only add hiding; its row's flags are the authority (security).
            with_flags(v, &flag_if(n.title_flagged, "title"), false)
        }
        Cell::Edge(e) => {
            let mut v = json!({
                "id": e.id.to_text(), "address": e.address, "graph": e.graph, "type": e.kind,
                "from": e.from.to_text(), "to": e.to.to_text(),
            });
            if let Some(s) = &e.from_selector {
                v["from_selector"] = s.clone();
            }
            if let Some(s) = &e.to_selector {
                v["to_selector"] = s.clone();
            }
            if copies {
                v["copy"] = json!(e.graph);
            }
            v
        }
        Cell::Provenance(e) => {
            let mut value = json!({
                "id": null, "graph": e.graph, "type": e.kind,
                "from": e.from.to_text(), "to": e.to.to_text(), "derived": true,
            });
            if copies {
                value["copy"] = json!(e.graph);
            }
            value
        }
        // Derived from the lists: no id and no address (it is not a record). Named copies: its copy, as above.
        Cell::Similar(e) => {
            let mut v = json!({
                "id": null, "address": null, "graph": e.graph, "type": umx_core::ontology::SIMILAR_TO,
                "from": e.from.to_text(), "to": e.to.to_text(),
                "score": umx_core::similar::format_units(e.score).parse::<f64>().unwrap_or(0.0),
                "derived": true,
            });
            if copies {
                v["copy"] = json!(e.graph);
            }
            v
        }
    }
}

/// `graph scan` (ws002 s17): a list of rows, path first, plus what a write did.
fn scan_json(v: &umx_services::Scanned) -> Value {
    let rows: Vec<Value> = v
        .rows
        .iter()
        .map(|r| {
            json!({
                "path": r.path, "alias": r.alias, "id": r.id.map(|i| i.to_text()), "state": r.state,
                "from": r.from, "found_id": r.found_id.map(|i| i.to_text()), "secured": r.secured,
                "next": r.next.as_ref().map(|c| c.as_str()), "unique": r.unique,
            })
        })
        .collect();
    let columns = [
        "path", "alias", "id", "state", "from", "found_id", "secured", "next", "unique",
    ];
    let mut data = list(&columns, rows, Some((None, v.truncated)));
    data["total"] = json!(v.rows.len());
    data["repo"] = json!(v.repo.as_str());
    data["graphfile"] = json!(v.file.as_str());
    data["unreadable"] = json!(v.unreadable);
    data["added"] = json!(v.added);
    data["moved"] = json!(v.moved);
    data["dropped"] = json!(v.dropped);
    data["written"] = json!(v.written);
    data
}

/// `graph list --graphfile` (ws002 s17): one row per entry, and the sets.
fn graphfile_list_json(v: &umx_services::GraphfileListing) -> Value {
    let rows: Vec<Value> = v
        .rows
        .iter()
        .map(|r| {
            json!({
                "path": r.path, "alias": r.alias, "id": r.id.to_text(), "state": r.state,
                "loaded_as": r.loaded_as,
            })
        })
        .collect();
    let mut data = list(&["path", "alias", "id", "state", "loaded_as"], rows, None);
    data["repo"] = json!(v.repo.as_str());
    data["graphfile"] = json!(v.file.as_str());
    data["sets"] = json!(v.sets);
    data
}

/// `graph load --graphfile` (ws002 s17): what loaded (and which graphs are new to this machine), what did not.
fn graphfile_load_json(v: &umx_services::GraphfileLoad) -> Value {
    json!({
        "repo": v.repo.as_str(), "graphfile": v.file.as_str(), "set": v.set,
        "loaded": v.loaded.iter().map(|l| json!({
            "alias": l.alias, "path": l.path, "new_to_machine": l.new_to_machine, "already": l.already,
        })).collect::<Vec<_>>(),
        "not_loaded": v.not_loaded.iter().map(|n| json!({
            "alias": n.alias, "path": n.path, "code": n.code,
        })).collect::<Vec<_>>(),
    })
}

/// `--related` or `--no-related` as the call gave it (req-123).
fn related_flag(c: Command, related: Option<bool>) -> Command {
    match related {
        Some(true) => c.word("--related"),
        Some(false) => c.word("--no-related"),
        None => c,
    }
}

/// The graph and the nodes a node add or applied batch reply says it added, in order (req-123); None for any other
/// reply, a dry run or a failure. A node add resolved to an existing node by its entity key added none.
pub(crate) fn added_nodes(verb: &str, env: &Envelope) -> Option<(String, Vec<Id>)> {
    if env.status == umx_protocol::Status::Error {
        return None;
    }
    let data = &env.data;
    let parse = |v: &Value| v.as_str().and_then(|t| t.parse::<Id>().ok());
    match verb {
        // An add that `--on-key resolve` resolved to a node already there added nothing new (ws003 s08).
        "node add" if data["resolved"] == json!(true) => None,
        "node add" => Some((
            data["node"]["graph"].as_str()?.to_string(),
            vec![parse(&data["node"]["id"])?],
        )),
        "batch" if data["applied"] == json!(true) => Some((
            data["graph"].as_str()?.to_string(),
            data["rows"]
                .as_array()?
                .iter()
                .filter(|r| r["op"] == json!("node add") && r["existed"] != json!(true))
                .filter_map(|r| parse(&r["id"]))
                .collect(),
        )),
        _ => None,
    }
}

/// Puts the related check's rows in a node add or batch reply (each checked node's `related`: id, address, title,
/// score; a flagged title names `field:title`, F228) and adds its `related_skipped` hints.
pub(crate) fn with_related(
    mut env: Envelope,
    verb: &str,
    related: &umx_services::related::Related,
) -> Envelope {
    let alias = related.alias.as_str();
    let rows = |id: &Id| -> Option<Value> {
        let rows = related.rows.get(id)?;
        Some(json!(
            rows.iter()
                .map(|r| with_flags(
                    json!({
                        "id": r.id.to_text(),
                        "address": umx_services::address(alias, r.id),
                        "title": r.title,
                        "score": r.score,
                    }),
                    &flag_if(r.title_flagged, "title"),
                    false,
                ))
                .collect::<Vec<_>>()
        ))
    };
    let id_of = |v: &Value| v.as_str().and_then(|t| t.parse::<Id>().ok());
    // The floor the rows were cut at, and where it came from (embeddings-expert, ws004 s28).
    if let Some(f) = &related.floor {
        env.data["related_floor"] = json!({
            "value": f.value, "source": f.source.word(), "use": f.use_word(),
        });
    }
    match verb {
        "node add" => {
            if let Some(found) = id_of(&env.data["node"]["id"]).and_then(|id| rows(&id)) {
                env.data["related"] = found;
            }
        }
        _ => {
            if let Some(list) = env.data["rows"].as_array_mut() {
                for row in list.iter_mut().filter(|r| r["op"] == json!("node add")) {
                    if let Some(found) = id_of(&row["id"]).and_then(|id| rows(&id)) {
                        row["related"] = found;
                    }
                }
            }
        }
    }
    related
        .hints
        .iter()
        .fold(env, |env, p| env.note(issue(p, verb)))
}

/// The 007 list shape. `total` is null when the service could not count without a full scan (a lazy walk).
pub(crate) fn list(
    columns: &[&str],
    rows: Vec<Value>,
    paging: Option<(Option<usize>, bool)>,
) -> Value {
    let (total, truncated) = paging.unwrap_or((Some(rows.len()), false));
    json!({ "columns": columns, "rows": rows, "total": total, "truncated": truncated })
}

/// The wire's `--along` as the services' request (D71 step 12), mapped here once for every read that takes it.
fn along_request(along: &Option<umx_protocol::Along>) -> Option<umx_services::AlongRequest> {
    along.as_ref().map(|a| umx_services::AlongRequest {
        spine: a.spine.clone(),
        after: a.after.clone(),
        before: a.before.clone(),
    })
}

/// A filtered read's `along` object (D71 step 12): the spine and ends used, inclusive, and what it left out. Data,
/// beside the list; the hints say the same counts with their next commands.
fn with_along(data: &mut Value, along: Option<&umx_services::AlongInfo>) {
    if let Some(a) = along {
        data["along"] = json!({
            "spine": a.spine,
            "source": a.source.as_str(),
            "clock": a.clock.as_str(),
            "after": a.after,
            "before": a.before,
            "ends": "inclusive",
            "not_placed": a.not_placed,
            "by_rule": a.by_rule,
            "coarse": a.coarse,
            "spines": a.spines,
        });
    }
}

fn node_json(alias: &str, n: &Node) -> Value {
    let mut v = serde_json::to_value(n).expect("a node serialises");
    v["address"] = json!(umx_services::address(alias, n.id));
    v["graph"] = json!(alias);
    v
}

fn virtual_node_json(alias: &str, n: &Node, columns: Option<&[String]>) -> Value {
    let value = json!({
        "id": n.id.to_text(), "address": format!("umx:{alias}"),
        "type": "graph", "title": n.title, "alias": alias,
    });
    match columns {
        Some(columns) => Value::Object(
            columns
                .iter()
                .map(|column| {
                    (
                        column.clone(),
                        value.get(column).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect(),
        ),
        None => value,
    }
}

fn edge_json(alias: &str, e: &umx_core::Edge) -> Value {
    let mut v = serde_json::to_value(e).expect("an edge serialises");
    v["address"] = json!(umx_services::address(alias, e.id));
    v["graph"] = json!(alias);
    v
}

/// `edge delete` (`deleted`) or `edge edit` (`changed`): the edge as removed or as it is now, and its sentence,
/// flagged `field:sentence` when it holds a flagged title (F234).
fn edge_changed(v: &EdgeChanged, deleted: bool) -> Value {
    let mut out = json!({ "edge": edge_json(&v.alias, &v.edge), "sentence": v.sentence });
    if deleted {
        out["deleted"] = json!(true);
    } else {
        out["changed"] = json!(v.changed);
    }
    with_flags(out, &flag_if(v.sentence_flagged, "sentence"), false)
}

/// `umx graph guide` (plan 079). Every string here may be graph text (titles, purpose, descriptions): it is all
/// under `data`, untrusted (req-41). An example whose title, or a title in whose sentence, is flagged carries
/// `secret_suspected` (`field:title`, `field:sentence`; F234).
fn guide_json(g: &Guide) -> Value {
    let cmd = |c: &Option<umx_core::Command>| c.as_ref().map(|c| c.as_str().to_owned());
    let example = |e: &Option<GuideExample>| match e {
        None => Value::Null,
        Some(GuideExample::Node {
            id,
            title,
            title_flagged,
        }) => with_flags(
            json!({ "id": id.to_text(), "title": title }),
            &flag_if(*title_flagged, "title"),
            false,
        ),
        Some(GuideExample::Edge {
            id,
            from_graph,
            from,
            to_graph,
            to,
            sentence,
            sentence_flagged,
        }) => with_flags(
            json!({
                "id": id.to_text(), "from_graph": from_graph.to_text(), "from": from.to_text(),
                "to_graph": to_graph.to_text(), "to": to.to_text(), "sentence": sentence,
            }),
            &flag_if(*sentence_flagged, "sentence"),
            false,
        ),
    };
    let entry = |e: &GuideEntry| {
        json!({
            "name": e.name, "declared": e.declared, "description": e.description, "count": e.count,
            "example": example(&e.example), "next": cmd(&e.next), "std": e.std,
        })
    };
    let types: Vec<Value> = g
        .types
        .iter()
        .map(|t| {
            let mut v = entry(&t.entry);
            v["fields"] = t
                .fields
                .iter()
                .map(|f| {
                    json!({
                        "name": f.name, "kind": f.kind, "description": f.description, "next": cmd(&f.next),
                        "std": f.std,
                        "values": f.values,
                    })
                })
                .collect();
            v
        })
        .collect();
    let relations: Vec<Value> = g
        .relations
        .iter()
        .map(|r| {
            let mut v = entry(r);
            v["symmetric"] = json!(r.symmetric);
            v
        })
        .collect();
    json!({
        "graph": { "alias": g.alias, "title": g.title, "nodes": g.nodes, "edges": g.edges },
        "purpose": g.purpose, "purpose_next": cmd(&g.purpose_next),
        "conventions": g.conventions, "conventions_next": cmd(&g.conventions_next),
        "types": types,
        "statuses": g.statuses.iter().map(entry).collect::<Vec<_>>(),
        "relations": relations,
        "spine": spine_line_json(&g.spine),
        "rules": g.rules.iter().map(|rule| json!({
            "name":rule.name,"source":rule.source,"sentence":rule.sentence,"description":rule.description,
        })).collect::<Vec<_>>(),
        "hubs": hub_rows_json(&g.hubs),
    })
}

/// The best offered spine and its cover, or null, with the command that lists every candidate (B4). Graph guide and
/// check carry it as data, never as a warning.
fn spine_line_json(line: &SpineLine) -> Value {
    let best = line.best.as_ref().map(|(name, clock, cover)| {
        json!({ "candidate": name, "clock": clock.as_str(), "cover": cover_json(cover) })
    });
    let mut v = json!({ "best": best, "next": line.next.as_str() });
    // B4a (project manager review): the nodes the best places only by rule, when there are any.
    if let Some(next) = &line.by_rule_next {
        v["by_rule_next"] = json!(next.as_str());
    }
    v
}

/// One page of listed ids (`--unplaced` or `--by-rule`).
fn listed_json(u: &umx_services::Unplaced) -> Value {
    json!({
        "candidate": u.candidate, "total": u.total, "offset": u.offset,
        "ids": u.ids.iter().map(|id| id.to_text()).collect::<Vec<_>>(),
    })
}

fn cover_json(c: &umx_core::spine::Cover) -> Value {
    json!({
        "total": c.total, "by_field": c.by_field, "by_link": c.by_link, "by_evidence": c.by_evidence,
        "by_rule": c.by_rule, "not_placed": c.not_placed,
    })
}

fn spread_json(s: &umx_core::spine::Spread) -> Value {
    json!({
        "bands": s.bands,
        "largest_band_share": s.largest_band_share,
        "distinct_values": s.distinct_values,
    })
}

/// `umx graph spine set` and `confirm` (ws008 TM5): the declaration as stored, and the graph.json format after it.
fn spine_set_json(v: &SpineSet) -> Value {
    json!({
        "graph": v.alias,
        "spine": serde_json::to_value(&v.spine).expect("a spine serialises"),
        "replaced": v.replaced,
        "unchanged": v.unchanged,
        "format": v.format,
    })
}

/// `umx graph spine show` (ws008 TM5): each declared spine with its cover and spread, judged as propose judges.
fn spines_shown_json(v: &SpinesShown) -> Value {
    let spines: Vec<Value> = v
        .spines
        .iter()
        .map(|d| {
            json!({
                "name": d.decl.name,
                "decl": serde_json::to_value(&d.decl).expect("a spine serialises"),
                "clock": d.clock.as_str(),
                "cover": cover_json(&d.cover),
                "spread": spread_json(&d.spread),
                "offered": d.offered,
                "why_not": d.why_not.map(|w| w.as_str()),
                "by_rule_next": d.by_rule_next.as_ref().map(|c| c.as_str()),
            })
        })
        .collect();
    let mut out = json!({
        "graph": v.alias,
        "total": v.total,
        "spines": spines,
        "next": v.next.as_ref().map(|c| c.as_str()),
    });
    if let Some(u) = &v.by_rule {
        out["by_rule"] = listed_json(u);
    }
    if let Some(u) = &v.unplaced {
        out["unplaced"] = listed_json(u);
    }
    out
}

/// `umx graph spine propose` (ws005 s07). Names in it are vocabulary names and ids; `decl.title` is built from them.
fn spine_json(p: &SpineProposal) -> Value {
    let candidates: Vec<Value> = p
        .candidates
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "decl": c.decl.as_ref().map(|d| serde_json::to_value(d).expect("a spine serialises")),
                "source": c.source.as_str(),
                "builtin": c.builtin,
                "clock": c.clock.as_str(),
                "cover": cover_json(&c.cover),
                "spread": spread_json(&c.spread),
                "offered": c.offered,
                "why_not": c.why_not.map(|w| w.as_str()),
            })
        })
        .collect();
    let in_the_way: Vec<Value> = p
        .in_the_way
        .iter()
        .map(|w| {
            json!({
                "kind": w.kind.as_str(), "about": w.about, "count": w.count,
                "examples": w.examples.iter().map(|id| id.to_text()).collect::<Vec<_>>(),
                "next": w.next.as_str(),
            })
        })
        .collect();
    let hint =
        |count: usize, next: &umx_core::Command| json!({ "count": count, "next": next.as_str() });
    let not_placed = p.hints.not_placed.as_ref().map(|(name, h)| {
        let mut v = hint(h.count, &h.next);
        v["candidate"] = json!(name);
        v
    });
    let by_rule = p.hints.by_rule.as_ref().map(|(name, h)| {
        let mut v = hint(h.count, &h.next);
        v["candidate"] = json!(name);
        v
    });
    let mut v = json!({
        "graph": p.alias,
        "total": p.total,
        "candidates": candidates,
        "in_the_way": in_the_way,
        "hints": {
            "not_offered": hint(p.hints.not_offered.count, &p.hints.not_offered.next),
            "not_placed": not_placed,
            "by_rule": by_rule,
        },
    });
    if let Some(u) = &p.unplaced {
        v["unplaced"] = listed_json(u);
    }
    if let Some(u) = &p.by_rule {
        v["by_rule"] = listed_json(u);
    }
    v
}

/// `umx check`'s data: its rows, `graphs` for a list or scope, `spines`, `sources` (usage 220), and `lint` with
/// `--lint` (F139).
fn check_json(checked: &umx_services::Checked) -> Value {
    let page = &checked.page;
    // F113: with a list or scope, each row names the graph it was found in, and `graphs` counts each one.
    let owners = page.graphs.as_ref().map(|g| &g.rows);
    let rows = page
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut row = json!({ "kind": r.kind, "file": r.file.path(), "subject": r.subject, "lines": r.lines });
            if let Some(owners) = owners {
                row["graph"] = json!(owners[i]);
            }
            row
        })
        .collect();
    let mut columns = vec!["kind", "file", "subject", "lines"];
    if owners.is_some() {
        columns.push("graph");
    }
    let mut v = list(&columns, rows, Some((page.total, page.truncated)));
    if owners.is_some() {
        v["graphs"] = json!(
            checked
                .graphs
                .iter()
                .map(|(graph, total)| json!({ "graph": graph, "total": total }))
                .collect::<Vec<_>>()
        );
    }
    if let Some(lint) = &checked.lint {
        v["lint"] = lint_json(lint);
    }
    v["sources"] = sources_json(&checked.sources);
    v["rules"] = json!(
        checked
            .rules
            .iter()
            .map(|rules| {
                let mut value = json!({
                    "graph":rules.graph,"total":rules.total,"unmet_nodes":rules.unmet_nodes,
                    "not_checked":rules.not_checked,"cut":rules.cut,"after":rules.after,
                    "next":rules.next.as_ref().map(Command::as_str),
                    "resume":rules.resume.as_ref().map(Command::as_str),
                });
                if let Some(rows) = &rules.rows {
                    value["rows"] = json!(rows.iter().map(|row| json!({
                "rule":row.rule,"node":row.node,"state":row.state,"sentence":row.sentence,
                "next":row.next.as_str(),
            })).collect::<Vec<_>>());
                }
                value
            })
            .collect::<Vec<_>>()
    );
    v["spines"] = json!(
        checked
            .spines
            .iter()
            .map(|(graph, line)| json!({ "graph": graph, "spine": spine_line_json(line) }))
            .collect::<Vec<_>>()
    );
    v
}

/// `data.sources` (usage 220): each cited file that changed or cannot be read, by graph, node id and place
/// (`evidence:<i>` or `locator`), with what reading it gave now and both lengths; never the source or its content.
fn sources_json(sources: &umx_services::SourcesChecked) -> Value {
    let rows: Vec<Value> = sources
        .rows
        .iter()
        .map(|r| {
            json!({
                "graph": r.graph,
                "kind": r.kind.as_str(),
                "id": r.id.to_text(),
                "place": r.place,
                "state": r.state.as_str(),
                "old_length": r.old_length,
                "new_length": r.new_length,
            })
        })
        .collect();
    json!({
        "rows": rows,
        "checked": sources.checked,
        "unchecked": sources.unchecked,
        "changed": sources.changed,
        "missing": sources.missing,
        "grew": sources.grew,
        "too_large": sources.too_large,
        "skipped": sources.skipped,
        "bytes": sources.bytes,
        "graphs": sources
            .graphs
            .iter()
            .map(|g| {
                json!({ "graph": g.graph, "changed": g.changed, "missing": g.missing, "skipped": g.skipped })
            })
            .collect::<Vec<_>>(),
    })
}

/// `--lint`'s request from the wire (F139): `kind` and `offset` only with `lint`, and `kind` one umx knows.
fn lint_request(
    lint: bool,
    kind: Option<&str>,
    offset: Option<usize>,
) -> Result<Option<umx_services::LintRequest>, Problem> {
    if !lint {
        return match (kind, offset) {
            (None, None) => Ok(None),
            _ => Err(umx_services::problems::usage_invalid(
                "--kind and --offset go with --lint",
                "graph",
            )),
        };
    }
    let kind = match kind {
        None => None,
        Some(k) => Some(umx_services::LintKind::parse(k).ok_or_else(|| {
            umx_services::problems::usage_invalid(
                "--kind is orphans or proposed_keys or terms or double_negatives or raised_in_evidence or sensitivity_label",
                "graph",
            )
        })?),
    };
    Ok(Some(umx_services::LintRequest {
        kind,
        offset: offset.unwrap_or(0),
    }))
}

/// `data.lint` (F139): each finding names its graph, kind, the node (or edge) id and the place as F188's read flags
/// name it, never a term or the text; `raised_in_evidence` adds the anchor and both ranges. `skipped` says which kinds
/// did not run, and why.
fn lint_json(lint: &umx_services::Lint) -> Value {
    let rows: Vec<Value> = lint
        .rows
        .iter()
        .map(|r| {
            let mut row = json!({
                "graph": r.graph,
                "kind": r.kind.as_str(),
                "id": r.id.map(|id| id.to_text()),
                "place": r.place,
            });
            if let (Some(anchor), Some(ranges)) = (r.anchor, r.ranges) {
                row["anchor"] = json!(anchor.to_text());
                row["ranges"] = json!({
                    "unit": ranges.unit,
                    "node": [ranges.node.0, ranges.node.1],
                    "anchor": [ranges.anchor.0, ranges.anchor.1],
                });
            }
            row
        })
        .collect();
    let skipped: Vec<Value> = lint
        .skipped
        .iter()
        .map(|(kind, why)| json!({ "kind": kind.as_str(), "why": why }))
        .collect();
    json!({ "rows": rows, "skipped": skipped })
}

/// `umx resolve`: ok carries what the address names; every other status is refused with its problem, so a script
/// sees the failure and the code says why (graph_not_loaded, id_ambiguous, node_not_found, copy_ambiguous,
/// field_not_found).
fn resolved(command: &str, outcome: Result<Done<Resolved>, Problem>) -> Envelope {
    let done = match outcome {
        Ok(done) => done,
        Err(p) => return Envelope::fail(command, issue(&p, command)),
    };
    let r = &done.value;
    if let Some(p) = &r.problem {
        return Envelope::fail(command, issue(p, command));
    }
    let alias = r.alias.as_deref().unwrap_or_default();
    let (kind, node, edge, id) = match &r.target {
        Some(Target::Node(n)) if Some(n.id) == r.graph_id => {
            ("node", virtual_node_json(alias, n, None), Value::Null, n.id)
        }
        Some(Target::Node(n)) => ("node", node_json(alias, n), Value::Null, n.id),
        Some(Target::Edge(e)) => ("edge", Value::Null, edge_json(alias, e), e.id),
        None => return Envelope::fail(command, internal("resolve_without_target")),
    };
    let address = match &r.target {
        Some(Target::Node(n)) if Some(n.id) == r.graph_id => format!("umx:{alias}"),
        _ => umx_services::address(alias, id),
    };
    let data = json!({
        "status": r.status.as_str(),
        "address": address,
        "graph": alias,
        "graph_id": r.graph_id.map(|g| g.to_text()),
        "kind": kind,
        "node": node,
        "edge": edge,
        "field": r.field.as_ref().map(|(f, _)| f),
        "value": r.field.as_ref().map(|(_, v)| v),
        "selector": r.selector,
        "read": r.selector.as_ref().map(|_| false),
        "repo": r.repo,
    });
    let mut data = data;
    if let Some(from) = &r.redirected_from {
        data["redirected_from"] = json!(from);
    }
    done.notes
        .iter()
        .fold(Envelope::ok(command, data), |env, w| {
            env.note(issue(w, command))
        })
}

fn edge_row(r: &EdgeRow) -> Value {
    let mut row = json!({
        "id": r.edge.id.map(|id| id.to_text()), "type": r.edge.kind, "from": r.edge.from.to_text(), "to": r.edge.to.to_text(),
        "graph": r.alias, "graph_id": r.graph.to_text(),
        "from_graph": r.from_graph.to_text(), "to_graph": r.to_graph.to_text(), "sentence": r.sentence,
        "evidence_count": r.edge.evidence.len(), "evidence_kinds": r.edge.evidence_kinds(),
    });
    match r.edge.id {
        Some(id) => row["address"] = json!(umx_services::address(&r.alias, id)),
        None => row["derived"] = json!(true),
    }
    for (end, state, title) in [
        ("from", r.from_state, &r.from_title),
        ("to", r.to_state, &r.to_title),
    ] {
        if let Some(state) = state {
            row[format!("{end}_state")] = json!(state);
        } else {
            row[format!("{end}_title")] = json!(title);
        }
    }
    for (end, candidates) in [("from", &r.from_candidates), ("to", &r.to_candidates)] {
        if !candidates.is_empty() {
            row[format!("{end}_candidates")] = json!(candidates);
        }
    }
    row
}

/// Where `umx edge show` shows an older edge's `evidence` field that is not an evidence list (R210-2).
const EVIDENCE_FIELD_KEPT: &str = "evidence_field_kept";

/// `umx edge show` (req-124): the edge list row, with the whole stored edge (custom fields and the full evidence list).
/// `evidence` is always the list; an older edge's own `evidence` field that is not one is shown, as kept, under
/// `evidence_field_kept` (R210-2), never hidden. `secret_suspected` names what the secret scan flags in its evidence
/// and in the stored fields it shows (F190): a stored field a computed one of the same name hides (`sentence`,
/// `from_title`) is not shown, so not flagged (review on #385), nor is a stored custom `evidence_field_kept` the kept
/// value replaces (F222); the kept prose itself is flagged under `field:evidence_field_kept`, where it shows (F223).
/// Left out when nothing is. A far end's flagged title is named too, where the row shows it (F228).
fn edge_shown(services: &Services, r: &EdgeRow) -> Value {
    let mut v = edge_row(r);
    let stored = r
        .edge
        .clone()
        .into_stored()
        .map(|e| edge_json(&r.alias, &e));
    let mut shown_fields: Vec<(String, String)> = Vec::new();
    if let (Some(row), Some(Value::Object(stored))) = (v.as_object_mut(), stored) {
        for (k, value) in stored {
            if !row.contains_key(&k) {
                if r.edge.fields.contains_key(&k) {
                    shown_fields.push((k.clone(), k.clone()));
                }
                row.insert(k, value);
            }
        }
        row.insert("evidence".into(), json!(r.edge.evidence));
        if let Some(kept) = r.edge.invalid_evidence_field() {
            row.insert(EVIDENCE_FIELD_KEPT.into(), kept.clone());
            // F222: a stored custom field of that name is no longer shown. F223: the kept prose is, under that key.
            shown_fields
                .retain(|(stored, _)| stored != EVIDENCE_FIELD_KEPT && stored != "evidence");
            shown_fields.push(("evidence".into(), EVIDENCE_FIELD_KEPT.into()));
        }
    }
    let mut flags = edge_title_flags(services, core::slice::from_ref(r), |_| true).remove(0);
    flags.extend(umx_services::edge_secret_flags(
        &r.edge,
        &shown_fields,
        true,
    ));
    with_flags(v, &flags, false)
}

/// F228: for each edge row, the far-end titles it shows (`from_title`, `to_title`, and `sentence`, which carries
/// both) that umx's secret check flags, as a node read flags the node's own title, for the columns `shown` lets
/// through. An end in a graph the read does not show (a state) has no title, so nothing to flag.
fn edge_title_flags(
    services: &Services,
    rows: &[EdgeRow],
    shown: impl Fn(&str) -> bool,
) -> Vec<Vec<String>> {
    services
        .edge_titles_flagged(rows)
        .into_iter()
        .map(|[from, to]| {
            [
                ("from_title", from),
                ("to_title", to),
                ("sentence", from || to),
            ]
            .into_iter()
            .filter(|(column, on)| *on && shown(column))
            .map(|(column, _)| umx_core::place::named_place(column))
            .collect()
        })
        .collect()
}

/// F234: `["field:<key>"]` when the reply key `key` holds a flagged title, else none.
fn flag_if(flagged: bool, key: &str) -> Vec<String> {
    if flagged {
        vec![umx_core::place::named_place(key)]
    } else {
        Vec::new()
    }
}

/// The title a node row shows: none when the node is only a stored id in a graph the read does not show (a state).
fn shown_title<'n>(state: Option<&str>, node: &'n NodeCell) -> &'n str {
    if state.is_none() { &node.title } else { "" }
}

/// Whether some row carries `secret_suspected`, so the list names the column (F188's rule).
fn any_flagged(rows: &[Value]) -> bool {
    rows.iter().any(|r| r.get(SECRET_SUSPECTED).is_some())
}

/// F228: `["field:title"]` for each node row whose shown title umx's secret check flags, else `[]`.
fn title_flags(
    services: &Services,
    titles: &[(GraphKey<'_>, umx_core::Id, &str)],
) -> Vec<Vec<String>> {
    services
        .titles_flagged(titles)
        .into_iter()
        .map(|on| flag_if(on, "title"))
        .collect()
}

fn history_row(r: &HistoryRow) -> Value {
    json!({
        "at": r.at, "by": r.by, "kind": r.kind, "from": r.from, "to": r.to, "label": r.label, "reason": r.reason,
    })
}

/// The asked-for columns of an edge list row (F74): a computed column from `row`, null when the row leaves it out
/// (never a same-named custom field, B1), else `created`, `created_by` or a custom field; null where the edge lacks
/// it.
fn pick_edge(row: &Value, edge: &umx_services::ReadEdge, columns: &[String]) -> Value {
    let mut picked = serde_json::Map::new();
    for c in columns {
        let v = match c.as_str() {
            computed if EDGE_COMPUTED_COLUMNS.contains(&computed) => {
                row.get(computed).cloned().unwrap_or(Value::Null)
            }
            "created" => json!(edge.created),
            "created_by" => json!(edge.created_by),
            other => edge.fields.get(other).cloned().unwrap_or(Value::Null),
        };
        picked.insert(c.clone(), v);
    }
    Value::Object(picked)
}

/// The prefix of a computed `node list` column: `spine:<name>` is where that declared spine places the row's node.
const SPINE_COLUMN: &str = "spine:";

/// Fills each `spine:<name>` column of a node list page: `{band, by, anchor}` by the one placement rule, run once per
/// graph on the page (umx_services::SpinePlace), or null where the row's graph declares no spine of that name. A field
/// a node stores under such a name is never shown: the computed column wins.
fn spine_cells(
    services: &Services,
    cols: &[String],
    aliases: &[&str],
    nodes: &[umx_services::ListedNode<'_>],
    rows: &mut [Value],
) {
    for col in cols {
        let Some(name) = col.strip_prefix(SPINE_COLUMN) else {
            continue;
        };
        let graphs: BTreeSet<&str> = aliases.iter().copied().collect();
        for graph in graphs {
            let at: Vec<usize> = (0..rows.len()).filter(|&i| aliases[i] == graph).collect();
            let ids: Vec<umx_core::Id> = at.iter().map(|&i| nodes[i].id()).collect();
            let places = services.spine_places(graph, name, &ids);
            for (k, &i) in at.iter().enumerate() {
                rows[i][col.as_str()] = places.as_ref().map_or(Value::Null, |p| {
                    let p = p[k];
                    json!({ "band": p.band, "by": p.by.as_str(), "anchor": p.anchor })
                });
            }
        }
    }
}

/// The asked-for columns of a node, read by `field`; a column the node lacks is null. Values come from `Node::field`
/// or a listed node's `field` (the same names and values), so derived fields such as `evidence_kinds` are computed
/// below the daemon. `locator` is the whole locator, as the default read shows it (F300): `field` answers nothing for
/// it, only for its flat parts.
fn pick(
    alias: &str,
    id: umx_core::Id,
    field: impl Fn(&str) -> Option<Value>,
    locator: impl Fn() -> Option<umx_core::Locator>,
    columns: &[String],
) -> Value {
    let mut row = serde_json::Map::new();
    for c in columns {
        let v = match c.as_str() {
            "address" => json!(umx_services::address(alias, id)),
            "graph" => json!(alias),
            umx_core::locator::LOCATOR_FIELD => json!(locator()),
            other => field(other).unwrap_or(Value::Null),
        };
        row.insert(c.clone(), v);
    }
    Value::Object(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F242: the daemon flags a candidate's title by the keys umx-protocol names, so they must be the keys a candidate
    /// is written with: renamed on one side only, the flag would silently stop.
    #[test]
    fn a_candidate_is_written_with_the_keys_its_title_flag_reads() {
        let c = candidate(&Candidate {
            id: "01JC8XA4H7".into(),
            title: Some("Cache".into()),
            score: None,
        });
        assert_eq!(c[umx_protocol::CANDIDATE_ID], "01JC8XA4H7", "{c}");
        assert_eq!(c[umx_protocol::CANDIDATE_TITLE], "Cache", "{c}");
    }

    /// Four rows over three nodes in two graphs, titles that need escaping included.
    fn pairs_sample() -> umx_services::PairsResult {
        use umx_services::{Pair, PairNode, PairsMin, PairsResult};
        let node = |graph: u16, id: u128, kind: &str, title: &str| PairNode {
            graph,
            id: umx_core::Id(id),
            kind: kind.into(),
            title: title.into(),
            // F228: the empty-titled node stands in for a flagged one, so the writer's flags path is checked too.
            title_flagged: id == 42,
        };
        PairsResult {
            pairs: vec![
                Pair {
                    node: 0,
                    other: 1,
                    score: 10_000,
                    rank: 1,
                },
                Pair {
                    node: 1,
                    other: 2,
                    score: -1_234,
                    rank: 10,
                },
                Pair {
                    node: 2,
                    other: 0,
                    score: 0,
                    rank: 2,
                },
                Pair {
                    node: 0,
                    other: 2,
                    score: 8_731,
                    rank: 3,
                },
            ],
            names: vec![
                node(
                    0,
                    1,
                    "problem",
                    "Quote \" backslash \\ tab \t nul \u{0} bell \u{7}",
                ),
                node(1, u128::MAX, "idea", "Ünïcødé ✓ 🦀 \u{2028} </script>"),
                node(1, 42, "kind \"x\"", ""),
            ],
            total: 4,
            truncated: false,
            model: None,
            model_source: umx_services::ModelSource::Flag,
            k: 10,
            min: PairsMin {
                units: 0,
                source: "none",
                calibrated: false,
                run: None,
            },
            graphs: vec!["alpha".into(), "beta-2".into()],
            policies: vec!["open", "secured"],
            digests: vec![[1; 32], [2; 32]],
            skipped_model: vec![umx_services::Skipped {
                graph: "gamma".into(),
                model: None,
                reason: "no vectors yet",
            }],
            same_graph: false,
            nodes: 3,
            fields: umx_services::PairRows::Full,
        }
    }

    /// ws004 s34a: pairs' rows written straight from the result are byte for byte what a `Value` object per row gives,
    /// titles that need escaping included, and a client reads the same envelope either way.
    #[test]
    fn pairs_rows_written_from_the_result_match_a_value_per_row() {
        let v = pairs_sample();
        // The form the reply had before s34a, as the reference, with F228's flags where a row has some.
        let reference: Vec<Value> = v
            .rows()
            .iter()
            .map(|r| {
                let mut row = json!({
                    "graph": r.graph, "address": umx_services::address(r.graph, r.id), "policy": r.policy,
                    "id": r.id.to_text(), "type": r.kind, "title": r.title,
                    "other_graph": r.other_graph, "other_address": umx_services::address(r.other_graph, r.other_id),
                    "other_policy": r.other_policy, "other_id": r.other_id.to_text(), "other_type": r.other_kind,
                    "other_title": r.other_title, "score": pair_score(r.score), "rank": r.rank,
                });
                let flags: Vec<&str> = [(r.title_flagged, "field:title"), (r.other_title_flagged, "field:other_title")]
                    .into_iter()
                    .filter_map(|(on, flag)| on.then_some(flag))
                    .collect();
                if !flags.is_empty() {
                    row["secret_suspected"] = json!(flags);
                }
                row
            })
            .collect();
        let written = pairs_rows_json(&v);
        assert_eq!(written, serde_json::to_string(&reference).unwrap());
        let env = pairs_reply(
            "similarity pairs",
            Ok(Done {
                value: v,
                notes: Vec::new(),
            }),
        );
        let read = Envelope::from_json(env.to_json().as_bytes()).unwrap();
        assert_eq!(read.data["rows"], Value::Array(reference));
        assert_eq!(read.data["total"], 4);
        // ws004 s36 (F206): each compared graph with its digest, in graphs order; a skipped graph is never listed.
        assert_eq!(
            read.data["compared"],
            json!([
                {"graph": "alpha", "vectors_digest": "01".repeat(32)},
                {"graph": "beta-2", "vectors_digest": "02".repeat(32)},
            ])
        );
        assert_eq!(read.data["skipped_model"][0]["graph"], "gamma");
        let (head, rows, tail) = env.json_parts();
        assert_eq!(
            rows, written,
            "the rows are written as they are, after data.rows' key"
        );
        assert!(
            head.ends_with("\"rows\":") && tail.starts_with(','),
            "{head} / {tail}"
        );
    }

    /// ws004 s34b: each `--rows ids` row is the full row's graph, id, other_graph, other_id and score, byte for byte
    /// and in the same row order, and nothing else; the columns name those five.
    #[test]
    fn pairs_id_rows_are_the_full_rows_five_fields() {
        let full = pairs_sample();
        let mut lean = full.clone();
        lean.fields = umx_services::PairRows::Ids;
        // An ids finish leaves type and title empty: the lean rows must not need them.
        for n in &mut lean.names {
            n.kind.clear();
            n.title.clear();
        }
        let full_rows: Vec<Value> = serde_json::from_str(&pairs_rows_json(&full)).unwrap();
        let five: Vec<String> = full_rows
            .iter()
            .map(|r| {
                let pick: serde_json::Map<String, Value> = PAIRS_ID_COLUMNS
                    .iter()
                    .map(|c| ((*c).to_string(), r[*c].clone()))
                    .collect();
                serde_json::to_string(&pick).unwrap()
            })
            .collect();
        let written = pairs_rows_json(&lean);
        assert_eq!(written, format!("[{}]", five.join(",")));
        let env = pairs_reply(
            "similarity pairs",
            Ok(Done {
                value: lean,
                notes: Vec::new(),
            }),
        );
        let read = Envelope::from_json(env.to_json().as_bytes()).unwrap();
        assert_eq!(
            read.data["columns"],
            json!(["graph", "id", "other_graph", "other_id", "score"])
        );
        assert_eq!(read.data["total"], 4);
        let (_, rows, _) = env.json_parts();
        assert_eq!(rows, written);
    }

    /// A `?repo=` hint is checked twice: by umx-core's SafeText::repo_url when the problem is built and by
    /// umx-protocol's Fill::repo_url when it is rendered (architect FU1). The two rules must agree on every URL.
    #[test]
    fn both_repo_url_checks_agree() {
        let long = format!("https://example.com/{}", "a".repeat(236));
        let urls: [(&str, bool); 18] = [
            ("https://github.com/org/x.git", true),
            ("https://example.invalid/a/b~c+d:e_f-g.git", true),
            ("git@github.com:org/x.git", true),
            ("git@host-1.example:path/to~x+y_z.git", true),
            ("https://", false),
            ("https://-x/y", false),
            ("https://x/y --upload-pack=touch", false),
            ("https://x/y;rm", false),
            ("https://x/y`id`", false),
            ("https://x/$(id)", false),
            ("http://x/y.git", false),
            ("ssh://git@x/y.git", false),
            ("git@:path", false),
            ("git@host:", false),
            ("git@-host:path", false),
            ("git@host:-path", false),
            ("git@host:pa th", false),
            ("--upload-pack=touch", false),
        ];
        for (url, ok) in urls {
            assert_eq!(
                umx_core::SafeText::repo_url(url).is_some(),
                ok,
                "core {url}"
            );
            assert_eq!(Fill::repo_url(url).is_some(), ok, "protocol {url}");
        }
        let longer = format!("{long}a");
        for url in [long.as_str(), longer.as_str()] {
            assert_eq!(
                umx_core::SafeText::repo_url(url).is_some(),
                Fill::repo_url(url).is_some(),
                "{} chars",
                url.len()
            );
        }
        assert_eq!((long.len(), longer.len()), (256, 257));
        assert!(Fill::repo_url(&long).is_some() && Fill::repo_url(&longer).is_none());
    }

    #[test]
    fn embedder_mismatch_renders_both_model_keys_unchanged() {
        let key = |t: &str| umx_core::ports::ModelKey::parse(t).unwrap();
        let p = umx_services::problems::embedder_mismatch(
            &ValidName::alias("demo").unwrap(),
            &key("text-embedding-3-small@256"),
            &key("qwen3-embedding-0.6b@256+p1"),
        );
        let i = issue(&p, "graph load");
        assert_eq!(i.code(), "embedder_mismatch");
        assert_eq!(
            i.message(),
            "Graph demo is embedded with text-embedding-3-small@256; the configured embedder is \
             qwen3-embedding-0.6b@256+p1. Its vectors stay as they are and are not mixed, and new ones wait \
             (pending). Either configure an embedder for text-embedding-3-small@256 (umx docs get config), or \
             re-embed the graph with qwen3-embedding-0.6b@256+p1; the next command shows that cost first and \
             changes nothing."
        );
        // req-75: the two ways out; the next is the free, read-only one.
        assert_eq!(i.next(), "umx embeddings reembed --graph demo --dry-run");
    }

    /// Review R2: the warnings after a write of this machine's policy record say what happened and nothing about a
    /// policy they cannot know; a confirmed downgrade that did not sync is open, not "still secured".
    #[test]
    fn policy_record_warnings_render_what_happened() {
        let file = GraphDir::parse("/h/state/graph-policies.json").unwrap();
        let i = issue(
            &umx_services::problems::local_state_not_durable(
                &file,
                "sync state: permission denied",
            ),
            "graph confirm-policy",
        );
        assert_eq!(
            (i.code(), i.message(), i.next()),
            (
                "local_state_not_durable",
                "/h/state/graph-policies.json was replaced but the folder sync after it failed: sync state: \
                 permission denied. The change may not survive a crash; umx graph list shows the policies in \
                 effect.",
                "umx graph list"
            )
        );
        assert!(!i.message().contains("secured"));
        let i = issue(
            &umx_services::problems::local_state_not_saved(
                &ValidName::alias("demo").unwrap(),
                &file,
                "write graph-policies.json: permission denied",
            ),
            "graph new",
        );
        assert_eq!(
            i.message(),
            "Recording graph demo's secured policy in /h/state/graph-policies.json failed: write \
             graph-policies.json: permission denied. This daemon keeps it secured and will not lower its policy \
             until the record is saved."
        );
    }

    /// Fill::model_key and ModelKey::parse accept the same keys: any key that parses renders unchanged.
    #[test]
    fn model_keys_at_their_limits_render_unchanged() {
        let longest = format!("{}@{}+p{}", "a".repeat(128), ModelKey::MAX_DIMS, u32::MAX);
        for text in ["m@8+p4294967295", "m@8192", "0@1", longest.as_str()] {
            let key = umx_core::ports::ModelKey::parse(text).unwrap();
            let p = umx_services::problems::embedder_mismatch(
                &ValidName::alias("demo").unwrap(),
                &key,
                &key,
            );
            assert!(issue(&p, "graph load").message().contains(text), "{text}");
        }
        assert!(umx_core::ports::ModelKey::parse(&format!("{}@8", "a".repeat(129))).is_err());
        assert!(
            umx_core::ports::ModelKey::parse("m@8193").is_err(),
            "dims above MAX_DIMS"
        );
    }

    /// ws002 X1: services' hint list (what a batch counts apart) is exactly the catalogue's Hint rows it returns.
    #[test]
    fn services_hints_are_the_catalogue_hint_rows() {
        let hints: Vec<&str> = umx_services::problems::CODES
            .iter()
            .copied()
            .filter(|c| Code::from_code(c).is_some_and(|c| c.spec().kind == Kind::Hint))
            .collect();
        let mut listed = umx_services::problems::HINTS.to_vec();
        let mut found = hints.clone();
        listed.sort_unstable();
        found.sort_unstable();
        assert_eq!(listed, found);
    }

    /// Services bound a batch reply by the hints it will show; that is the protocol's cap and its marker.
    #[test]
    fn services_count_the_hints_a_reply_shows() {
        assert_eq!(
            umx_services::problems::HINTS_SHOWN,
            umx_protocol::MAX_HINTS + 1
        );
    }

    #[test]
    fn every_services_code_has_a_catalogue_row() {
        for code in umx_services::problems::CODES
            .iter()
            .chain(umx_core::config::problems::CODES)
        {
            assert!(
                Code::from_code(code).is_some(),
                "{code} has no row in umx-protocol's catalogue"
            );
        }
    }

    /// The CLI cannot depend on umx-core (r-cli-is-a-client), so umx-protocol mirrors the batch caps; this keeps
    /// the two the same numbers.
    #[test]
    fn the_batch_caps_are_the_same_on_both_sides_of_the_socket() {
        use umx_core::batch;
        assert_eq!(umx_protocol::MAX_BATCH_BYTES, batch::MAX_BATCH_BYTES);
        assert_eq!(umx_protocol::MAX_BATCH_OPS, batch::MAX_BATCH_OPS);
        assert_eq!(umx_protocol::MAX_BATCH_LINE, batch::MAX_BATCH_LINE);
        assert_eq!(umx_protocol::MAX_BATCH_EVIDENCE, batch::MAX_BATCH_EVIDENCE);
        assert_eq!(umx_protocol::MAX_IMPORTED_REFS, batch::MAX_IMPORTED_REFS);
        const { assert!(batch::MAX_BATCH_BYTES <= umx_protocol::MAX_FRAME / 2) };
    }

    /// ws003 s09: the CLI checks an idempotency key with umx-protocol's copy of core's rule, in the same words.
    #[test]
    fn the_idempotency_key_rule_is_the_same_on_both_sides_of_the_socket() {
        use umx_core::receipts;
        assert_eq!(umx_protocol::IDEMPOTENCY_KEY_RULE, receipts::KEY_RULE);
        let long = "k".repeat(128);
        let longer = "k".repeat(129);
        for key in [
            "",
            "a",
            "window-08",
            "run:1.a_b-c",
            "a b",
            "a/b",
            "é",
            "a\n",
            &long,
            &longer,
        ] {
            assert_eq!(
                umx_protocol::idempotency_key_ok(key),
                receipts::check_key(key),
                "{key:?}"
            );
        }
        for c in 0u8..=127 {
            let key = (c as char).to_string();
            assert_eq!(
                umx_protocol::idempotency_key_ok(&key),
                receipts::check_key(&key),
                "{c}"
            );
        }
    }

    /// ws003 s09 (architect C6): the canonical input of a fixed batch has this digest. The canonical form rests on
    /// serde_json sorting object keys (no `preserve_order`); a crate that turns that feature on unifies it across
    /// the workspace and changes every stored input digest, so a retry after the upgrade would be refused. This
    /// test fails first. Line numbers, `dry_run`, `echo_sentences` and the key are not part of the input.
    #[test]
    fn a_batch_input_digest_is_pinned() {
        let line = |n: u32, json: &str| umx_protocol::BatchLine {
            line: n,
            op: umx_protocol::BatchOp::from_line(json).ok().unwrap(),
        };
        let sample = |lines: Vec<umx_protocol::BatchLine>, dry_run: bool, key: &str| Op::Batch {
            related: None,
            lines,
            dry_run,
            echo_sentences: dry_run,
            refs: [("q0".to_owned(), "01JC8XA4H7B3KT9M2NZ5QW6RDG".to_owned())].into(),
            refs_graph: Some("01JC8XA4H7B3KT9M2NZ5QW6RDH".into()),
            reason: Some("stale".into()),
            raised_in: Some("umx:demo/01JC8XA4H7".into()),
            idempotency_key: Some(key.into()),
        };
        let ops = [
            r#"{"op":"node add","ref":"q","type":"question","title":"Storage choice","sets":[{"field":"meta","value":{"b":1,"a":[2,{"d":3,"c":4}]}}]}"#,
            r#"{"op":"edge add","from_ref":"q","relation":"answers","to_ref":"q0"}"#,
        ];
        let digest = |op: &Op| umx_core::receipts::input_digest(&op.batch_input().unwrap());
        let first = sample(vec![line(1, ops[0]), line(2, ops[1])], false, "k1");
        assert_eq!(
            digest(&first),
            "ce2b3150a3bbda0148dba427d3714dd7172626fe2c412e8f8409f24c35697d02"
        );
        let moved = sample(vec![line(4, ops[0]), line(9, ops[1])], true, "k2");
        assert_eq!(
            digest(&moved),
            digest(&first),
            "line numbers, dry run, echo and key"
        );
        let reordered = sample(vec![line(1, ops[1]), line(2, ops[0])], false, "k1");
        assert_ne!(digest(&reordered), digest(&first), "op order is the input");
        assert_eq!(
            Op::GraphList {
                all: false,
                graphfile: None
            }
            .batch_input(),
            None,
            "only a batch has an input"
        );
    }

    /// Plan 205: the CLI checks refs with umx-protocol's copy of core's rule (r-cli-is-a-client), and says why in the
    /// same words the daemon does. Every edge of the rule, both sides.
    #[test]
    fn the_cli_and_the_daemon_refuse_the_same_refs_in_the_same_words() {
        use umx_core::batch::{self, RefError as Core};
        use umx_protocol::RefError as Wire;
        assert_eq!(umx_protocol::MAX_REF_LEN, batch::MAX_REF_LEN);
        let max = format!("a{}", "_".repeat(batch::MAX_REF_LEN - 1));
        let over = format!("{max}x");
        for text in [
            "", "q", "q1", "opt-2", "risk_7", "9abc", "_a", "-a", "Q1", "brd_exA", "a.b", "a b",
            "aé", "é", "a\u{7}", &max, &over,
        ] {
            let core = batch::check_ref(text);
            let wire = umx_protocol::check_ref(text);
            let same = match (core, wire) {
                (Ok(()), Ok(())) => true,
                (Err(Core::Syntax), Err(Wire::Syntax))
                | (Err(Core::TooLong), Err(Wire::TooLong)) => true,
                (
                    Err(Core::Character {
                        character: c,
                        position: p,
                    }),
                    Err(Wire::Character {
                        character: wc,
                        position: wp,
                    }),
                ) => c == wc && p == wp,
                _ => false,
            };
            assert!(same, "{text:?}: {core:?} vs {wire:?}");
            if let (Err(core), Err(wire)) = (core, wire) {
                assert_eq!(
                    umx_services::problems::batch_ref_invalid(1, core).slot("reason"),
                    Some(umx_protocol::ref_error_words(wire)),
                    "{text:?}"
                );
            }
        }
    }

    /// F224: the CLI reads places with umx-protocol's copy of core's grammar (r-cli-is-a-client). Every place kind
    /// core writes, the edge forms and the list base included, parses alike on both sides, and both refuse alike.
    #[test]
    fn the_cli_and_the_daemon_read_every_place_alike() {
        use umx_core::place::{self, custom_place, edge_place, list_place, named_place};
        assert_eq!(umx_protocol::place::LIST_BASE, place::LIST_BASE);
        let id = "01JC8XE1RM4TQ7W2N5V8B3K6YP";
        let written = [
            custom_place(1),
            custom_place(12),
            named_place("note"),
            named_place("evidence_field_kept"),
            named_place("locator_source"),
            list_place("key", 0),
            list_place("evidence", 9),
            list_place("status_reason", 0),
            list_place("status_evidence", 2),
            list_place("evidence_removed", 0),
            list_place("status_history", 1),
            list_place(&custom_place(2), 0),
            edge_place(id, &custom_place(3)),
            edge_place(id, &list_place("evidence", 1)),
            edge_place(id, &named_place("reason")),
            edge_place(id, "record"),
            "record".to_string(),
        ];
        let refused = [
            "",
            "field:",
            "field 0",
            "field 01",
            "field -1",
            "field 2x",
            "evidence:0",
            "evidence:",
            "key:01",
            "Key:1",
            "graph:title",
            "edge:/field 1",
            "edge:01A",
            "edge:01A/edge:01B/field 1",
            "shape",
            "field:a:b",
            "evidence:+1",
            "field 2:0",
            "edge:01A/evidence:0",
        ];
        for text in written.iter().map(String::as_str).chain(refused) {
            let core = place::parse(text);
            let wire = umx_protocol::place::parse(text);
            assert_eq!(format!("{core:?}"), format!("{wire:?}"), "{text:?}");
        }
        for text in &written {
            assert!(
                place::parse(text).is_some(),
                "core reads what it writes: {text:?}"
            );
        }
    }

    #[test]
    fn wire_evidence_kinds_match_core_and_preserve_an_omitted_default() {
        use umx_core::evidence::EvidenceKind as Core;
        use umx_protocol::EvidenceKind as Wire;
        for kind in [
            Core::Room,
            Core::Chat,
            Core::Screen,
            Core::Document,
            Core::Analyst,
        ] {
            let wire = match kind {
                Core::Room => Wire::Room,
                Core::Chat => Wire::Chat,
                Core::Screen => Wire::Screen,
                Core::Document => Wire::Document,
                Core::Analyst => Wire::Analyst,
            };
            assert_eq!(serde_json::to_value(wire).unwrap(), json!(kind.as_str()));
            let mapped = core_evidence(vec![umx_protocol::EvidencePointer {
                pointer: "notes.md#L1".into(),
                kind: Some(wire),
            }]);
            assert_eq!(mapped[0].kind, Some(kind));
        }
        let mapped = core_evidence(vec![umx_protocol::EvidencePointer {
            pointer: "notes.md#L1".into(),
            kind: None,
        }]);
        assert_eq!(mapped[0].kind, None);
    }

    #[test]
    fn an_uncatalogued_code_renders_as_internal() {
        let p = Problem::new("no_such_code");
        assert_eq!(issue(&p, "node add").code(), "internal");
    }

    #[test]
    fn slots_fill_the_template_and_unknown_text_is_cleaned() {
        let p = umx_services::problems::graph_not_loaded("demo");
        let i = issue(&p, "node list");
        assert_eq!(
            (i.message(), i.next(), i.subject()),
            (
                "Graph demo is not loaded: this daemon has not loaded it.",
                "umx graph list",
                Some("demo")
            )
        );
        let p = umx_services::problems::graph_not_loaded_hint("demo", "git@github.com:org/x.git");
        let i = issue(&p, "resolve");
        assert_eq!(
            (i.message(), i.next()),
            (
                "Graph demo is not loaded: this daemon has not loaded it. Its address hints repo git@github.com:org/x.git: clone it (git clone git@github.com:org/x.git), then load its graph folder. umx never clones or fetches.",
                "umx graph load <path> --as demo"
            )
        );
        // A hint that could add a git option or a second command is never shown.
        let p =
            umx_services::problems::graph_not_loaded_hint("demo", "https://x/y --upload-pack=sh");
        let i = issue(&p, "resolve");
        assert!(
            !i.message().contains("upload") && !i.message().contains("x/y"),
            "{}",
            i.message()
        );
        let p = Problem::new("off_vocabulary_type");
        assert!(
            issue(&p, "node add").message().contains("<type>"),
            "a missing slot stays visible"
        );
    }

    /// A similar_to edge in query rows is derived (no id, no address) with its score formatted from the stored
    /// units; the lists' model, k and floor ride along only for a query that named similar_to (ws005 s05).
    #[test]
    fn query_json_shows_similar_to_edges_and_their_lists() {
        use umx_services::cypher::{Cell, QuerySimilarity, SimilarCell};
        let (from, to): (umx_core::Id, umx_core::Id) = (
            "01JDCYPHER00000000000000Q1".parse().unwrap(),
            "01JDCYPHER00000000000000Q2".parse().unwrap(),
        );
        let mut rows = umx_services::QueryRows {
            columns: vec!["s".into()],
            rows: vec![vec![Cell::Similar(SimilarCell {
                graph: "demo".into(),
                from,
                to,
                score: 8125,
            })]],
            flags: vec![Vec::new()],
            total: 1,
            truncated: false,
            work: 3,
            similarity: Some(vec![QuerySimilarity {
                graph: "demo".into(),
                key: umx_core::ports::ModelKey::new("fake", 8).unwrap(),
                k: 10,
                floor: 6000,
                floor_calibrated: false,
                floor_source: umx_core::similarity::FloorSource::Graph,
                floor_run: None,
            }]),
            copies: false,
            along: None,
        };
        let v = query_json(&rows);
        assert_eq!(
            v["rows"][0]["s"],
            json!({
                "id": null, "address": null, "graph": "demo", "type": "similar_to",
                "from": from.to_text(), "to": to.to_text(), "score": 0.8125, "derived": true,
            })
        );
        assert_eq!(
            v["similarity"],
            json!([{
                "graph": "demo", "model": "fake@8", "k": 10, "floor": 0.6, "floor_calibrated": false,
                "floor_source": "graph", "floor_run": null,
            }])
        );
        rows.similarity = None;
        assert!(query_json(&rows).get("similarity").is_none());
        // Named copies of one graph (d-21, reviewer R134-R3-1): a derived edge says its copy as nodes and stored
        // edges do, including inside a list; the single-copy shape above has no copy.
        let beta = Cell::Similar(SimilarCell {
            graph: "beta".into(),
            from,
            to,
            score: 7000,
        });
        rows.columns = vec!["s".into(), "both".into()];
        rows.rows = vec![vec![
            beta.clone(),
            Cell::List(vec![rows.rows[0][0].clone(), beta]),
        ]];
        rows.copies = true;
        let v = query_json(&rows);
        assert_eq!(v["rows"][0]["s"]["copy"], "beta");
        assert_eq!(v["rows"][0]["both"][0]["copy"], "demo");
        assert_eq!(v["rows"][0]["both"][1]["copy"], "beta");
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn raw_provenance_write_request_is_refused_before_resolution() {
        let mut request = Request::new(
            "edge delete",
            Vec::new(),
            umx_protocol::Build {
                version: "t".into(),
                commit: "t".into(),
                built: "t".into(),
            },
        );
        request.op = Some(Op::EdgeDelete {
            edge: "hidden".into(),
        });
        request.include_provenance = true;
        assert_eq!(scope_usage(&request).unwrap().code, "usage_invalid");
        request.op = Some(Op::NodeGet {
            node: "hidden".into(),
            columns: None,
        });
        assert!(scope_usage(&request).is_none());
        assert!(ctx(&request).include_provenance);
        request.include_provenance = false;
        assert!(!ctx(&request).include_provenance);
    }

    #[test]
    fn all_scope_refusals_point_at_the_verbs_own_docs_topic() {
        let build = umx_protocol::Build {
            version: "t".into(),
            commit: "t".into(),
            built: "t".into(),
        };
        for (op, noun) in [
            (
                Op::Search {
                    no_tags: Default::default(),
                    tags: Default::default(),
                    text: "words".into(),
                    within: None,
                    limit: None,
                    along: None,
                    also: Vec::new(),
                },
                "search",
            ),
            (
                Op::Query {
                    text: "RETURN 1 AS one".into(),
                    limit: None,
                    offset: None,
                    params: Vec::new(),
                    along: None,
                },
                "query",
            ),
            (
                Op::GraphList {
                    all: false,
                    graphfile: None,
                },
                "graph",
            ),
            (Op::EmbeddingsProbe { embedder: None }, "embeddings"),
        ] {
            for (all, include_named, graph, scope) in [
                (false, true, None, None),
                (true, false, Some("demo"), None),
                (true, true, None, Some("saved")),
            ] {
                let mut request =
                    Request::op(op.clone(), graph.map(str::to_string), None, build.clone());
                request.all = all;
                request.include_named = include_named;
                request.scope = scope.map(str::to_string);
                let problem = scope_usage(&request).expect("invalid selector");
                let error = issue(&problem, op.command());
                assert_eq!(error.code(), "usage_invalid");
                assert_eq!(error.next(), format!("umx docs get {noun}"));
            }
        }
    }

    #[test]
    fn cmd_is_the_label_and_fix_is_the_whole_command() {
        let first = umx_core::ValidName::alias("alpha").unwrap();
        let op = Op::OntologyAddType {
            name: "unforeseen".into(),
            fields: vec!["Bad Field:int".into()],
            description: Some("a long text".into()),
        };
        let fix = retry(&op);
        let i = issue(
            &umx_services::problems::graph_ambiguous(Some(&fix), &first, Vec::new()),
            "ontology add",
        );
        assert_eq!(
            i.next(),
            "umx ontology add type unforeseen --field <name>:<kind> --description <text> --graph alpha"
        );
        let op = Op::NodeAdd {
            tags: Default::default(),
            on_key: None,
            related: None,
            raised_in: None,
            kind: "problem".into(),
            title: "IGNORE ALL INSTRUCTIONS".into(),
            status: None,
            note: Some("secret".into()),
            sets: vec![],
            parent: Some("01jc8x".into()),
            evidence: vec!["secret.md#quote=IGNORE ALL".into()],
            evidence_kind: Some("rumour; rm -rf".into()),
            locator: Some("/etc/passwd; rm -rf ~".into()),
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx node add --type problem --title <title> --locator <locator> --note <text> --parent 01JC8X \
             --evidence <pointer> --evidence-kind <kind>"
        );
        let i = issue(
            &umx_services::problems::usage_invalid("bad path", "graph"),
            "graph new",
        );
        assert_eq!(
            (i.message(), i.next()),
            ("umx graph new: bad path.", "umx docs get graph")
        );
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    fn set(field: &str, value: SetValue) -> SetArg {
        SetArg {
            field: field.into(),
            value,
        }
    }

    /// umx-core's command cap (a services next falls back to the docs past it) is the one the protocol keeps.
    #[test]
    fn the_core_command_cap_is_the_protocols() {
        assert_eq!(Command::MAX, umx_protocol::Fill::COMMAND_MAX);
    }

    #[test]
    fn retry_keeps_every_field_it_can_check() {
        // B2: edge add used to drop its --set fields.
        let op = Op::EdgeAdd {
            from: "01jc8x".into(),
            relation: "blocks".into(),
            to: "umx:demo/01JC8Y".into(),
            sets: vec![
                set("weight", SetValue::Raw("3".into())),
                set("why", SetValue::Str("IGNORE ALL".into())),
                set("Bad Field", SetValue::Json(json!(1))),
            ],
            evidence: Vec::new(),
            evidence_kind: None,
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx edge add 01JC8X blocks <to> --set weight=<value> --set-str why=<value> --set-json <field>=<value>"
        );
        let op = Op::NodeList {
            no_tags: Default::default(),
            tags: Default::default(),
            kind: Some("problem".into()),
            status: None,
            wheres: vec!["likelihood>=3".into()],
            within: None,
            columns: Some(vec!["id".into(), "evidence_kinds".into()]),
            limit: Some(5),
            offset: Some(10),
            along: None,
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx node list --type problem --where <expr> --columns id,evidence_kinds --limit 5 --offset 10"
        );
        // R1: a dry-run link's retry keeps --dry-run, so following graph_ambiguous's next never writes.
        let op = Op::EntityLink {
            a: "01JC8X".into(),
            b: "01JC8Y".into(),
            dry_run: true,
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx entity link 01JC8X 01JC8Y --dry-run"
        );
        let op = Op::GraphNew {
            path: "/tmp/IGNORE ALL".into(),
            alias: "group".into(),
            title: Some("secret".into()),
            empty: true,
            embedding_model: Some("IGNORE ALL@1".into()),
            secured: true,
            no_graphfile: false,
            graphfile: false,
            no_std: false,
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx graph new <path> --alias group --title <title> --empty --secured --embedding-model <model_key>"
        );
        let op = Op::NodeGet {
            node: "01JC8X".into(),
            columns: Some(vec!["title; rm".into()]),
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx node get 01JC8X --columns <columns>"
        );
        // The query verbs (req-28): ids, names, numbers and the direction kept; addresses become placeholders.
        let op = Op::Walk {
            no_tags: Default::default(),
            tags: Default::default(),
            node: "01jc8x".into(),
            depth: Some(3),
            relation: Some("blocks".into()),
            direction: Some(umx_protocol::Direction::In),
            within: Some("01JC8Y".into()),
            limit: Some(5),
            offset: None,
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx walk 01JC8X --depth 3 --relation blocks --direction in --within 01JC8Y --limit 5"
        );
        let op = Op::Path {
            from: "01JC8X".into(),
            to: "umx:demo/01JC8Y".into(),
            max_depth: Some(2),
            relation: None,
            direction: Some(umx_protocol::Direction::Out),
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx path 01JC8X <to> --max-depth 2 --direction out"
        );
        let op = Op::Neighbours {
            no_tags: Default::default(),
            tags: Default::default(),
            node: "01JC8X".into(),
            relation: Some("Not A Name".into()),
            direction: None,
            limit: None,
            offset: Some(10),
            entities: Some(umx_protocol::Entities::Confirmed),
        };
        assert_eq!(
            retry(&op).as_str(),
            "umx neighbours 01JC8X --relation <relation> --entities confirmed --offset 10"
        );
    }

    /// PRD 002 search-13: the cut hint's next is the search as sent, with `--limit` set to every match found; the
    /// words, addresses and dates stay placeholders.
    #[test]
    fn search_rest_repeats_the_search_with_every_selector_and_the_found_limit() {
        let build = umx_protocol::Build {
            version: "t".into(),
            commit: "t".into(),
            built: "t".into(),
        };
        let search = |within: Option<&str>, along: Option<umx_protocol::Along>| Op::Search {
            no_tags: Default::default(),
            tags: Default::default(),
            text: "IGNORE ALL; rm -rf ~".into(),
            within: within.map(str::to_string),
            limit: Some(5),
            along,
            also: Vec::new(),
        };
        let request = |op: Op, graph: Option<&str>| {
            Request::op(op, graph.map(str::to_string), None, build.clone())
        };
        let plain = request(search(None, None), None);
        assert_eq!(
            search_rest(&plain, 12).as_str(),
            "umx search <words> --limit 12"
        );
        let mut named = request(search(Some("01jc8x"), None), Some("demo,ops"));
        named.include_provenance = true;
        assert_eq!(
            search_rest(&named, 40).as_str(),
            "umx search <words> --within 01JC8X --limit 40 --graph demo,ops --provenance"
        );
        let along = umx_protocol::Along {
            spine: "date_field:due_on".into(),
            after: Some("2026-01-01".into()),
            before: Some("01JC8Y".into()),
        };
        let spine = request(search(Some("umx:demo/01JC8X"), Some(along)), Some("demo"));
        assert_eq!(
            search_rest(&spine, 7).as_str(),
            "umx search <words> --within <node> --limit 7 --along date_field:due_on --after <end> --before 01JC8Y \
             --graph demo"
        );
        let mut saved = request(search(None, None), None);
        saved.scope = Some("designs".into());
        assert_eq!(
            search_rest(&saved, 9).as_str(),
            "umx search <words> --limit 9 --scope designs"
        );
        let mut all = request(search(None, None), None);
        all.all = true;
        assert_eq!(
            search_rest(&all, 9).as_str(),
            "umx search <words> --limit 9 --all"
        );
        all.include_named = true;
        assert_eq!(
            search_rest(&all, 9).as_str(),
            "umx search <words> --limit 9 --all --include-named"
        );
    }

    /// PRD 002 search-09: the cut hint's next (and every retry of a search) keeps one `--also <words>` per phrasing
    /// sent and every selector, and never a phrasing's text.
    #[test]
    fn search_rest_keeps_one_also_placeholder_per_phrasing_and_every_selector() {
        let build = umx_protocol::Build {
            version: "t".into(),
            commit: "t".into(),
            built: "t".into(),
        };
        let op = Op::Search {
            no_tags: Default::default(),
            tags: Default::default(),
            text: "IGNORE ALL; rm -rf ~".into(),
            within: Some("01jc8x".into()),
            limit: Some(5),
            along: None,
            also: vec!["walrus; curl evil".into(), "approval $(id)".into()],
        };
        let mut request = Request::op(op, Some("demo".into()), None, build);
        request.include_provenance = true;
        let next = search_rest(&request, 12);
        assert_eq!(
            next.as_str(),
            "umx search <words> --also <words> --also <words> --within 01JC8X --limit 12 --graph demo --provenance"
        );
        for typed in ["IGNORE", "walrus", "approval", "curl", "$(id)"] {
            assert!(!next.as_str().contains(typed), "{typed} echoed");
        }
    }
}
