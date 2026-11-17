//! The embedding service (blueprint 001, d-12): holds only the Embedder port. It never stores the graph store: its
//! methods take `&GraphStore` to read and `&mut GraphStore` to write, through the store's pub fns, so the
//! daemon's writer stays the only writer (d-7).
//!
//! Work on a node runs in three steps so a slow provider never holds the writer (req-35, architect F1):
//! [`EmbeddingService::plan`] reads (shared access), [`embed`] calls the provider with no access to the store at
//! all, and [`EmbeddingService::apply`] writes the vectors back through the store (the writer). A vector is
//! current when its line's text hash equals the hash of the node's text now, so an unchanged node plans nothing,
//! and `apply` drops any result whose text changed while it was being embedded.
//!
//! [`choose`] and [`probe`] are free functions over an embedder the caller holds: the daemon clones the embedder
//! out of its lock and calls them on the request's own thread, so a slow provider never holds the writer up.

mod pacing;

use crate::graph_store::GraphStore;
use crate::problems;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use umx_core::config::{EmbedderRefused, Resolved, embedder_for};
use umx_core::ports::{Credential, EmbedError, Embedder, GraphDir, Locality, ModelKey, TextKind};
use umx_core::secret_allows::SecretAllows;
use umx_core::secrets;
use umx_core::vectors::{self, ShardFile, ShardLine, TextHash, VectorLine, quantise};
use umx_core::{EmbedderRule, Id, Policy, Problem, ValidName};

pub use pacing::{
    EmbedQueue, MAX_BATCH_TEXTS, Pacer, QUIET_RECOVERY_MS, Step, Ticket, batch_tokens, batches,
    estimate_tokens, settle, step,
};

/// A node whose vectors may need work.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EmbedJob {
    pub alias: String,
    pub node: Id,
}

/// One text of a node to embed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub field: String,
    pub hash: TextHash,
    pub text: String,
}

/// What a node needs: texts to embed (maybe none, when only stale or repeated lines must go).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub job: EmbedJob,
    pub texts: Vec<Wanted>,
}

/// What a reembed of one vector space costs ([`EmbeddingService::needs`]): the nodes with texts to embed, and of
/// those (PRD 002 search-04) the whole-node texts that replace a vector made from other text, and the field vectors
/// the rewrite drops because the node no longer embeds their field (a field now data among them), which cost no call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    pub planned: Vec<Planned>,
    pub whole_node_texts_changed: usize,
    pub field_vectors_dropped: usize,
}

/// Fresh vectors for a node, by field, with the hash of the text each came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Vectors {
    pub job: EmbedJob,
    /// The vector space the vectors were made in; `None` for a tidy with no vectors. `apply` drops vectors whose
    /// space is no longer the graph's (its instance changed while they were being made), so models never mix.
    pub key: Option<ModelKey>,
    pub vectors: Vec<(String, TextHash, Vec<f32>)>,
}

/// What `apply` did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub shards_written: usize,
    /// similar_to files (shards and `_meta.tsv`) written after the vectors changed.
    pub similar_files_written: usize,
    pub vectors_written: usize,
}

/// What a scan of a graph's shards found (graph load, `umx embeddings repair`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Nodes with a missing or stale vector: queue them.
    pub to_embed: Vec<Id>,
    /// Nodes whose every vector is current but whose shard has repeated or leftover lines for them.
    pub to_tidy: Vec<Id>,
    /// Lines naming a node that is not in the graph: (shard, 1-based line, node).
    pub orphans: Vec<(ShardFile, usize, Id)>,
    /// Lines umx cannot read: (shard, 1-based line).
    pub kept: Vec<(ShardFile, usize)>,
    /// Vector lines whose width is not their space's dims: (shard, 1-based line, width found). Never current.
    pub wrong_width: Vec<(ShardFile, usize, usize)>,
    /// Nodes without a current vector in the graph's own space (its recorded key, else the embedder's), whether or
    /// not this embedder may make them (req-75: pending, never made with another model).
    pub pending: Vec<Id>,
    /// The pending nodes whose committed whole-node vector was made from text that included a field now data
    /// (req-97, `text_form_outdated`): never queued from a scan; reembed or a role change remakes them.
    pub outdated: Vec<Id>,
}

/// The one text `umx embeddings probe` embeds: umx's own words, never graph text.
pub const PROBE_TEXT: &str =
    "umx embeddings probe: one fixed sentence that umx owns, never graph text.";

/// What a probe learned. The vector stays in the daemon, which reports only its width and length.
#[derive(Clone, Debug, PartialEq)]
pub struct Probed {
    pub key: ModelKey,
    pub vector: Vec<f32>,
    pub locality: Locality,
    pub adapter: &'static str,
    pub credential: Option<Credential>,
}

/// Checks the embedder can run (`ready()`, which sends no text), then embeds [`PROBE_TEXT`] once and checks the
/// one vector has the key's width. `requested` is the `--embedder` given, for the next command.
pub fn probe(
    instance: &str,
    requested: Option<&str>,
    embedder: &dyn Embedder,
) -> Result<Probed, Problem> {
    let fail = |e: &EmbedError| problems::embed_error(instance, requested, e);
    embedder.ready().map_err(|e| fail(&e))?;
    let key = embedder.key();
    let out = embedder
        .embed(&[PROBE_TEXT.to_string()], TextKind::Document)
        .map_err(|e| fail(&e))?;
    let vector = out
        .vectors
        .into_iter()
        .next()
        .filter(|v| v.len() == key.dims())
        .ok_or_else(|| {
            fail(&EmbedError::BadResponse(
                "no vector of the configured width".to_string(),
            ))
        })?;
    Ok(Probed {
        key,
        vector,
        locality: embedder.locality(),
        adapter: embedder.adapter_id(),
        credential: embedder.credential(),
    })
}

/// A built instance by name: `None` if the config defines no such instance, else the adapter or why it was not
/// built.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<Result<Arc<dyn Embedder>, String>>;

/// One instance as the selection rule sees it: its name, the space it makes and where its text goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub key: ModelKey,
    pub locality: Locality,
}

/// The one instance-selection rule (s17, R106-3, D53, D62) over `items`: among the instances `usable(name)`
/// allows and the graph's embedder setting admits (every instance is in the machine config, so trusted), the one
/// that makes the graph's `recorded` space, the default first, then by order. With no recorded space: the default
/// if admitted, else (a setting that is not `any`) the first admitted. The filters run before the tie-break.
pub fn pick<T>(
    items: &[T],
    view: &dyn Fn(&T) -> Candidate<'_>,
    default: Option<usize>,
    rule: EmbedderRule,
    recorded: Option<&ModelKey>,
    usable: &dyn Fn(&str) -> bool,
) -> Option<usize> {
    let allowed = |c: &Candidate<'_>| usable(c.name) && rule.admits(c.locality, true);
    let fits = |i: usize| {
        let c = view(&items[i]);
        allowed(&c) && recorded.is_none_or(|k| c.key == *k)
    };
    let first = || (0..items.len()).find(|&i| fits(i));
    match recorded {
        None if rule != EmbedderRule::Any => default.filter(|&i| fits(i)).or_else(first),
        None => default.filter(|&i| fits(i)),
        Some(_) => default.filter(|&i| fits(i)).or_else(first),
    }
}

