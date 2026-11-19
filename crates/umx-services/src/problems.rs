//! Constructors for the workshop 007 section 9 catalogue. Each returns a code and named slots; umx-protocol holds
//! the message and next templates and the daemon renders them. Slot names match the template's `<slot>` names.
//! Where the shape of `next` depends on the case, the template's next is `<fix>` and the slot is a whole
//! command built here from umx's own words and checked values. `<cmd>` is a command label the daemon fills.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::codec::{DecodeError, GraphFile, Repeat};
use umx_core::model::FORMAT_VERSION;
use umx_core::name::{is_snake_case, suggest_name};
use umx_core::ontology::Described;
use umx_core::ports::{EmbedError, FileRef, FilesError, GraphDir, ModelKey};
use umx_core::problem::Candidate;
use umx_core::settings::{self, SETTINGS};
use umx_core::vectors::ShardFile;
use umx_core::{Command, FieldKind, Id, IdText, Policy, Problem, Role, SafeText, Slot, ValidName};

/// The codes in [`CODES`] that are hints (catalogue kind Hint, ws002 X1): advice a batch lists after its warnings,
/// never counted toward `batch_warnings_truncated`. The daemon's test checks this is exactly the catalogue's Hint
/// rows that services return.
pub const HINTS: &[&str] = &[
    "search_results_cut",
    "similarity_unavailable",
    "similarity_no_embedder",
    "similarity_incomplete",
    "secured_graphs_skipped",
    "purpose_missing",
    "descriptions_missing",
    "field_looks_like_labels",
    "field_looks_like_prose",
    "field_data_by_default",
    "reason_missing",
    "near_duplicate",
    "near_duplicate_line",
    "related_skipped",
    "find_no_match",
    "resource_elsewhere",
    "entity_key_is_title",
    "graph_loaded_on_demand",
    "graphfile_no_repo",
    "segment_without_parent",
    "std_entries_shown",
    "already_std",
    "adopt_rename_suggested",
    "edge_direction_normalised",
    "lint_orphans",
    "lint_proposed_keys",
    "lint_terms",
    "lint_double_negatives",
    "lint_raised_in_evidence",
    "lint_truncated",
    "lint_terms_unusable",
    "along_not_placed",
    "along_by_rule",
    "batch_use_ref_field",
    "alias_redirected",
    "on_key_field_held",
    "batch_replayed",
    "idempotency_key_expired",
];

/// The most hints a reply shows: umx-protocol's MAX_HINTS and its `hints_truncated` (the daemon's test keeps the two
/// the same; services cannot read the catalogue). A batch keeps every hint and counts only this many in its reply
/// bound (ws002 X1, review R1).
pub const HINTS_SHOWN: usize = 9;

/// Whether a code is one of [`HINTS`].
pub fn is_hint(code: &str) -> bool {
    HINTS.contains(&code)
}

/// Every code umx-services can return. The daemon's test checks each has a row in umx-protocol's catalogue.
pub const CODES: &[&str] = &[
    "graphfile_skipped_secured",
    "graphfile_no_repo",
    "graphfile_not_canonical",
    "graphfile_too_large",
    "graphfile_changed",
    "graphfile_unusable",
    "graphfile_id_mismatch",
    "graphfile_entry_skipped",
    "graphfile_invalid",
    "graphfile_newer",
    "graphfile_missing",
    "graphfile_repo_none",
    "graphfile_unknown",
    "graphfile_load_too_many",
    "actor_invalid",
    "reserved_type",
    "reserved_relation",
    "derived_relation",
    "reserved_relation_stored",
    "std_field_conflict",
    "std_field_conflict_stored",
    "adopt_field_kind_clash",
    "adopt_reserved_relation",
    "adopt_map_unknown",
    "adopt_map_ambiguous",
    "adopt_map_merge",
    "adopt_map_symmetry",
    "adopt_map_duplicate_edge",
    "adopt_value_kind",
    "adopt_field_to_tag_refused",
    "adopt_tag_value_invalid",
    "already_std",
    "adopt_rename_suggested",
    "session_agent_conflict",
    "duplicate_prov_key",
    "node_id_is_graph_id",
    "external_edges_skipped",
    "config_import_refused",
    "config_import_exists",
    "scope_format_newer",
    "scope_member_moved",
    "scope_exists",
    "scope_unknown",
    "scope_graph_not_loaded",
    "scope_includes_secured",
    "copy_dropped",
    "scope_store_invalid",
    "scope_store_not_saved",
    "scope_limit",
    "query_busy",
    "copies_skipped",
    "query_graph_twice",
    "copy_ambiguous",
    "primary_is_secured",
    "batch_too_large",
    "batch_line_invalid",
    "duplicate_field",
    "batch_use_ref_field",
    "id_invalid",
    "id_taken",
    "id_in_future",
    "id_in_other_graph",
    "batch_ref_unknown",
    "batch_ref_repeated",
    "batch_ref_conflict",
    "batch_refs_other_graph",
    "idempotency_key_reused",
    "idempotency_receipt_stale",
    "receipts_unreadable",
    "batch_replayed",
    "idempotency_key_expired",
    "batch_errors_truncated",
    "batch_warnings_truncated",
    "batch_failed",
    "graph_none_loaded",
    "graph_ambiguous",
    "graph_mismatch",
    "graph_not_loaded",
    "registry_unreadable",
    "registry_not_saved",
    "graph_id_changed",
    "graph_forget_loaded",
    "graph_forget_not_saved",
    "registry_update_not_saved",
    "graph_not_known",
    "alias_ambiguous",
    "alias_redirected",
    "former_alias_ambiguous",
    "graphfile_not_updated",
    "graph_move_refused",
    "graph_move_cross_repo",
    "evidence_root_stale",
    "graph_loaded_on_demand",
    "graph_not_found",
    "graph_exists",
    "graph_orphan_records",
    "graph_invalid",
    "graph_format_newer",
    "graph_too_large",
    "unsorted_file",
    "duplicate_id",
    "provenance_key_invalid",
    "dangling_edge",
    "symmetric_duplicate",
    "duplicate_locator",
    "locator_not_normalised",
    "locator_field_kept",
    "locator_field_invalid",
    "resource_exists",
    "resource_elsewhere",
    "locator_in_use",
    "locator_changed",
    "not_a_resource",
    "locator_invalid",
    "locator_absolute",
    "symmetric_refused",
    "load_warnings_not_shown",
    "unknown_key",
    "embedder_mismatch",
    "embedder_unavailable",
    "secured_shards_remain",
    "secured_shards_unchecked",
    "embedder_invalid",
    "alias_invalid",
    "alias_in_use",
    "path_outside_roots",
    "usage_invalid",
    "node_not_found",
    "id_ambiguous",
    "who_ambiguous",
    "who_unknown",
    "name_invalid",
    "reserved_name",
    "entity_key_invalid",
    "duplicate_key",
    "on_key_ambiguous",
    "on_key_field_held",
    "entity_key_not_canonical",
    "entity_key_unusable",
    "tag_invalid",
    "tag_limit",
    "tag_needs_std",
    "tag_off_vocabulary",
    "tag_not_canonical",
    "tag_unusable",
    "tag_filter_not_std",
    "tag_filter_untested",
    "entity_link_invalid",
    "entity_type_mismatch",
    "entity_graph_twice",
    "entity_key_ignored",
    "entity_link_overlap",
    "entity_types_mixed",
    "entity_key_is_title",
    "entity_join_denied",
    "already_declared",
    "description_too_long",
    "description_undeclared",
    "purpose_missing",
    "descriptions_missing",
    "kind_invalid",
    "type_unknown",
    "ui_no_view",
    "ui_waiters_busy",
    "ui_selection_hidden",
    "field_conflict",
    "field_role_conflict",
    "ontology_errors_truncated",
    "field_missing",
    "ontology_in_use",
    "ontology_builtin",
    "relation_field_conflict",
    "where_invalid",
    "cross_graph_not_yet",
    "write_failed",
    "write_not_durable",
    "recovery_conflict",
    "graph_files_changed",
    "stale_files",
    "gitignore_line_missing",
    "internal",
    "off_vocabulary_type",
    "off_vocabulary_status",
    "off_vocabulary_relation",
    "off_vocabulary_field",
    "off_vocabulary_edge_field",
    "near_duplicate",
    "near_duplicate_line",
    "related_skipped",
    "find_no_match",
    "edge_exists",
    "evidence_appended",
    "evidence_field_invalid",
    "evidence_field_kept",
    "edge_exists_differs",
    "edge_not_found",
    "edge_would_duplicate",
    "edge_derived",
    "edge_reserved_field_in_use",
    "edge_repeated",
    "edge_reverse_symmetric",
    "external_end_unchecked",
    "link_into_secured",
    "own_graph_end",
    "type_column_unknown",
    "edge_broken",
    "link_targets_unnamed",
    "field_kind_mismatch",
    "time_no_offset",
    "time_as_text",
    "anchor_undated",
    "segment_without_parent",
    "edge_direction_normalised",
    "date_not_a_date",
    "relation_endpoint_unexpected",
    "std_newer",
    "std_entries_shown",
    "lint_orphans",
    "lint_proposed_keys",
    "lint_terms",
    "lint_double_negatives",
    "lint_raised_in_evidence",
    "lint_truncated",
    "lint_terms_unusable",
    "spine_name_unknown",
    "spine_main_taken",
    "spine_not_found",
    "spine_builtin_candidate",
    "spine_name_reserved",
    "spine_name_reserved_stored",
    "along_end_invalid",
    "along_not_placed",
    "along_by_rule",
    "use_status_verb",
    "reason_missing",
    "evidence_pointer_invalid",
    "evidence_outside_roots",
    "dangling_evidence",
    "evidence_unchecked",
    "evidence_not_found",
    "evidence_ambiguous",
    "already_removed",
    "redacted_in_files_only",
    "evidence_redact_blocked",
    "evidence_missing_skipped",
    "node_reserved_field_in_use",
    "field_not_found",
    "root_unknown",
    "root_unavailable",
    "setting_unknown",
    "floor_uncalibrated",
    "similar_to_incomplete",
    "similarity_pairs_too_large",
    "similarity_pairs_timed_out",
    "similarity_model_mismatch",
    "similarity_setting_invalid",
    "vector_orphan",
    "vector_line_kept",
    "embedder_not_configured",
    "embedder_not_local_for_secured",
    "embedder_route_changed",
    "vector_wrong_width",
    "vectors_unreadable",
    "secured_embed_refused",
    "vector_shard_unreadable",
    "reembed_model_not_configured",
    "policy_downgrade_unconfirmed",
    "local_state_unreadable",
    "local_state_not_saved",
    "local_state_not_durable",
    "policy_not_recorded",
    "policy_lowered",
    "policy_not_lowered",
    "search_results_cut",
    "secured_graphs_skipped",
    "secured_graphs_read",
    "similarity_unavailable",
    "similarity_no_embedder",
    "similarity_incomplete",
    "word_prefix_capped",
    "secret_suspected",
    "secret_allow_refused",
    "setting_secret_suspected",
    "text_form_outdated",
    "role_changed",
    "field_looks_like_labels",
    "field_looks_like_prose",
    "field_data_by_default",
    "role_not_embeddable",
    "field_not_declared",
];

pub fn actor_invalid(field: &'static str) -> Problem {
    Problem::new("actor_invalid").with("field", Slot::name_or(field, "actor"))
}

pub fn reserved_type(kind: &ValidName) -> Problem {
    Problem::new("reserved_type").with("type", Slot::Name(kind.clone()))
}

pub fn reserved_relation(relation: &ValidName) -> Problem {
    Problem::new("reserved_relation").with("relation", Slot::Name(relation.clone()))
}

pub fn derived_relation(relation: &ValidName) -> Problem {
    Problem::new("derived_relation").with("relation", Slot::Name(relation.clone()))
}

pub fn session_agent_conflict(session: Id, agent: Id) -> Problem {
    Problem::new("session_agent_conflict")
        .with("session", Slot::Id(session))
        .with("agent", Slot::Id(agent))
}

pub fn duplicate_prov_key(node: Id, in_use: Id) -> Problem {
    Problem::new("duplicate_prov_key")
        .with("id", Slot::Id(node))
        .with("in_use", Slot::Id(in_use))
}

pub fn node_id_is_graph_id(node: Id, line: usize) -> Problem {
    Problem::new("node_id_is_graph_id")
        .with("node", Slot::Id(node))
        .with("line", Slot::Count(line as u64))
        .subject(node.to_text())
}

pub fn external_edges_skipped(count: usize) -> Problem {
    Problem::new("external_edges_skipped").with("count", Slot::Count(count as u64))
}

pub fn scope_exists(scope: &str) -> Problem {
    Problem::new("scope_exists").with("name", Slot::name_or(scope, "(that name)"))
}

pub fn scope_unknown(scope: &str) -> Problem {
    Problem::new("scope_unknown").with("name", Slot::name_or(scope, "(that name)"))
}

pub fn scope_includes_secured(alias: &str) -> Problem {
    Problem::new("scope_includes_secured").with("name", Slot::name_or(alias, "(that alias)"))
}

pub fn copy_dropped(alias: &str) -> Problem {
    Problem::new("copy_dropped").with("name", Slot::name_or(alias, "(that alias)"))
}

pub fn scope_graph_not_loaded(alias: &str, folder: &GraphDir) -> Problem {
    Problem::new("scope_graph_not_loaded")
        .with("name", Slot::name_or(alias, "(that alias)"))
        .with("folder", Slot::path(folder))
        .with(
            "fix",
            Slot::Command(Command::new("graph load").path(folder)),
        )
}

pub fn scope_store_invalid(file: &GraphDir, reason: &str) -> Problem {
    Problem::new("scope_store_invalid")
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

pub fn scope_store_not_saved(file: &GraphDir, reason: &str) -> Problem {
    Problem::new("scope_store_not_saved")
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

pub fn scope_limit(file: &GraphDir) -> Problem {
    Problem::new("scope_limit").with("file", Slot::path(file))
}

pub fn scope_format_newer(file: &GraphDir, found: u64) -> Problem {
    Problem::new("scope_format_newer")
        .with("file", Slot::path(file))
        .with("found", Slot::Count(found))
        .with("supported", Slot::Count(1))
}

pub fn scope_member_moved(
    scope: &str,
    member: &str,
    saved: &GraphDir,
    actual: &GraphDir,
) -> Problem {
    Problem::new("scope_member_moved")
        .with("scope", Slot::name_or(scope, "(that scope)"))
        .with("name", Slot::name_or(member, "(that member)"))
        .with("saved", Slot::path(saved))
        .with("actual", Slot::path(actual))
}

/// A config file write the OS refused.
pub fn config_write_failed(file: &GraphDir, e: &umx_core::ports::ConfigFilesError) -> Problem {
    Problem::new("write_failed")
        .with("file", Slot::path(file))
        .with("os error", Slot::Os(SafeText::lossy(&e.to_string())))
}

pub const WHY_ONLY_FS3: &str = "umx imports only from flowspace3";
pub const WHY_FS3_MISSING: &str =
    "the flowspace3 config.toml was not found (in FS3_CONFIG_DIR or ~/.config/flowspace3)";

/// What a `umx config import` was asked, so every command it suggests repeats it exactly (R177-2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportAsk<'a> {
    /// `--as`, or the instance the import resolved to.
    pub name: Option<&'a str>,
    pub model: Option<&'a str>,
    pub dims: Option<u32>,
    pub force: bool,
    pub dry_run: bool,
}

impl ImportAsk<'_> {
    /// The import command with these options; `placeholder` adds `--as <name>` or `--model <model>` for the one
    /// the user must still give.
    pub fn command(&self, placeholder: Option<&'static str>) -> Command {
        let mut c = Command::new("config import --from flowspace3");
        match (placeholder, self.name.and_then(ValidName::alias)) {
            (Some("--as"), _) => c = c.word("--as").word("<name>"),
            (_, Some(n)) => c = c.word("--as").name(&n),
            _ => {}
        }
        match (placeholder, self.model) {
            (Some("--model"), _) => c = c.word("--model").word("<model>"),
            (_, Some(m)) => c = c.word("--model").model(m),
            _ => {}
        }
        if let Some(d) = self.dims {
            c = c.word("--dims").count(u64::from(d));
        }
        if self.force {
            c = c.word("--force");
        }
        if self.dry_run {
            c = c.word("--dry-run");
        }
        c
    }
}

/// `umx config import` cannot map the other tool's config. `flag` (`--model`, `--as`) is the flag that fixes it;
/// the fix repeats the rest of `ask`.
pub fn import_refused(why: &'static str, flag: Option<&'static str>, ask: &ImportAsk) -> Problem {
    let fix = match flag {
        Some(f) => ask.command(Some(f)),
        None => Command::new("docs get config"),
    };
    Problem::new("config_import_refused")
        .with("why", Slot::Word(why))
        .with("fix", Slot::Command(fix))
}

/// `umx config import` would replace an instance the machine config defines. The fix names that instance with
/// `--as`, so following it replaces only the instance this command meant, and keeps the rest of `ask`.
pub fn import_exists(instance: &str, ask: &ImportAsk) -> Problem {
    let fix = ImportAsk {
        name: Some(instance),
        force: true,
        ..*ask
    };
    Problem::new("config_import_exists")
        .with("embedder", Slot::name_or(instance, "(that instance)"))
        .with("fix", Slot::Command(fix.command(None)))
        .subject(instance)
}

fn name(n: &ValidName) -> Slot {
    Slot::Name(n.clone())
}

pub fn graph_none_loaded() -> Problem {
    Problem::new("graph_none_loaded").with(
        "fix",
        Slot::Command(Command::new("graph load").word("<path>")),
    )
}

/// `retry` is the command rebuilt from its parsed form; the fix adds `--graph <first>` to it.
pub fn graph_ambiguous(retry: Option<&Command>, first: &ValidName, all: Vec<Candidate>) -> Problem {
    let fix = match retry {
        Some(cmd) => cmd.clone().word("--graph").name(first),
        None => Command::new("graph list"),
    };
    Problem::new("graph_ambiguous")
        .with("fix", Slot::Command(fix))
        .candidates(all)
}

/// An address names graph `address`; an explicit `--graph` names `given`.
pub fn graph_mismatch(address: &str, given: &str) -> Problem {
    Problem::new("graph_mismatch")
        .with("alias", Slot::name_or(address, "(that alias)"))
        .with("other", Slot::name_or(given, "(that alias)"))
}

