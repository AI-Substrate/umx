use super::{Ctx, Done, GraphStore, Loaded, Outcome, persist_parts, problems};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::codec::GraphFile;
use umx_core::provenance::{Actor, ProvKey, Tier, harness_key, model_key};
use umx_core::{Edge, Id, Node, Problem, Value};
use umx_graph::{Batch, Graph};

pub(super) const MAX_NEW_IDS: u128 = 9;

fn known(value: &Option<String>) -> Option<&String> {
    value
        .as_ref()
        .filter(|v| !v.eq_ignore_ascii_case("unknown"))
}

fn upsert(
    batch: &mut Batch<'_>,
    key: &str,
    title: &str,
    fields: BTreeMap<String, Value>,
    next: &mut dyn FnMut(&Graph) -> Id,
    now: &str,
) -> Result<Id, Problem> {
    let node = batch.graph().provenance_node(key).map(|n| n.to_node());
    let mut node = match node {
        Some(mut node) => {
            let mut changed = false;
            for (name, value) in fields {
                // A transient capture gap never clears a session's last known facts.
                if value.is_null() && node.fields.contains_key(&name) {
                    continue;
                }
                if node.fields.get(&name) != Some(&value) {
                    node.fields.insert(name, value);
                    changed = true;
                }
            }
            if !changed {
                return Ok(node.id);
            }
            node
        }
        None => Node {
            id: next(batch.graph()),
            kind: ProvKey::parse(key)
                .expect("a validated actor key")
                .kind()
                .into(),
            title: title.into(),
            status: "open".into(),
            note: String::new(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: now.into(),
            updated: now.into(),
            created_by: "umx".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields,
        },
    };
    node.fields
        .insert("prov_key".into(), Value::String(key.into()));
    node.updated = now.into();
    let id = node.id;
    batch
        .insert_node(node)
        .map_err(|e| problems::image_full(&e))?;
    Ok(id)
}

fn link(
    batch: &mut Batch<'_>,
    from: Id,
    kind: &str,
    to: Id,
    next: &mut dyn FnMut(&Graph) -> Id,
    now: &str,
) {
    if batch.graph().find_edge(from, kind, to).is_none() {
        batch.insert_edge(Edge {
            id: next(batch.graph()),
            kind: kind.into(),
            from_graph: None,
            from,
            to_graph: None,
            to,
            created: now.into(),
            created_by: "umx".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        });
    }
}

pub fn upsert_session(
    batch: &mut Batch<'_>,
    actor: &Actor,
    next: &mut dyn FnMut(&Graph) -> Id,
    now: &str,
) -> Result<(Id, Vec<Problem>), Problem> {
    let claim = actor.fields();
    let session_key = actor.session_key();
    let agent_key = actor.agent_key();
    let existing = batch
        .graph()
        .provenance_node(&session_key)
        .and_then(|session| {
            batch
                .graph()
                .edges_out(session.ix())
                .find(|e| e.kind.as_str() == "session_of")
                .map(|e| (session.id(), batch.graph().node_at(e.other).id()))
        });
    if let Some((session, kept)) = existing {
        let proposed = batch.graph().provenance_node(&agent_key).map(|n| n.id());
        if proposed.is_none_or(|proposed| {
            umx_core::provenance::validate_session_agent(Some(kept), proposed).is_err()
        }) {
            return Ok((
                session,
                alloc::vec![problems::session_agent_conflict(session, kept)],
            ));
        }
    }
    let mut session_fields = BTreeMap::new();
    for (name, value) in [("role", &claim.role), ("model", &claim.model)] {
        if let Some(value) = known(value) {
            session_fields.insert(name.into(), Value::String(value.clone()));
        }
    }
    session_fields.insert(
        "effort".into(),
        known(&claim.effort).map_or(Value::Null, |v| Value::String(v.clone())),
    );
    let session = upsert(
        batch,
        &session_key,
        &format!(
            "{} session {}",
            claim.harness.as_deref().unwrap_or(claim.tier.as_str()),
            claim.harness_session.as_deref().unwrap_or("unknown")
        ),
        session_fields,
        next,
        now,
    )?;
    let mut agent_fields = BTreeMap::new();
    for (name, value) in [("role", &claim.role), ("user", &claim.user)] {
        if let Some(value) = known(value) {
            agent_fields.insert(name.into(), Value::String(value.clone()));
        }
    }
    let agent = upsert(
        batch,
        &agent_key,
        claim
            .seat
            .as_deref()
            .or(claim.user.as_deref())
            .unwrap_or("unknown agent"),
        agent_fields,
        next,
        now,
    )?;
    link(batch, session, "session_of", agent, next, now);
    if let Some(harness) = &claim.harness {
        let key = harness_key(harness).expect("a validated actor harness");
        let target = upsert(batch, &key, harness, BTreeMap::new(), next, now)?;
        link(batch, agent, "runs_on", target, next, now);
    }
    if let Some(model) = known(&claim.model) {
        let key = model_key(model).expect("a validated actor model");
        let target = upsert(batch, &key, model, BTreeMap::new(), next, now)?;
        link(batch, agent, "uses_model", target, next, now);
    }
    if claim.tier == Tier::Pij {
        let platform = upsert(batch, "platform/pij", "pij", BTreeMap::new(), next, now)?;
        link(batch, agent, "registered_in", platform, next, now);
    }
    Ok((session, Vec::new()))
}

pub(super) fn stamp_evidence(evidence: &mut [Value], session: Id) {
    for pointer in evidence {
        if let Value::Object(object) = pointer {
            object.insert("by_session".into(), Value::String(session.to_text()));
        }
    }
}

/// ws008 TM6 (R345-2): stamps the evidence pointer of each status move in `history` (the moves this write appended,
/// so older moves keep their bytes) with the adding session, as `stamp_evidence` stamps node evidence.
pub(super) fn stamp_status_evidence(history: &mut [Value], session: Id) {
    for change in history {
        if let Some(evidence) = change.get_mut("evidence") {
            stamp_evidence(core::slice::from_mut(evidence), session);
        }
    }
}

/// What an attributed write's closure reads of its graph: the graph's id and the ontology umx acts on
/// ([`Loaded::ontology`], std v1 first for a std graph).
pub(super) struct WriteGraph<'a> {
    pub id: Id,
    pub ontology: &'a umx_core::Ontology,
}