/// The instance to use (req-72, req-73): `requested` (`--embedder`) else `embedder.active` in `resolved`. For a
/// graph, `resolved` must come from the graph's folder (its repo layer may only narrow) and the graph's embedder
/// setting beats every layer (req-46, req-91): [`EmbedderRule::admits`] on the built adapter's `locality()`, never
/// its kind or name. Every built instance is defined in the machine config, so it is trusted (D62).
pub fn choose(
    resolved: &Resolved,
    requested: Option<&str>,
    graph: Option<(&ValidName, Policy)>,
    lookup: Lookup<'_>,
) -> Result<(String, Arc<dyn Embedder>), Problem> {
    let rule = graph.map_or(EmbedderRule::Any, |(_, p)| p.embedder);
    let refused = |instance: &str| {
        let graph = graph.map_or("", |(g, _)| g.as_str());
        let reason = alloc::format!(
            "graph {graph} is secured so its text stays on this machine and {instance} is hosted"
        );
        let hint = "choose a local instance with umx config set embedder.active";
        problems::embedder_unavailable(instance, requested, &reason, hint)
    };
    let chosen = match embedder_for(resolved, rule, requested) {
        Ok(Some(chosen)) => chosen,
        Ok(None) => return Err(problems::embedder_not_configured()),
        Err(EmbedderRefused::Unknown(p)) => return Err(p),
        Err(EmbedderRefused::NotAdmitted { instance, .. }) => return Err(refused(&instance)),
    };
    let built = match lookup(&chosen.name) {
        Some(built) => built,
        // Defined in this folder's config but not built at start: the daemon was started before it was added.
        None => {
            return Err(problems::embedder_invalid(
                &chosen.name,
                "it was not defined when the daemon started (restart the daemon)",
            ));
        }
    };
    let embedder = built.map_err(|why| problems::embedder_invalid(&chosen.name, &why))?;
    // Built from the machine config, so trusted (D62).
    if !rule.admits(embedder.locality(), true) {
        return Err(refused(&chosen.name));
    }
    Ok((chosen.name, embedder))
}

/// Whether two handles are the same built embedder (by address; vtables may differ across codegen units).
fn same_embedder(a: &Arc<dyn Embedder>, b: &Arc<dyn Embedder>) -> bool {
    core::ptr::eq(Arc::as_ptr(a).cast::<()>(), Arc::as_ptr(b).cast::<()>())
}

/// One built embedder instance, by its config name.
#[derive(Clone)]
pub struct Instance {
    pub name: String,
    pub embedder: Arc<dyn Embedder>,
}

/// Every built embedder instance (ws004 s17, embeddings-expert ruling). Each graph embeds with the instance
/// whose key is the graph's recorded key and whose locality its effective policy allows; `embedder.active` (the
/// default) serves graphs with no recorded key and is reembed's target. Instances are fixed when the daemon starts.
pub struct EmbeddingService {
    /// Sorted by name.
    instances: Vec<Instance>,
    /// Index into `instances` of the default (`embedder.active`).
    default: Option<usize>,
    /// Each instance's background pace in tokens a minute (`tokens_per_minute`, ws004 s25); one not named is
    /// unpaced.
    pace: BTreeMap<String, u64>,
    /// How many new nodes one related check (req-123) embeds with each instance, by name: about 2 s of its work
    /// (embeddings-expert, ws004 s28). One not named gets [`crate::related::RELATED_CAP`].
    related_caps: BTreeMap<String, usize>,
}

impl EmbeddingService {
    /// One embedder (or none) that is also the default: real mode wires none until one is configured, never the
    /// fake.
    pub fn new(embedder: Option<Arc<dyn Embedder>>) -> EmbeddingService {
        let instances: Vec<Instance> = embedder
            .into_iter()
            .map(|embedder| Instance {
                name: String::new(),
                embedder,
            })
            .collect();
        let default = (!instances.is_empty()).then_some(0);
        EmbeddingService {
            instances,
            default,
            pace: BTreeMap::new(),
            related_caps: BTreeMap::new(),
        }
    }

    /// Several named instances; `default` names the one `embedder.active` chose, if it is among them.
    pub fn with_instances(mut instances: Vec<Instance>, default: Option<&str>) -> EmbeddingService {
        instances.sort_by(|a, b| a.name.cmp(&b.name));
        instances.dedup_by(|a, b| a.name == b.name);
        let default = default.and_then(|d| instances.iter().position(|i| i.name == d));
        EmbeddingService {
            instances,
            default,
            pace: BTreeMap::new(),
            related_caps: BTreeMap::new(),
        }
    }

    /// Whether `name` is trusted (D62): it is one of these instances, all built from the machine config, which only
    /// the machine layer can define ([`umx_core::config::trusted`]).
    pub fn trusted(&self, name: &str) -> bool {
        self.instances.iter().any(|i| i.name == name)
    }

    /// Each instance's background pace, by name (ws004 s25).
    pub fn with_pace(mut self, pace: BTreeMap<String, u64>) -> EmbeddingService {
        self.pace = pace;
        self
    }

    /// Each instance's related-check cap, by name (req-123).
    pub fn with_related_caps(mut self, caps: BTreeMap<String, usize>) -> EmbeddingService {
        self.related_caps = caps;
        self
    }

    /// How many new nodes one related check embeds with instance `name`.
    pub fn related_cap(&self, name: &str) -> usize {
        self.related_caps
            .get(name)
            .copied()
            .unwrap_or(crate::related::RELATED_CAP)
    }

    /// How fast background embedding may send to instance `name`, in tokens a minute; None: unpaced.
    pub fn pace(&self, name: &str) -> Option<u64> {
        self.pace.get(name).copied()
    }

    /// The one routing rule (D53, D62) for an instance.
    fn admits(&self, rule: EmbedderRule, i: &Instance) -> bool {
        rule.admits(i.embedder.locality(), self.trusted(&i.name))
    }

    fn default_instance(&self) -> Option<&Instance> {
        self.default.map(|i| &self.instances[i])
    }

    /// The default's name (`embedder.active`), or `None` when there is no default.
    pub fn default_name(&self) -> Option<String> {
        self.default_instance().map(|i| i.name.clone())
    }