pub fn graph_not_loaded(alias: &str) -> Problem {
    graph_not_loaded_because(
        alias,
        "this daemon has not loaded it",
        Command::new("graph list"),
    )
}

/// [`graph_not_loaded`] with why and the command that loads it, when the graph store knows (F72).
pub fn graph_not_loaded_because(alias: &str, why: &str, fix: Command) -> Problem {
    let p = Problem::new("graph_not_loaded")
        .with("alias", Slot::name_or(alias, "(that alias)"))
        .with("why", reason_slot(why))
        .with("hint", Slot::Word(""))
        .with("fix", Slot::Command(fix));
    // The alias as the subject: the daemon loads a remembered graph on demand from it (F72, architect).
    match ValidName::alias(alias) {
        Some(name) => p.subject(name.as_str()),
        None => p,
    }
}

/// [`graph_not_loaded`] for an address with a `?repo=` hint (workshop 002 section 7.9): the message names the repo
/// when it is a plain URL ([`SafeText::repo_url`]), and next loads the graph once it is cloned. umx never clones.
pub fn graph_not_loaded_hint(alias: &str, repo: &str) -> Problem {
    let load = match ValidName::alias(alias) {
        Some(a) => Command::new("graph load")
            .word("<path>")
            .word("--as")
            .name(&a),
        None => Command::new("graph load")
            .word("<path>")
            .word("--as")
            .word("<alias>"),
    };
    let p = Problem::new("graph_not_loaded")
        .with("alias", Slot::name_or(alias, "(that alias)"))
        .with("why", reason_slot("this daemon has not loaded it"))
        .with("hint", Slot::RepoHint(SafeText::repo_url(repo)))
        .with("fix", Slot::Command(load));
    // The subject names the alias, so the daemon's on-demand load (F72) can find it.
    match ValidName::alias(alias) {
        Some(a) => p.subject(a.as_str()),
        None => p,
    }
}

/// A `umx:` evidence source in loaded graph `graph` that names no single node: `reason` says which, and an
/// ambiguous prefix brings its candidates.
pub fn dangling_evidence(graph: &str, reason: &'static str, candidates: Vec<Candidate>) -> Problem {
    let find = Command::new("node find").word("<words>").word("--graph");
    let find = match ValidName::alias(graph) {
        Some(a) => find.name(&a),
        None => find.word("<alias>"),
    };
    Problem::new("dangling_evidence")
        .with("graph", Slot::name_or(graph, "(that alias)"))
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(find))
        .candidates(candidates)
}

/// A `umx:` evidence source in graph `graph`, which is not loaded or is secured, so its node was not checked.
pub fn evidence_unchecked(graph: &str) -> Problem {
    let load = Command::new("graph load").word("<path>").word("--as");
    let load = match ValidName::alias(graph) {
        Some(a) => load.name(&a),
        None => load.word("<alias>"),
    };
    Problem::new("evidence_unchecked")
        .with("graph", Slot::name_or(graph, "(that alias)"))
        .with("fix", Slot::Command(load))
}

/// `umx resolve` of `<address>#<field>` where node or edge `id` in `graph` has no such field; `fix` shows its fields.
pub fn field_not_found(graph: &ValidName, id: Id, field: &ValidName, fix: Command) -> Problem {
    Problem::new("field_not_found")
        .with("id", Slot::Id(id))
        .with("graph", name(graph))
        .with("field", name(field))
        .with("fix", Slot::Command(fix))
}

/// A remembered graph's folder now holds another graph id: it is not loaded under the remembered alias (security,
/// F72), so a swapped folder never inherits an alias, a scope membership or a primary role.
pub fn graph_id_changed(alias: &str, folder: &GraphDir) -> Problem {
    Problem::new("graph_id_changed")
        .with("alias", Slot::name_or(alias, "(that alias)"))
        .with("path", Slot::path(folder))
        .with(
            "fix",
            Slot::Command(Command::new("graph load").path(folder)),
        )
}

/// A command named a graph this machine remembers as loaded but the daemon did not hold (it unloaded when idle, or
/// did not reload at start); the daemon loaded it and ran the command (req-15, F72).
pub fn graph_loaded_on_demand(alias: &ValidName, folder: &GraphDir) -> Problem {
    Problem::new("graph_loaded_on_demand")
        .with("alias", name(alias))
        .with("path", Slot::path(folder))
}

/// This machine's graph registry (F72) is refused, does not parse or is newer: nothing reloads, nothing is written.
pub fn registry_unreadable(file: &GraphDir, reason: &str) -> Problem {
    Problem::new("registry_unreadable")
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// A graph loaded, but its row in the registry was not saved: it will not come back after a daemon restart.
pub fn registry_not_saved(alias: &str, file: &GraphDir, reason: &str) -> Problem {
    Problem::new("registry_not_saved")
        .with("alias", Slot::name_or(alias, "(that alias)"))
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// `graph forget` named a loaded graph (req-116): it removes only graphs that are not loaded.
pub fn graph_forget_loaded(alias: &ValidName) -> Problem {
    Problem::new("graph_forget_loaded")
        .with("alias", name(alias))
        .with(
            "fix",
            Slot::Command(Command::new("graph unload").name(alias)),
        )
        .subject(alias.as_str())
}

/// `graph forget` could not write the index: nothing was forgotten (review R193-B1).
pub fn graph_forget_not_saved(alias: &str, file: &GraphDir, reason: &str) -> Problem {
    Problem::new("graph_forget_not_saved")
        .with("alias", Slot::name_or(alias, "(that alias)"))
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// A registry change that is not a load (index details, a no-primary mark) was not written (review R193-B1, B2).
pub fn registry_update_not_saved(alias: &str, file: &GraphDir, reason: &str) -> Problem {
    Problem::new("registry_update_not_saved")
        .with("alias", Slot::name_or(alias, "(that alias)"))
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// This machine's graph index has no row for `alias`, or cannot be read (req-116).
pub fn graph_not_known(alias: &str, why: &str) -> Problem {
    Problem::new("graph_not_known")
        .with("alias", Slot::name_or(alias, "(that alias)"))
        .with("why", reason_slot(why))
}

/// ws006 S23 (graph-addressing-expert): the request named `former`, a former alias of the graph now named `live`, so
/// umx used `live`. A hint, once per pair: next is `retry`, the request as given with its former aliases read as live
/// ones (the daemon builds it, review R19), else `graph guide <live>`.
pub fn alias_redirected(former: &str, live: &str, retry: Option<Command>) -> Problem {
    let fix = match (retry, ValidName::alias(live)) {
        (Some(c), _) => c,
        (None, Some(l)) => Command::new("graph guide").name(&l),
        _ => Command::new("graph list"),
    };
    Problem::new("alias_redirected")
        .with("former", Slot::name_or(former, "(that alias)"))
        .with("live", Slot::name_or(live, "(the graph)"))
        .with("fix", Slot::Command(fix))
}

/// ws006 S23 (graph-addressing-expert): two loaded graphs (different ids) list `name` as a former alias, so umx will
/// not pick one. `n` counts them all; each candidate is a live alias an unnamed read may show (review R16), never a
/// secured graph's.
pub fn former_alias_ambiguous(name: &str, n: usize, live: Vec<String>) -> Problem {
    Problem::new("former_alias_ambiguous")
        .with("n", Slot::Count(n as u64))
        .with("alias", Slot::name_or(name, "(that alias)"))
        .with("fix", Slot::Command(Command::new("graph list")))
        .candidates(
            live.into_iter()
                .map(|alias| Candidate {
                    id: alias,
                    title: None,
                    score: None,
                })
                .collect(),
        )
}

/// `graph load <alias>` found the alias on rows for two or more folders (graph-addressing-expert, plan 098): each
/// candidate is a folder with the command that loads it.
pub fn alias_ambiguous(alias: &ValidName, rows: &[(GraphDir, Command)]) -> Problem {
    Problem::new("alias_ambiguous")
        .with("n", Slot::Count(rows.len() as u64))
        .with("alias", name(alias))
        .with(
            "fix",
            Slot::Command(Command::new("graph list").word("--all")),
        )
        .candidates(
            rows.iter()
                .map(|(dir, load)| Candidate {
                    id: SafeText::path(dir).map_or_else(|| "<path>".into(), |p| p.as_str().into()),
                    title: Some(load.as_str().into()),
                    score: None,
                })
                .collect(),
        )
}

pub fn external_end_unchecked(address: &str) -> Problem {
    Problem::new("external_end_unchecked").subject(address.to_string())
}

pub fn link_into_secured(address: &str) -> Problem {
    Problem::new("link_into_secured").subject(address.to_string())
}

/// F96: on load, type `kind`'s default columns name `column`, which is neither a standard node field nor one the
/// type declares.
pub fn type_column_unknown(graph: &ValidName, kind: &ValidName, column: &str) -> Problem {
    Problem::new("type_column_unknown")
        .with("graph", name(graph))
        .with("type", name(kind))
        .with("column", Slot::name_or(column, "?"))
}

pub fn own_graph_end(edge: Id, end: &'static str) -> Problem {
    Problem::new("own_graph_end")
        .with("id", Slot::Id(edge))
        .with("end", Slot::Word(end))
}

/// F113: `count` external ends in `graph` reach graphs the check did not name. A count only (security ruling a).
pub fn link_targets_unnamed(graph: &ValidName, count: usize) -> Problem {
    Problem::new("link_targets_unnamed")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
}

pub fn edge_broken(edge: Id, end: &'static str, graph: Id, node: Id) -> Problem {
    Problem::new("edge_broken")
        .with("id", Slot::Id(edge))
        .with("end", Slot::Word(end))
        .with("target graph", Slot::Id(graph))
        .with("node", Slot::Id(node))
}

pub fn graph_not_found(path: &GraphDir) -> Problem {
    let next = Command::new("graph new")
        .path(path)
        .word("--alias")
        .word("<alias>");
    Problem::new("graph_not_found")
        .with("path", Slot::path(path))
        .with("fix", Slot::Command(next))
}

pub fn graph_exists(path: &GraphDir) -> Problem {
    Problem::new("graph_exists").with("path", Slot::path(path))
}

/// `file` holds records but the folder has no graph.json.
pub fn graph_orphan_records(path: &GraphDir, file: GraphFile) -> Problem {
    Problem::new("graph_orphan_records")
        .with("path", Slot::path(path))
        .with("file", Slot::file(file))
}

pub fn graph_invalid(e: &DecodeError) -> Problem {
    Problem::new("graph_invalid")
        .with("file", Slot::file(e.file))
        .with("n", Slot::Count(e.line as u64))
        .with("reason", Slot::Decode(e.reason))
}

/// `file` is `size` bytes, over the `cap` umx reads or writes for its kind: refused unread, or not written.
pub fn graph_too_large(file: &FileRef, size: u64, cap: u64) -> Problem {
    Problem::new("graph_too_large")
        .with("file", Slot::File(file.clone()))
        .with("size", Slot::Count(size))
        .with("cap", Slot::Count(cap))
}

/// A node write refused because nodes.jsonl's image would pass 4 GiB; nothing changed.
pub fn image_full(e: &umx_graph::ImageFull) -> Problem {
    graph_too_large(&GraphFile::Nodes.into(), e.size, e.limit)
}

/// The file's `TooLarge` as `graph_too_large`, any other error as `other` makes it.
pub fn files_error(e: &FilesError, other: impl FnOnce(&FilesError) -> Problem) -> Problem {
    match e {
        FilesError::TooLarge { file, size, cap } => graph_too_large(file, *size, *cap),
        e => other(e),
    }
}

/// A file that does not decode, or holds more than umx can (`TooLarge`: `size` is the file's bytes).
pub fn decode_failed(e: &DecodeError, size: u64) -> Problem {
    match e.reason {
        umx_core::codec::DecodeReason::TooLarge => {
            let file = FileRef::from(e.file);
            graph_too_large(&file, size, file.cap())
        }
        _ => graph_invalid(e),
    }
}

/// `file` is out of id order or has blank lines, first at line `line` (workshop 008 section 5.1 rule 2).
pub fn unsorted_file(graph: &ValidName, file: GraphFile, line: usize) -> Problem {
    Problem::new("unsorted_file")
        .with("file", Slot::file(file))
        .with("n", Slot::Count(line as u64))
        .with("fix", check_command(graph))
}

/// graph.json at `path` is format `found`, newer than this build reads.
pub fn graph_format_newer(path: &GraphDir, found: u32) -> Problem {
    Problem::new("graph_format_newer")
        .with("path", Slot::path(path))
        .with("found", Slot::Count(found.into()))
        .with("supported", Slot::Count(FORMAT_VERSION.into()))
}

/// `umx check --graph <graph>`, the next of the load-rule warnings.
fn check_command(graph: &ValidName) -> Slot {
    Slot::Command(Command::new("check").word("--graph").name(graph))
}

/// An id on several lines of a file (req-67): the last is in use, every line is kept.
/// An entity key that is not `scheme:value` (ws008 E1). The key is never shown (it is the caller's text); `fixed`
/// is the scheme in snake_case when one can be made, for the `next`.
pub fn entity_key_invalid(reason: &'static str, fixed_scheme: Option<&str>) -> Problem {
    let fix = fixed_scheme.and_then(ValidName::alias).map_or_else(
        || Command::new("docs get").word("entity"),
        |scheme| {
            Command::new("entity key")
                .word("<node>")
                .word("add")
                .name_with(&scheme, ':', "<value>")
        },
    );
    Problem::new("entity_key_invalid")
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(fix))
}

/// An entity link, denial or the edge write behind one refused (ws008 E2, E2c): `reason` says why and `fix` what to
/// run instead; nothing is written.
pub fn entity_link_invalid(reason: &'static str, fix: Command) -> Problem {
    Problem::new("entity_link_invalid")
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(fix))
}

/// A join an entity read skipped because it would put a denied pair in one entity (ws008 E2c): what would have
/// joined (a key's scheme, never its value, or a link's id) and the denial (its graph and id). All of it is stored in
/// graphs the read covered; the next removes the denial.
pub fn entity_join_denied(
    joined: &Joined,
    denial: (&ValidName, Id),
    a: (&ValidName, Id),
    b: (&ValidName, Id),
) -> Problem {
    let (by, what) = match joined {
        Joined::Key(scheme) => ("an entity key with scheme", Slot::Name(scheme.clone())),
        Joined::Link(edge) => ("the same_as link", Slot::Id(*edge)),
    };
    Problem::new("entity_join_denied")
        .with("by", Slot::Word(by))
        .with("what", what)
        .with("denial", Slot::Id(denial.1))
        .with("graph", name(denial.0))
        .with(
            "fix",
            Slot::Command(
                Command::new("entity undeny")
                    .address(a.0, a.1)
                    .address(b.0, b.1),
            ),
        )
}

/// One node of an entity problem: the graph it is in (a covered one), its id and its type.
pub type EntityEnd<'a> = (&'a ValidName, Id, &'a str);

/// Two nodes of different types are linked as one entity (ws008 E2, `how` = "are linked as one entity"), or hold the
/// same entity key so never match (ws008 E2b G5, "hold the same entity key"). Either way nothing is refused; the
/// next retypes `b` to `a`'s type. Both are in graphs the request covered (d-20).
pub fn entity_type_mismatch(a: EntityEnd<'_>, b: EntityEnd<'_>, how: &'static str) -> Problem {
    let fix = Command::new("node edit").address(b.0, b.1).word("--type");
    let fix = match ValidName::new(a.2) {
        Some(kind) => fix.name(&kind),
        None => fix.word("<type>"),
    };
    Problem::new("entity_type_mismatch")
        .with("a", Slot::Id(a.1))
        .with("a type", Slot::name_or(a.2, "(one type)"))
        .with("a graph", name(a.0))
        .with("b", Slot::Id(b.1))
        .with("b type", Slot::name_or(b.2, "(another type)"))
        .with("b graph", name(b.0))
        .with("how", Slot::Word(how))
        .with("fix", Slot::Command(fix))
}

/// What joined a member to its entity, for [`entity_graph_twice`]: a key (its scheme) or a `same_as` link (its id).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Joined {
    Key(ValidName),
    Link(Id),
}

/// An entity holds `count` nodes of covered graph `graph`, `node` and `other` the two lowest (ws008 E2b G1, D69): a
/// real entity has one node per graph. Names what joined `other`: a key's scheme (never its value) or the link's id.
pub fn entity_graph_twice(
    graph: &ValidName,
    count: usize,
    node: Id,
    other: Id,
    joined: &Joined,
) -> Problem {
    let (by, what) = match joined {
        Joined::Key(scheme) => ("its entity key with scheme", Slot::Name(scheme.clone())),
        Joined::Link(edge) => ("the same_as link", Slot::Id(*edge)),
    };
    Problem::new("entity_graph_twice")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
        .with("node", Slot::Id(node))
        .with("other", Slot::Id(other))
        .with("by", Slot::Word(by))
        .with("what", what)
        .with(
            "fix",
            Slot::Command(Command::new("entity show").address(graph, other)),
        )
}

/// A key with two or more holders of type `kind` in covered graph `graph` joined nothing there (ws008 E2b G2). The
/// key's scheme is named, never its value.
pub fn entity_key_ignored(graph: &ValidName, kind: &str, scheme: &ValidName) -> Problem {
    Problem::new("entity_key_ignored")
        .with("graph", name(graph))
        .with("type", Slot::name_or(kind, "(that type)"))
        .with("scheme", name(scheme))
        .with(
            "fix",
            Slot::Command(
                Command::new("node list")
                    .word("--where")
                    .name_with(&ValidName::new("entity_key").expect("a name"), '=', "<key>")
                    .word("--graph")
                    .name(graph),
            ),
        )
}

/// `umx entity link` stored a link that joins `end` to an entity already holding `node` in `end`'s graph `graph`
/// (ws008 E2b G3): that entity now holds two nodes of one graph.
pub fn entity_link_overlap(graph: &ValidName, node: Id, end: Id) -> Problem {
    Problem::new("entity_link_overlap")
        .with("graph", name(graph))
        .with("node", Slot::Id(node))
        .with("end", Slot::Id(end))
        .with(
            "fix",
            Slot::Command(Command::new("entity show").address(graph, node)),
        )
}

/// An entity's members hold `count` types (ws008 E2b G4); `a` and `b` are the first members of the first two.
pub fn entity_types_mixed(count: usize, a: Id, a_kind: &str, b: Id, b_kind: &str) -> Problem {
    Problem::new("entity_types_mixed")
        .with("count", Slot::Count(count as u64))
        .with("a", Slot::Id(a))
        .with("a type", Slot::name_or(a_kind, "(one type)"))
        .with("b", Slot::Id(b))
        .with("b type", Slot::name_or(b_kind, "(another type)"))
}

/// A key `node` was given holds its own title (ws008 E2b F7): a name stored as a confirmed identity. Names the
/// scheme, never the value.
pub fn entity_key_is_title(node: Id, scheme: &ValidName) -> Problem {
    Problem::new("entity_key_is_title")
        .with("node", Slot::Id(node))
        .with("scheme", name(scheme))
        .subject(node.to_text())
}

/// Node `d.node` holds an entity key that the lower-id node `d.in_use` of the same type holds too (ws008 E1). The key
/// itself is graph text and is not shown; `entity key` lists it.
pub fn duplicate_key(graph: &ValidName, kind: &str, d: &umx_graph::DuplicateEntityKey) -> Problem {
    Problem::new("duplicate_key")
        .with("node", Slot::Id(d.node))
        .with("in use", Slot::Id(d.in_use))
        .with("type", Slot::name_or(kind, "(that type)"))
        .with("graph", name(graph))
        .with(
            "fix",
            Slot::Command(Command::new("node get").address(graph, d.node)),
        )
        .subject(d.node.to_text())
}

/// ws003 s08: a node add with on_key resolve whose entity keys name two nodes of its type in `graph`, `node` and
/// `other` (a key both hold, or keys held one by each); nothing is written. Names nodes by address, never a key value.
pub fn on_key_ambiguous(graph: &ValidName, kind: &str, node: Id, other: Id) -> Problem {
    Problem::new("on_key_ambiguous")
        .with("type", Slot::name_or(kind, "(that type)"))
        .with("graph", name(graph))
        .with("node", Slot::Id(node))
        .with("other", Slot::Id(other))
        .with(
            "fix",
            Slot::Command(
                Command::new("entity key")
                    .address(graph, other)
                    .word("remove")
                    .word("<key>"),
            ),
        )
        .subject(other.to_text())
}

/// What an on_key resolve kept on its node rather than overwrite (ws003 s08), for [`on_key_field_held`]'s fix.
pub enum Held {
    Title,
    Status,
    Note,
    Parent,
    Field(ValidName),
    /// A std graph's tags (ws008 t2b): changed with `node edit --tag`, never `--set`.
    Tags,
}

impl Held {
    fn name(&self) -> ValidName {
        let word = match self {
            Held::Title => "title",
            Held::Status => "status",
            Held::Note => "note",
            Held::Parent => "parent",
            Held::Tags => umx_core::tags::TAGS,
            Held::Field(field) => return field.clone(),
        };
        ValidName::alias(word).expect("snake_case")
    }
}

/// ws003 s08: node `node`, found by an on_key resolve, already holds `held` with other values; they were kept. Names
/// the parts, never their values. The fix changes the first by hand.
pub fn on_key_field_held(graph: &ValidName, node: Id, held: &[Held]) -> Problem {
    let fix = match held.first() {
        Some(Held::Title) => Command::new("node edit")
            .address(graph, node)
            .word("--title")
            .word("<title>"),
        Some(Held::Status) => Command::new("node status")
            .address(graph, node)
            .word("<status>")
            .word("--reason")
            .word("<why>"),
        Some(Held::Note) => Command::new("node note")
            .address(graph, node)
            .word("--append")
            .word("<text>"),
        Some(Held::Parent) => Command::new("edge list")
            .word("--from")
            .address(graph, node)
            .word("--type")
            .word("part_of"),
        Some(Held::Field(field)) => Command::new("node edit")
            .address(graph, node)
            .word("--set")
            .name_with(field, '=', "<value>"),
        Some(Held::Tags) => Command::new("node edit")
            .address(graph, node)
            .word("--tag")
            .word("<tag>"),
        None => Command::new("node get").address(graph, node),
    };
    Problem::new("on_key_field_held")
        .with("graph", name(graph))
        .with("node", Slot::Id(node))
        .with("fields", Slot::Names(held.iter().map(Held::name).collect()))
        .with("fix", Slot::Command(fix))
        .subject(node.to_text())
}

/// A loaded node's entity key not in stored form (ws008 E1); never shows the key.
pub fn entity_key_not_canonical(graph: &ValidName, node: Id) -> Problem {
    Problem::new("entity_key_not_canonical")
        .with("node", Slot::Id(node))
        .with("graph", name(graph))
        .with(
            "fix",
            Slot::Command(Command::new("node get").address(graph, node)),
        )
        .subject(node.to_text())
}

/// A loaded node's entity_key entry umx cannot use (ws008 E1); never shows the entry.
pub fn entity_key_unusable(graph: &ValidName, node: Id) -> Problem {
    Problem::new("entity_key_unusable")
        .with("node", Slot::Id(node))
        .with("graph", name(graph))
        .with(
            "fix",
            Slot::Command(Command::new("node get").address(graph, node)),
        )
        .subject(node.to_text())
}

/// A std tag refused (ws008 t2b, req-117): `reason` says why, `fix` is the command again with the tag in snake_case
/// (or the tags docs when none can be made). Never shows the tag as given.
pub fn tag_invalid(reason: &'static str, fix: Command) -> Problem {
    Problem::new("tag_invalid")
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(fix))
}