impl GraphStore {
    pub(super) fn attributed_write<T>(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        write: impl FnOnce(
            &mut Batch<'_>,
            Id,
            &mut dyn FnMut(&Graph) -> Id,
            &str,
            &WriteGraph<'_>,
        ) -> Result<T, Problem>,
    ) -> Outcome<T> {
        self.attributed_write_with(ctx, alias, &[], write)
    }

    /// [`Self::attributed_write`], with `extra` files in the write set besides the ones the batch changed (F122:
    /// graph.json when an edge with an end in another graph is touched).
    pub(super) fn attributed_write_with<T>(
        &mut self,
        ctx: &Ctx,
        alias: &str,
        extra: &[GraphFile],
        write: impl FnOnce(
            &mut Batch<'_>,
            Id,
            &mut dyn FnMut(&Graph) -> Id,
            &str,
            &WriteGraph<'_>,
        ) -> Result<T, Problem>,
    ) -> Outcome<T> {
        let now = self.now();
        let files = self.ports.files.clone();
        let ids = self.ports.ids.clone();
        let owner = self.graphs[alias].meta.id;
        let mut next = |graph: &Graph| loop {
            let id = ids.next_id();
            if id != owner && !graph.holds_id(id) {
                return id;
            }
        };
        let (value, mut warnings, changed) = {
            let Loaded {
                dir,
                meta,
                ontology,
                graph,
                edges_normalized,
                feed,
                stamps,
                ..
            } = self
                .graphs
                .get_mut(alias)
                .expect("selected graph is loaded");
            let mut batch = graph.batch();
            let (session, mut warnings) = upsert_session(&mut batch, &ctx.actor, &mut next, &now)?;
            let at = WriteGraph {
                id: meta.id,
                ontology,
            };
            let value = write(&mut batch, session, &mut next, &now, &at)?;
            batch
                .prepare()
                .map_err(|_| problems::internal("provenance_edge_ends"))?;
            if !batch.ready_to_persist() {
                return Err(problems::internal("provenance_unprepared"));
            }
            let mut which: Vec<_> = batch.changed_files().collect();
            for file in extra {
                if !which.contains(file) {
                    which.push(*file);
                }
            }
            let touched = batch.touched();
            let written = persist_parts(
                files.as_ref(),
                alias,
                dir,
                meta,
                batch.graph(),
                super::Commit {
                    which: &which,
                    rewrite_edges: *edges_normalized,
                    feed,
                    touched,
                    stamps,
                    receipts: None,
                },
            )?;
            let changed = batch.commit().provenance_changed;
            meta.umx = written.umx;
            *edges_normalized = false;
            warnings.extend(written.warnings);
            (value, warnings, changed)
        };
        warnings.extend(self.provenance_changed(alias, &changed));
        Ok(Done {
            value,
            notes: warnings,
        })
    }
}