    /// Every instance, by name.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// The default embedder's adapter id (E-1), or `None` when there is none.
    pub fn adapter_id(&self) -> Option<&'static str> {
        self.default_instance().map(|i| i.embedder.adapter_id())
    }

    /// The default's vector space, or `None` when there is no default.
    pub fn key(&self) -> Option<ModelKey> {
        self.default_instance().map(|i| i.embedder.key())
    }

    /// The instance `alias` embeds with under `policy` (its effective policy, read by the caller): the one whose key
    /// is the graph's recorded key and that the graph's embedder setting admits ([`EmbedderRule::admits`], on
    /// `locality()` and trust (D62), never kind or name). Several match: the default first, then by name. A graph
    /// with no recorded key uses the default if admitted; one whose setting is not `any` falls back to the first
    /// admitted instance by name (embeddings-expert, s20). None: nothing is embedded for the graph.
    pub fn instance_for(
        &self,
        store: &GraphStore,
        alias: &str,
        policy: Policy,
    ) -> Option<&Instance> {
        self.instance_for_where(store, alias, policy, &|_| true)
    }

    /// [`Self::instance_for`] among only the instances `usable(name)` allows (the daemon's folder rule): the filters
    /// run first and the tie-break (default, then name) among what is left, so a forbidden default never hides an
    /// allowed match (R106-3).
    pub fn instance_for_where(
        &self,
        store: &GraphStore,
        alias: &str,
        policy: Policy,
        usable: &dyn Fn(&str) -> bool,
    ) -> Option<&Instance> {
        let recorded = store
            .loaded(alias)?
            .meta
            .embedding
            .as_ref()
            .and_then(|e| e.key.as_ref());
        pick(
            &self.instances,
            &|i| Candidate {
                name: &i.name,
                key: i.embedder.key(),
                locality: i.embedder.locality(),
            },
            self.default,
            policy.embedder,
            recorded,
            usable,
        )
        .map(|i| &self.instances[i])
    }

    /// How many loaded graphs that are not open would send their text somewhere new after a restart because
    /// instance `changed` moved (D62, security s20b, R202-1): each graph's route now (these built instances) against
    /// its route with `after` and `after_default` (the config as written), by the one rule ([`pick`]) and the folder
    /// rule `usable`, keyless graphs included. A graph counts when its route after is hosted and is `changed`, or
    /// was `changed` before and is another instance now. Counted only, never named.
    pub fn rerouted(
        &self,
        store: &GraphStore,
        after: &[Candidate<'_>],
        after_default: Option<&str>,
        changed: &str,
        usable: &dyn Fn(&GraphDir, &str) -> bool,
    ) -> usize {
        let after_default = after_default.and_then(|d| after.iter().position(|c| c.name == d));
        store
            .aliases()
            .filter(|alias| {
                let (Some(policy), Some(loaded)) =
                    (store.effective_policy(alias), store.loaded(alias))
                else {
                    return false;
                };
                if policy == Policy::OPEN {
                    return false;
                }
                let folder = |n: &str| usable(&loaded.dir, n);
                let before = self
                    .instance_for_where(store, alias, policy, &folder)
                    .map(|i| i.name.as_str());
                let recorded = loaded.meta.embedding.as_ref().and_then(|e| e.key.as_ref());
                let now = pick(
                    after,
                    &|c| c.clone(),
                    after_default,
                    policy.embedder,
                    recorded,
                    &folder,
                )
                .map(|i| &after[i]);
                now.is_some_and(|c| {
                    c.locality != Locality::Local
                        && (c.name == changed || (before == Some(changed) && c.name != changed))
                })
            })
            .count()
    }

    /// Why a graph whose space an instance makes gets none: its embedder setting is `local` and every such instance
    /// is hosted. `trusted` and `any` admit every instance (all are defined in the machine config, D62).
    pub fn not_admitted(&self, graph: &ValidName) -> Problem {
        problems::embedder_not_local_for_secured(graph)
    }

    /// Whether `rule` admits any instance this machine built (all are in the machine config, so trusted: D62). False
    /// only for a `local` graph where every instance is hosted, or when there is no instance.
    pub fn admits_any(&self, rule: EmbedderRule) -> bool {
        self.instances.iter().any(|i| self.admits(rule, i))
    }

    /// Whether `alias`'s embedder setting admits instance `name` now, its effective policy read fresh. Fail closed: a graph not loaded or an unknown instance is not admitted.
    pub fn admits_now(&self, store: &GraphStore, alias: &str, name: &str) -> bool {
        let Some(policy) = store.effective_policy(alias) else {
            return false;
        };
        self.instances
            .iter()
            .find(|i| i.name == name)
            .is_some_and(|i| self.admits(policy.embedder, i))
    }

    /// [`Self::instance_for`] reading the graph's effective policy now (fail closed: a graph not loaded gets none).
    pub fn embedder_for(&self, store: &GraphStore, alias: &str) -> Option<Instance> {
        self.embedder_for_where(store, alias, &|_| true)
    }

    /// [`Self::embedder_for`] among the instances `usable(name)` allows.
    pub fn embedder_for_where(
        &self,
        store: &GraphStore,
        alias: &str,
        usable: &dyn Fn(&str) -> bool,
    ) -> Option<Instance> {
        self.instance_for_where(store, alias, store.effective_policy(alias)?, usable)
            .cloned()
    }

    /// The vector space this service embeds `alias` into: the embedder's key, unless the graph records a different
    /// one (D32: then nothing is embedded for it, so models never mix, and the load warns `embedder_mismatch`).
    ///
    /// A secured graph's text never goes to an embedder that is not local (req-46, architect B1): then this is
    /// `None`, so nothing for it is queued, planned, embedded or written. Decided by `locality()`, never by the
    /// adapter's kind or name.
    pub fn key_for(&self, store: &GraphStore, alias: &str) -> Option<ModelKey> {
        // Read fresh before any text goes out; a graph that is not loaded fails closed.
        self.key_with(store, alias, store.effective_policy(alias)?)
    }

    /// Each graph's effective policy for one operation, read once per graph (architect): fresh for each
    /// operation, never kept across calls. None for a graph that is not loaded.
    fn policies<'a>(
        store: &GraphStore,
        aliases: impl IntoIterator<Item = &'a str>,
    ) -> BTreeMap<&'a str, Option<Policy>> {
        let mut out = BTreeMap::new();
        for alias in aliases {
            out.entry(alias)
                .or_insert_with(|| store.effective_policy(alias));
        }
        out
    }

    /// [`Self::key_for`] with the graph's effective policy already read by the caller, so a batch reads it once
    /// per graph (architect): fresh for each batch, never kept across calls.
    fn key_with(&self, store: &GraphStore, alias: &str, policy: Policy) -> Option<ModelKey> {
        self.instance_for(store, alias, policy)
            .map(|i| i.embedder.key())
    }

    /// Warnings for a loaded graph against the configured embedder (D32): `embedder_mismatch` when the graph
    /// records a different model key, and `embedder_not_local_for_secured` when the graph's effective policy is secured
    /// and the embedder's texts leave the machine (its locality, never its kind or name). No embedder means no
    /// warning; the graph never adopts the embedder's key.
    pub fn graph_warnings(&self, store: &GraphStore, alias: &str) -> Vec<Problem> {
        let mut out = Vec::new();
        let (Some(default), Some(loaded), Some(graph)) = (
            self.default_instance().or(self.instances.first()),
            store.loaded(alias),
            ValidName::alias(alias),
        ) else {
            return out;
        };
        let policy = store.effective_policy(alias).unwrap_or(Policy::SECURED);
        if self.instance_for(store, alias, policy).is_some() {
            return out;
        }
        let recorded = loaded.meta.embedding.as_ref().and_then(|e| e.key.as_ref());
        // An instance makes the graph's space but the policy refuses every one of them: secured, none local.
        let same_space: Vec<&Instance> = match recorded {
            Some(key) => self
                .instances
                .iter()
                .filter(|i| i.embedder.key() == *key)
                .collect(),
            None => alloc::vec![default],
        };
        if !same_space.is_empty() {
            out.push(self.not_admitted(&graph));
        } else if let Some(graph_key) = recorded {
            out.push(problems::embedder_mismatch(
                &graph,
                graph_key,
                &default.embedder.key(),
            ));
        }
        out
    }

    /// The credential the wired embedder last signed in with, never a token or key; `None` before one worked.
    pub fn credential(&self) -> Option<Credential> {
        self.default_instance()
            .and_then(|i| i.embedder.credential())
    }

    /// Whether the embedder could embed now, asked without sending any text (`umx doctor`); `None` when there is no
    /// embedder.
    pub fn ready(&self) -> Option<Result<(), EmbedError>> {
        self.default_instance().map(|i| i.embedder.ready())
    }

    /// A query's vector for searching `alias` (s08 builds search on it), under the same guard as documents: `Ok(None)`
    /// when there is no embedder, the graph is not loaded, it records another model, or it is secured and the
    /// embedder is not local, or the embedder is not local and the query looks like it holds a secret (req-49), and
    /// then the embedder is never called. Errors are the provider's.
    pub fn embed_query(
        &self,
        store: &GraphStore,
        alias: &str,
        text: &str,
    ) -> Result<Option<Vec<f32>>, EmbedError> {
        let Some(Instance { embedder, .. }) = self.embedder_for(store, alias) else {
            return Ok(None);
        };
        // A query that looks like it holds a secret never goes to a hosted embedder (req-49, security s08).
        if embedder.locality() != Locality::Local && secrets::scan(text).is_some() {
            return Ok(None);
        }
        let texts = [alloc::string::ToString::to_string(text)];
        let mut out = embedder.embed(&texts, TextKind::Query)?;
        Ok(out.vectors.pop())
    }

    /// `batch` without what may no longer go to the embedder, read again right before it is sent (security):
    /// for an embedder that is not local, every node of a graph whose effective policy is no longer open (it was
    /// secured, or unloaded, since it was planned) is dropped, and so is every text the secret scan holds (req-49,
    /// [`hold_secrets`]). Every instance drops provenance and texts that changed or became data since planning.
    pub fn still_sendable(
        &self,
        store: &GraphStore,
        embedder: &Arc<dyn Embedder>,
        batch: Vec<Planned>,
    ) -> Vec<Planned> {
        self.still_sendable_where(store, embedder, batch, &|_, _| true)
    }

    /// [`Self::still_sendable`] with the daemon's folder rule, `usable(graph folder, instance name)`, applied before
    /// the choice, as when the batch was grouped.
    pub fn still_sendable_where(
        &self,
        store: &GraphStore,
        embedder: &Arc<dyn Embedder>,
        mut batch: Vec<Planned>,
        usable: &dyn Fn(&GraphDir, &str) -> bool,
    ) -> Vec<Planned> {
        // Each graph's instance, chosen again now: a graph secured (or reembedded, or unloaded) since planning that
        // no longer maps to this embedder is dropped (security, per instance).
        let policies = Self::policies(store, batch.iter().map(|p| p.job.alias.as_str()));
        let keep: BTreeMap<String, bool> = policies
            .iter()
            .map(|(alias, policy)| {
                let same = policy
                    .zip(store.loaded(alias))
                    .and_then(|(p, l)| {
                        self.instance_for_where(store, alias, p, &|n| usable(&l.dir, n))
                    })
                    .is_some_and(|i| same_embedder(&i.embedder, embedder));
                ((*alias).to_string(), same)
            })
            .collect();
        batch.retain(|p| keep.get(&p.job.alias).copied().unwrap_or(false));
        // Only what each node would embed now may go (R189-1): a text planned before a role change or an edit, which
        // the node's current form no longer has (a field now data, the whole-node text that held it), is dropped
        // here, before the embedder sees it. A node left with nothing to send leaves the batch; its change queued it.
        batch.retain_mut(|p| {
            let Some(loaded) = store.loaded(&p.job.alias) else {
                return false;
            };
            loaded
                .graph
                .node_ix(p.job.node)
                .is_some_and(|ix| !loaded.graph.is_provenance(ix))
                && still_current(store, p)
        });
        // The secret hold (req-49, SEC-10): judged on the texts about to go, with this machine's allows read once.
        if embedder.locality() != Locality::Local && !batch.is_empty() {
            let allows = store.secret_allows();
            for p in &mut batch {
                hold_secrets(store, &allows, p);
            }
        }
        batch
    }

    /// The default embedder (`embedder.active`): reembed's target.
    pub fn embedder(&self) -> Option<Arc<dyn Embedder>> {
        self.default_instance().map(|i| i.embedder.clone())
    }

    /// What `job`'s node needs, or None when its vectors are current and its shard is tidy (also when the graph
    /// or node is gone, or there is no embedder).
    pub fn plan(&self, store: &GraphStore, job: &EmbedJob) -> Result<Option<Planned>, Problem> {
        let Some(policy) = store.effective_policy(&job.alias) else {
            return Ok(None);
        };
        self.plan_in(store, job, policy, self.key_with(store, &job.alias, policy))
    }

    /// [`Self::plan`] for a batch, reading each graph's policy once for the whole batch.
    pub fn plan_all(
        &self,
        store: &GraphStore,
        jobs: &[EmbedJob],
    ) -> Vec<Result<Option<Planned>, Problem>> {
        let policies = Self::policies(store, jobs.iter().map(|j| j.alias.as_str()));
        jobs.iter()
            .map(|j| match policies[j.alias.as_str()] {
                Some(policy) => {
                    self.plan_in(store, j, policy, self.key_with(store, &j.alias, policy))
                }
                None => Ok(None),
            })
            .collect()
    }

    /// Which of `ids` lack a current vector in the graph's own space (see [`Self::space`]); empty when it has
    /// none. Reads each node's shard; for the few nodes a write touched.
    pub fn pending_among(
        &self,
        store: &GraphStore,
        alias: &str,
        ids: &[Id],
    ) -> Result<Vec<(Id, bool)>, Problem> {
        let (Some(space), Some(policy)) =
            (self.block_key(store, alias), store.effective_policy(alias))
        else {
            return Ok(Vec::new());
        };
        ids.iter()
            .map(|id| {
                let job = EmbedJob {
                    alias: alias.into(),
                    node: *id,
                };
                let planned = self.plan_in(store, &job, policy, Some(space.clone()))?;
                Ok((*id, planned.is_some_and(|p| !p.texts.is_empty())))
            })
            .collect()
    }

    /// Every node of `alias` that needs a vector in `key`'s space, with the texts to embed: what a re-embed
    /// costs. Reads each shard of that space once.
    pub fn needs(&self, store: &GraphStore, alias: &str, key: &ModelKey) -> Result<Needs, Problem> {
        let Some(policy) = store.effective_policy(alias) else {
            return Ok(Needs::default());
        };
        let mut found: BTreeMap<Id, Vec<VectorLine>> = BTreeMap::new();
        for shard in store.list_shards(alias, policy, key)? {
            for line in store.read_shard(alias, policy, &shard)? {
                if let ShardLine::Vector(v) = line
                    && v.values.len() == key.dims()
                {
                    found.entry(v.node).or_default().push(v);
                }
            }
        }
        let whole = vectors::whole_node_field();
        let mut needs = Needs::default();
        for id in store.node_ids(alias) {
            let Some(texts) = store.node_texts(alias, id) else {
                continue;
            };
            let mine: Vec<&VectorLine> = found
                .get(&id)
                .map(|v| v.iter().collect())
                .unwrap_or_default();
            let job = EmbedJob {
                alias: alias.into(),
                node: id,
            };
            let Some(p) = plan_node(&job, &texts, &mine).filter(|p| !p.texts.is_empty()) else {
                continue;
            };
            if p.texts.iter().any(|w| w.field == whole) && mine.iter().any(|l| is_whole(&l.field)) {
                needs.whole_node_texts_changed += 1;
            }
            needs.field_vectors_dropped += mine
                .iter()
                .filter(|l| !is_whole(&l.field) && !texts.iter().any(|(f, _)| *f == l.field))
                .count();
            needs.planned.push(p);
        }
        Ok(needs)
    }

    fn plan_in(
        &self,
        store: &GraphStore,
        job: &EmbedJob,
        policy: Policy,
        key: Option<ModelKey>,
    ) -> Result<Option<Planned>, Problem> {
        let Some(key) = key else {
            return Ok(None);
        };
        let Some(texts) = store.node_texts(&job.alias, job.node) else {
            return Ok(None);
        };
        let dims = key.dims();
        let lines = store.read_shard(&job.alias, policy, &ShardFile::of(key, job.node))?;
        // A line of the wrong width is never current, whatever its hash (reviewer R60-3).
        let mine: Vec<&VectorLine> = lines
            .iter()
            .filter_map(ShardLine::vector)
            .filter(|v| v.node == job.node && v.values.len() == dims)
            .collect();
        Ok(plan_node(job, &texts, &mine))
    }

    /// Writes `results` into their shards: each node's current vectors replace its old lines, lines for fields it
    /// no longer has go, and repeated lines for one field keep the first in sort order (req-36). A result whose
    /// text changed since it was planned is dropped (a later job embeds the new text). Each touched shard is
    /// read and written once; a shard whose bytes would not change is not written. Other nodes' lines, orphans and
    /// lines umx cannot read are kept as they are.
    pub fn apply(
        &self,
        store: &mut GraphStore,
        results: &[Vectors],
        defer_similar: bool,
    ) -> Result<(Applied, Vec<Problem>), Problem> {
        let mut by_shard: BTreeMap<(String, ShardFile), Vec<&Vectors>> = BTreeMap::new();
        let policies = Self::policies(store, results.iter().map(|r| r.job.alias.as_str()));
        for r in results {
            if !store.embeddable_node(&r.job.alias, r.job.node) {
                continue;
            }
            // A graph that records another model key gets no vectors from this embedder (D32).
            let Some(key) =
                policies[r.job.alias.as_str()].and_then(|p| self.key_with(store, &r.job.alias, p))
            else {
                continue;
            };
            if r.key.as_ref().is_some_and(|made| *made != key) {
                continue;
            }
            by_shard
                .entry((r.job.alias.clone(), ShardFile::of(key, r.job.node)))
                .or_default()
                .push(r);
        }
        let (mut applied, mut warnings) = (Applied::default(), Vec::new());
        let mut whole_changed: BTreeMap<String, Vec<Id>> = BTreeMap::new();
        for ((alias, shard), nodes) in by_shard {
            let Some(policy) = policies[alias.as_str()] else {
                continue;
            };
            let ids: Vec<Id> = nodes.iter().map(|r| r.job.node).collect();
            let before = store.read_shard(&alias, policy, &shard)?;
            let mut lines = before.clone();
            for r in nodes {
                let Some(texts) = store.node_texts(&alias, r.job.node) else {
                    continue;
                };
                let current: BTreeMap<&str, TextHash> = texts
                    .iter()
                    .map(|(f, t)| (f.as_str(), TextHash::of(t)))
                    .collect();
                let fresh: Vec<VectorLine> = r
                    .vectors
                    .iter()
                    .filter(|(f, h, _)| current.get(f.as_str()) == Some(h))
                    .map(|(f, h, v)| VectorLine {
                        node: r.job.node,
                        field: f.clone(),
                        hash: *h,
                        values: quantise(v),
                    })
                    .collect();
                applied.vectors_written += fresh.len();
                lines = tidy_node(lines, r.job.node, &current, fresh, shard.key().dims());
            }
            let (written, w) = store.write_shard(&alias, policy, &shard, &lines, &before)?;
            let changed = store.set_block_vectors(&alias, shard.key(), &ids, &before, &lines);
            whole_changed
                .entry(alias.clone())
                .or_default()
                .extend(changed);
            applied.shards_written += usize::from(written);
            warnings.extend(w);
        }
        // similar_to follows the whole-node vectors that changed, once per graph per batch (workshop 008 section 9).
        for (alias, ids) in whole_changed {
            let Some(policy) = policies[alias.as_str()] else {
                continue;
            };
            let (written, w) = store.update_similar(&alias, policy, &ids, defer_similar)?;
            applied.similar_files_written += written;
            warnings.extend(w);
        }
        Ok((applied, warnings))
    }

    /// The one vector space whose committed vectors `alias` holds in memory (embeddings-expert, R81-2): the key its
    /// graph.json records, whatever embedder is configured, so node-to-node similarity works from stored vectors
    /// (req-75); with none recorded, the space of the instance it embeds with (s17), else the default's. Never a
    /// space only because a folder exists.
    pub fn block_key(&self, store: &GraphStore, alias: &str) -> Option<ModelKey> {
        store
            .loaded(alias)
            .and_then(|l| l.meta.embedding.as_ref())
            .and_then(|e| e.key.clone())
            .or_else(|| {
                store
                    .effective_policy(alias)
                    .and_then(|p| self.instance_for(store, alias, p))
                    .map(|i| i.embedder.key())
            })
            .or_else(|| self.key())
    }

    /// Reads every shard of `alias` once, one shard at a time. Orphans and unreadable lines are file facts, found in
    /// every vector space the folder holds, embedder or not; the nodes that need work are sorted out for this
    /// service's space only, shard by shard (every vector of a node is in its one shard), so no line outlives its
    /// shard. Nodes with no texts need nothing. The same pass loads the graph's block space ([`Self::block_key`])
    /// into its vector blocks (ws004 s13), stale lines included (req-75 keeps them searchable), so it takes the
    /// writer.
    pub fn scan(&self, store: &mut GraphStore, alias: &str) -> Result<Scan, Problem> {
        // The policy is read once for the whole pass (architect).
        let Some(policy) = store.effective_policy(alias) else {
            return Ok(Scan::default());
        };
        let key = self.key_with(store, alias, policy);
        // Pending vectors are counted in the graph's block space (its recorded key, else the embedder's) even when
        // this embedder may not make them (req-75, s06 B); to_embed and to_tidy only when it may.
        let blocks = self.block_key(store, alias);
        let embeddable = key.is_some();
        // A secured graph's shards are a local cache, not the file of record: a stale or extra line there is
        // garbage, not data, so it raises no warning (graph-storage-expert).
        let cache = policy != Policy::OPEN;
        let bound = store.loaded(alias).map_or(0, |l| l.graph.node_bound());
        // Nodes planned from their lines, by handle; the rest are planned after, with no lines.
        let mut planned = alloc::vec![0u64; bound.div_ceil(64)];
        let mut scan = Scan::default();
        store.read_vectors(
            alias,
            policy,
            blocks.as_ref(),
            |space, shard, lines, graph| {
                let mut mine: BTreeMap<Id, Vec<VectorLine>> = BTreeMap::new();
                for (n, line) in lines {
                    match line {
                        ShardLine::Vector(v) if v.values.len() != space.dims() => {
                            if !cache {
                                scan.wrong_width.push((shard.clone(), n, v.values.len()))
                            }
                        }
                        ShardLine::Vector(v) if !graph.has_node(v.node) => {
                            if !cache {
                                scan.orphans.push((shard.clone(), n, v.node))
                            }
                        }
                        ShardLine::Vector(v) if Some(space) == blocks.as_ref() => {
                            mine.entry(v.node).or_default().push(v)
                        }
                        ShardLine::Vector(_) => {}
                        ShardLine::Kept(_) if !cache => scan.kept.push((shard.clone(), n)),
                        ShardLine::Kept(_) => {}
                    }
                }
                for (id, lines) in mine {
                    let Some(texts) = graph.node_texts(id) else {
                        continue;
                    };
                    if let Some(h) = graph.node_index(id) {
                        planned[h / 64] |= 1 << (h % 64);
                    }
                    let refs: Vec<&VectorLine> = lines.iter().collect();
                    plan_into(
                        &mut scan,
                        alias,
                        id,
                        &texts,
                        &refs,
                        &|| graph.whole_former(id),
                        embeddable,
                    );
                }
            },
        )?;
        if blocks.is_none() {
            return Ok(scan);
        }
        if let Some(loaded) = store.loaded(alias) {
            for node in loaded
                .graph
                .nodes()
                .filter(|n| !loaded.graph.is_provenance(n.ix()))
            {
                let h = node.ix().index();
                if planned[h / 64] & (1 << (h % 64)) == 0 {
                    let texts = vectors::node_texts(&node.to_node(), &loaded.meta.ontology);
                    plan_into(
                        &mut scan,
                        alias,
                        node.id(),
                        &texts,
                        &[],
                        &Vec::new,
                        embeddable,
                    );
                }
            }
        }
        // In id order, each node once (a hand-moved line can put a node's lines in two shards).
        scan.to_embed.sort();
        scan.to_embed.dedup();
        scan.to_tidy.sort();
        scan.to_tidy.dedup();
        scan.to_tidy
            .retain(|id| scan.to_embed.binary_search(id).is_err());
        scan.pending.sort();
        scan.pending.dedup();
        scan.outdated.sort();
        scan.outdated.dedup();
        Ok(scan)
    }

    /// Warnings for a scan: each orphan, unreadable and wrong-width line, by shard and line number only.
    pub fn scan_warnings(graph: &ValidName, scan: &Scan) -> Vec<Problem> {
        scan.orphans
            .iter()
            .map(|(s, n, id)| problems::vector_orphan(graph, s, *n, *id))
            .chain(
                scan.kept
                    .iter()
                    .map(|(s, n)| problems::vector_line_kept(graph, s, *n)),
            )
            .chain(
                scan.wrong_width
                    .iter()
                    .map(|(s, n, w)| problems::vector_wrong_width(graph, s, *n, *w)),
            )
            .collect()
    }
}