/// A node would hold more than [`umx_core::tags::MAX_TAGS`] tags (ws008 t2b); nothing is written.
pub fn tag_limit(count: usize, fix: Command) -> Problem {
    Problem::new("tag_limit")
        .with("count", Slot::Count(count as u64))
        .with("fix", Slot::Command(fix))
}

/// A std tag verb or filter on a graph that does not use std (ws008 t2b): there `tags` is an ordinary custom field.
pub fn tag_needs_std(graph: &ValidName) -> Problem {
    Problem::new("tag_needs_std")
        .with("graph", name(graph))
        .with(
            "fix",
            Slot::Command(
                Command::new("ontology adopt-std")
                    .word("--dry-run")
                    .word("--graph")
                    .name(graph),
            ),
        )
}

/// A tag in a facet with declared tags, while neither it nor `<facet>:*` is (ws008 t2b); written all the same.
/// `fix` declares it.
pub fn tag_off_vocabulary(graph: &ValidName, facet: &ValidName, fix: Command) -> Problem {
    Problem::new("tag_off_vocabulary")
        .with("facet", Slot::Name(facet.clone()))
        .with("graph", name(graph))
        .with("fix", Slot::Command(fix))
}

/// A loaded node's tags not in stored form (ws008 t2b); never shows them.
pub fn tag_not_canonical(graph: &ValidName, node: Id) -> Problem {
    Problem::new("tag_not_canonical")
        .with("node", Slot::Id(node))
        .with("graph", name(graph))
        .with(
            "fix",
            Slot::Command(Command::new("node get").address(graph, node)),
        )
        .subject(node.to_text())
}

/// A loaded node's tags entry umx cannot use (ws008 t2b); never shows it.
pub fn tag_unusable(graph: &ValidName, node: Id) -> Problem {
    Problem::new("tag_unusable")
        .with("node", Slot::Id(node))
        .with("graph", name(graph))
        .with(
            "fix",
            Slot::Command(Command::new("node get").address(graph, node)),
        )
        .subject(node.to_text())
}

/// A tag filter over several graphs read `count` that do not use std (ws008 t2b): nothing in them matched.
pub fn tag_filter_not_std(count: usize) -> Problem {
    Problem::new("tag_filter_not_std").with("count", Slot::Count(count as u64))
}

/// A tag filter left out `count` rows whose node the read cannot show (ws008 t2b, d-20): never tested, only counted,
/// so the warning carries the count alone.
pub fn tag_filter_untested(count: usize) -> Problem {
    Problem::new("tag_filter_untested").with("count", Slot::Count(count as u64))
}

pub fn duplicate_id(graph: &ValidName, r: &Repeat) -> Problem {
    let line = |l: Option<&usize>| Slot::Count(l.copied().unwrap_or(0) as u64);
    Problem::new("duplicate_id")
        .with("file", Slot::file(r.file))
        .with("n", Slot::Count(r.lines.len() as u64))
        .with("id", Slot::Id(r.id))
        .with("first", line(r.lines.first()))
        .with("last", line(r.lines.last()))
        .with("fix", check_command(graph))
        .subject(r.id.to_text())
}

pub fn provenance_key_invalid(graph: &ValidName, issue: &umx_graph::InvalidProvenance) -> Problem {
    Problem::new("provenance_key_invalid")
        .with("file", Slot::file(issue.file))
        .with("n", Slot::Count(issue.line as u64))
        .with("key", Slot::name_or(issue.key, "provenance"))
        .with("fix", check_command(graph))
        .subject(issue.record.to_text())
}

/// `edge delete` on an edge whose id has earlier lines kept in edges.jsonl (R196 B2): deleting the line in use would
/// bring an earlier one back on the next load, so it is refused until one line is left.
pub fn edge_repeated(graph: &ValidName, r: &Repeat) -> Problem {
    let line = |l: Option<&usize>| Slot::Count(l.copied().unwrap_or(0) as u64);
    Problem::new("edge_repeated")
        .with("id", Slot::Id(r.id))
        .with("n", Slot::Count(r.lines.len() as u64))
        .with("first", line(r.lines.first()))
        .with("last", line(r.lines.last()))
        .with("fix", check_command(graph))
        .subject(r.id.to_text())
}

/// Edge `edge`'s `end` (`from` or `to`) names `node`, which is not in this graph (req-76).
pub fn dangling_edge(graph: &ValidName, edge: Id, end: &'static str, node: Id) -> Problem {
    Problem::new("dangling_edge")
        .with("id", Slot::Id(edge))
        .with("end", Slot::Word(end))
        .with("node", Slot::Id(node))
        .with("fix", check_command(graph))
        .subject(edge.to_text())
}

/// Edge `edge` is the reverse of `in_use` on a symmetric relation (D49): one edge both ways, so `edge` is kept in the
/// file and left out of the graph.
pub fn symmetric_duplicate(graph: &ValidName, edge: Id, in_use: Id) -> Problem {
    Problem::new("symmetric_duplicate")
        .with("id", Slot::Id(edge))
        .with("in use", Slot::Id(in_use))
        .with("fix", check_command(graph))
        .subject(edge.to_text())
}

/// Two resource nodes hold one locator (ws006 S3, workshop 010 9a), as after a merge where both sides added the same
/// source: both load and stay; lookups use `in use`, the lower id.
pub fn duplicate_locator(graph: &ValidName, node: Id, in_use: Id) -> Problem {
    Problem::new("duplicate_locator")
        .with("id", Slot::Id(node))
        .with("in use", Slot::Id(in_use))
        .with("fix", check_command(graph))
        .subject(node.to_text())
}

/// A resource node's stored source is not in normal form (a hand edit); it is keyed as it normalises.
pub fn locator_not_normalised(graph: &ValidName, node: Id) -> Problem {
    Problem::new("locator_not_normalised")
        .with("id", Slot::Id(node))
        .with("fix", check_command(graph))
        .subject(node.to_text())
}

/// A node holds a custom field named `locator` that is not a locator (S241-1): kept and written back as it is.
pub fn locator_field_kept(graph: &ValidName, node: Id) -> Problem {
    Problem::new("locator_field_kept")
        .with("id", Slot::Id(node))
        .with("fix", check_command(graph))
        .subject(node.to_text())
}

/// A write that would set a locator name as a plain custom field (S241-1). Reason words have no `_`, `,` or `;`
/// (the message slot keeps only plain text).
pub fn locator_field_invalid(given: &str, reason: &'static str) -> Problem {
    Problem::new("locator_field_invalid")
        .with("name", Slot::name_or(given, "(that name)"))
        .with("reason", Slot::Word(reason))
}

/// Why a write may not set `name` as a custom field: a locator name is the resource locator.
pub fn locator_name(given: &str) -> Option<Problem> {
    umx_core::locator::LOCATOR_NAMES
        .contains(&given)
        .then(|| locator_field_invalid(given, "it names the resource locator"))
}

/// `node add --locator` met a resource node already holding it (ws006 S3b): that node is returned.
pub fn resource_exists(graph: &ValidName, node: Id) -> Problem {
    Problem::new("resource_exists")
        .with("id", Slot::Id(node))
        .with(
            "fix",
            Slot::Command(Command::new("resource show").address(graph, node)),
        )
        .subject(node.to_text())
}

/// Another loaded graph holds the same resource (by what it names, not by its text).
pub fn resource_elsewhere(other: &ValidName, node: Id) -> Problem {
    Problem::new("resource_elsewhere")
        .with("graph", name(other))
        .with("id", Slot::Id(node))
        .with(
            "fix",
            Slot::Command(Command::new("resource show").address(other, node)),
        )
        .subject(node.to_text())
}

/// `node edit --locator` names one another resource node holds.
pub fn locator_in_use(graph: &ValidName, holder: Id) -> Problem {
    Problem::new("locator_in_use")
        .with("id", Slot::Id(holder))
        .with(
            "fix",
            Slot::Command(Command::new("resource show").address(graph, holder)),
        )
}

/// A node's locator was re-pointed or removed: what cited the old source.
pub fn locator_changed(graph: &ValidName, node: Id, edges: usize, evidence: usize) -> Problem {
    Problem::new("locator_changed")
        .with("id", Slot::Id(node))
        .with("edges", Slot::Count(edges as u64))
        .with("evidence", Slot::Count(evidence as u64))
        .with(
            "fix",
            Slot::Command(Command::new("node get").address(graph, node)),
        )
        .subject(node.to_text())
}

pub fn not_a_resource(node: Id) -> Problem {
    Problem::new("not_a_resource").with("id", Slot::Id(node))
}

/// Locator text refused. Reasons are umx's own words, never the text (no `_`, `,` or `;`: the slot keeps plain text).
pub fn locator_invalid(reason: &'static str) -> Problem {
    Problem::new("locator_invalid").with("reason", Slot::Word(reason))
}

/// An absolute path or file: URL as a locator. `fix` is the command with the root: form to use.
pub fn locator_absolute(fix: Command) -> Problem {
    Problem::new("locator_absolute").with("fix", Slot::Command(fix))
}

/// `ontology add relation <r> --symmetric` on a relation that is directed by what it means (R3).
pub fn symmetric_refused(relation: &ValidName) -> Problem {
    Problem::new("symmetric_refused").with("relation", name(relation))
}

/// The load had `n` more load-rule warnings than it lists.
pub fn load_warnings_not_shown(graph: &ValidName, n: usize) -> Problem {
    Problem::new("load_warnings_not_shown")
        .with("n", Slot::Count(n as u64))
        .with("fix", check_command(graph))
}

/// A key in `file` that this build does not know, kept as it is. `path` runs from the top of the file; a part that
/// is not snake_case shows as `?`, since keys are graph text.
pub fn unknown_key(graph: &ValidName, file: GraphFile, path: &[&str]) -> Problem {
    let shown: Vec<&str> = path
        .iter()
        .map(|k| if is_snake_case(k) { *k } else { "?" })
        .collect();
    Problem::new("unknown_key")
        .with("file", Slot::file(file))
        .with("key", Slot::Path(SafeText::lossy(&shown.join("."))))
        .with("graph", name(graph))
}

/// The graph's vectors are in `graph_key`'s space; the configured embedder writes `embedder_key`'s (D32, req-75).
pub fn embedder_mismatch(
    graph: &ValidName,
    graph_key: &ModelKey,
    embedder_key: &ModelKey,
) -> Problem {
    Problem::new("embedder_mismatch")
        .with("graph", name(graph))
        .with("graph key", Slot::ModelKey(graph_key.clone()))
        .with("embedder key", Slot::ModelKey(embedder_key.clone()))
}

/// `umx embeddings probe`, with `--embedder <name>` when one was named, so the fix re-runs the same probe.
fn probe_again(embedder: Option<&str>) -> Command {
    match embedder.and_then(ValidName::alias) {
        Some(n) => Command::new("embeddings probe").word("--embedder").name(&n),
        None => Command::new("embeddings probe"),
    }
}

fn embedder_slot(name: &str) -> Slot {
    Slot::name_or(name, "(that instance)")
}

/// The embedder cannot be used now: no credentials (no az login token), offline, rate limited, or a secured graph
/// meeting a hosted instance. `reason` and `hint` are the adapter's words, made safe; they may name the endpoint.
pub fn embedder_unavailable(
    instance: &str,
    requested: Option<&str>,
    reason: &str,
    hint: &str,
) -> Problem {
    Problem::new("embedder_unavailable")
        .with("embedder", embedder_slot(instance))
        .with("reason", Slot::Os(SafeText::lossy(reason)))
        .with("hint", Slot::Os(SafeText::lossy(hint)))
        .with("fix", Slot::Command(probe_again(requested)))
}

/// The instance's config is incomplete or wrong: it was not built, or the provider refused or answered it as
/// configured (a deployment serving another model or width).
pub fn embedder_invalid(instance: &str, reason: &str) -> Problem {
    Problem::new("embedder_invalid")
        .with("embedder", embedder_slot(instance))
        .with("reason", Slot::Os(SafeText::lossy(reason)))
}

/// The code for an [`EmbedError`] from `instance`. No variant carries input text, keys or tokens.
pub fn embed_error(instance: &str, requested: Option<&str>, e: &EmbedError) -> Problem {
    match e {
        EmbedError::Unavailable { reason, next } => {
            embedder_unavailable(instance, requested, reason, next)
        }
        EmbedError::RateLimited { retry_after_ms } => {
            let reason = match retry_after_ms {
                Some(ms) => alloc::format!("the provider is rate limiting (retry after {ms} ms)"),
                None => "the provider is rate limiting".to_string(),
            };
            embedder_unavailable(
                instance,
                requested,
                &reason,
                "wait a little and run it again",
            )
        }
        other => embedder_invalid(
            instance,
            &match other {
                EmbedError::Config(m) => m.clone(),
                e => e.to_string(),
            },
        ),
    }
}

/// A secured graph's folder still holds `shards` (paths under the graph folder) from when it was open. The paths
/// go in candidates (untrusted), the count in the message.
pub fn secured_shards_remain(graph: &ValidName, shards: &[String]) -> Problem {
    Problem::new("secured_shards_remain")
        .with("graph", name(graph))
        .with("count", Slot::Count(shards.len() as u64))
        .candidates(
            shards
                .iter()
                .map(|s| Candidate {
                    id: s.clone(),
                    title: None,
                    score: None,
                })
                .collect(),
        )
}

