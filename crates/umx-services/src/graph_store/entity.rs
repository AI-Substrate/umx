//! ws008 E2 (req-127): `same_as` links and `umx entity show`. A link is one stored symmetric edge, usually between
//! nodes in two graphs, written through the cross-graph edge path (ws005 s10). An entity is built at read time, in
//! memory, from the graphs the read covers: the nodes that share an `entity_key` within one type, or are joined by
//! `same_as`, transitively; `possibly_same_as` partners are listed as proposed and never joined. Nothing merged is
//! stored (D63).

use super::edge::{check_deletable, external_files, new_edge, stored_sentence};
use super::external::{StoredEnd, find_edge, ordered_ends, set_ends};
use super::{Ctx, GraphStore, alias_name, persist, problems, resolve_read_node, split_address};
use crate::{Done, Outcome};
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::codec::GraphFile;
use umx_core::entity::{ENTITY_KEY, stored_keys};
use umx_core::ontology::{DENY_REASON, NOT_SAME_AS, POSSIBLY_SAME_AS, SAME_AS};
use umx_core::ui::Touched;
use umx_core::{Command, Edge, Id, Problem, ValidName};

/// What `umx entity link` stored, or found already stored (`unchanged`).
#[derive(Clone, Debug, PartialEq)]
pub struct EntityLinked {
    pub alias: String,
    /// The stored edge; None in a dry run that would add one (no id is drawn).
    pub edge: Option<Edge>,
    /// "<a title> same_as <b title>". Graph text.
    pub sentence: String,
    /// F234: the sentence holds an end's title umx's secret check flags.
    pub sentence_flagged: bool,
    pub unchanged: bool,
    /// `--dry-run` (F179): checked and warned as a real link, nothing written.
    pub dry_run: bool,
}

/// What `umx entity deny` stored, or found already stored (`unchanged`), and the `possibly_same_as` it replaced.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityDenied {
    pub alias: String,
    pub edge: Edge,
    /// "<a title> not same as <b title>", with an end in another graph as its stored address. Graph text.
    pub sentence: String,
    /// F234: the sentence holds a local end's title umx's secret check flags.
    pub sentence_flagged: bool,
    pub unchanged: bool,
    /// The id of the proposal (`possibly_same_as`, same graph) the denial replaced in the same write.
    pub replaced: Option<Id>,
}

/// What `umx entity unlink` removed: the edge and the graph that stored it, or nothing (`unchanged`).
#[derive(Clone, Debug, PartialEq)]
pub struct EntityUnlinked {
    pub removed: Option<(String, Edge)>,
}

/// How one member joined its entity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityMatch {
    /// The node `entity show` was asked about.
    Asked,
    /// It holds this key (stored form), and another member of the same type holds it too.
    Key(String),
    /// A `same_as` edge, stored in graph `alias`, touches it.
    Link { alias: String, edge: Id },
}

/// One node of an entity, in a graph the read covers.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityMember {
    pub alias: String,
    pub id: Id,
    pub kind: String,
    pub title: String,
    pub keys: Vec<String>,
    pub matched: Vec<EntityMatch>,
    /// F188: what the secret scan flags among the title and `keys` shown (`field:title`, `key:<i>` into `keys`).
    pub secret_suspected: Vec<String>,
}

/// A node a `possibly_same_as` edge proposes as the same as a member. Never joined (one hop only).
#[derive(Clone, Debug, PartialEq)]
pub struct EntityProposed {
    pub alias: String,
    pub id: Id,
    pub kind: String,
    pub title: String,
    /// The member it is proposed for: its graph and id.
    pub with: (String, Id),
    /// The `possibly_same_as` edge: the graph that stores it and its id.
    pub edge: (String, Id),
    /// F188: `field:title` when the secret scan flags the title shown.
    pub secret_suspected: Vec<String>,
}

/// `umx entity show`: the entity of one node, over the graphs the read covered.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityShown {
    pub alias: String,
    pub node: Id,
    /// The graphs the read covered, in scope order. Never an unnamed secured graph (d-20).
    pub graphs: Vec<String>,
    /// The asked node first, then by graph in scope order and id.
    pub members: Vec<EntityMember>,
    pub proposed: Vec<EntityProposed>,
    /// The lead member (F4, graph-addressing-expert): the lowest (graph id, node id) among the members shown, as its
    /// graph's alias, graph id and node id. Derived from the covered graphs, never stored.
    pub lead: (String, Id, Id),
    /// The keys two or more members hold, sorted, in stored form.
    pub keys: Vec<String>,
    /// F188: the positions in `keys` the secret scan flags (`key:<i>`).
    pub keys_secret_suspected: Vec<String>,
}

/// A node as the walk meets it: a graph id and a node id.
type At = (Id, Id);

/// What a walk found besides its members: the identity edges it met, the keys it did not use in a graph because
/// two or more nodes of the type hold them there (G2: graph alias, type, key), and the nodes of another type holding
/// a member's key in a covered graph (G5: the member and its type, the holder, its graph alias and type).
#[derive(Default)]
struct Walked {
    reached: BTreeSet<At>,
    met: Vec<Met>,
    ignored: BTreeSet<(String, String, String)>,
    other_types: BTreeSet<(At, String, At, String, String)>,
}

/// A stored identity edge the walk met: the graph storing it, its id, kind and both ends.
struct Met {
    owner: String,
    edge: Id,
    same_as: bool,
    /// A `not_same_as` denial (E2c): never followed, only kept to split groups.
    denied: bool,
    from: At,
    to: At,
}

/// The entity of one node once denials have split it (ws008 E2c): the start's group, the joins skipped because they
/// would have put a denied pair in one group (each as a warning), and every reached node's group root.
struct Grouped {
    group: BTreeSet<At>,
    skipped: Vec<Problem>,
    roots: BTreeMap<At, At>,
    /// Each root's denied partners, for dropping proposals between denied groups.
    denied: BTreeMap<At, BTreeSet<At>>,
}