/// Drops from `planned` each text that looks like it holds a secret and is not allowed on this machine (req-49,
/// SEC-10), judged on the text that would be sent, not on the node now, so an edit between plan and send cannot slip
/// through. A field text is judged with its field's name in front, as the whole-node text shows it. The whole-node
/// text holds every field, so it is held when any part of it is: it is scanned as rebuilt without the allowed
/// fields. Only for embedders that are not local; a local one may embed everything.
/// Keeps only `planned`'s texts that its node would embed now (same field, same hash); false when it planned texts
/// and none is left, or its node is gone. A tidy-only plan (no texts) is kept.
fn still_current(store: &GraphStore, planned: &mut Planned) -> bool {
    if planned.texts.is_empty() {
        return true;
    }
    let Some(now) = store.node_texts(&planned.job.alias, planned.job.node) else {
        return false;
    };
    let now: Vec<(&str, TextHash)> = now
        .iter()
        .map(|(f, t)| (f.as_str(), TextHash::of(t)))
        .collect();
    planned
        .texts
        .retain(|w| now.contains(&(w.field.as_str(), w.hash)));
    !planned.texts.is_empty()
}

pub fn hold_secrets(store: &GraphStore, allows: &SecretAllows, planned: &mut Planned) {
    let Some(graph) = store.loaded(&planned.job.alias).map(|l| l.meta.id) else {
        planned.texts.clear();
        return;
    };
    let node = planned.job.node;
    let whole = vectors::whole_node_field();
    let allowed = |field: &str, value: &str| allows.allows(graph, node, field, TextHash::of(value));
    planned.texts.retain(|w| {
        if w.field == whole {
            return true;
        }
        let value = w.text.strip_prefix(vectors::FIELD_FORM).unwrap_or(&w.text);
        secrets::field_suspect(&w.field, value).is_none() || allowed(&w.field, value)
    });
    if let Some(at) = planned.texts.iter().position(|w| w.field == whole) {
        // Field by field first (R120-1): any flagged field that is not allowed holds the whole node.
        let flagged = store
            .node_values(&planned.job.alias, node)
            .unwrap_or_default()
            .iter()
            .any(|(f, v)| secrets::field_suspect(f, v).is_some() && !allowed(f, v));
        // Then the whole-node text rebuilt from the node now with each allowed field left out (never a text search,
        // so no other text can hide inside an allowed one). If the node changed since it was planned, the text to
        // send is not that node's: it is judged as it is, with no allow.
        let judged = match store.whole_texts(&planned.job.alias, node, &|f, v| {
            secrets::field_suspect(f, v).is_some() && allowed(f, v)
        }) {
            Some((now, without)) if now == planned.texts[at].text => without,
            _ => planned.texts[at].text.clone(),
        };
        if flagged || secrets::scan(&judged).is_some() {
            planned.texts.remove(at);
        }
    }
}