/// A secured graph's folder could not be listed for shards from when it was open: whether any remain is unknown.
pub fn secured_shards_unchecked(graph: &ValidName, e: &FilesError) -> Problem {
    Problem::new("secured_shards_unchecked")
        .with("graph", name(graph))
        .with("os error", Slot::os(e))
}

/// Why this machine's policy record failed, in umx's words or the OS's. Error text keeps no parentheses, so
/// "x (do y)" reads "x: do y" rather than losing them to `?`.
fn reason_slot(reason: &str) -> Slot {
    Slot::Os(SafeText::lossy(
        &reason.replace(" (", ": ").replace(')', ""),
    ))
}

/// graph.json now says a lower policy than this machine's record for the graph, lowered outside umx (an edit, a
/// pull, a deleted key or section); the graph stays at the recorded policy until a umx call confirms it (D83).
pub fn policy_downgrade_unconfirmed(graph: &ValidName, recorded: Policy, file: Policy) -> Problem {
    Problem::new("policy_downgrade_unconfirmed")
        .with("graph", name(graph))
        .with("recorded", Slot::Word(recorded.name()))
        .with("file", Slot::Word(file.name()))
}

/// This machine's policy record is refused or does not parse: every graph counts as secured until it is fixed.
pub fn local_state_unreadable(file: &GraphDir, reason: &str) -> Problem {
    Problem::new("local_state_unreadable")
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// Recording a graph's secured policy on this machine failed. The daemon holds it secured in memory, retries on
/// every later write, and refuses to lower graph.json's policy until it is recorded.
pub fn local_state_not_saved(graph: &ValidName, file: &GraphDir, reason: &str) -> Problem {
    Problem::new("local_state_not_saved")
        .with("graph", name(graph))
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// This machine's record was replaced, but the folder sync after it failed: it may not survive a crash.
pub fn local_state_not_durable(file: &GraphDir, reason: &str) -> Problem {
    Problem::new("local_state_not_durable")
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// What loosened between two effective policies, in umx's words: the visibility, the embedder rule, or both.
/// None when nothing did.
fn loosened(from: Policy, to: Policy) -> Option<&'static str> {
    use umx_core::{EmbedderRule as E, Visibility as V};
    let visibility = (from.visibility, to.visibility) == (V::Named, V::Open);
    Some(match ((from.embedder, to.embedder), visibility) {
        ((E::Local, E::Trusted), false) => "embedder rule local to trusted",
        ((E::Local, E::Any), false) => "embedder rule local to any",
        ((E::Trusted, E::Any), false) => "embedder rule trusted to any",
        ((E::Local, E::Trusted), true) => {
            "visibility named to open and embedder rule local to trusted"
        }
        ((E::Local, E::Any), true) => "visibility named to open and embedder rule local to any",
        ((E::Trusted, E::Any), true) => "visibility named to open and embedder rule trusted to any",
        (_, true) => "visibility named to open",
        (_, false) => return None,
    })
}

/// D83: a umx call lowered the effective policy of graph `graph` from `from` to `to` (both the stricter of graph.json
/// and this machine's record), and the record now holds it. A warning, never a block: it names what loosened, the
/// embedder instances of this machine that may now get the graph's text and did not before (`receivers`, after the
/// folder rule; empty: none), when it was recorded, and, when `to` is open, that new vectors go in the graph's own
/// folder. None when nothing loosened.
pub fn policy_lowered(
    graph: &ValidName,
    from: Policy,
    to: Policy,
    receivers: Vec<umx_core::problem::Receiver>,
    at: Option<&str>,
) -> Option<Problem> {
    let what = loosened(from, to)?;
    let receivers = if receivers.is_empty() {
        Slot::Word("no embedder instance of this machine")
    } else {
        Slot::Receivers(receivers)
    };
    let store = if to == Policy::OPEN {
        " New vectors now go in the graph folder itself (git tracks them if that folder is in a repository)."
    } else {
        ""
    };
    Some(
        Problem::new("policy_lowered")
            .with("graph", name(graph))
            .with("from", Slot::Word(from.name()))
            .with("to", Slot::Word(to.name()))
            .with("loosened", Slot::Word(what))
            .with("receivers", receivers)
            .with(
                "at",
                at.map_or(Slot::Word("the time of this call"), |t| {
                    Slot::Os(SafeText::lossy(t))
                }),
            )
            .with("store", Slot::Word(store)),
    )
}

/// D83: `graph set` wrote graph.json's lower policy but could not record it on this machine, so the graph stays at
/// its recorded policy (a failed record never turns into an unrecorded downgrade) and nothing new gets its text.
pub fn policy_not_lowered(graph: &ValidName, file: &GraphDir, reason: &str) -> Problem {
    Problem::new("policy_not_lowered")
        .with("graph", name(graph))
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// `graph confirm-policy`, or `graph set` about to lower a policy, could not read or write this machine's record;
/// nothing changed.
pub fn policy_not_recorded(file: &GraphDir, reason: &str) -> Problem {
    Problem::new("policy_not_recorded")
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

/// `fix` builds the same command with a good alias.
pub fn alias_invalid(alias: &str, fix: &dyn Fn(&ValidName) -> Command) -> Problem {
    let good = suggest_name(alias).map(|s| {
        s.strip_suffix("_item")
            .map(alloc::string::String::from)
            .unwrap_or(s)
    });
    let cmd = good
        .as_deref()
        .and_then(ValidName::alias)
        .map_or_else(|| Command::new("docs get").word("graph"), |g| fix(&g));
    Problem::new("alias_invalid")
        .with("alias", Slot::Word("(that alias)"))
        .with("fix", Slot::Command(cmd))
}

/// `cmd` is `umx graph new <path> --alias <alias>_2` or `umx graph load <path> --as <alias>_2`.
pub fn alias_in_use(alias: &ValidName, cmd: Command) -> Problem {
    Problem::new("alias_in_use")
        .with("alias", name(alias))
        .with("fix", Slot::Command(cmd))
}

/// Names the roots (config paths, sanitised like any path) so the agent sees where graphs may live.
pub fn path_outside_roots(path: &GraphDir, roots: &[GraphDir]) -> Problem {
    let roots = match roots {
        [] => Slot::Word("none"),
        roots => {
            let shown: Vec<String> = roots
                .iter()
                .map(|r| SafeText::path(r).map_or_else(|| "<path>".into(), |t| t.as_str().into()))
                .collect();
            Slot::Path(SafeText::lossy(&shown.join(" and ")))
        }
    };
    Problem::new("path_outside_roots")
        .with("path", Slot::path(path))
        .with("roots", roots)
}

/// The daemon fills `<cmd>` with the command's label.
/// `next` is `umx graph set <graph>` with the first setting and its placeholder, never the value given.
pub fn setting_unknown(given: &str, graph: &ValidName) -> Problem {
    let first = &SETTINGS[0];
    Problem::new("setting_unknown")
        .with("setting", Slot::name_or(given, "(that setting)"))
        .with("settings", settings::names())
        .with(
            "fix",
            Slot::Command(
                Command::new("graph set")
                    .name(graph)
                    .word(first.flag)
                    .word(first.placeholder),
            ),
        )
}

/// A search used no vectors for `graph` (or, with None, for any graph searched), for `cause`: its results there are
/// word matches only (req-53), never an empty "nothing similar".
pub fn similarity_unavailable(graph: Option<&ValidName>, cause: &'static str) -> Problem {
    let scope = graph.map_or(Slot::Word("every graph searched"), name);
    Problem::new("similarity_unavailable")
        .with("scope", scope)
        .with("cause", Slot::Word(cause))
}

/// A search reached `graph`, whose embedder `setting` admits none of this machine's instances: it was searched by
/// words only and its query never sent. It points to adding a local instance; allowing hosted ones is the graph
/// owner's decision, so no command that lowers the policy is printed (D83, security (c)). With no showable name
/// (None), the graph is never named: the hint says "a graph searched", so the reply is never silent.
pub fn similarity_no_embedder(
    graph: Option<&ValidName>,
    setting: umx_core::EmbedderRule,
) -> Problem {
    Problem::new("similarity_no_embedder")
        .with("graph", graph.map_or(Slot::Word("a graph searched"), name))
        .with("setting", Slot::Word(setting.as_str()))
}

/// `n` nodes searched have no whole-node vector yet (pending or failed), so only words can find them.
pub fn similarity_incomplete(n: usize, held: usize) -> Problem {
    Problem::new("similarity_incomplete")
        .with("n", Slot::Count(n as u64))
        .with("held", Slot::Count(held as u64))
}

/// Query word `word` starts `n` words, more than word search's prefix cap: only the `cap` most common were
/// searched as prefix matches (req-92).
pub fn word_prefix_capped(word: &str, n: usize, cap: usize) -> Problem {
    Problem::new("word_prefix_capped")
        .with(
            "word",
            // Only lower-case ASCII words are prefix-matched, so this always passes.
            SafeText::key(word).map_or(Slot::Word("a search word"), Slot::Key),
        )
        .with("n", Slot::Count(n as u64))
        .with("cap", Slot::Count(cap as u64))
}

/// An unnamed search left out `n` secured graphs (SEC-5, blueprint d-20): the count only, never their aliases.
fn alias_candidates(aliases: &[String]) -> Vec<Candidate> {
    aliases
        .iter()
        .map(|a| Candidate {
            id: a.clone(),
            title: None,
            score: None,
        })
        .collect()
}

/// A read with no graph named took one copy of a graph loaded more than once (its primary); the other copies,
/// named as candidates, were left out (d-21). Never a secured graph's copies: d-20 skips those first.
pub fn copies_skipped(others: &[String]) -> Problem {
    Problem::new("copies_skipped")
        .with("n", Slot::Count(others.len() as u64))
        .candidates(alias_candidates(others))
}

/// `--graph` named two or more copies of one graph: their rows are unioned (d-21).
pub fn graph_twice(copies: &[String]) -> Problem {
    Problem::new("query_graph_twice")
        .with("n", Slot::Count(copies.len() as u64))
        .candidates(alias_candidates(copies))
}

/// A graph loaded as two or more copies lost its primary (it was unloaded), so a read with no graph named cannot
/// choose (d-21): name a copy, as `retry` with `--graph <first>`.
/// Copies of one graph id with none primary (d-21). The next makes the first candidate primary (F98); naming a copy
/// with --graph also works, as the message says.
pub fn copy_ambiguous(copies: &[String]) -> Problem {
    let fix = match copies.first().and_then(|c| ValidName::alias(c)) {
        Some(first) => Command::new("graph primary").name(&first),
        None => Command::new("graph list"),
    };
    Problem::new("copy_ambiguous")
        .with("n", Slot::Count(copies.len() as u64))
        .with("fix", Slot::Command(fix))
        .candidates(alias_candidates(copies))
}

/// F98: the new primary's effective policy is not open. The next is a placeholder, never another copy's alias.
pub fn primary_is_secured() -> Problem {
    Problem::new("primary_is_secured").with(
        "fix",
        Slot::Command(Command::new("graph primary").word("<another copy>")),
    )
}

/// `umx query` found every query permit taken (graph-storage-expert F2); nothing ran.
pub fn query_busy() -> Problem {
    Problem::new("query_busy")
}

/// PRD 002 search-13: a search found `found` matches and returned `shown` (its `--limit`). `fix` repeats the search with
/// a limit that returns them all, on the graphs the caller named (never one it did not).
pub fn search_results_cut(shown: usize, found: usize, fix: Command) -> Problem {
    Problem::new("search_results_cut")
        .with("shown", Slot::Count(shown as u64))
        .with("found", Slot::Count(found as u64))
        .with("fix", Slot::Command(fix))
}

pub fn secured_graphs_skipped(n: usize) -> Problem {
    Problem::new("secured_graphs_skipped")
        .with("n", Slot::Count(n as u64))
        .with(
            "fix",
            Slot::Command(
                Command::new("search")
                    .word("<words>")
                    .word("--graph")
                    .word("<name>"),
            ),
        )
}

pub fn secured_graphs_read(aliases: &[String]) -> Problem {
    Problem::new("secured_graphs_read")
        .with("n", Slot::Count(aliases.len() as u64))
        .candidates(alias_candidates(aliases))
}

pub fn usage_invalid(what: &'static str, noun: &'static str) -> Problem {
    Problem::new("usage_invalid")
        .with("what", Slot::Word(what))
        .with("noun", Slot::Word(noun))
}

pub fn node_not_found(given: &str, alias: &ValidName, nearest: Option<&Id>) -> Problem {
    let next = match nearest {
        Some(id) => Command::new("node get").address(alias, *id),
        None => Command::new("node list").word("--graph").name(alias),
    };
    Problem::new("node_not_found")
        .with("id", Slot::id_text_or(given, "(that id)"))
        .with("alias", name(alias))
        .with("fix", Slot::Command(next))
}

pub fn id_ambiguous(graph: &ValidName, prefix: &IdText, ids: &[Id]) -> Problem {
    Problem::new("id_ambiguous")
        .with("graph", name(graph))
        .with("prefix", Slot::IdText(prefix.clone()))
        .with("n", Slot::Count(ids.len() as u64))
        .with("first candidate id", Slot::Id(ids[0]))
}

/// The most keys `who_ambiguous` lists; the message counts them all.
pub const WHO_CANDIDATES: usize = 20;

/// `umx who <bare id>`: `keys` (sorted, distinct) are every agent or session prov_key in the `graphs` read whose last
/// segment is the id. `secured` counts the graphs d-20 left out, never which (R5).
pub fn who_ambiguous(keys: &[String], graphs: usize, secured: usize) -> Problem {
    Problem::new("who_ambiguous")
        .with("n", Slot::Count(keys.len() as u64))
        .with("graphs", Slot::Count(graphs as u64))
        .with("secured", Slot::Count(secured as u64))
        .candidates(
            keys.iter()
                .take(WHO_CANDIDATES)
                .map(|key| Candidate {
                    id: key.clone(),
                    title: None,
                    score: None,
                })
                .collect(),
        )
}

/// `umx who`: no agent or session in the `graphs` read matches; `secured` as for [`who_ambiguous`].
pub fn who_unknown(graphs: usize, secured: usize) -> Problem {
    Problem::new("who_unknown")
        .with("graphs", Slot::Count(graphs as u64))
        .with("secured", Slot::Count(secured as u64))
}

/// `what` is the kind of name ("node type", "status", "relation", "field name"); `fix` builds the command again
/// with the suggested name.
pub fn name_invalid(
    given: &str,
    what: &'static str,
    fix: &dyn Fn(&ValidName) -> Command,
) -> Problem {
    let reason = if umx_core::name::is_snake_case(given) {
        "it is a reserved word"
    } else {
        "names are snake_case ([a-z][a-z0-9_]*)"
    };
    let cmd = suggest_name(given)
        .as_deref()
        .and_then(ValidName::new)
        .map_or_else(
            || Command::new("docs get").word("ontology"),
            |good| fix(&good),
        );
    Problem::new("name_invalid")
        .with("name", Slot::name_or(given, "(that name)"))
        .with("kind of name", Slot::Word(what))
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(cmd))
}

/// A source root name that is not snake_case, is over 32 characters, or is `repo` (built in).
pub fn root_name_invalid(given: &str) -> Problem {
    let reason = if given == umx_core::config::REPO_ROOT {
        "repo is built in as root:repo (the git folder holding the graph)"
    } else if is_snake_case(given) {
        "it is longer than 32 characters"
    } else {
        "names are snake_case ([a-z][a-z0-9_]*)"
    };
    let fix = suggest_name(given)
        .as_deref()
        .and_then(ValidName::new)
        .filter(|g| umx_core::config::is_root_name(g.as_str()))
        .map_or_else(
            || Command::new("root add").word("<name>").word("<folder>"),
            |good| Command::new("root add").name(&good).word("<folder>"),
        );
    Problem::new("name_invalid")
        .with("name", Slot::name_or(given, "(that name)"))
        .with("kind of name", Slot::Word("source root name"))
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(fix))
}

/// `similar_to` is umx's own relation (workshop 008 section 9): kept from the vectors, never added by hand
/// (the project manager and the cypher-expert, ws004 s09). Read it with `umx neighbours <node> --relation similar_to`.
pub fn similar_to_reserved() -> Problem {
    Problem::new("name_invalid")
        .with("name", Slot::name_or("similar_to", "(that name)"))
        .with("kind of name", Slot::Word("relation"))
        .with(
            "reason",
            Slot::Word(
                "umx keeps similar_to edges itself from the vectors (read them and never add them)",
            ),
        )
        .with(
            "fix",
            Slot::Command(
                Command::new("neighbours")
                    .word("<node>")
                    .word("--relation")
                    .word("similar_to"),
            ),
        )
}

/// `authored_by` and `edited_by` are read from node provenance (ws007) and never stored, so no edge is added or
/// retyped into one (R196 B1). `kind` is one of DERIVED_RELATIONS.
pub fn derived_relation_reserved(kind: &'static str) -> Problem {
    Problem::new("name_invalid")
        .with("name", Slot::name_or(kind, "(that name)"))
        .with("kind of name", Slot::Word("relation"))
        .with(
            "reason",
            Slot::Word(
                "umx derives this relation from node provenance and never stores it (read it)",
            ),
        )
        .with(
            "fix",
            Slot::Command(
                Command::new("neighbours")
                    .word("<node>")
                    .word("--relation")
                    .word(kind),
            ),
        )
}

pub fn reserved_name(given: &str) -> Problem {
    reserved_name_because(given, "it is a standard field name")
}

/// `reserved_name` with why umx keeps the name (ws008 T2b): `std` itself, a provenance type.
pub fn reserved_name_because(given: &str, why: &'static str) -> Problem {
    Problem::new("reserved_name")
        .with("name", Slot::name_or(given, "(that name)"))
        .with("why", Slot::Word(why))
}

/// `std` as a type, relation or field name, in every graph (ws008 T2b, req-99).
pub fn std_reserved() -> Problem {
    reserved_name_because(
        umx_core::standard::STD,
        "it qualifies umx's standard vocabulary (std:person)",
    )
}

/// A provenance type declared or given by hand (ws008 T2b): umx alone makes those records.
pub fn provenance_type_reserved(kind: &str) -> Problem {
    reserved_name_because(kind, "umx records provenance with that type")
}

/// ws008 T2c (req-100): `ontology remove field` names a field std gives the type or relation in a std graph.
pub fn std_field_kept(field: &ValidName) -> Problem {
    reserved_name_because(
        field.as_str(),
        "it is a std field: a graph adds fields to a std type but never removes std's own",
    )
}

/// ws008 T2b (req-99): a std graph declares a field std has, with another kind than std's.
pub fn std_field_conflict(graph: &ValidName, field: &ValidName, std_kind: FieldKind) -> Problem {
    Problem::new("std_field_conflict")
        .with("graph", name(graph))
        .with("field", name(field))
        .with("kind", Slot::Word(std_kind.as_str()))
}

/// ws008 T3 (req-101, F285): `ontology adopt-std` finds a field declared with another kind than std's. `fix` removes
/// the graph's declaration (refused with `ontology_in_use` while nodes hold the field, whose next lists them).
/// `relation`: the field is declared on a relation's edges. `fix` lists the holders while any is left, else removes
/// the declaration.
pub fn adopt_field_kind_clash(
    graph: &ValidName,
    (on, relation): (&str, bool),
    field: &ValidName,
    (kind, std_kind): (FieldKind, FieldKind),
    fix: Command,
) -> Problem {
    Problem::new("adopt_field_kind_clash")
        .with("graph", name(graph))
        .with("on", Slot::name_or(on, "(a type)"))
        .with("field", name(field))
        .with("kind", Slot::Word(kind.as_str()))
        .with("std kind", Slot::Word(std_kind.as_str()))
        .with(
            "holders",
            Slot::Word(if relation { "edges" } else { "nodes" }),
        )
        .with("fix", Slot::Command(fix))
}

/// F285: a std graph's graph.json declares a field with another kind than std's; it loads, std's kind is used.
pub fn std_field_conflict_stored(
    graph: &ValidName,
    on: &str,
    field: &ValidName,
    (kind, std_kind): (FieldKind, FieldKind),
    fix: Command,
) -> Problem {
    Problem::new("std_field_conflict_stored")
        .with("graph", name(graph))
        .with("on", Slot::name_or(on, "(a type)"))
        .with("field", name(field))
        .with("kind", Slot::Word(kind.as_str()))
        .with("std kind", Slot::Word(std_kind.as_str()))
        .with("fix", Slot::Command(fix))
}

/// ws008 T3 (req-101): the graph stores `count` edges of `relation`, which a std graph reserves or derives.
pub fn adopt_reserved_relation(graph: &ValidName, relation: &ValidName, count: usize) -> Problem {
    Problem::new("adopt_reserved_relation")
        .with("graph", name(graph))
        .with("relation", name(relation))
        .with("count", Slot::Count(count as u64))
        .with(
            "fix",
            Slot::Command(
                Command::new("edge list")
                    .word("--type")
                    .name(relation)
                    .word("--graph")
                    .name(graph),
            ),
        )
}

/// `umx ontology show --graph <graph>`: what a `--map` problem's next shows.
fn show_ontology(graph: &ValidName) -> Slot {
    Slot::Command(Command::new("ontology show").word("--graph").name(graph))
}

pub fn adopt_map_unknown(graph: &ValidName, from: &ValidName) -> Problem {
    Problem::new("adopt_map_unknown")
        .with("graph", name(graph))
        .with("from", name(from))
        .with("fix", show_ontology(graph))
}

pub fn adopt_map_ambiguous(graph: &ValidName, from: &ValidName) -> Problem {
    Problem::new("adopt_map_ambiguous")
        .with("graph", name(graph))
        .with("from", name(from))
        .with("fix", show_ontology(graph))
}

/// `item` is a field name, `columns`, `description` or a key graph.json holds that this build does not know.
pub fn adopt_map_merge(
    graph: &ValidName,
    what: &'static str,
    (from, to): (&ValidName, &ValidName),
    item: &str,
) -> Problem {
    Problem::new("adopt_map_merge")
        .with("graph", name(graph))
        .with("what", Slot::Word(what))
        .with("from", name(from))
        .with("to", name(to))
        .with("item", Slot::name_or(item, "(a key)"))
        .with("fix", show_ontology(graph))
}

pub fn adopt_map_symmetry(graph: &ValidName, from: &ValidName, to: &ValidName) -> Problem {
    Problem::new("adopt_map_symmetry")
        .with("from", name(from))
        .with("to", name(to))
        .with("fix", show_ontology(graph))
}

/// Edge `id`, renamed, would have the ends and relation edge `other` has.
pub fn adopt_map_duplicate_edge(
    graph: &ValidName,
    (from, to): (&ValidName, &ValidName),
    id: Id,
    other: Id,
) -> Problem {
    Problem::new("adopt_map_duplicate_edge")
        .with("from", name(from))
        .with("to", name(to))
        .with("id", Slot::Id(id))
        .with("other", Slot::Id(other))
        .with(
            "fix",
            Slot::Command(Command::new("edge show").address(graph, id)),
        )
        .subject(id.to_text())
}

/// `count` nodes of type `on` (edges of relation `on` when `edges`) hold std field `field` with a value that is not
/// std's `std_kind`; `first` is one of them.
pub fn adopt_value_kind(
    graph: &ValidName,
    (on, edges): (&ValidName, bool),
    field: &ValidName,
    std_kind: FieldKind,
    (count, first): (usize, Id),
) -> Problem {
    let show = if edges {
        Command::new("edge show")
    } else {
        Command::new("node get")
    };
    Problem::new("adopt_value_kind")
        .with("count", Slot::Count(count as u64))
        .with("on", name(on))
        .with("records", Slot::Word(if edges { "edges" } else { "nodes" }))
        .with("field", name(field))
        .with("kind", Slot::Word(std_kind.as_str()))
        .with("fix", Slot::Command(show.address(graph, first)))
}

/// `--field-to-tag <field>` cannot move `field` (ws008 t2b): `fix` shows why.
pub fn adopt_field_to_tag_refused(
    graph: &ValidName,
    field: &str,
    reason: &'static str,
    fix: Command,
) -> Problem {
    Problem::new("adopt_field_to_tag_refused")
        .with("graph", name(graph))
        .with("field", Slot::name_or(field, "(a field)"))
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(fix))
}

/// `count` values of `field` stay in it for `reason` (ws008 t2b); `listed` are the first node ids and values (a
/// secret-shaped value masked), as candidates: graph data, never in the message.
pub fn adopt_tag_value_invalid(
    graph: &ValidName,
    field: &ValidName,
    reason: &'static str,
    count: usize,
    listed: Vec<umx_core::problem::Candidate>,
) -> Problem {
    Problem::new("adopt_tag_value_invalid")
        .with("graph", name(graph))
        .with("field", name(field))
        .with("reason", Slot::Word(reason))
        .with("count", Slot::Count(count as u64))
        .with(
            "fix",
            Slot::Command(
                Command::new("node list")
                    .word("--columns")
                    .names(
                        &[ValidName::alias("id").expect("a name"), field.clone()],
                        ",",
                    )
                    .word("--graph")
                    .name(graph),
            ),
        )
        .candidates(listed)
}

pub fn already_std(graph: &ValidName, std: u32) -> Problem {
    Problem::new("already_std")
        .with("graph", name(graph))
        .with("std", Slot::Count(u64::from(std)))
        .with("fix", show_ontology(graph))
}

/// `--dry-run` offers `--map <from>=<to>`; never applied.
pub fn adopt_rename_suggested(
    graph: &ValidName,
    what: &'static str,
    from: &ValidName,
    to: &ValidName,
) -> Problem {
    Problem::new("adopt_rename_suggested")
        .with("what", Slot::Word(what))
        .with("from", name(from))
        .with("to", name(to))
        .with(
            "fix",
            Slot::Command(
                Command::new("ontology adopt-std")
                    .word("--map")
                    .names(&[from.clone(), to.clone()], "=")
                    .word("--dry-run")
                    .word("--graph")
                    .name(graph),
            ),
        )
}

/// ws008 T2b (req-99): a std graph's edges.jsonl holds an edge of a relation umx derives or keeps for provenance
/// between non-provenance nodes, as an older umx could write. Kept and loaded as written.
pub fn reserved_relation_stored(relation: &ValidName, id: Id) -> Problem {
    Problem::new("reserved_relation_stored")
        .with("relation", name(relation))
        .with("id", Slot::Id(id))
        .with("fix", Slot::Command(Command::new("edge show").id(id)))
        .subject(id.to_text())
}

/// `next` shows the ontology; [`already_declared_describe`] is the case where a description came with it.
pub fn already_declared(graph: &ValidName, what: &'static str, n: &ValidName) -> Problem {
    let fix = Command::new("ontology show").word("--graph").name(graph);
    Problem::new("already_declared")
        .with("graph", name(graph))
        .with("kind of name", Slot::Word(what))
        .with("name", name(n))
        .with("fix", Slot::Command(fix))
}

/// `ontology add ... --description` on a name already declared: `next` sets the description on it instead (plan
/// 079). `field` is the field of type `n`, for a field.
pub fn already_declared_describe(
    graph: &ValidName,
    kind: Described,
    n: &ValidName,
    field: Option<&ValidName>,
) -> Problem {
    let fix = Command::new("graph set")
        .name(graph)
        .word("--description")
        .described(kind, n, field);
    Problem::new("already_declared")
        .with("graph", name(graph))
        .with("kind of name", Slot::Word(kind.as_str()))
        .with("name", name(field.unwrap_or(n)))
        .with("fix", Slot::Command(fix))
}

/// `umx graph guide` on a graph with no purpose.
pub fn purpose_missing(graph: &ValidName) -> Problem {
    let fix = Command::new("graph set")
        .name(graph)
        .word("--purpose")
        .word("<text>");
    Problem::new("purpose_missing")
        .with("graph", name(graph))
        .with("fix", Slot::Command(fix))
}

/// `umx graph guide`: `n` entries have no description; `fix` adds the first one's.
pub fn descriptions_missing(graph: &ValidName, n: usize, fix: Command) -> Problem {
    Problem::new("descriptions_missing")
        .with("graph", name(graph))
        .with("n", Slot::Count(n as u64))
        .with("fix", Slot::Command(fix))
}

/// `ontology add field` on several types or `--all-types` (F71) names a type the graph does not declare; nothing
/// changed.
pub fn type_unknown(graph: &ValidName, kind: &ValidName) -> Problem {
    Problem::new("type_unknown")
        .with("graph", name(graph))
        .with("type", name(kind))
}

/// `ontology add field` or `add type` (F71): `kind` already has the field with another kind; nothing changed.
pub fn field_conflict(
    graph: &ValidName,
    kind: &ValidName,
    field: &ValidName,
    has: FieldKind,
) -> Problem {
    Problem::new("field_conflict")
        .with("graph", name(graph))
        .with("type", name(kind))
        .with("field", name(field))
        .with("kind", Slot::Word(has.as_str()))
}

/// `ontology add field` names a field `kind` already has with the same kind but another role (D56 with F71): an
/// identical add is unchanged, so one that differs in role is refused, never a silent role change.
pub fn field_role_conflict(
    graph: &ValidName,
    kind: &ValidName,
    field: &ValidName,
    has: Role,
) -> Problem {
    Problem::new("field_role_conflict")
        .with("graph", name(graph))
        .with("type", name(kind))
        .with("field", name(field))
        .with("role", Slot::Word(has.as_str()))
}

/// `ontology add field --relation` names a field the relation has with another kind (F92): nothing changed.
pub fn relation_field_conflict(
    graph: &ValidName,
    relation: &ValidName,
    field: &ValidName,
    has: FieldKind,
) -> Problem {
    Problem::new("relation_field_conflict")
        .with("graph", name(graph))
        .with("relation", name(relation))
        .with("field", name(field))
        .with("kind", Slot::Word(has.as_str()))
}

/// `ontology add type` re-declares a type that lacks one of the given fields (F71): nothing changed; `fix` adds it.
pub fn field_missing(
    graph: &ValidName,
    kind: &ValidName,
    field: &ValidName,
    fix: Command,
) -> Problem {
    Problem::new("field_missing")
        .with("graph", name(graph))
        .with("type", name(kind))
        .with("field", name(field))
        .with("fix", Slot::Command(fix))
}

/// `ontology add field` on many types refused more types than it lists (F71).
/// `ontology remove` of an entry `count` records use (F76): nothing changed; `fix` lists them. A count, never titles.
pub fn ontology_in_use(
    graph: &ValidName,
    what: &'static str,
    n: &ValidName,
    count: usize,
    users: &'static str,
    fix: Command,
) -> Problem {
    Problem::new("ontology_in_use")
        .with("graph", name(graph))
        .with("kind of name", Slot::Word(what))
        .with("name", name(n))
        .with("count", Slot::Count(count as u64))
        .with("users", Slot::Word(users))
        .with("fix", Slot::Command(fix))
}

/// `ontology remove` of an entry umx itself relies on (F76). `given` is what was asked, which may not be a valid name
/// (a `std:` entry).
pub fn ontology_builtin(what: &'static str, given: &str, reason: &'static str) -> Problem {
    Problem::new("ontology_builtin")
        .with("kind of name", Slot::Word(what))
        .with("name", Slot::name_or(given, "(that name)"))
        .with("reason", Slot::Word(reason))
}

pub fn ontology_errors_truncated(failed: usize, shown: usize) -> Problem {
    Problem::new("ontology_errors_truncated")
        .with("failed", Slot::Count(failed as u64))
        .with("shown", Slot::Count(shown as u64))
}

pub fn kind_invalid(kind: &str) -> Problem {
    Problem::new("kind_invalid").with("kind", Slot::name_or(kind, "(that kind)"))
}

pub fn where_invalid(reason: &'static str) -> Problem {
    Problem::new("where_invalid")
        .with("expr", Slot::Word("(that expression)"))
        .with("reason", Slot::Word(reason))
}

/// `where_invalid` for a date field given a value that is not a date (D82): the field is named in the expression's
/// place, and the reason shows the dates it takes.
pub fn where_not_a_date(field: &str) -> Problem {
    Problem::new("where_invalid")
        .with("expr", Slot::name_or(field, "(that expression)"))
        .with(
            "reason",
            Slot::Word(
                "it is a date field and wants a date such as 2026-01-15 or 2026-01-15T09:00:00Z",
            ),
        )
}

pub fn cross_graph_not_yet() -> Problem {
    Problem::new("cross_graph_not_yet")
}

pub fn write_failed(file: GraphFile, e: &FilesError) -> Problem {
    Problem::new("write_failed")
        .with("file", Slot::file(file))
        .with("os error", Slot::os(e))
}

/// The shard files could not be read or listed (an I/O error; outside the roots is caught at load).
pub fn shard_read_failed(e: &FilesError) -> Problem {
    Problem::new("write_failed")
        .with("file", Slot::Word("an embeddings shard"))
        .with("os error", Slot::os(e))
}

/// A shard could not be written; it holds its old bytes.
pub fn shard_write_failed(shard: &ShardFile, e: &FilesError) -> Problem {
    Problem::new("write_failed")
        .with("file", Slot::File(FileRef::Shard(shard.clone())))
        .with("os error", Slot::os(e))
}

/// A similar_to file could not be written; every file of its set holds its old bytes.
pub fn similar_write_failed(file: &FileRef, e: &FilesError) -> Problem {
    Problem::new("write_failed")
        .with("file", Slot::File(file.clone()))
        .with("os error", Slot::os(e))
}

/// A similar_to set was written but the folder sync after it failed: the change is made.
pub fn similar_not_durable(file: &FileRef, e: &FilesError) -> Problem {
    Problem::new("write_not_durable")
        .with("file", Slot::File(file.clone()))
        .with("os error", Slot::os(e))
}

/// graph.json's `embedding.similarity` holds a value out of range or of the wrong type: the default is used and the
/// file's value kept as it is.
pub fn similarity_setting_invalid(graph: &ValidName, setting: &'static str) -> Problem {
    Problem::new("similarity_setting_invalid")
        .with("graph", name(graph))
        .with("setting", Slot::Word(setting))
}

/// A loaded graph's model has no floor for `uses` (`search`, `similar_to` or both): not calibrated, and neither the
/// machine config nor (for similar_to) graph.json sets one, so those uses cut nothing below their limit (ws004 s26).
/// Once per load.
pub fn floor_uncalibrated(key: &umx_core::ports::ModelKey, uses: &'static str) -> Problem {
    Problem::new("floor_uncalibrated")
        .with("model", Slot::ModelKey(key.clone()))
        .with("uses", Slot::Word(uses))
}

/// `umx similarity pairs` would pass a cap (ws004 s31): `unit` names which, `need` what this call needs.
pub fn similarity_pairs_too_large(
    nodes: usize,
    dims: usize,
    need: u64,
    cap: u64,
    unit: &'static str,
) -> Problem {
    Problem::new("similarity_pairs_too_large")
        .with("nodes", Slot::Count(nodes as u64))
        .with("dims", Slot::Count(dims as u64))
        .with("need", Slot::Count(need))
        .with("cap", Slot::Count(cap))
        .with("unit", Slot::Word(unit))
}

/// `umx similarity pairs` was cancelled: past its backstop, or the daemon is restarting or stopping.
pub fn similarity_pairs_timed_out(cause: &'static str) -> Problem {
    Problem::new("similarity_pairs_timed_out").with("cause", Slot::Word(cause))
}

/// Graphs in scope that `umx similarity pairs` left out because their recorded model is not `model`.
pub fn similarity_model_mismatch(model: &ModelKey, graphs: &[String]) -> Problem {
    Problem::new("similarity_model_mismatch")
        .with(
            "graphs",
            Slot::Names(graphs.iter().filter_map(|g| ValidName::alias(g)).collect()),
        )
        .with("model", Slot::ModelKey(model.clone()))
        .candidates(alias_candidates(graphs))
}

/// A graph's stored similar_to lists do not match its vectors or settings (`_meta.tsv` differs, as after a merge):
/// they are being rebuilt, and until then similar_to results are incomplete.
pub fn similar_to_incomplete(graph: &ValidName) -> Problem {
    Problem::new("similar_to_incomplete").with("graph", name(graph))
}

/// A shard was replaced but the folder sync after it failed: the change is made.
pub fn shard_not_durable(shard: &ShardFile, e: &FilesError) -> Problem {
    Problem::new("write_not_durable")
        .with("file", Slot::File(FileRef::Shard(shard.clone())))
        .with("os error", Slot::os(e))
}

/// A graph's committed whole-node vectors that included a field now data (req-97): kept and searched until reembed.
pub fn text_form_outdated(graph: &ValidName, count: usize) -> Problem {
    Problem::new("text_form_outdated")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
}

/// `ontology set field` changed a field's role; `count` nodes have pending vectors until they are remade (req-97).
pub fn role_changed(
    graph: &ValidName,
    type_name: &ValidName,
    field: &ValidName,
    role: Role,
    count: usize,
) -> Problem {
    Problem::new("role_changed")
        .with("graph", name(graph))
        .with("type", name(type_name))
        .with("field", name(field))
        .with("role", Slot::Word(role.as_str()))
        .with("count", Slot::Count(count as u64))
}

/// search-05: a content field that holds labels; `fix` is the `ontology set field ... --role data` that moves it.
pub fn field_looks_like_labels(
    graph: &ValidName,
    type_name: &ValidName,
    field: &ValidName,
    count: usize,
    distinct: usize,
) -> Problem {
    Problem::new("field_looks_like_labels")
        .with("graph", name(graph))
        .with("type", name(type_name))
        .with("field", name(field))
        .with("count", Slot::Count(count as u64))
        .with("distinct", Slot::Count(distinct as u64))
        .with(
            "fix",
            Slot::Command(set_role(graph, type_name, field, Role::Data)),
        )
}

/// search-05: a data field that holds prose; `fix` is the `ontology set field ... --role content` that moves it.
pub fn field_looks_like_prose(
    graph: &ValidName,
    type_name: &ValidName,
    field: &ValidName,
    count: usize,
    long: usize,
) -> Problem {
    Problem::new("field_looks_like_prose")
        .with("graph", name(graph))
        .with("type", name(type_name))
        .with("field", name(field))
        .with("count", Slot::Count(count as u64))
        .with("long", Slot::Count(long as u64))
        .with(
            "fix",
            Slot::Command(set_role(graph, type_name, field, Role::Content)),
        )
}

/// PRD 002 search-04: a string field just declared with no role is data, so search by meaning never sees it; `fix`
/// is the `ontology set field ... --role content` that makes it content.
pub fn field_data_by_default(
    graph: &ValidName,
    type_name: &ValidName,
    field: &ValidName,
) -> Problem {
    Problem::new("field_data_by_default")
        .with("graph", name(graph))
        .with("type", name(type_name))
        .with("field", name(field))
        .with(
            "fix",
            Slot::Command(set_role(graph, type_name, field, Role::Content)),
        )
}

/// `umx ontology set field <type> <field> --role <role> --graph <graph>`: the one command that changes a role.
fn set_role(graph: &ValidName, type_name: &ValidName, field: &ValidName, role: Role) -> Command {
    Command::new("ontology set field")
        .name(type_name)
        .name(field)
        .word("--role")
        .word(role.as_str())
        .word("--graph")
        .name(graph)
}

pub fn role_not_embeddable(field: &ValidName, kind: FieldKind) -> Problem {
    Problem::new("role_not_embeddable")
        .with("field", name(field))
        .with("kind", Slot::Word(kind.as_str()))
}

pub fn field_not_declared(graph: &ValidName, type_name: &ValidName, field: &ValidName) -> Problem {
    Problem::new("field_not_declared")
        .with("graph", name(graph))
        .with("type", name(type_name))
        .with("field", name(field))
}

/// A vector line names a node that is not in the graph (as after a merge where one branch deleted the node). The
/// line is kept. Names the node id and the line, never vector or text content.
pub fn vector_orphan(graph: &ValidName, shard: &ShardFile, line: usize, node: Id) -> Problem {
    Problem::new("vector_orphan")
        .with("file", Slot::File(FileRef::Shard(shard.clone())))
        .with("n", Slot::Count(line as u64))
        .with("node", Slot::Id(node))
        .with("graph", name(graph))
        .subject(node.to_text())
}

/// A shard line umx cannot read (a hand edit or a merge marker); it is kept and written back.
pub fn vector_line_kept(graph: &ValidName, shard: &ShardFile, line: usize) -> Problem {
    Problem::new("vector_line_kept")
        .with("file", Slot::File(FileRef::Shard(shard.clone())))
        .with("n", Slot::Count(line as u64))
        .with("graph", name(graph))
}

/// `umx config set` or `unset` repointed hosted instance `instance`, and `graphs` loaded non-open graphs of its model
/// will embed through it after a restart (D62, security s20b). Counted, never named.
pub fn embedder_route_changed(instance: &str, graphs: usize) -> Problem {
    Problem::new("embedder_route_changed")
        .with("embedder", embedder_slot(instance))
        .with("n", Slot::Count(graphs as u64))
}

/// A secured graph is loaded while the configured embedder is not local: nothing of it is embedded (req-46).
pub fn embedder_not_local_for_secured(graph: &ValidName) -> Problem {
    Problem::new("embedder_not_local_for_secured").with("graph", name(graph))
}

/// A vector line whose width is not its space's dims: never used, and its node is embedded again.
pub fn vector_wrong_width(
    graph: &ValidName,
    shard: &ShardFile,
    line: usize,
    found: usize,
) -> Problem {
    Problem::new("vector_wrong_width")
        .with("file", Slot::File(FileRef::Shard(shard.clone())))
        .with("n", Slot::Count(line as u64))
        .with("found", Slot::Count(found as u64))
        .with("dims", Slot::Count(shard.key().dims() as u64))
        .with("graph", name(graph))
}

/// A graph's vector shards could not be read at load: nothing was queued from them. `cause` is the failed read's
/// own (safe) problem.
/// After a write to a node (or applying its vectors), its shard could not be read to check its vectors: the node
/// counts as pending rather than current (review R85-1).
pub fn vector_shard_unreadable(graph: &ValidName, node: Id, cause: &Problem) -> Problem {
    let why = cause
        .slot("os error")
        .unwrap_or_else(|| String::from("read error"));
    Problem::new("vector_shard_unreadable")
        .with("graph", name(graph))
        .with("node", Slot::Id(node))
        .with("os error", Slot::Os(SafeText::lossy(&why)))
}

pub fn vectors_unreadable(graph: &ValidName, cause: &Problem) -> Problem {
    let why = cause
        .slot("os error")
        .unwrap_or_else(|| String::from("read error"));
    Problem::new("vectors_unreadable")
        .with("graph", name(graph))
        .with("os error", Slot::Os(SafeText::lossy(&why)))
}

/// A command that needs an embedder runs with none configured.
pub fn embedder_not_configured() -> Problem {
    Problem::new("embedder_not_configured")
}

/// `umx embeddings reembed` on a secured graph with an embedder that is not local: refused, nothing sent (req-46).
pub fn secured_embed_refused(graph: &ValidName) -> Problem {
    Problem::new("secured_embed_refused").with("graph", name(graph))
}

/// `umx embeddings reembed --model <m>` where `m` is not the configured embedder's model; nothing changed.
pub fn reembed_model_not_configured(
    graph: &ValidName,
    asked: &ModelKey,
    configured: &ModelKey,
) -> Problem {
    Problem::new("reembed_model_not_configured")
        .with("graph", name(graph))
        .with("asked", Slot::ModelKey(asked.clone()))
        .with("embedder key", Slot::ModelKey(configured.clone()))
}

/// `file` was replaced but the folder sync after it failed: the change is made.
/// `write_not_durable` for a keyed batch's receipts-only set (ws003 s09): the receipt stands.
pub fn receipts_not_durable(e: &FilesError) -> Problem {
    Problem::new("write_not_durable")
        .with("file", Slot::File(FileRef::Receipts))
        .with("os error", Slot::os(e))
}

/// `write_failed` for a keyed batch's receipts-only set (ws003 s09): nothing changed.
pub fn receipts_write_failed(e: &FilesError) -> Problem {
    Problem::new("write_failed")
        .with("file", Slot::File(FileRef::Receipts))
        .with("os error", Slot::os(e))
}

pub fn write_not_durable(file: GraphFile, e: &FilesError) -> Problem {
    Problem::new("write_not_durable")
        .with("file", Slot::file(file))
        .with("os error", Slot::os(e))
}

/// A crash cut off a set of writes in `dir`, and `files` changed since (a pull, a checkout), so the adapter will
/// not finish it and every call on the folder fails until a person resolves it. `graph` is the loaded graph's
/// alias, if any, for the `umx check` next; otherwise the next is the graph docs, which say how to resolve it.
pub fn recovery_conflict(dir: &GraphDir, files: &[FileRef], graph: Option<&ValidName>) -> Problem {
    let names: Vec<String> = files.iter().map(FileRef::path).collect();
    let fix = match graph {
        Some(g) => Command::new("check").word("--graph").name(g),
        None => Command::new("docs get graph"),
    };
    Problem::new("recovery_conflict")
        .with("path", Slot::path(dir))
        .with("files", Slot::Os(SafeText::lossy(&names.join(" "))))
        .with("fix", Slot::Command(fix))
}

/// F144: a write to `graph` refused because its content `files` changed on disk since it was loaded or last written
/// (a checkout, a pull, a hand edit, or only a touch): nothing was written and memory is as it was. Never reloaded
/// for the caller: the next is to unload and load it again.
pub fn graph_files_changed(graph: &ValidName, dir: &GraphDir, files: &[FileRef]) -> Problem {
    let names: Vec<String> = files.iter().map(FileRef::path).collect();
    files_moved(Problem::new("graph_files_changed"), graph, dir, &names)
}

/// F144: a read of `graph` answered from memory while its content `files` changed on disk since it was loaded.
pub fn stale_files(graph: &ValidName, dir: &GraphDir, files: &[GraphFile]) -> Problem {
    let names: Vec<String> = files.iter().map(|f| f.file_name().to_string()).collect();
    files_moved(Problem::new("stale_files"), graph, dir, &names)
}

/// The next for both: unload and load the folder again under the same alias, the folder quoted when it holds a
/// space ([`Command::path`]) so the line runs as given.
fn files_moved(p: Problem, graph: &ValidName, dir: &GraphDir, names: &[String]) -> Problem {
    let fix = Command::new("graph unload")
        .name(graph)
        .then("graph load")
        .path(dir)
        .word("--as")
        .name(graph);
    p.with("graph", name(graph))
        .with("files", Slot::Os(SafeText::lossy(&names.join(" "))))
        .with("fix", Slot::Command(fix))
}

/// The graph folder's `.gitignore` does not hide umx's temp and marker files (`umx check`).
pub fn gitignore_line_missing(dir: &GraphDir) -> Problem {
    Problem::new("gitignore_line_missing").with("path", Slot::path(dir))
}

/// A bug in umx itself, named by a fixed code; never graph text.
pub fn internal(code: &'static str) -> Problem {
    Problem::new("internal").with("code", Slot::Word(code))
}

/// A graph-view command sent while no view has waited for events in the last 30 s (ws009 S6).
/// No view is open (ws009 S6); `fix` brings one up (ws009 M1b2).
pub fn ui_no_view(fix: Command) -> Problem {
    Problem::new("ui_no_view").with("fix", Slot::Command(fix))
}

/// `umx ui open` named a graph the view may not open (ws009 M1b1): `left_out` when others were sent, else refused.
pub fn ui_open_refused(alias: &str, why: &str, fix: Command, left_out: bool) -> Problem {
    Problem::new(if left_out {
        "ui_open_left_out"
    } else {
        "ui_open_refused"
    })
    .with("alias", Slot::name_or(alias, "(that alias)"))
    .with("why", reason_slot(why))
    .with("fix", Slot::Command(fix))
}

/// A graph view's wait while [`umx_core::ui::MAX_WAITERS`] already wait (architect W5).
pub fn ui_waiters_busy() -> Problem {
    Problem::new("ui_waiters_busy")
}

/// `n` selected nodes in graphs a read that names none never shows (d-20, security T4).
pub fn ui_selection_hidden(n: usize) -> Problem {
    Problem::new("ui_selection_hidden").with("n", Slot::Count(n as u64))
}

pub fn off_vocabulary_type(graph: &ValidName, n: &ValidName, nearest: Vec<Candidate>) -> Problem {
    Problem::new("off_vocabulary_type")
        .with("graph", name(graph))
        .with("type", name(n))
        .candidates(nearest)
}

pub fn off_vocabulary_status(graph: &ValidName, n: &ValidName, nearest: Vec<Candidate>) -> Problem {
    Problem::new("off_vocabulary_status")
        .with("graph", name(graph))
        .with("status", name(n))
        .candidates(nearest)
}

pub fn off_vocabulary_relation(
    graph: &ValidName,
    n: &ValidName,
    nearest: Vec<Candidate>,
) -> Problem {
    Problem::new("off_vocabulary_relation")
        .with("graph", name(graph))
        .with("relation", name(n))
        .candidates(nearest)
}

pub fn off_vocabulary_field(
    graph: &ValidName,
    kind: &ValidName,
    field: &ValidName,
    inferred: &'static str,
) -> Problem {
    Problem::new("off_vocabulary_field")
        .with("graph", name(graph))
        .with("type", name(kind))
        .with("field", name(field))
        .with("inferred kind", Slot::Word(inferred))
}

/// An edge sets a custom field its relation does not declare (F92): written all the same, like a node's.
pub fn off_vocabulary_edge_field(
    graph: &ValidName,
    relation: &ValidName,
    field: &ValidName,
    inferred: &'static str,
) -> Problem {
    Problem::new("off_vocabulary_edge_field")
        .with("graph", name(graph))
        .with("relation", name(relation))
        .with("field", name(field))
        .with("inferred kind", Slot::Word(inferred))
}

/// A new node has near duplicates (`node add`, `umx batch`); the best is named with its type, since a near duplicate
/// of another type is a different kind of thing worded alike (ws004 s30).
pub fn near_duplicate(
    graph: &ValidName,
    subject: Id,
    first: Id,
    first_kind: &str,
    candidates: Vec<Candidate>,
) -> Problem {
    Problem::new("near_duplicate")
        .with("graph", name(graph))
        .with("candidate id", Slot::Id(first))
        .with("candidate type", Slot::name_or(first_kind, "(that type)"))
        .subject(subject.to_text())
        .candidates(candidates)
}

/// A batch line's best near duplicate is the node an earlier `line` of the same batch adds, so the warning names that
/// line (ws004 s30): `umx node get` on its id, which the batch shows as its dry run where the id will not exist.
pub fn near_duplicate_line(
    graph: &ValidName,
    subject: Id,
    line: u32,
    first: Id,
    first_kind: &str,
    candidates: Vec<Candidate>,
) -> Problem {
    Problem::new("near_duplicate_line")
        .with("candidate line", Slot::Count(u64::from(line)))
        .with("candidate type", Slot::name_or(first_kind, "(that type)"))
        .with(
            "fix",
            Slot::Command(Command::new("node get").address(graph, first)),
        )
        .subject(subject.to_text())
        .candidates(candidates)
}

/// The related check (req-123) did not run for `count` new nodes of `graph`, for `reason` (its words); `fix` is
/// the command that turns it on or shows what is missing.
pub fn related_skipped(count: usize, reason: &'static str, fix: Command) -> Problem {
    Problem::new("related_skipped")
        .with("count", Slot::Count(count as u64))
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(fix))
}