impl GraphStore {
    /// `umx entity link <a> <b>` (ws008 E2): stores one `same_as` edge in the graph `--graph` names, else in `a`'s
    /// graph (`UMX_GRAPH` only resolves a bare id). Linking a node to itself, or two copies of one graph, is refused; a link
    /// that already exists is `unchanged`; ends of two types still link and warn `entity_type_mismatch`.
    pub fn entity_link(
        &mut self,
        ctx: &Ctx,
        a: &str,
        b: &str,
        dry_run: bool,
    ) -> Outcome<EntityLinked> {
        let author = self.link_author(ctx, a)?;
        let (from, to) = (self.write_end(&author, a)?.0, self.write_end(&author, b)?.0);
        let own = self.graphs[&author].meta.id;
        let ends = [self.end_alias(&author, a), self.end_alias(&author, b)];
        let mut aliases: Vec<&str> = alloc::vec![author.as_str()];
        aliases.extend(ends.iter().flatten().map(String::as_str));
        if aliases.iter().any(|x| {
            aliases
                .iter()
                .any(|y| x != y && self.graphs[*x].meta.id == self.graphs[*y].meta.id)
        }) {
            return Err(problems::entity_link_invalid(
                "its ends name two copies of one graph",
                Command::new("docs get entity"),
            ));
        }
        let (at_a, at_b) = (at(own, from), at(own, to));
        // Each end's type as the copy it names holds it (R250-4), read before `ends` is consumed below.
        let kinds = [(&ends[0], at_a), (&ends[1], at_b)].map(|(alias, at)| {
            let l = self.graphs.get(alias.as_deref()?)?;
            Some(l.graph.node(at.1)?.kind().to_string())
        });
        if at_a == at_b {
            return Err(problems::entity_link_invalid(
                "a node cannot be linked to itself",
                Command::new("docs get entity"),
            ));
        }
        // A same_as pair is stored once (review, the project manager's ruling): one already in the author's graph or
        // either end's graph is the link, whatever the order and whatever --graph names.
        let end_aliases = ends.clone();
        let mut owners: Vec<String> = alloc::vec![author.clone()];
        for alias in ends.into_iter().flatten() {
            if !owners.contains(&alias) {
                owners.push(alias);
            }
        }
        // E2c (graph-addressing-expert): a pair denied in any of those graphs is never linked; the denial goes first.
        if owners
            .iter()
            .any(|owner| self.stored_at(owner, at_a, at_b, NOT_SAME_AS).is_some())
        {
            return Err(problems::entity_link_invalid(
                "these two nodes are denied as the same (not_same_as)",
                pair_command("entity undeny", &end_aliases, (at_a.1, at_b.1)),
            ));
        }
        for owner in &owners {
            if let Some(id) = self.stored_at(owner, at_a, at_b, SAME_AS) {
                let edge = self.graphs[owner].graph.edge(id).expect("found").to_edge();
                let row = self.edge_read_row(&ctx.clone().graph(owner), &[], owner, id);
                let value = EntityLinked {
                    alias: owner.clone(),
                    edge: Some(edge),
                    sentence_flagged: row.as_ref().is_some_and(|row| {
                        self.edge_titles_flagged(core::slice::from_ref(row))[0].contains(&true)
                    }),
                    sentence: row.map(|row| row.sentence).unwrap_or_default(),
                    unchanged: true,
                    dry_run,
                };
                return Ok(Done::ok(value));
            }
        }
        let mut warnings = Vec::new();
        if let (Some(ka), Some(kb), Some(ga), Some(gb)) =
            (&kinds[0], &kinds[1], &end_aliases[0], &end_aliases[1])
            && ka != kb
        {
            warnings.push(problems::entity_type_mismatch(
                (&alias_name(ga), at_a.1, ka),
                (&alias_name(gb), at_b.1, kb),
                "are linked as one entity",
            ));
        }
        warnings.extend(self.link_overlaps(ctx, (a, b), (at_a, at_b), &owners)?);
        if dry_run {
            return self.link_dry_run(ctx, &author, (a, b), warnings);
        }
        let added = self.edge_add(&ctx.clone().graph(&author), a, SAME_AS, b, &[])?;
        warnings.extend(added.notes.into_iter().filter(|w| w.code != "edge_exists"));
        let v = added.value;
        let value = EntityLinked {
            alias: v.alias,
            edge: Some(v.edge),
            sentence: v.sentence,
            sentence_flagged: v.sentence_flagged,
            unchanged: v.existed,
            dry_run: false,
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// `umx entity unlink <a> <b>` (ws008 E2): removes the `same_as` edge between them from the graph `--graph`
    /// names, else from `a`'s graph or, failing that, `b`'s. None stored is `unchanged`, not an error.
    pub fn entity_unlink(&mut self, ctx: &Ctx, a: &str, b: &str) -> Outcome<EntityUnlinked> {
        self.remove_pair(ctx, a, b, SAME_AS)
    }

    /// `umx entity deny <a> <b> --reason <text>` (ws008 E2c, req-137): stores one `not_same_as` edge, so no key or
    /// link joins the two in any read that covers its graph and both nodes. Stored in the graph `--graph` names, else
    /// `a`'s, which must hold one of the two (security). A far node must be readable: one in a hidden or unloaded graph
    /// is refused exactly as an unknown node. Refused: a node with itself, two copies of one graph, a pair this graph
    /// links (unlink first). A repeat is `unchanged` (the reason stays). A `possibly_same_as` between the pair in this
    /// graph is replaced in the same write.
    pub fn entity_deny(
        &mut self,
        ctx: &Ctx,
        a: &str,
        b: &str,
        reason: Option<&str>,
    ) -> Outcome<EntityDenied> {
        let reason = super::evidence_edit::check_reason(
            reason,
            "entity",
            "entity deny needs --reason (why the two are not the same thing)",
        )?;
        let author = self.link_author(ctx, a)?;
        self.files_unchanged(&author)?;
        let own = self.graphs[&author].meta.id;
        let access = self.external_access(ctx)?;
        let refs = [Some(a), Some(b)];
        let (from, mut warnings) = self.deny_end(&access, &author, a, &refs)?;
        let (to, more) = self.deny_end(&access, &author, b, &refs)?;
        warnings.extend(more);
        let (at_a, at_b) = (at(own, from), at(own, to));
        let ends = [self.end_alias(&author, a), self.end_alias(&author, b)];
        let deny_fix = || {
            Command::new("entity deny")
                .word("<a>")
                .word("<b>")
                .word("--reason")
                .word("<text>")
        };
        if at_a.0 != own && at_b.0 != own {
            return Err(problems::entity_link_invalid(
                "the graph that stores a denial must hold one of its two nodes",
                deny_fix(),
            ));
        }
        let mut aliases: Vec<&str> = alloc::vec![author.as_str()];
        aliases.extend(ends.iter().flatten().map(String::as_str));
        if aliases.iter().any(|x| {
            aliases
                .iter()
                .any(|y| x != y && self.graphs[*x].meta.id == self.graphs[*y].meta.id)
        }) {
            return Err(problems::entity_link_invalid(
                "its ends name two copies of one graph",
                Command::new("docs get entity"),
            ));
        }
        if at_a == at_b {
            return Err(problems::entity_link_invalid(
                "a node cannot be denied with itself",
                Command::new("docs get entity"),
            ));
        }
        if self.stored_between(&author, from, to, SAME_AS).is_some() {
            return Err(problems::entity_link_invalid(
                "this graph links the two as the same (same_as) so unlink them first",
                pair_command("entity unlink", &ends, (at_a.1, at_b.1)),
            ));
        }
        let graph = alias_name(&author);
        // A denial is stored once, as a link is: one in the author's graph or either end's is the denial, whatever the
        // order (a repeat keeps its first reason).
        let mut owners: Vec<String> = alloc::vec![author.clone()];
        for alias in ends.iter().flatten() {
            if !owners.contains(alias) {
                owners.push(alias.clone());
            }
        }
        for owner in &owners {
            if let Some(id) = self.stored_at(owner, at_a, at_b, NOT_SAME_AS) {
                let edge = self.graphs[owner].graph.edge(id).expect("found").to_edge();
                let value = EntityDenied {
                    sentence: stored_sentence(&self.graphs[owner].graph, &edge),
                    sentence_flagged: self.stored_sentence_flagged(owner, &edge),
                    alias: owner.clone(),
                    edge,
                    unchanged: true,
                    replaced: None,
                };
                return Ok(Done {
                    value,
                    notes: warnings,
                });
            }
        }
        let old = self
            .stored_between(&author, from, to, POSSIBLY_SAME_AS)
            .and_then(|id| self.graphs[&author].graph.edge(id))
            .map(|e| e.to_edge());
        // S323-1 (graph-storage-expert): replacing a proposal deletes it, so it must be deletable, as `edge delete`
        // requires: a repeated id would come back from its earlier line on the next load.
        if let Some(old) = &old {
            check_deletable(&self.graphs[&author].graph, &graph, old)?;
        }
        let (from, to) = ordered_ends(own, true, from, to);
        let fields: BTreeMap<String, umx_core::Value> =
            BTreeMap::from([(DENY_REASON.to_string(), umx_core::Value::String(reason))]);
        let external = from.graph.is_some() || to.graph.is_some();
        let extra: &[GraphFile] = if external || old.as_ref().is_some_and(|o| o.has_external_end())
        {
            &[GraphFile::Graph]
        } else {
            &[]
        };
        let done =
            self.attributed_write_with(ctx, &author, extra, |batch, session, next, now, _| {
                if let Some(old) = &old {
                    batch.remove_edge(old.id);
                }
                let mut edge = set_ends(
                    new_edge(
                        next(batch.graph()),
                        from.node,
                        NOT_SAME_AS,
                        to.node,
                        now,
                        &ctx.by,
                        fields,
                    ),
                    from,
                    to,
                );
                warnings.extend(
                    super::secrets::edge_write_secrets(&edge.fields, &[])
                        .into_iter()
                        .map(|hit| hit.warning(&graph, edge.id, "Edge")),
                );
                edge.record_author(session);
                batch.insert_edge(edge.clone());
                Ok(edge)
            })?;
        warnings.extend(done.notes);
        let edge = done.value;
        // After the commit only: the proposal's entries out, the denial's in (as F122's edit).
        let l = &self.graphs[&author];
        if let Some(old) = &old {
            self.incoming.remove_edge(l.instance, old);
        }
        self.incoming
            .add_edge(l.instance, l.graph.edge(edge.id).expect("the denial"));
        let value = EntityDenied {
            sentence: stored_sentence(&l.graph, &edge),
            sentence_flagged: self.stored_sentence_flagged(&author, &edge),
            alias: author,
            edge,
            unchanged: false,
            replaced: old.map(|o| o.id),
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// One end of a denial, resolved for the graph `author` that stores it. A far end (another graph) must be one the
    /// writer can read (security, d-20): a node in a hidden or unloaded graph, a graph not loaded, or a node a loaded
    /// graph does not hold all give the same `node_not_found`, byte for byte, so the reply never says which.
    fn deny_end(
        &self,
        access: &Ctx,
        author: &str,
        text: &str,
        refs: &[Option<&str>],
    ) -> Result<(StoredEnd, Vec<Problem>), Problem> {
        let (selector, node) = split_address(text);
        let own = self.graphs[author].meta.id;
        let far = selector.is_some_and(|s| self.addressed_graph(s).ok().flatten() != Some(own));
        let unknown = || {
            let alias = selector
                .and_then(ValidName::alias)
                .or_else(|| ValidName::new("graph"))
                .expect("a name");
            problems::node_not_found(node, &alias, None)
        };
        match self.write_end(author, text) {
            Ok((end, warnings)) => {
                if let Some(graph) = end.graph {
                    let read = self.read_end(access, refs, graph, end.node);
                    if read.loaded.is_none() || read.state.is_some() {
                        return Err(unknown());
                    }
                }
                Ok((end, warnings))
            }
            // R2 (review): only a fragment or selector is a grammar error whatever is loaded; every other failure of a
            // far end (an id prefix only a loaded graph resolves, say) depends on what is loaded, so it is unknown.
            Err(p) if p.code == "usage_invalid" && (text.contains('#') || text.contains('?')) => {
                Err(p)
            }
            Err(_) if far => Err(unknown()),
            Err(p) => Err(p),
        }
    }

    /// `umx entity undeny <a> <b>` (ws008 E2c): removes the `not_same_as` edge between them, looking where unlink
    /// looks. None stored is `unchanged`.
    pub fn entity_undeny(&mut self, ctx: &Ctx, a: &str, b: &str) -> Outcome<EntityUnlinked> {
        self.remove_pair(ctx, a, b, NOT_SAME_AS)
    }

    /// Unlink and undeny: the `kind` edge between `a` and `b`, from the graph `--graph` names, else `a`'s graph or,
    /// failing that, `b`'s.
    fn remove_pair(&mut self, ctx: &Ctx, a: &str, b: &str, kind: &str) -> Outcome<EntityUnlinked> {
        if ctx.graph.is_some() {
            let owner = self.select(ctx, &[])?;
            return match self.stored_pair(&owner, a, b, kind)? {
                Some(id) => self.remove_link(&owner, id),
                None => Ok(Done::ok(EntityUnlinked { removed: None })),
            };
        }
        // a's graph, then b's (R250-3). A graph that is not loaded and not remembered (a link may name it by its full
        // id) is skipped, so it never blocks the other; a remembered one answers graph_not_loaded so the daemon loads
        // it on demand and runs this again; any other error stands.
        let mut owners: Vec<String> = Vec::new();
        let mut held: Option<Problem> = None;
        for text in [a, b] {
            match self.select(ctx, &[Some(text)]) {
                Ok(alias) => {
                    if !owners.contains(&alias) {
                        owners.push(alias);
                    }
                }
                Err(p) if p.code == "graph_not_loaded" && !self.remembered_end(text) => {
                    held.get_or_insert(p);
                }
                Err(p) if p.code == "graph_not_loaded" && !owners.is_empty() => {
                    held = Some(p);
                }
                Err(p) => return Err(p),
            }
        }
        for owner in &owners {
            if let Some(id) = self.stored_pair(owner, a, b, kind)? {
                return self.remove_link(owner, id);
            }
        }
        match held {
            Some(p)
                if owners.is_empty()
                    || (p.code == "graph_not_loaded" && self.remembered_end(b)) =>
            {
                Err(p)
            }
            _ => Ok(Done::ok(EntityUnlinked { removed: None })),
        }
    }

    /// `umx entity show <node>` (ws008 E2): the node's entity over the graphs the read covers. With `--scope` or a
    /// `--graph` list, those graphs, and the node must be in one of them; else every loaded open graph plus the node's own graph (named by its address
    /// or `--graph`, so a secured one counts, d-20). One copy per graph id (d-21), so two copies never group.
    pub fn entity_show(&self, ctx: &Ctx, node: &str) -> Outcome<EntityShown> {
        let own = self.external_read_alias(ctx, &[Some(node)])?;
        let id = resolve_read_node(&self.graphs[&own], ctx, &own, node)?;
        let explicit = ctx.scope.is_some() || ctx.graph.as_deref().is_some_and(|g| g.contains(','));
        let listed = if explicit {
            self.graphs_in_scope(ctx, None, "entity")?.aliases
        } else {
            self.unnamed_scope_with_names(core::slice::from_ref(&own))?
                .aliases
        };
        let (graphs, by_id) = self.covered(core::slice::from_ref(&own), listed);
        let start = (self.graphs[&own].meta.id, id);
        let access = self.external_access(ctx)?;
        let walked = self.walk_entity((&access, &[Some(node)]), &graphs, &by_id, start);
        let grouped = self.group_of(start, &walked, &by_id);
        let (group, met) = (&grouped.group, &walked.met);
        let shown = |at: &At| -> Option<(String, umx_graph::NodeRef<'_>)> {
            let alias = by_id.get(&at.0)?;
            Some((alias.clone(), self.graphs[alias].graph.node(at.1)?))
        };
        let order = |alias: &str| graphs.iter().position(|g| g == alias).unwrap_or(usize::MAX);
        // F188 (R324-3): one lazy allows snapshot for every member and proposal this read flags.
        let allows = core::cell::OnceCell::new();
        let mut members: Vec<EntityMember> = group
            .iter()
            .filter_map(|at| {
                let (alias, n) = shown(at)?;
                let keys = n
                    .field(ENTITY_KEY)
                    .map(|v| stored_keys(&v))
                    .unwrap_or_default();
                let secret_suspected = self.member_flags(&allows, &alias, at.1, n.title(), &keys);
                Some(EntityMember {
                    keys,
                    kind: n.kind().to_string(),
                    title: n.title().to_string(),
                    id: at.1,
                    matched: if *at == start {
                        alloc::vec![EntityMatch::Asked]
                    } else {
                        Vec::new()
                    },
                    secret_suspected,
                    alias,
                })
            })
            .collect();
        // A key match is a key both members hold usably (R3: one held twice in a graph joined neither, so a link did).
        let usable = |m: &EntityMember, key: &str| {
            !self.graphs[&m.alias].graph.entity_holders(&m.kind, key).1
        };
        for i in 0..members.len() {
            for key in members[i].keys.clone() {
                let shared = usable(&members[i], &key)
                    && members.iter().enumerate().any(|(j, m)| {
                        j != i
                            && m.kind == members[i].kind
                            && m.keys.contains(&key)
                            && usable(m, &key)
                    });
                if shared {
                    members[i].matched.push(EntityMatch::Key(key));
                }
            }
        }
        // E2c: a proposal to a node the entity's group is denied with, or to a group holding one, is dropped. R5
        // (review): a target the walk did not reach is grouped on its own, under the same coverage, only to check this;
        // its group never joins the entity.
        let start_denied = grouped.denied.get(&grouped.roots[&start]);
        let blocked = |other: &At| {
            let Some(partners) = start_denied else {
                return false;
            };
            if partners.contains(other) {
                return true;
            }
            if grouped.roots.contains_key(other) {
                return partners
                    .iter()
                    .any(|p| grouped.roots.get(p) == grouped.roots.get(other));
            }
            if !by_id.contains_key(&other.0) {
                return false;
            }
            let theirs = self.walk_entity((&access, &[Some(node)]), &graphs, &by_id, *other);
            let group = self.group_of(*other, &theirs, &by_id).group;
            partners.iter().any(|p| group.contains(p))
        };
        let mut proposed: Vec<EntityProposed> = Vec::new();
        for m in met.iter().filter(|m| !m.denied) {
            let ends = [m.from, m.to];
            for (i, end) in ends.iter().enumerate() {
                let Some(member) = members
                    .iter_mut()
                    .find(|x| by_id.get(&end.0) == Some(&x.alias) && x.id == end.1)
                else {
                    continue;
                };
                if m.same_as {
                    // Only a link the group kept (E2c: a skipped one joined nothing).
                    if group.contains(&m.from) && group.contains(&m.to) {
                        member.matched.push(EntityMatch::Link {
                            alias: m.owner.clone(),
                            edge: m.edge,
                        });
                    }
                    continue;
                }
                let other = ends[1 - i];
                if group.contains(&other) || blocked(&other) {
                    continue;
                }
                let with = (member.alias.clone(), member.id);
                if let Some((alias, n)) = shown(&other)
                    && !proposed.iter().any(|p| p.alias == alias && p.id == other.1)
                {
                    proposed.push(EntityProposed {
                        id: other.1,
                        kind: n.kind().to_string(),
                        title: n.title().to_string(),
                        secret_suspected: self.member_flags(
                            &allows,
                            &alias,
                            other.1,
                            n.title(),
                            &[],
                        ),
                        alias,
                        with,
                        edge: (m.owner.clone(), m.edge),
                    });
                }
            }
        }
        for m in &mut members {
            m.matched.sort();
            m.matched.dedup();
        }
        members.sort_by_key(|m| {
            (
                m.matched.first() != Some(&EntityMatch::Asked),
                order(&m.alias),
                m.id,
            )
        });
        proposed.sort_by_key(|p| (order(&p.alias), p.id));
        let gid = |alias: &str| self.graphs[alias].meta.id;
        let lead = members
            .iter()
            .map(|m| (gid(&m.alias), m.id, &m.alias))
            .min()
            .map(|(g, id, alias)| (alias.clone(), g, id))
            .expect("the asked node is a member");
        let mut keys: Vec<String> = members
            .iter()
            .flat_map(|m| m.keys.iter())
            .filter(|k| members.iter().filter(|m| m.keys.contains(k)).count() > 1)
            .cloned()
            .collect();
        keys.sort();
        keys.dedup();
        let mut warnings = guards(&members, &walked, group, &by_id, |a: &str| order(a));
        warnings.extend(grouped.skipped);
        let value = EntityShown {
            alias: own,
            node: id,
            graphs,
            members,
            proposed,
            lead,
            keys_secret_suspected: Self::key_flags(&keys),
            keys,
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// G5 (ws008 E2b) for a key lookup over `aliases` (the covered graphs): `entity_type_mismatch` when `key` is held
    /// under two types there, or under a type other than `kind` when the lookup named one. The first holder of the
    /// asked (else the first) type is `a`; the first holder of another type is `b`.
    pub(crate) fn key_type_mismatch(
        &self,
        aliases: &[&str],
        key: &str,
        kind: Option<&str>,
    ) -> Option<Problem> {
        let mut holders: Vec<(&str, &str, Id)> = Vec::new();
        for alias in aliases {
            let Some(l) = self.graphs.get(*alias) else {
                continue;
            };
            for (t, n) in l.graph.entity_key_types(key) {
                holders.push((alias, t, n.id()));
            }
        }
        let a = match kind {
            Some(k) => holders.iter().find(|h| h.1 == k).copied(),
            None => holders.first().copied(),
        };
        let wanted = kind.or(a.map(|h| h.1))?;
        let b = holders.iter().find(|h| h.1 != wanted)?;
        let a = a?;
        Some(problems::entity_type_mismatch(
            (&alias_name(a.0), a.2, a.1),
            (&alias_name(b.0), b.2, b.1),
            "hold the same entity key",
        ))
    }

    /// The covered graphs of an entity read: `own` first, then `listed`, one copy per graph id (d-21), with the alias
    /// each graph id is read through.
    fn covered(&self, own: &[String], listed: Vec<String>) -> (Vec<String>, BTreeMap<Id, String>) {
        let mut graphs: Vec<String> = Vec::new();
        let mut by_id: BTreeMap<Id, String> = BTreeMap::new();
        for alias in own.iter().cloned().chain(listed) {
            let gid = self.graphs[&alias].meta.id;
            if let alloc::collections::btree_map::Entry::Vacant(e) = by_id.entry(gid) {
                e.insert(alias.clone());
                graphs.push(alias);
            }
        }
        (graphs, by_id)
    }

    /// Every node joined to `start` by a shared key (same type) or a `same_as` edge, transitively, and the identity
    /// edges met on the way. Keys are looked up in, and edges read from, the covered graphs only (`by_id`). An end in
    /// a graph the read does not cover (unloaded, or secured and unnamed) still joins the walk, through the links
    /// covered graphs store to it, but is never shown.
    fn walk_entity(
        &self,
        (ctx, refs): (&Ctx, &[Option<&str>]),
        graphs: &[String],
        by_id: &BTreeMap<Id, String>,
        start: At,
    ) -> Walked {
        let mut w = Walked::default();
        let (reached, met) = (&mut w.reached, &mut w.met);
        let mut queue: VecDeque<At> = VecDeque::new();
        let mut seen_edges: BTreeSet<(String, Id)> = BTreeSet::new();
        reached.insert(start);
        queue.push_back(start);
        while let Some(at) = queue.pop_front() {
            let mut next: Vec<At> = Vec::new();
            let edges: Vec<(String, Id)> = match by_id.get(&at.0) {
                Some(alias) => {
                    let Some(n) = self.graphs[alias].graph.node(at.1) else {
                        continue;
                    };
                    let keys = n
                        .field(ENTITY_KEY)
                        .map(|v| stored_keys(&v))
                        .unwrap_or_default();
                    for key in &keys {
                        // G2 (R2): a key two nodes of this type hold in their own graph names neither for certain,
                        // so neither joins by it anywhere, in either direction. Key joins stay symmetric, so every
                        // member's walk finds the same entity.
                        let usable = !self.graphs[alias].graph.entity_holders(n.kind(), key).1;
                        if !usable {
                            w.ignored
                                .insert((alias.clone(), n.kind().into(), key.clone()));
                        }
                        for other in graphs {
                            let (g, gid) = (&self.graphs[other].graph, self.graphs[other].meta.id);
                            match g.entity_holders(n.kind(), key) {
                                (Some(_), true) => {
                                    w.ignored
                                        .insert((other.clone(), n.kind().into(), key.clone()));
                                }
                                (Some(m), false) if usable => next.push((gid, m.id())),
                                _ => {}
                            }
                            // G5: the same key under another type never matches; say so.
                            for (kind, m) in g.entity_key_types(key) {
                                if kind != n.kind() {
                                    w.other_types.insert((
                                        at,
                                        n.kind().into(),
                                        (gid, m.id()),
                                        kind.into(),
                                        other.clone(),
                                    ));
                                }
                            }
                        }
                    }
                    self.incident_edges(ctx, refs, alias, at.1)
                }
                None => self
                    .incoming
                    .entries(at.0, at.1)
                    .iter()
                    .filter_map(|&(instance, edge)| {
                        self.graphs
                            .iter()
                            .find(|(_, l)| l.instance == instance)
                            .map(|(owner, _)| (owner.clone(), edge))
                    })
                    .collect(),
            };
            for (owner, eid) in edges {
                if !graphs.contains(&owner) || seen_edges.contains(&(owner.clone(), eid)) {
                    continue;
                }
                let l = &self.graphs[&owner];
                let Some(e) = l.graph.edge(eid) else {
                    continue;
                };
                let same_as = e.kind() == SAME_AS;
                let denied = e.kind() == NOT_SAME_AS;
                if !same_as && !denied && e.kind() != POSSIBLY_SAME_AS {
                    continue;
                }
                seen_edges.insert((owner.clone(), eid));
                let ends = (
                    (e.from_graph().unwrap_or(l.meta.id), e.from()),
                    (e.to_graph().unwrap_or(l.meta.id), e.to()),
                );
                if same_as {
                    next.push(if ends.0 == at { ends.1 } else { ends.0 });
                }
                met.push(Met {
                    owner,
                    edge: eid,
                    same_as,
                    denied,
                    from: ends.0,
                    to: ends.1,
                });
            }
            for n in next {
                if reached.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        w
    }

    /// The start's entity once denials split it (ws008 E2c, graph-addressing-expert's join order with security's
    /// coverage). The walk found every node a key or link could join; here they are joined again in one fixed order, so
    /// every machine with the same covered graphs skips the same joins: keys by (type, stored key), each key's holders
    /// by (graph id, node id), then `same_as` links by (storing graph id, edge id). A join that would put a denied pair
    /// in one group is skipped. A denial counts only while its storing graph and both its ends are covered; otherwise
    /// it is ignored and never mentioned. A key two nodes of one type hold in a graph (G2) never anchors a join there.
    fn group_of(&self, start: At, walked: &Walked, by_id: &BTreeMap<Id, String>) -> Grouped {
        let gid_of = |alias: &str| self.graphs[alias].meta.id;
        let mut parent: BTreeMap<At, At> = walked.reached.iter().map(|at| (*at, *at)).collect();
        let mut members: BTreeMap<At, BTreeSet<At>> = walked
            .reached
            .iter()
            .map(|at| (*at, BTreeSet::from([*at])))
            .collect();
        let mut denials: BTreeMap<(At, At), (String, Id)> = BTreeMap::new();
        let mut denied: BTreeMap<At, BTreeSet<At>> = BTreeMap::new();
        for m in walked.met.iter().filter(|m| m.denied) {
            // Both ends covered (security); an end the walk did not reach still blocks a proposal to it.
            if by_id.contains_key(&m.from.0) && by_id.contains_key(&m.to.0) {
                denials.insert((m.from, m.to), (m.owner.clone(), m.edge));
                denials.insert((m.to, m.from), (m.owner.clone(), m.edge));
                denied.entry(m.from).or_default().insert(m.to);
                denied.entry(m.to).or_default().insert(m.from);
            }
        }
        fn find(parent: &mut BTreeMap<At, At>, at: At) -> At {
            let mut root = at;
            while parent[&root] != root {
                root = parent[&root];
            }
            let mut x = at;
            while parent[&x] != root {
                let next = parent[&x];
                parent.insert(x, root);
                x = next;
            }
            root
        }
        // The joins, in their fixed order.
        let mut joins: Vec<(At, At, problems::Joined)> = Vec::new();
        let mut by_key: BTreeMap<(String, String), BTreeSet<At>> = BTreeMap::new();
        for at in &walked.reached {
            let Some(alias) = by_id.get(&at.0) else {
                continue;
            };
            let Some(n) = self.graphs[alias].graph.node(at.1) else {
                continue;
            };
            for key in n
                .field(ENTITY_KEY)
                .map(|v| stored_keys(&v))
                .unwrap_or_default()
            {
                by_key
                    .entry((n.kind().to_string(), key))
                    .or_default()
                    .insert(*at);
            }
        }
        for ((kind, key), holders) in &by_key {
            let single = |at: &At| {
                by_id
                    .get(&at.0)
                    .is_some_and(|a| !self.graphs[a].graph.entity_holders(kind, key).1)
            };
            let Some(anchor) = holders.iter().find(|h| single(h)) else {
                continue;
            };
            let Some(scheme) = key.split_once(':').and_then(|(s, _)| ValidName::new(s)) else {
                continue;
            };
            // R4 (review): a holder of a key held twice in its own graph joins nobody by it, as anchor or target.
            for h in holders.iter().filter(|h| *h != anchor && single(h)) {
                joins.push((*anchor, *h, problems::Joined::Key(scheme.clone())));
            }
        }
        let mut links: Vec<&Met> = walked.met.iter().filter(|m| m.same_as).collect();
        links.sort_by_key(|m| (gid_of(&m.owner), m.edge));
        for m in links {
            joins.push((m.from, m.to, problems::Joined::Link(m.edge)));
        }
        let mut skips: Vec<(problems::Joined, At, At)> = Vec::new();
        for (x, y, joined) in joins {
            if !parent.contains_key(&x) || !parent.contains_key(&y) {
                continue;
            }
            let (rx, ry) = (find(&mut parent, x), find(&mut parent, y));
            if rx == ry {
                continue;
            }
            let clash = members[&rx].iter().find_map(|p| {
                denied
                    .get(p)
                    .and_then(|d| d.iter().find(|q| members[&ry].contains(q)))
                    .map(|q| (*p, *q))
            });
            if let Some((p, q)) = clash {
                skips.push((joined, p, q));
                continue;
            }
            let moved = members.remove(&ry).unwrap_or_default();
            members.entry(rx).or_default().extend(moved);
            parent.insert(ry, rx);
        }
        let reached: Vec<At> = walked.reached.iter().copied().collect();
        let roots: BTreeMap<At, At> = reached
            .iter()
            .map(|at| (*at, find(&mut parent, *at)))
            .collect();
        let root = roots[&start];
        let group = members.get(&root).cloned().unwrap_or_default();
        let mut root_denied: BTreeMap<At, BTreeSet<At>> = BTreeMap::new();
        for (at, partners) in &denied {
            if let Some(r) = roots.get(at) {
                root_denied.entry(*r).or_default().extend(partners);
            }
        }
        let mut skipped: Vec<Problem> = Vec::new();
        for (joined, p, q) in skips {
            if !group.contains(&p) && !group.contains(&q) {
                continue;
            }
            // Only covered ends are ever named (security); a skip that cannot be named is not reported.
            let (Some((owner, edge)), Some(ga), Some(gb)) =
                (denials.get(&(p, q)), by_id.get(&p.0), by_id.get(&q.0))
            else {
                continue;
            };
            let warning = problems::entity_join_denied(
                &joined,
                (&alias_name(owner), *edge),
                (&alias_name(ga), p.1),
                (&alias_name(gb), q.1),
            );
            if !skipped.contains(&warning) {
                skipped.push(warning);
            }
        }
        Grouped {
            group,
            skipped,
            roots,
            denied: root_denied,
        }
    }

    /// F179 (ws008 E2b): `entity link --dry-run`. The edge goes through a dry-run batch of one `edge add` line in the
    /// author's graph, so it meets every check and warning a real link would (secured and unchecked ends, a changed
    /// file); nothing is written and no id is drawn. A line's error is returned as itself.
    fn link_dry_run(
        &mut self,
        ctx: &Ctx,
        author: &str,
        (a, b): (&str, &str),
        mut warnings: Vec<Problem>,
    ) -> Outcome<EntityLinked> {
        self.files_unchanged(author)?;
        let req = super::BatchRequest {
            raised_in: None,
            lines: alloc::vec![super::BatchLine {
                line: 1,
                op: super::BatchOp::EdgeAdd {
                    id: None,
                    from: Some(a.to_string()),
                    from_ref: None,
                    relation: SAME_AS.to_string(),
                    to: Some(b.to_string()),
                    to_ref: None,
                    sets: Vec::new(),
                    evidence: Default::default(),
                },
            }],
            dry_run: true,
            reason: None,
            echo_sentences: true,
            refs: BTreeMap::new(),
            refs_graph: None,
        };
        let done = self
            .batch(&ctx.clone().graph(author), req)
            .map_err(|errors| {
                let first = errors.iter().find(|p| p.code != "batch_failed");
                first
                    .or(errors.first())
                    .cloned()
                    .expect("a failed batch says why")
            })?;
        // The link's own type check already ran (the batch's line copy of it would repeat it); edge_exists is
        // `unchanged`, found above.
        for w in done.notes {
            if !matches!(w.code, "edge_exists" | "entity_type_mismatch") && !warnings.contains(&w) {
                warnings.push(w);
            }
        }
        let (sentence, sentence_flagged) = done
            .value
            .sentences
            .and_then(|s| s.into_iter().next())
            .map(|s| (s.sentence, s.sentence_flagged))
            .unwrap_or_default();
        let value = EntityLinked {
            alias: author.to_string(),
            edge: None,
            sentence,
            sentence_flagged,
            unchanged: false,
            dry_run: true,
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// G3 (ws008 E2b): before a link is stored, each end's entity as it is now, over every open graph plus the graphs
    /// the request names (`named`); a member of one end's entity in the other end's graph, other than either end, is
    /// warned once per side: the link would put two nodes of one graph in one entity. The link is still stored.
    fn link_overlaps(
        &self,
        ctx: &Ctx,
        (a, b): (&str, &str),
        (at_a, at_b): (At, At),
        named: &[String],
    ) -> Result<Vec<Problem>, Problem> {
        // R5: the overlap check only warns, so a scope it cannot build (an unrelated graph's copies with no primary)
        // skips it rather than refusing the link.
        let Ok(scope) = self.unnamed_scope_with_names(named) else {
            return Ok(Vec::new());
        };
        let (graphs, by_id) = self.covered(named, scope.aliases);
        let access = self.external_access(ctx)?;
        let mut out = Vec::new();
        // R4: each walk carries the whole request's graph names, so a named secured graph's links count on both sides.
        let refs = [Some(a), Some(b)];
        for (start, end) in [(at_a, at_b), (at_b, at_a)] {
            let Some(alias) = by_id.get(&end.0) else {
                continue;
            };
            let walked = self.walk_entity((&access, &refs), &graphs, &by_id, start);
            let group = self.group_of(start, &walked, &by_id).group;
            if group.contains(&end) {
                continue;
            }
            if let Some(held) = group.iter().find(|x| {
                **x != start && x.0 == end.0 && self.graphs[alias].graph.node(x.1).is_some()
            }) {
                out.push(problems::entity_link_overlap(
                    &alias_name(alias),
                    held.1,
                    end.1,
                ));
            }
        }
        Ok(out)
    }

    /// The symmetric `kind` edge `owner` stores between two ends already resolved to (graph id, node id), in either
    /// order (R1, review): the owner search of link and deny rebases the ends the author resolved, rather than reading
    /// a bare end's text again in each owner, where it would name another node.
    fn stored_at(&self, owner: &str, a: At, b: At, kind: &str) -> Option<Id> {
        let gid = self.graphs[owner].meta.id;
        let end = |x: At| StoredEnd {
            graph: (x.0 != gid).then_some(x.0),
            node: x.1,
        };
        self.stored_between(owner, end(a), end(b), kind)
    }

    /// The symmetric `kind` edge (same_as, not_same_as or possibly_same_as) between the nodes `a` and `b` name that
    /// graph `owner` stores, in either order, or None; an end `owner` cannot resolve is its error.
    fn stored_pair(
        &self,
        owner: &str,
        a: &str,
        b: &str,
        kind: &str,
    ) -> Result<Option<Id>, Problem> {
        let from = self.write_end(owner, a)?.0;
        let to = self.write_end(owner, b)?.0;
        Ok(self.stored_between(owner, from, to, kind))
    }

    /// [`Self::stored_pair`] for two resolved ends.
    fn stored_between(
        &self,
        owner: &str,
        from: StoredEnd,
        to: StoredEnd,
        kind: &str,
    ) -> Option<Id> {
        let l = &self.graphs[owner];
        let (from, to) = ordered_ends(l.meta.id, true, from, to);
        find_edge(&l.graph, from, kind, to)
            .or_else(|| find_edge(&l.graph, to, kind, from))
            .map(|e| e.id())
    }

    /// Removes link `id` from `owner` and writes its edges, as `edge delete` does (F122, F144): refused up front when
    /// `owner`'s files changed on disk, graph.json joins the write set when an end is in another graph, and the
    /// incoming index follows once the write has committed.
    fn remove_link(&mut self, owner: &str, id: Id) -> Outcome<EntityUnlinked> {
        // F144's up-front check, as `edge delete` makes through select_write (graph-storage-expert).
        self.files_unchanged(owner)?;
        let l = &self.graphs[owner];
        let current = l.graph.edge(id).expect("found above").to_edge();
        check_deletable(&l.graph, &alias_name(owner), &current)?;
        let files = self.ports.files.clone();
        let l = self.graphs.get_mut(owner).expect("a loaded graph");
        l.graph.remove_edge(id);
        let mut touched = Touched::default();
        touched.edges.removed.insert(id);
        let mut which = alloc::vec![GraphFile::Edges];
        which.extend_from_slice(external_files(&[&current]));
        let warnings = match persist(files.as_ref(), owner, l, &which, touched) {
            Ok(w) => w,
            Err(p) => {
                l.graph.insert_edge(current);
                return Err(p);
            }
        };
        self.incoming.remove_edge(l.instance, &current);
        let value = EntityUnlinked {
            removed: Some((owner.to_string(), current)),
        };
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// Whether `text` addresses a graph this machine remembers as loaded but is not loaded now (F72).
    fn remembered_end(&self, text: &str) -> bool {
        split_address(text)
            .0
            .is_some_and(|selector| self.remembered_loaded(selector).is_some())
    }

    /// The graph a link verb writes to: the one `--graph` names, else `a`'s. `UMX_GRAPH` never beats `a`'s address
    /// (graph-addressing-expert F1); `select` falls back to it only for a bare id.
    fn link_author(&self, ctx: &Ctx, a: &str) -> Result<String, Problem> {
        if ctx.graph.is_some() {
            self.select(ctx, &[])
        } else {
            self.select(ctx, &[Some(a)])
        }
    }

    /// The loaded graph an end's text names: its address's alias (or the primary copy of a graph id), else `author`.
    pub(super) fn end_alias(&self, author: &str, text: &str) -> Option<String> {
        match split_address(text).0 {
            None => Some(author.to_string()),
            Some(selector) if self.graphs.contains_key(selector) => Some(selector.to_string()),
            Some(selector) => match selector.parse::<Id>() {
                Ok(id) => self.primary_of(id).map(ToString::to_string),
                // ws006 S23 (review R9): a former alias names the graph `write_end` resolved it to, so its owner,
                // type and denial checks run there.
                Err(_) => self.former_owner(selector, &|_| true).ok().flatten(),
            },
        }
    }
}

/// The read-time guards on one entity (ws008 E2b): G1, two nodes of one graph; G2, keys not used in a graph; G4,
/// mixed types; G5, a member's key held under another type. Every one names covered graphs and nodes only (d-20) and
/// no key value or graph text (ap-33).
fn guards(
    members: &[EntityMember],
    walked: &Walked,
    group: &BTreeSet<At>,
    by_id: &BTreeMap<Id, String>,
    order: impl Fn(&str) -> usize,
) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut by_graph: BTreeMap<(usize, &str), Vec<&EntityMember>> = BTreeMap::new();
    for m in members {
        by_graph
            .entry((order(&m.alias), &m.alias))
            .or_default()
            .push(m);
    }
    for ((_, alias), mut held) in by_graph {
        if held.len() < 2 {
            continue;
        }
        held.sort_by_key(|m| m.id);
        let (node, other) = (held[0], held[1]);
        let joined = [other, node].into_iter().find_map(|m| {
            m.matched.iter().find_map(|x| match x {
                EntityMatch::Key(k) => k
                    .split_once(':')
                    .and_then(|(scheme, _)| ValidName::new(scheme))
                    .map(problems::Joined::Key),
                EntityMatch::Link { edge, .. } => Some(problems::Joined::Link(*edge)),
                EntityMatch::Asked => None,
            })
        });
        if let Some(joined) = joined {
            out.push(problems::entity_graph_twice(
                &alias_name(alias),
                held.len(),
                node.id,
                other.id,
                &joined,
            ));
        }
    }
    for (alias, kind, key) in &walked.ignored {
        if let Some(scheme) = key.split_once(':').and_then(|(s, _)| ValidName::new(s)) {
            let p = problems::entity_key_ignored(&alias_name(alias), kind, &scheme);
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    let mut kinds: Vec<&EntityMember> = Vec::new();
    for m in members {
        if !kinds.iter().any(|k| k.kind == m.kind) {
            kinds.push(m);
        }
    }
    if kinds.len() > 1 {
        out.push(problems::entity_types_mixed(
            kinds.len(),
            kinds[0].id,
            &kinds[0].kind,
            kinds[1].id,
            &kinds[1].kind,
        ));
    }
    let mut pairs: BTreeSet<(String, String, String, String)> = BTreeSet::new();
    for (member, kind, holder, holder_kind, holder_alias) in &walked.other_types {
        if !group.contains(member) {
            continue;
        }
        let Some(member_alias) = by_id.get(&member.0) else {
            continue;
        };
        let pair = (
            kind.clone(),
            member_alias.clone(),
            holder_kind.clone(),
            holder_alias.clone(),
        );
        if pairs.insert(pair) {
            out.push(problems::entity_type_mismatch(
                (&alias_name(member_alias), member.1, kind),
                (&alias_name(holder_alias), holder.1, holder_kind),
                "hold the same entity key",
            ));
        }
    }
    out
}

/// `umx <path> <a> <b>` with each end's address where its graph is known, else a placeholder.
fn pair_command(path: &'static str, aliases: &[Option<String>; 2], ids: (Id, Id)) -> Command {
    let end = |c: Command, alias: &Option<String>, id: Id, placeholder: &'static str| match alias {
        Some(a) => c.address(&alias_name(a), id),
        None => c.word(placeholder),
    };
    let c = end(Command::new(path), &aliases[0], ids.0, "<a>");
    end(c, &aliases[1], ids.1, "<b>")
}

/// G5 for `umx check` (ws008 E2b): over the checked graphs as just read from their files (R6), each entity key
/// held under two types gives one `entity_type_mismatch` per pair of (type, graph), its first holders as `a` and
/// `b`. One pass over the nodes.
pub(crate) fn check_key_types(checked: &[(&str, &umx_graph::Graph)]) -> Vec<Problem> {
    let mut by_key: BTreeMap<String, Vec<(&str, String, Id)>> = BTreeMap::new();
    for (alias, graph) in checked {
        for n in graph.nodes() {
            if let Some(v) = n.field(ENTITY_KEY) {
                for key in stored_keys(&v) {
                    by_key
                        .entry(key)
                        .or_default()
                        .push((alias, n.kind().to_string(), n.id()));
                }
            }
        }
    }
    let mut seen: BTreeSet<(String, &str, String, &str)> = BTreeSet::new();
    let mut out = Vec::new();
    for holders in by_key.values() {
        let a = &holders[0];
        for b in holders.iter().filter(|b| b.1 != a.1) {
            if seen.insert((a.1.clone(), a.0, b.1.clone(), b.0)) {
                out.push(problems::entity_type_mismatch(
                    (&alias_name(a.0), a.2, &a.1),
                    (&alias_name(b.0), b.2, &b.1),
                    "hold the same entity key",
                ));
            }
        }
    }
    out
}

fn at(owner: Id, end: StoredEnd) -> At {
    (end.graph.unwrap_or(owner), end.node)
}