/// Whether the secret hold keeps any of node `id`'s texts from a hosted embedder now (what `vectors_held` counts).
pub fn holds_secret(store: &GraphStore, allows: &SecretAllows, alias: &str, id: Id) -> bool {
    let Some(texts) = store.node_texts(alias, id) else {
        return false;
    };
    let mut planned = Planned {
        job: EmbedJob {
            alias: alias.into(),
            node: id,
        },
        texts: texts
            .into_iter()
            .map(|(field, text)| Wanted {
                hash: TextHash::of(&text),
                field,
                text,
            })
            .collect(),
    };
    let before = planned.texts.len();
    hold_secrets(store, allows, &mut planned);
    planned.texts.len() < before
}

/// Something `embed` did to one text instead of embedding it whole. Names the graph, node and field, never the
/// text, so it is safe to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedNote {
    pub job: EmbedJob,
    pub field: String,
    pub what: NoteKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    /// Longer than the embedder's input cap (by umx's estimate): cut to about this many tokens before sending.
    Cut { max_tokens: usize },
    /// The provider still refused it as too long: not embedded this time; the others in its batch were.
    Refused,
    /// The provider cut it itself (`Embedded::truncated`): its vector covers only the start of the text.
    Truncated,
}

impl fmt::Display for EmbedNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "graph {} node {} field {}: ",
            self.job.alias, self.job.node, self.field
        )?;
        match self.what {
            NoteKind::Cut { max_tokens } => {
                write!(
                    f,
                    "longer than the embedder takes; cut to about {max_tokens} tokens"
                )
            }
            NoteKind::Refused => f.write_str("refused by the embedder as too long; not embedded"),
            NoteKind::Truncated => {
                f.write_str("truncated by the embedder; its vector covers the start only")
            }
        }
    }
}