/// node find returned no rows: find reads titles only (ws004 s24 field report), so say where else to look.
pub fn find_no_match() -> Problem {
    Problem::new("find_no_match")
}

pub fn edge_exists(graph: &ValidName, from: Id, kind: &ValidName) -> Problem {
    Problem::new("edge_exists")
        .with("graph", name(graph))
        .with("from", Slot::Id(from))
        .with("type", name(kind))
}

/// edge add named an existing edge `id` and gave `n` evidence pointers it did not hold: they were appended (req-124).
pub fn evidence_appended(id: Id, n: usize) -> Problem {
    Problem::new("evidence_appended")
        .with("id", Slot::Id(id))
        .with("n", Slot::Count(n as u64))
        .with("fix", Slot::Command(Command::new("edge show").id(id)))
}

/// Evidence given to edge `id`, whose own custom `evidence` field is not an evidence list: refused, nothing changed
/// (B1). The fix moves that field to another name with `edge edit --set-json`, which keeps its JSON value as it is,
/// and `--unset evidence` (F134).
pub fn evidence_field_invalid(graph: &ValidName, id: Id) -> Problem {
    Problem::new("evidence_field_invalid")
        .with("id", Slot::Id(id))
        .with(
            "fix",
            Slot::Command(
                Command::new("edge edit")
                    .address(graph, id)
                    .word("--set-json")
                    .word("<field>=<value>")
                    .word("--unset")
                    .word("evidence"),
            ),
        )
}