/// Embeds every text in `batch` (document kind) and hands each node its vectors, plus notes. Takes no store: the
/// caller holds no lock while the provider works.
///
/// One text never fails the others (embeddings-expert, round 1): a text over `max_input_tokens` (estimated at
/// two bytes a token) is cut on a character boundary before sending; if the provider still refuses one
/// (`InputTooLong { index }`), only that text is dropped and the rest are sent again. A cut text's vector keeps
/// the hash of the whole text, so it counts as current and is not planned again until the text changes.
pub fn embed(
    embedder: &dyn Embedder,
    batch: &[Planned],
) -> Result<(Vec<Vectors>, Vec<EmbedNote>), EmbedError> {
    embed_counted(embedder, batch).map(|(vectors, notes, _)| (vectors, notes))
}

/// Vectors, notes, and the tokens the provider says it counted (when every call it took says).
pub type Counted = (Vec<Vectors>, Vec<EmbedNote>, Option<u64>);

/// [`embed`], with the tokens the provider counted, for pacing (ws004 s25).
pub fn embed_counted(embedder: &dyn Embedder, batch: &[Planned]) -> Result<Counted, EmbedError> {
    let max_tokens = embedder.max_input_tokens();
    let max_bytes = max_tokens.saturating_mul(2);
    let mut notes = Vec::new();
    let note = |p: &Planned, t: usize, what| EmbedNote {
        job: p.job.clone(),
        field: p.texts[t].field.clone(),
        what,
    };
    // (node in batch, text in node, text to send)
    let mut items: Vec<(usize, usize, String)> = Vec::new();
    for (b, p) in batch.iter().enumerate() {
        for (t, w) in p.texts.iter().enumerate() {
            let mut text = w.text.clone();
            if text.len() > max_bytes {
                let mut cut = max_bytes;
                while !text.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.truncate(cut);
                notes.push(note(p, t, NoteKind::Cut { max_tokens }));
            }
            items.push((b, t, text));
        }
    }
    let mut got: Vec<(usize, usize, Vec<f32>)> = Vec::new();
    let mut counted = None;
    while !items.is_empty() {
        let texts: Vec<String> = items.iter().map(|i| i.2.clone()).collect();
        match embedder.embed(&texts, TextKind::Document) {
            Ok(out) => {
                counted = out.tokens;
                for i in out.truncated {
                    if let Some((b, t, _)) = items.get(i) {
                        notes.push(note(&batch[*b], *t, NoteKind::Truncated));
                    }
                }
                got.extend(
                    items
                        .iter()
                        .zip(out.vectors)
                        .map(|((b, t, _), v)| (*b, *t, v)),
                );
                break;
            }
            Err(EmbedError::InputTooLong { index }) if index < items.len() => {
                let (b, t, _) = items.remove(index);
                notes.push(note(&batch[b], t, NoteKind::Refused));
            }
            Err(e) => return Err(e),
        }
    }
    let vectors = batch
        .iter()
        .enumerate()
        .map(|(b, p)| Vectors {
            job: p.job.clone(),
            key: Some(embedder.key()),
            vectors: got
                .iter()
                .filter(|(gb, _, _)| *gb == b)
                .map(|(_, t, v)| (p.texts[*t].field.clone(), p.texts[*t].hash, v.clone()))
                .collect(),
        })
        .collect();
    Ok((vectors, notes, counted))
}

/// Plans one node of a scan: queued to embed, to tidy, or nothing. `embeddable`: this embedder may make the graph's
/// vectors, so needed work is queued; otherwise it is only counted as pending. `v1`: the hash of the node's whole-node
/// text in form v1 (no roles). A node whose only whole-node line is an older version made from exactly that text,
/// which differs from the text now, lost a field to the data role (req-97): it is pending and `outdated`, keeps its
/// lines (still searchable) and is never queued from a load; only reembed or an explicit role change remakes it.
fn plan_into(
    scan: &mut Scan,
    alias: &str,
    node: Id,
    texts: &[(String, String)],
    lines: &[&VectorLine],
    former: &dyn Fn() -> Vec<TextHash>,
    embeddable: bool,
) {
    let job = EmbedJob {
        alias: alias.into(),
        node,
    };
    match plan_node(&job, texts, lines) {
        Some(_) if outdated(texts, lines, former) => {
            scan.pending.push(node);
            scan.outdated.push(node);
        }
        Some(p) if p.texts.is_empty() => {
            if embeddable {
                scan.to_tidy.push(node)
            }
        }
        Some(_) => {
            scan.pending.push(node);
            if embeddable {
                scan.to_embed.push(node)
            }
        }
        None => {}
    }
}

/// Whether a node's whole-node vector is out of date only because a field it held is now data (see [`plan_into`]):
/// no whole-node line is current for its text now, and one was made from a text it had with a now-data field in
/// (`former`, computed only then). Any version: a role can change under `@node.v2` too (R189-2).
fn outdated(
    texts: &[(String, String)],
    lines: &[&VectorLine],
    former: &dyn Fn() -> Vec<TextHash>,
) -> bool {
    let Some(now) = texts.iter().find(|(f, _)| is_whole(f)) else {
        return false;
    };
    let now = TextHash::of(&now.1);
    let whole: Vec<TextHash> = lines
        .iter()
        .filter(|l| is_whole(&l.field))
        .map(|l| l.hash)
        .collect();
    if whole.is_empty() || whole.contains(&now) {
        return false;
    }
    let former = former();
    whole.iter().any(|h| former.contains(h))
}

/// Whether `field` is a whole-node field of any version.
fn is_whole(field: &str) -> bool {
    vectors::whole_node_version(field).is_some()
}

/// Whether line `l` is the current line for wanted field `field` with hash `hash`: the same field, or for the
/// whole node a line of any version made from the same text (same text, same key, same vector; embeddings-expert).
fn current_for(l: &VectorLine, field: &str, hash: TextHash) -> bool {
    l.hash == hash && (l.field == field || (is_whole(field) && is_whole(&l.field)))
}

/// What a node needs given its texts now and its lines in the shard: the texts with no current line, or an empty
/// list when only leftover or repeated lines must go; None when nothing is needed. Whole-node lines of every
/// version count as one field, so a carried older line is neither needed nor leftover.
fn plan_node(job: &EmbedJob, texts: &[(String, String)], lines: &[&VectorLine]) -> Option<Planned> {
    let wanted: Vec<Wanted> = texts
        .iter()
        .map(|(f, t)| Wanted {
            field: f.clone(),
            hash: TextHash::of(t),
            text: t.clone(),
        })
        .collect();
    let whole = vectors::whole_node_field();
    let mut per_field: BTreeMap<&str, usize> = BTreeMap::new();
    for l in lines {
        let field = if is_whole(&l.field) {
            whole.as_str()
        } else {
            l.field.as_str()
        };
        *per_field.entry(field).or_default() += 1;
    }
    let to_embed: Vec<Wanted> = wanted
        .iter()
        .filter(|w| !lines.iter().any(|l| current_for(l, &w.field, w.hash)))
        .cloned()
        .collect();
    let untidy = per_field.values().any(|n| *n > 1)
        || per_field
            .keys()
            .any(|f| !wanted.iter().any(|w| w.field == *f));
    (!to_embed.is_empty() || untidy).then(|| Planned {
        job: job.clone(),
        texts: to_embed,
    })
}