/// Edge `id` holds a custom `evidence` field that is not an evidence list: kept as written, never read as evidence
/// (req-124); said on load, by edge show and by umx check.
pub fn evidence_field_kept(id: Id) -> Problem {
    Problem::new("evidence_field_kept")
        .with("id", Slot::Id(id))
        .with("fix", Slot::Command(Command::new("edge show").id(id)))
}

/// The next names `umx edge edit <id> --set <field>=<value>` for the first differing field (plan 205; F134's
/// `edge edit --set`); `<value>` stays a placeholder: values are never shown.
pub fn edge_exists_differs(id: Id, fields: Vec<ValidName>) -> Problem {
    let edit = Command::new("edge edit").id(id).word("--set");
    let fix = match fields.first() {
        Some(field) => edit.name_with(field, '=', "<value>"),
        None => edit.word("<field>=<value>"),
    };
    Problem::new("edge_exists_differs")
        .with("id", Slot::Id(id))
        .with("fields", Slot::Names(fields))
        .with("fix", Slot::Command(fix))
        .subject(id.to_text())
}

/// `edge delete` or `edge edit` named an edge the graph does not hold (F75). `given` is shown only if it is id text.
pub fn edge_not_found(given: &str, alias: &ValidName) -> Problem {
    Problem::new("edge_not_found")
        .with("id", Slot::id_text_or(given, "(that id)"))
        .with("alias", name(alias))
}

/// `edge edit` would make edge `id` the same as edge `other` (F75).
pub fn edge_would_duplicate(graph: &ValidName, id: Id, other: Id) -> Problem {
    Problem::new("edge_would_duplicate")
        .with("graph", name(graph))
        .with("id", Slot::Id(id))
        .with("other", Slot::Id(other))
        .subject(id.to_text())
}

/// `edge delete` or `edge edit` on an edge whose relation umx derives (F75). `kind` is one of DERIVED_RELATIONS.
pub fn edge_derived(id: Id, kind: &'static str) -> Problem {
    Problem::new("edge_derived")
        .with("id", Slot::Id(id))
        .with("type", Slot::Word(kind))
        .subject(id.to_text())
}

/// `edge edit` on an edge holding its own `updated` or `updated_by` field, which is not umx's edit stamp (F75).
pub fn edge_reserved_field_in_use(id: Id) -> Problem {
    Problem::new("edge_reserved_field_in_use")
        .with("id", Slot::Id(id))
        .subject(id.to_text())
}

/// `edge edit --reverse` on a symmetric relation (D49, F75): no direction to reverse.
pub fn edge_reverse_symmetric(id: Id, kind: &ValidName) -> Problem {
    Problem::new("edge_reverse_symmetric")
        .with("id", Slot::Id(id))
        .with("type", name(kind))
        .subject(id.to_text())
}

/// F211: a date field's stored value on node or edge `id` is not a date in years 0001 to 9999 (`umx check`); it
/// loads as written and compares as text. The fix writes a date there.
pub fn date_not_a_date(graph: &ValidName, id: Id, edge: bool, field: &ValidName) -> Problem {
    let fix = Command::new(if edge { "edge edit" } else { "node edit" })
        .address(graph, id)
        .word("--set")
        .name_with(field, '=', "<date>");
    Problem::new("date_not_a_date")
        .with("id", Slot::Id(id))
        .with("field", name(field))
        .with("fix", Slot::Command(fix))
        .subject(id.to_text())
}

/// An edit stored a symmetric edge with its ends swapped, the lower (graph id, node id) first (D49, F182); names the
/// relation and the ends as stored.
pub fn edge_direction_normalised(id: Id, kind: &ValidName, from: Id, to: Id) -> Problem {
    Problem::new("edge_direction_normalised")
        .with("id", Slot::Id(id))
        .with("type", name(kind))
        .with("from", Slot::Id(from))
        .with("to", Slot::Id(to))
        .subject(id.to_text())
}

/// ws008 TM3: a date-time with a clock and no offset, stored as universal time (D71 R4).
pub fn time_no_offset(graph: &ValidName, field: &ValidName) -> Problem {
    Problem::new("time_no_offset")
        .with("graph", name(graph))
        .with("field", name(field))
}

/// ws008 TM3: a text field that holds a time in the world; `why` says what gave it away.
pub fn time_as_text(
    graph: &ValidName,
    field: &ValidName,
    why: &'static str,
    fix: Command,
) -> Problem {
    Problem::new("time_as_text")
        .with("graph", name(graph))
        .with("field", name(field))
        .with("why", Slot::Word(why))
        .with("fix", Slot::Command(fix))
}

/// ws008 TM3: a meeting, event, segment or transcript with no starts_on, which no time spine can place.
pub fn anchor_undated(graph: &ValidName, kind: &ValidName, fix: Command) -> Problem {
    Problem::new("anchor_undated")
        .with("graph", name(graph))
        .with("type", name(kind))
        .with("fix", Slot::Command(fix))
}

/// ws002 X1: a std segment with no `part_of` edge out of it (a hint). `fix` links it to a parent.
pub fn segment_without_parent(id: Id, fix: Command) -> Problem {
    Problem::new("segment_without_parent")
        .with("id", Slot::Id(id))
        .with("fix", Slot::Command(fix))
        .subject(id.to_text())
}

/// ws008 T2a (req-98): graph.json's `std` is `std`, newer than this build's `known`; the graph loads on std v`known`.
pub fn std_newer(graph: &ValidName, std: u32, known: u32) -> Problem {
    Problem::new("std_newer")
        .with("graph", name(graph))
        .with("std", Slot::Count(u64::from(std)))
        .with("known", Slot::Count(u64::from(known)))
}

/// ws008 T2a (D69, a hint): a vocabulary read of a std graph shows std's entries besides the graph's own.
pub fn std_entries_shown(graph: &ValidName, std: u32) -> Problem {
    Problem::new("std_entries_shown")
        .with("graph", name(graph))
        .with("std", Slot::Count(u64::from(std)))
}

/// A std relation's end is of a type it does not expect (ws008 TM4 for raised_in; soft, never refused): names the
/// relation, the end's type and the types it expects.
pub fn relation_endpoint_unexpected(
    graph: &ValidName,
    relation: &ValidName,
    kind: &ValidName,
    expected: Vec<ValidName>,
) -> Problem {
    Problem::new("relation_endpoint_unexpected")
        .with("graph", name(graph))
        .with("relation", name(relation))
        .with("type", name(kind))
        .with("expected", Slot::Names(expected))
}

/// F139 `check --lint`: the node a hint points at first.
fn lint_node(graph: &ValidName, id: Id) -> Slot {
    Slot::Command(Command::new("node get").address(graph, id))
}

/// F139: content nodes with no edge (anchors and provenance left out).
pub fn lint_orphans(graph: &ValidName, count: usize, first: Id) -> Problem {
    Problem::new("lint_orphans")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
        .with("id", Slot::Id(first))
        .with("fix", lint_node(graph, first))
}

/// F139: an entity_key on a node whose status is proposed.
pub fn lint_proposed_keys(graph: &ValidName, count: usize, first: Id) -> Problem {
    Problem::new("lint_proposed_keys")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
        .with("id", Slot::Id(first))
        .with("fix", lint_node(graph, first))
}

/// F139 (security): places holding one of this machine's lint terms. `at` is the first place as data.lint names it
/// (an id and a place built from umx's own words and ordinals, R377-6): never the term, the text, a field name or a
/// candidate.
pub fn lint_terms(graph: &ValidName, count: usize, first: Option<Id>, at: &str) -> Problem {
    let p = Problem::new("lint_terms")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
        .with("first", Slot::Os(SafeText::lossy(at)));
    match first {
        Some(_) => p.with(
            "fix",
            Slot::Command(
                Command::new("check")
                    .word("--graph")
                    .name(graph)
                    .word("--lint")
                    .word("--kind")
                    .word("terms"),
            ),
        ),
        None => p.with(
            "fix",
            Slot::Command(Command::new("graph guide").name(graph)),
        ),
    }
}

/// F139: titles that say "lacks no".
pub fn lint_double_negatives(graph: &ValidName, count: usize, first: Id) -> Problem {
    Problem::new("lint_double_negatives")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
        .with("id", Slot::Id(first))
        .with(
            "fix",
            Slot::Command(
                Command::new("node edit")
                    .address(graph, first)
                    .word("--title")
                    .word("<text>"),
            ),
        )
}

/// ws008 TM5: a spine declaration names a type, relation or field that is neither this graph's own nor std. `at`
/// says which part of the declaration (`anchors.type`, `anchors.inside`, `order`, `place`); the name is a checked
/// vocabulary name.
pub fn spine_name_unknown(
    graph: &ValidName,
    spine: &str,
    at: &'static str,
    given: &str,
    fix: Command,
) -> Problem {
    Problem::new("spine_name_unknown")
        .with("graph", name(graph))
        .with("spine", Slot::name_or(spine, "(that spine)"))
        .with("at", Slot::Word(at))
        .with("name", Slot::name_or(given, "(that name)"))
        .with("fix", Slot::Command(fix))
}

/// ws008 TM5: a graph has at most one main spine; `main` is the one that is. Its name came from graph.json, so it is
/// shown only if it is a valid name (R366-3).
pub fn spine_main_taken(graph: &ValidName, main: &str) -> Problem {
    let show = Command::new("graph spine show").name(graph);
    let fix = match ValidName::new(main) {
        Some(m) => show.name(&m),
        None => show,
    };
    Problem::new("spine_main_taken")
        .with("graph", name(graph))
        .with("main", Slot::name_or(main, "(the main spine)"))
        .with("fix", Slot::Command(fix))
}

/// ws008 TM5: no declared spine has that name; the declared ones are the candidates.
pub fn spine_not_found(graph: &ValidName, given: &str, declared: Vec<Candidate>) -> Problem {
    Problem::new("spine_not_found")
        .with("graph", name(graph))
        .with("spine", Slot::name_or(given, "(that spine)"))
        .with(
            "fix",
            Slot::Command(Command::new("graph spine show").name(graph)),
        )
        .candidates(declared)
}

/// ws008 TM5: a built-in candidate has no declaration to confirm; its next is set.
pub fn spine_builtin_candidate(graph: &ValidName, candidate: &str) -> Problem {
    Problem::new("spine_builtin_candidate")
        .with("graph", name(graph))
        .with("candidate", Slot::name_or(candidate, "(that candidate)"))
        .with(
            "fix",
            Slot::Command(
                Command::new("graph spine set")
                    .name(graph)
                    .word("--file")
                    .word("<decl.json>"),
            ),
        )
}

/// D71 step 12: an `--along` end the spine cannot use; `spine` and `end` are slots the caller made safe (a name, id
/// text, or a date in umx's own form), `why` is umx's own words, and the candidates are the spine's first anchors.
pub fn along_end_invalid(graph: &ValidName, spine: Slot, end: Slot, why: &'static str) -> Problem {
    Problem::new("along_end_invalid")
        .with("graph", name(graph))
        .with("spine", spine)
        .with("end", end)
        .with("why", Slot::Word(why))
        .with(
            "fix",
            Slot::Command(Command::new("graph spine show").name(graph)),
        )
}

/// D71 step 12 (B4a, Kind Hint): `--along` left out `count` rows (`what`: nodes or edges) its spine does not place;
/// `fix` lists them when there is a command for it.
pub fn along_not_placed(
    count: usize,
    what: &'static str,
    spine: Slot,
    fix: Option<Command>,
) -> Problem {
    along_hint(Problem::new("along_not_placed"), count, what, spine, fix)
}

/// D71 step 12 (B4a, Kind Hint): `--along` left out `count` rows its spine places only by rule.
pub fn along_by_rule(
    count: usize,
    what: &'static str,
    spine: Slot,
    fix: Option<Command>,
) -> Problem {
    along_hint(Problem::new("along_by_rule"), count, what, spine, fix)
}

fn along_hint(
    p: Problem,
    count: usize,
    what: &'static str,
    spine: Slot,
    fix: Option<Command>,
) -> Problem {
    let p = p
        .with("count", Slot::Count(count as u64))
        .with("what", Slot::Word(what))
        .with("spine", spine);
    match fix {
        Some(c) => p.with("fix", Slot::Command(c)),
        None => p,
    }
}

/// D71 step 12 (pm-001): a declaration may not take a built-in spine's name, which `--along` reads as the built-in.
/// The message suggests `<name>_spine` when that is a valid name.
pub fn spine_name_reserved(graph: &ValidName, given: &str) -> Problem {
    let base = given.strip_prefix("date_field:").unwrap_or(given);
    let suggest = ValidName::new(&alloc::format!("{base}_spine"))
        .map_or(Slot::Word("another name"), Slot::Name);
    Problem::new("spine_name_reserved")
        .with("graph", name(graph))
        .with("spine", spine_name_slot(given))
        .with("suggest", suggest)
        .with(
            "fix",
            Slot::Command(
                Command::new("graph spine set")
                    .name(graph)
                    .word("--file")
                    .word("<decl.json>"),
            ),
        )
}

/// D71 step 12: umx check found a stored spine under a built-in's name (declared before umx reserved them).
pub fn spine_name_reserved_stored(graph: &ValidName, given: &str) -> Problem {
    Problem::new("spine_name_reserved_stored")
        .with("graph", name(graph))
        .with("spine", spine_name_slot(given))
        .with(
            "fix",
            Slot::Command(Command::new("graph spine show").name(graph)),
        )
}

/// A spine name in a message: a valid name as is; `date_field:<f>` with a valid field is umx's prefix and a checked
/// name, so it is safe text; anything else a placeholder.
fn spine_name_slot(given: &str) -> Slot {
    match given.strip_prefix("date_field:").map(ValidName::new) {
        Some(Some(_)) => Slot::Path(SafeText::lossy(given)),
        Some(None) => Slot::Word("(that spine)"),
        None => Slot::name_or(given, "(that spine)"),
    }
}

/// F139: a node's evidence and its raised_in anchor's never meet in a source they share; names both ranges.
#[allow(clippy::too_many_arguments)]
pub fn lint_raised_in_evidence(
    graph: &ValidName,
    count: usize,
    first: Id,
    anchor_id: Id,
    unit: &'static str,
    node: (u64, u64),
    anchor: (u64, u64),
) -> Problem {
    Problem::new("lint_raised_in_evidence")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
        .with("id", Slot::Id(first))
        .with("anchor", Slot::Id(anchor_id))
        .with("unit", Slot::Word(unit))
        .with("from", Slot::Count(node.0))
        .with("to", Slot::Count(node.1))
        .with("anchor_from", Slot::Count(anchor.0))
        .with("anchor_to", Slot::Count(anchor.1))
        .with("fix", lint_node(graph, first))
}

/// F139: a graph's kind found more than one page; the next page.
pub fn lint_truncated(
    graph: &ValidName,
    kind: &'static str,
    more: usize,
    offset: usize,
) -> Problem {
    Problem::new("lint_truncated")
        .with("graph", name(graph))
        .with("kind", Slot::Word(kind))
        .with("count", Slot::Count(more as u64))
        .with(
            "fix",
            Slot::Command(
                Command::new("check")
                    .word("--graph")
                    .name(graph)
                    .word("--lint")
                    .word("--kind")
                    .word(kind)
                    .word("--offset")
                    .count(offset as u64),
            ),
        )
}

/// F139 (security): this machine's lint terms file cannot be used; names the file only.
pub fn lint_terms_unusable(file: &GraphDir) -> Problem {
    Problem::new("lint_terms_unusable").with(
        "file",
        match SafeText::path(file) {
            Some(path) => Slot::Path(path),
            None => Slot::Word("(the lint terms file)"),
        },
    )
}

pub fn field_kind_mismatch(graph: &ValidName, field: &ValidName, kind: &'static str) -> Problem {
    Problem::new("field_kind_mismatch")
        .with("graph", name(graph))
        .with("field", name(field))
        .with("kind", Slot::Word(kind))
}

/// `umx node status umx:<graph>/<id> <status> --reason <why>`: the command that records a status move.
pub fn status_command(graph: &ValidName, id: Id, status: Option<&ValidName>) -> Command {
    let cmd = Command::new("node status").address(graph, id);
    match status {
        Some(s) => cmd.name(s),
        None => cmd.word("<status>"),
    }
    .word("--reason")
    .word("<why>")
}

/// Status set through `--set` or `--unset` on node edit; the status verb records the move.
pub fn use_status_verb(fix: Command) -> Problem {
    Problem::new("use_status_verb").with("fix", Slot::Command(fix))
}

/// A status move without `--reason`. The move still happened.
pub fn reason_missing(id: Id, fix: Command) -> Problem {
    Problem::new("reason_missing")
        .with("fix", Slot::Command(fix))
        .subject(id.to_text())
}

pub fn evidence_pointer_invalid(reason: &'static str) -> Problem {
    Problem::new("evidence_pointer_invalid").with("reason", Slot::Word(reason))
}

pub fn evidence_outside_roots() -> Problem {
    Problem::new("evidence_outside_roots")
}

/// `umx node evidence <node> list` for the node, or its trail with `--removed`.
fn evidence_list(graph: &ValidName, id: Id, removed: bool) -> Command {
    let c = Command::new("node evidence")
        .address(graph, id)
        .word("list");
    if removed { c.word("--removed") } else { c }
}

/// No live pointer on the node matches the one given (req-126); the node's pointers are the candidates.
pub fn evidence_not_found(graph: &ValidName, id: Id) -> Problem {
    Problem::new("evidence_not_found")
        .with("id", Slot::Id(id))
        .with("graph", name(graph))
        .with("fix", Slot::Command(evidence_list(graph, id, false)))
        .subject(id.to_text())
}

/// Several live pointers match (they differ only by kind, or are duplicates); the candidates are their positions.
pub fn evidence_ambiguous(graph: &ValidName, id: Id, count: usize) -> Problem {
    Problem::new("evidence_ambiguous")
        .with("id", Slot::Id(id))
        .with("count", Slot::Count(count as u64))
        .with("fix", Slot::Command(evidence_list(graph, id, false)))
        .subject(id.to_text())
}

/// The pointer is in the node's trail and no live pointer matches: removing it again changed nothing.
pub fn already_removed(graph: &ValidName, id: Id) -> Problem {
    Problem::new("already_removed")
        .with("id", Slot::Id(id))
        .with("fix", Slot::Command(evidence_list(graph, id, true)))
        .subject(id.to_text())
}

/// F183: an evidence redact scrubbed the pointer from this graph's files only (git history and copies remain).
pub fn redacted_in_files_only(id: Id) -> Problem {
    Problem::new("redacted_in_files_only").subject(id.to_text())
}

/// F183: the pointer's text is also somewhere a redact never rewrites; `blocking` names each record and place (the
/// candidates' id and title), never the text. Nothing changed.
pub fn evidence_redact_blocked(graph: &ValidName, blocking: Vec<Candidate>) -> Problem {
    Problem::new("evidence_redact_blocked")
        .with("n", Slot::Count(blocking.len() as u64))
        .with("graph", name(graph))
        .candidates(blocking)
}

/// A batch `evidence remove` line with `"missing": "skip"` whose pointer matches nothing on the node.
pub fn evidence_missing_skipped(graph: &ValidName, id: Id) -> Problem {
    Problem::new("evidence_missing_skipped")
        .with("id", Slot::Id(id))
        .with("fix", Slot::Command(evidence_list(graph, id, false)))
        .subject(id.to_text())
}

/// The node holds its own field under a name umx now keeps for itself (`evidence_removed`); umx never overwrites it.
pub fn node_reserved_field_in_use(id: Id) -> Problem {
    Problem::new("node_reserved_field_in_use")
        .with("id", Slot::Id(id))
        .subject(id.to_text())
}

/// `root:<name>/...` or `root remove <name>` names a source root this machine does not set. The fix adds it, with
/// the folder left as a placeholder: umx never suggests a folder to widen what it reads (usage note 031).
pub fn root_unknown(name: &str) -> Problem {
    let fix = match ValidName::alias(name) {
        Some(n) => Command::new("root add").name(&n).word("<folder>"),
        None => Command::new("root add").word("<name>").word("<folder>"),
    };
    Problem::new("root_unknown")
        .with("root", Slot::name_or(name, "(that root)"))
        .with("fix", Slot::Command(fix))
}

/// Why a source root cannot be used now.
pub const ROOT_NO_REPO: &str = "the graph has no git folder at or above it inside the roots";
pub const ROOT_NOT_FOLDER: &str = "its folder is missing or is not a folder";
pub const ROOT_UNREADABLE: &str = "the operating system refused to resolve its folder";