/// `lines` with `node`'s lines rebuilt: for each field it has now, its fresh line, else its first line whose hash
/// is current, else its first line (stale, kept until a fresh one comes, so search still finds it); lines for
/// fields it no longer has are dropped (a data field's among them). For the whole node, lines of every version are
/// candidates, the current version first: a carried older line whose hash is current is written back under the
/// current version's name, with no embed call (embeddings-expert, s22).
fn tidy_node(
    lines: Vec<ShardLine>,
    node: Id,
    current: &BTreeMap<&str, TextHash>,
    fresh: Vec<VectorLine>,
    dims: usize,
) -> Vec<ShardLine> {
    let (mut mine, mut others): (Vec<ShardLine>, Vec<ShardLine>) = lines
        .into_iter()
        .partition(|l| l.vector().is_some_and(|v| v.node == node));
    // Sort order decides which repeated line is "first" (req-36).
    mine.sort_by_key(ShardLine::bytes);
    for (field, hash) in current {
        if let Some(f) = fresh.iter().find(|v| v.field == *field) {
            others.push(ShardLine::Vector(f.clone()));
            continue;
        }
        let rank = |v: &VectorLine| match v.field == *field {
            true => 0,
            false if is_whole(field) && is_whole(&v.field) => 1,
            false => 2,
        };
        let of_field = || {
            let mut found: Vec<&VectorLine> = mine
                .iter()
                .filter_map(ShardLine::vector)
                .filter(|v| rank(v) < 2 && v.values.len() == dims)
                .collect();
            found.sort_by_key(|v| rank(v));
            found
        };
        if let Some(keep) = of_field().into_iter().find(|v| v.hash == *hash) {
            let mut keep = keep.clone();
            keep.field = (*field).to_string();
            others.push(ShardLine::Vector(keep));
        } else if let Some(stale) = of_field().into_iter().next() {
            others.push(ShardLine::Vector(stale.clone()));
        }
    }
    others
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn line(node: u128, field: &str, text: &str, value: i8) -> VectorLine {
        VectorLine {
            node: Id(node),
            field: field.into(),
            hash: TextHash::of(text),
            values: vec![value; 4],
        }
    }

    fn job() -> EmbedJob {
        EmbedJob {
            alias: "g".into(),
            node: Id(1),
        }
    }

    fn texts(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(f, t)| (String::from(*f), String::from(*t)))
            .collect()
    }

    #[test]
    fn current_lines_plan_nothing_and_a_changed_field_plans_only_itself() {
        let a = line(1, "title", "A", 1);
        let n = line(1, "note", "N", 2);
        let t = texts(&[("note", "N"), ("title", "A")]);
        assert_eq!(plan_node(&job(), &t, &[&a, &n]), None);
        let edited = texts(&[("note", "N2"), ("title", "A")]);
        let p = plan_node(&job(), &edited, &[&a, &n]).unwrap();
        assert_eq!(
            p.texts.iter().map(|w| w.field.as_str()).collect::<Vec<_>>(),
            ["note"]
        );
    }

    #[test]
    fn a_repeated_or_leftover_line_plans_a_tidy_with_nothing_to_embed() {
        let a1 = line(1, "title", "A", 1);
        let a2 = line(1, "title", "A", 9);
        let gone = line(1, "note", "N", 2);
        let t = texts(&[("title", "A")]);
        assert_eq!(plan_node(&job(), &t, &[&a1, &a2]).unwrap().texts, vec![]);
        assert_eq!(plan_node(&job(), &t, &[&a1, &gone]).unwrap().texts, vec![]);
    }

    #[test]
    fn tidy_keeps_the_first_repeat_drops_leftovers_and_keeps_other_nodes() {
        let a9 = line(1, "title", "A", 9);
        let a1 = line(1, "title", "A", 1);
        let gone = line(1, "note", "N", 2);
        let other = line(2, "title", "B", 3);
        let kept = ShardLine::Kept(b"junk".to_vec());
        let lines = vec![
            ShardLine::Vector(a9.clone()),
            ShardLine::Vector(a1.clone()),
            ShardLine::Vector(gone),
            ShardLine::Vector(other.clone()),
            kept.clone(),
        ];
        let current: BTreeMap<&str, TextHash> = [("title", TextHash::of("A"))].into();
        let out = tidy_node(lines, Id(1), &current, Vec::new(), 4);
        let first = if a1.encode() < a9.encode() { a1 } else { a9 };
        assert_eq!(
            out,
            vec![ShardLine::Vector(other), kept, ShardLine::Vector(first)]
        );
    }

    #[test]
    fn an_older_whole_node_line_of_the_same_text_is_current_and_renamed_only_at_the_next_write() {
        let whole = vectors::whole_node_field();
        let v1 = line(1, "@node.v1", "W", 5);
        let t = texts(&[(whole.as_str(), "W"), ("title", "A")]);
        let a = line(1, "title", "A", 1);
        assert_eq!(plan_node(&job(), &t, &[&a, &v1]), None, "no embed, no tidy");
        let mut scan = Scan::default();
        plan_into(
            &mut scan,
            "g",
            Id(1),
            &t,
            &[&a, &v1],
            &|| vec![TextHash::of("W")],
            true,
        );
        assert_eq!(scan, Scan::default(), "zero pending and nothing queued");
        // When the shard is next written for another reason, the carried line takes the current name, same values.
        let current: BTreeMap<&str, TextHash> = [
            (whole.as_str(), TextHash::of("W")),
            ("title", TextHash::of("A")),
        ]
        .into();
        let lines = vec![ShardLine::Vector(a.clone()), ShardLine::Vector(v1.clone())];
        let out = tidy_node(lines, Id(1), &current, Vec::new(), 4);
        let renamed = VectorLine {
            field: whole.clone(),
            ..v1.clone()
        };
        assert_eq!(out, vec![ShardLine::Vector(renamed), ShardLine::Vector(a)]);
    }

    #[test]
    fn a_current_version_whole_node_line_made_with_a_now_data_field_is_outdated_too() {
        // R189-2: a role can change under @node.v2; the version in the name says nothing about the roles.
        let whole = vectors::whole_node_field();
        let v2 = line(1, &whole, "W with file", 5);
        let t = texts(&[(whole.as_str(), "W"), ("title", "A")]);
        let a = line(1, "title", "A", 1);
        let mut scan = Scan::default();
        plan_into(
            &mut scan,
            "g",
            Id(1),
            &t,
            &[&a, &v2],
            &|| vec![TextHash::of("W with file")],
            true,
        );
        assert_eq!(scan.outdated, [Id(1)]);
        assert!(scan.to_embed.is_empty(), "{scan:?}");
        // An edit since (no former text matches) is caught up as before.
        let mut scan = Scan::default();
        plan_into(&mut scan, "g", Id(1), &t, &[&a, &v2], &Vec::new, true);
        assert!(scan.outdated.is_empty());
        assert_eq!(scan.to_embed, [Id(1)]);
    }

    #[test]
    fn a_whole_node_line_made_from_text_with_a_now_data_field_is_outdated_and_never_queued_by_a_scan()
     {
        let whole = vectors::whole_node_field();
        // v1 held "file: x"; the text now leaves it out.
        let v1 = line(1, "@node.v1", "W with file", 5);
        let data = line(1, "file", "x", 6);
        let t = texts(&[(whole.as_str(), "W"), ("title", "A")]);
        let a = line(1, "title", "A", 1);
        let mut scan = Scan::default();
        let v1_hash = || vec![TextHash::of("W with file")];
        plan_into(&mut scan, "g", Id(1), &t, &[&a, &data, &v1], &v1_hash, true);
        assert_eq!(
            (scan.pending.as_slice(), scan.outdated.as_slice()),
            (&[Id(1)][..], &[Id(1)][..])
        );
        assert!(
            scan.to_embed.is_empty() && scan.to_tidy.is_empty(),
            "{scan:?}"
        );
        // A v1 line from some other text means the node changed since: stale as before, queued.
        let mut scan = Scan::default();
        let other = || vec![TextHash::of("W edited since")];
        plan_into(&mut scan, "g", Id(1), &t, &[&a, &data, &v1], &other, true);
        assert!(scan.outdated.is_empty());
        assert_eq!(scan.to_embed, [Id(1)]);
        // Planned for real (reembed or a role change): the whole node is embedded; the data line goes at the write.
        let p = plan_node(&job(), &t, &[&a, &data, &v1]).unwrap();
        assert_eq!(
            p.texts.iter().map(|w| w.field.as_str()).collect::<Vec<_>>(),
            [whole.as_str()]
        );
        let fresh = line(1, &whole, "W", 7);
        let current: BTreeMap<&str, TextHash> = [
            (whole.as_str(), TextHash::of("W")),
            ("title", TextHash::of("A")),
        ]
        .into();
        let lines = [&a, &data, &v1]
            .map(|l| ShardLine::Vector(l.clone()))
            .to_vec();
        let out = tidy_node(lines, Id(1), &current, vec![fresh.clone()], 4);
        assert_eq!(out, vec![ShardLine::Vector(fresh), ShardLine::Vector(a)]);
    }
}