/// A source root that is set (or root:repo) but cannot be used: `reason` is one of the `ROOT_*` words.
pub fn root_unavailable(name: &str, reason: &'static str) -> Problem {
    let fix = if name == umx_core::config::REPO_ROOT {
        Command::new("docs get evidence")
    } else {
        Command::new("root list")
    };
    Problem::new("root_unavailable")
        .with("root", Slot::name_or(name, "(that root)"))
        .with("reason", Slot::Word(reason))
        .with("fix", Slot::Command(fix))
}

/// Names as candidates (for off-vocabulary warnings).
pub fn names(nearest: Vec<alloc::string::String>) -> Vec<Candidate> {
    nearest
        .into_iter()
        .map(|n| Candidate {
            id: n,
            title: None,
            score: None,
        })
        .collect()
}

/// `umx batch --file <file> --dry-run`: checks a fixed batch file without writing.
fn batch_dry_run() -> Slot {
    Slot::Command(batch_dry_run_command())
}

/// `umx batch --file <file> --dry-run`: the fix a batch's problems name.
pub fn batch_dry_run_command() -> Command {
    Command::new("batch")
        .word("--file")
        .word("<file>")
        .word("--dry-run")
}

/// When the batch was given `--reason`, its dry run in `p` carries `--reason <reason>` too, so the rerun keeps it: a
/// placeholder, never the reason's text (review R236-N1).
pub fn with_batch_reason(mut p: Problem) -> Problem {
    let plain = batch_dry_run_command();
    for (_, slot) in &mut p.slots {
        if let Slot::Command(c) = slot
            && *c == plain
        {
            *c = plain.clone().word("--reason").word("<reason>");
        }
    }
    p
}

/// A batch over a cap: `what` is `ops` or `bytes`.
pub fn batch_too_large(size: u64, what: &'static str, cap: u64) -> Problem {
    Problem::new("batch_too_large")
        .with("size", Slot::Count(size))
        .with("what", Slot::Word(what))
        .with("cap", Slot::Count(cap))
}

/// A batch line that is not a valid op: `reason` is umx's own words, naming at most a field umx knows.
pub fn batch_line_invalid(line: u32, reason: &'static str) -> Problem {
    Problem::new("batch_line_invalid")
        .with("line", Slot::Count(line.into()))
        .with("reason", Slot::Word(reason))
        .at_line(line)
}

pub fn batch_secret_invalid(line: u32, field: &str, rule: &'static str) -> Problem {
    let reason = alloc::format!("{field} matches the secret rule {rule}");
    Problem::new("batch_line_invalid")
        .with("line", Slot::Count(line.into()))
        .with("reason", Slot::Os(SafeText::lossy(&reason)))
        .at_line(line)
}

/// A line sets one field more than once (plan 205): only the last value is kept, so it is a warning. `field` passed
/// the name check.
pub fn duplicate_field(line: u32, field: &ValidName) -> Problem {
    Problem::new("duplicate_field")
        .with("line", Slot::Count(line.into()))
        .with("field", name(field))
        .at_line(line)
}

/// A plain node field (`node`, `parent`, `from`, `to`) named no node, and its text is a ref this batch knows (plan
/// 205): the hint names the field and its `_ref` twin, never the ref.
pub fn batch_use_ref_field(line: u32, field: &'static str, ref_field: &'static str) -> Problem {
    Problem::new("batch_use_ref_field")
        .with("line", Slot::Count(line.into()))
        .with("field", Slot::Word(field))
        .with("ref_field", Slot::Word(ref_field))
        .with("fix", Slot::Command(batch_dry_run_command()))
        .at_line(line)
}

pub fn batch_ref_invalid(line: u32, error: umx_core::batch::RefError) -> Problem {
    use umx_core::batch::{MAX_REF_LEN, RefError};
    // The same words as umx-protocol's `ref_error_words`, which the CLI gives (umx-daemon's parity test).
    let reason = match error {
        RefError::Syntax => {
            String::from("the ref is empty - a ref starts with a lowercase letter a-z")
        }
        RefError::TooLong => {
            alloc::format!("the ref is longer than the limit of {MAX_REF_LEN} characters")
        }
        RefError::Character {
            character,
            position,
        } => {
            let shown = if character.is_ascii_alphanumeric() || "._/-".contains(character) {
                character.to_string()
            } else {
                alloc::format!("U+{:04X}", character as u32)
            };
            let rule = if position == 1 {
                "a ref starts with a lowercase letter a-z"
            } else {
                "a ref holds only lowercase a-z 0-9 _ and -"
            };
            alloc::format!("ref character {shown} at position {position} is not allowed - {rule}")
        }
    };
    // Only umx's words and one checked character/code point, never the ref's text. `_` stays readable (plan 205).
    Problem::new("batch_line_invalid")
        .with("line", Slot::Count(line.into()))
        .with("reason", Slot::Os(SafeText::words(&reason)))
        .at_line(line)
}

pub fn id_invalid() -> Problem {
    Problem::new("id_invalid")
}

pub fn id_taken(reason: &'static str) -> Problem {
    Problem::new("id_taken").with("reason", Slot::Word(reason))
}

pub fn id_in_future() -> Problem {
    Problem::new("id_in_future")
}

pub fn id_in_other_graph(id: Id, other: &ValidName) -> Problem {
    Problem::new("id_in_other_graph")
        .with("id", Slot::Id(id))
        .with("other", name(other))
}

/// A ref used before its line, never defined, defined by a line that failed, or imported by `--refs-from` with an id
/// that names no node of the graph (`why`). `name` passed the ref check.
pub fn batch_ref_unknown(line: u32, name: &str, why: &'static str) -> Problem {
    Problem::new("batch_ref_unknown")
        .with("line", Slot::Count(line.into()))
        .with("why", Slot::Word(why))
        .subject(name)
        .at_line(line)
}

/// A ref defined a second time. `name` passed the ref check.
pub fn batch_ref_repeated(line: u32, name: &str) -> Problem {
    Problem::new("batch_ref_repeated")
        .with("line", Slot::Count(line.into()))
        .subject(name)
        .at_line(line)
}

/// A ref with two meanings (`why`), such as a ref a line defines that `--refs-from` also imports: never shadowed.
/// `name` passed the ref check.
pub fn batch_ref_conflict(line: u32, name: &str, why: &'static str) -> Problem {
    Problem::new("batch_ref_conflict")
        .with("why", Slot::Word(why))
        .subject(name)
        .at_line(line)
}

/// A `--refs-from` receipt names another graph id than the batch's graph (ws003 s24).
pub fn batch_refs_other_graph() -> Problem {
    Problem::new("batch_refs_other_graph")
}

/// ws003 s09: an idempotency key the caller sent, for a message. Only a key that passed the key rule reaches here, so
/// the text is exactly what was sent.
fn idempotency_key(key: &str) -> Slot {
    Slot::Os(SafeText::lossy(key))
}

/// A receipt's time, for a message.
fn receipt_time(ms: u64) -> Slot {
    Slot::Os(SafeText::lossy(&umx_core::receipts::time_text(ms)))
}

/// ws003 s09: the key's receipt is of a batch with another input; nothing changed.
pub fn idempotency_key_reused(key: &str, graph: &ValidName, at: u64) -> Problem {
    Problem::new("idempotency_key_reused")
        .with("key", idempotency_key(key))
        .with("graph", name(graph))
        .with("at", receipt_time(at))
}

/// ws003 s09 (graph-addressing): the key's receipt names `missing` ids the graph no longer holds; never replayed.
pub fn idempotency_receipt_stale(key: &str, graph: &ValidName, missing: usize) -> Problem {
    Problem::new("idempotency_receipt_stale")
        .with("key", idempotency_key(key))
        .with("graph", name(graph))
        .with("missing", Slot::Count(missing as u64))
}

/// ws003 s09: the graph's receipts file cannot be used (`why`, umx's own words), so a keyed batch is refused.
pub fn receipts_unreadable(graph: &ValidName, why: &str) -> Problem {
    Problem::new("receipts_unreadable")
        .with("graph", name(graph))
        .with("why", Slot::Os(SafeText::lossy(why)))
}

/// ws003 s09: the key and input ran before; the reply is the receipt recorded at `at`.
pub fn batch_replayed(key: &str, at: u64) -> Problem {
    Problem::new("batch_replayed")
        .with("key", idempotency_key(key))
        .with("at", receipt_time(at))
}

/// ws003 s09 (prime ruling): the key was seen, but its receipt was dropped, so the batch runs as new.
pub fn idempotency_key_expired(key: &str, at: u64) -> Problem {
    Problem::new("idempotency_key_expired")
        .with("key", idempotency_key(key))
        .with("at", receipt_time(at))
}

/// More lines failed than a failed batch lists.
pub fn batch_errors_truncated(failed: usize, shown: usize) -> Problem {
    Problem::new("batch_errors_truncated")
        .with("failed", Slot::Count(failed as u64))
        .with("shown", Slot::Count(shown as u64))
        .with("fix", batch_dry_run())
}

/// More warnings than a batch lists; the batch still applied.
pub fn batch_warnings_truncated(warned: usize, shown: usize) -> Problem {
    Problem::new("batch_warnings_truncated")
        .with("warned", Slot::Count(warned as u64))
        .with("shown", Slot::Count(shown as u64))
        .with("fix", batch_dry_run())
}

/// The summary of a failed batch: how many lines failed; nothing was written.
pub fn batch_failed(failed: usize) -> Problem {
    Problem::new("batch_failed")
        .with("failed", Slot::Count(failed as u64))
        .with("fix", batch_dry_run())
}

/// A field of a node looks like it holds a secret (req-49): it is held from hosted embedders until redacted or
/// allowed. Names the rule, never the text.
pub fn secret_suspected(graph: &ValidName, node: Id, field: &str, rule: &'static str) -> Problem {
    let fix = Command::new("embeddings allow")
        .word("--graph")
        .name(graph)
        .word("--node")
        .id(node)
        .word("--field");
    let fix = match ValidName::new(field) {
        Some(field) => fix.name(&field),
        None => fix.word("<field>"),
    };
    Problem::new("secret_suspected")
        .with("record", Slot::Word("Node"))
        .with("graph", name(graph))
        .with("node", Slot::Id(node))
        .with("field", Slot::name_or(field, "a field"))
        .with("rule", Slot::Word(rule))
        .with(
            "effect",
            Slot::Word("It is not sent to a hosted embedder until you redact it or allow it."),
        )
        .with("fix", Slot::Command(fix))
}

pub fn metadata_secret_suspected(
    graph: &ValidName,
    id: Id,
    record: &'static str,
    field: &str,
    rule: &'static str,
) -> Problem {
    Problem::new("secret_suspected")
        .with("record", Slot::Word(record))
        .with("graph", name(graph))
        .with("node", Slot::Id(id))
        .with("field", Slot::name_or(field, "a field"))
        .with("rule", Slot::Word(rule))
        .with(
            "effect",
            Slot::Word("The change was stored. Remove or replace this value. This metadata has no allow yet."),
        )
        .with("fix", Slot::Command(Command::new("docs get").word("batch")))
}

/// `umx embeddings allow` could not record the allow in `file`; nothing was allowed.
pub fn secret_allow_refused(file: &GraphDir, reason: &str) -> Problem {
    Problem::new("secret_allow_refused")
        .with("file", Slot::path(file))
        .with("reason", reason_slot(reason))
}

// ws002 s17 (D66, req-128): the Graphfile. Its reasons come from the codec, which names lines and fields only.

fn scan_cmd(top: &GraphDir) -> Command {
    Command::new("graph scan").path(top)
}

/// `graph new` left a graph whose effective policy is not open out of the Graphfile (security, s17).
pub fn graphfile_skipped_secured(alias: &str) -> Problem {
    Problem::new("graphfile_skipped_secured").with("alias", Slot::name_or(alias, "(that graph)"))
}

/// `graph new --graphfile` outside any git repository: no Graphfile lists the graph (the default stays quiet).
pub fn graphfile_no_repo(alias: &ValidName, folder: &GraphDir) -> Problem {
    Problem::new("graphfile_no_repo")
        .with("alias", Slot::Name(alias.clone()))
        .with(
            "fix",
            Slot::Command(
                Command::new("graph new")
                    .path(folder)
                    .word("--alias")
                    .name(alias)
                    .word("--no-graphfile"),
            ),
        )
}

/// The Graphfile is not in umx's own form, so a create or scan leaves it; the explicit rewrite is named.
pub fn graphfile_not_canonical(file: &GraphDir, top: &GraphDir) -> Problem {
    Problem::new("graphfile_not_canonical")
        .with("file", Slot::path(file))
        .with(
            "fix",
            Slot::Command(scan_cmd(top).word("--add").word("--rewrite")),
        )
}

/// A write that would go over a cap: nothing was written.
pub fn graphfile_too_large(file: &GraphDir, why: &str) -> Problem {
    Problem::new("graphfile_too_large")
        .with("file", Slot::path(file))
        .with("why", Slot::Os(SafeText::lossy(why)))
}

/// The Graphfile changed under umx twice running: left as it is; `retry` runs the command again.
pub fn graphfile_changed(file: &GraphDir, retry: Command) -> Problem {
    Problem::new("graphfile_changed")
        .with("file", Slot::path(file))
        .with("fix", Slot::Command(retry))
}

/// The Graphfile could not be used for a change (invalid, newer, refused): left as it is. `why` is umx's own words
/// ([`Slot::Word`]) or a codec reason made safe.
pub fn graphfile_unusable(file: &GraphDir, why: Slot, top: &GraphDir) -> Problem {
    Problem::new("graphfile_unusable")
        .with("file", Slot::path(file))
        .with("why", why)
        .with("fix", Slot::Command(scan_cmd(top)))
}

/// ws006 S23: `graph move` did not move the graph (`why`: the new folder exists, its parent is missing, it is on
/// another file system). Nothing moved.
pub fn graph_move_refused(graph: &ValidName, why: &'static str, fix: Command) -> Problem {
    Problem::new("graph_move_refused")
        .with("graph", name(graph))
        .with("why", Slot::Word(why))
        .with("fix", Slot::Command(fix))
}

/// ws006 S23 (graph-addressing-expert (h)): the new folder is in another git repo than the graph, which would
/// change every `root:repo` pointer's base: refused. Move it with git, load it, then re-cite.
pub fn graph_move_cross_repo(graph: &ValidName, to: &GraphDir) -> Problem {
    Problem::new("graph_move_cross_repo")
        .with("graph", name(graph))
        .with("fix", Slot::Command(Command::new("graph load").path(to)))
}

/// ws006 S23: `count` `root:repo` evidence pointers of `graph` point into `old`, a repo-relative folder the graph
/// moved from (graph.json's `former_paths`); each is a check row. `new` is where the graph is now.
pub fn evidence_root_stale(graph: &ValidName, count: usize, old: &str, new: &str) -> Problem {
    Problem::new("evidence_root_stale")
        .with("graph", name(graph))
        .with("count", Slot::Count(count as u64))
        .with("old", Slot::Os(SafeText::lossy(old)))
        .with("new", Slot::Os(SafeText::lossy(new)))
        .with(
            "fix",
            Slot::Command(Command::new("check").word("--graph").name(graph)),
        )
}

/// ws006 S23 (pm-001): `graph rename` renamed the graph, but the Graphfile lists another graph under the new alias,
/// so its entry for this one still says `old`: a warning, never a refusal (graph.json is done).
pub fn graphfile_not_updated(file: &GraphDir, old: &str, new: &str, fix: Command) -> Problem {
    Problem::new("graphfile_not_updated")
        .with("file", Slot::path(file))
        .with("old", Slot::name_or(old, "(the old alias)"))
        .with("new", Slot::name_or(new, "(the new alias)"))
        .with("fix", Slot::Command(fix))
}

/// A Graphfile entry whose folder holds another graph id: not loaded.
pub fn graphfile_id_mismatch(alias: &str, folder: &GraphDir, top: &GraphDir) -> Problem {
    Problem::new("graphfile_id_mismatch")
        .with("alias", Slot::name_or(alias, "(that entry)"))
        .with("path", Slot::path(folder))
        .with("fix", Slot::Command(scan_cmd(top)))
}

/// A Graphfile entry `graph load --graphfile` did not load, with the code that stopped it.
pub fn graphfile_entry_skipped(
    alias: &str,
    folder: &GraphDir,
    why: &'static str,
    fix: Command,
) -> Problem {
    Problem::new("graphfile_entry_skipped")
        .with("alias", Slot::name_or(alias, "(that entry)"))
        .with("path", Slot::path(folder))
        .with("why", Slot::Word(why))
        .with("fix", Slot::Command(fix))
}

/// `graph list --graphfile` or `graph load --graphfile` on a Graphfile umx cannot use: nothing from it is used.
pub fn graphfile_invalid(file: &GraphDir, why: &str, top: &GraphDir) -> Problem {
    Problem::new("graphfile_invalid")
        .with("file", Slot::path(file))
        .with("why", Slot::Os(SafeText::lossy(why)))
        .with("fix", Slot::Command(scan_cmd(top)))
}

pub fn graphfile_newer(file: &GraphDir, version: i64) -> Problem {
    Problem::new("graphfile_newer")
        .with("file", Slot::path(file))
        .with("version", Slot::Count(u64::try_from(version).unwrap_or(0)))
}

pub fn graphfile_missing(top: &GraphDir) -> Problem {
    Problem::new("graphfile_missing")
        .with("repo", Slot::path(top))
        .with("fix", Slot::Command(scan_cmd(top).word("--add")))
}

/// A Graphfile verb on a folder in no git repository inside the roots.
pub fn graphfile_repo_none(folder: &GraphDir) -> Problem {
    Problem::new("graphfile_repo_none").with("path", Slot::path(folder))
}

/// `--set` or `--drop` naming what the Graphfile does not list (`what`: set or graph).
pub fn graphfile_unknown(
    file: &GraphDir,
    what: &'static str,
    name: &str,
    top: &GraphDir,
) -> Problem {
    Problem::new("graphfile_unknown")
        .with("file", Slot::path(file))
        .with("what", Slot::Word(what))
        .with("name", Slot::name_or(name, "(that name)"))
        .with(
            "fix",
            Slot::Command(Command::new("graph list").word("--graphfile").path(top)),
        )
}

/// More graphs than one `graph load --graphfile` loads.
pub fn graphfile_load_too_many(count: usize, top: &GraphDir) -> Problem {
    Problem::new("graphfile_load_too_many")
        .with("count", Slot::Count(count as u64))
        .with(
            "fix",
            Slot::Command(Command::new("graph list").word("--graphfile").path(top)),
        )
}
