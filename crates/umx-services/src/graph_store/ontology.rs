//! ontology show | add type | add status | add relation | add field | set type | set field | remove type | status |
//! relation | field.

use super::graph::files_problem;
use super::node::check_name;
use super::{Ctx, Done, GraphStore, Outcome, alias_name, persist, problems};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use umx_core::batch::MAX_BATCH_ERRORS;
use umx_core::codec::GraphFile;
use umx_core::name::{RESERVED_EDGE_FIELDS, RESERVED_NODE_FIELDS};
use umx_core::ontology::{
    DEFAULT_STATUS, DERIVED_RELATIONS, Described, INSTANCE_OF, MAX_TYPE_COLUMNS, PART_OF,
    PROVENANCE_DERIVED, PROVENANCE_RELATIONS, PROVENANCE_TYPES, SIMILAR_TO,
};
use umx_core::settings::{self, check_description};
use umx_core::standard;
use umx_core::ui::Touched;
use umx_core::{Command, FieldDecl, FieldKind, Id, Ontology, Problem, Role, TypeDecl, ValidName};
use umx_graph::{Graph, Tally, edge_lines};

/// What `ontology set field` did (D56, req-97).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleSet {
    pub alias: String,
    pub type_name: String,
    pub field: String,
    pub role: Role,
    pub values: Vec<String>,
    /// Whether graph.json changed (false: the field already stated this role).
    pub changed: bool,
    /// Nodes whose embedded text the change alters (the field holds text and its effective role changed). The
    /// services queue them all and keep here only those whose vectors are then pending (the reply's `pending`).
    pub affected: Vec<Id>,
}

/// Parses a `--role` value.
pub fn parse_role(role: &str) -> Result<Role, Problem> {
    Role::parse(role)
        .ok_or_else(|| problems::usage_invalid("--role is data or content", "ontology"))
}

/// What `ontology add field` on several types did (F71).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldsAdded {
    pub alias: String,
    /// The field's name.
    pub name: String,
    /// The types it was added to, in the order named (declared order with `--all-types`).
    pub types: Vec<String>,
    /// The types that already had it with the same kind, left as they were.
    pub unchanged: Vec<String>,
}

/// What `ontology remove` did (F76).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OntologyRemoved {
    pub alias: String,
    /// "type", "status", "relation", "field" or "relation field".
    pub removed: &'static str,
    pub name: String,
    /// It was not declared, so nothing was written (setup scripts can re-run, as F71's adds).
    pub unchanged: bool,
    /// req-100: removing the graph's own role for a std field brought std's role back (`ontology remove field`
    /// only). Set with `affected` when the field's effective role changed; the services treat it as `ontology set
    /// field` does (req-97).
    pub role: Option<Role>,
    /// The nodes whose embedded text that role change alters (see [`RoleSet::affected`]).
    pub affected: Vec<Id>,
}

/// Why umx refuses to remove `given` as a `what` (F76, architect): it relies on the entry itself. Checked on what was
/// asked, before the name rules, so a `std:` entry is named as one.
fn builtin(what: &'static str, given: &str) -> Option<Problem> {
    let reason = if given.starts_with("std:") {
        "std entries are not the graph's to remove"
    } else {
        match what {
            "type" if PROVENANCE_TYPES.contains(&given) => "umx records provenance with it",
            "status" if given == DEFAULT_STATUS => "node add gives it to every new node",
            "relation" if given == PART_OF => "node add --parent writes it",
            "relation" if DERIVED_RELATIONS.contains(&given) => "umx derives those edges itself",
            "relation" if PROVENANCE_RELATIONS.contains(&given) => "umx records provenance with it",
            "relation" if given == INSTANCE_OF => {
                "it is a std relation every graph knows (ws008 T6)"
            }
            "relation" if umx_core::ontology::STD_RELATIONS.contains(&given) => {
                "umx entity link writes it in every graph"
            }
            "field" if RESERVED_NODE_FIELDS.contains(&given) => {
                "it is a standard field of every node"
            }
            "relation field" if RESERVED_EDGE_FIELDS.contains(&given) => {
                "it is a standard field of every edge"
            }
            _ => return None,
        }
    };
    Some(problems::ontology_builtin(what, given, reason))
}

/// How many records the census counts for `name` (0 when none).
fn uses(tallies: &[Tally<'_>], name: &str) -> usize {
    tallies
        .iter()
        .find(|t| t.name == name)
        .map_or(0, |t| t.count)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OntologyAdded {
    pub alias: String,
    /// "type", "status", "relation", "field", or "columns" (ontology set type).
    pub added: &'static str,
    pub name: String,
    /// The same declaration was already there, so nothing was written (F71: setup scripts can re-run).
    pub unchanged: bool,
}

/// Why a provenance key cannot be a default column (security N1), naming the key; none for other names.
fn provenance_column(column: &str) -> Option<&'static str> {
    Some(match column {
        "prov_key" => "prov_key is a provenance field and cannot be a default column",
        "authored_by_session" => {
            "authored_by_session is a provenance field and cannot be a default column"
        }
        "edited_by_session" => {
            "edited_by_session is a provenance field and cannot be a default column"
        }
        _ => return None,
    })
}

/// Parses `<name>:<kind>`. `reserved` are the standard keys of a node or an edge (`reserved_name`); `fix` rebuilds
/// the command with a good field name.
fn field_decl(
    spec: &str,
    reserved: &[&str],
    fix: &dyn Fn(&ValidName) -> Command,
) -> Result<FieldDecl, Problem> {
    let (name, kind) = spec
        .split_once(':')
        .ok_or_else(|| problems::usage_invalid("a field is <name>:<kind>", "ontology"))?;
    if reserved.contains(&name) {
        return Err(problems::reserved_name(name));
    }
    if name == umx_core::standard::STD {
        return Err(problems::std_reserved());
    }
    let name = check_name(name, "field name", fix)?;
    let kind = FieldKind::parse(kind).ok_or_else(|| problems::kind_invalid(kind))?;
    Ok(FieldDecl::new(name.as_str(), kind))
}

/// A stated role must be one the field's kind can have: only a string field can be content.
fn check_role(field: &FieldDecl, role: Role) -> Result<(), Problem> {
    if role == Role::Content && !field.kind.may_be_content() {
        let name = ValidName::new(&field.name).expect("a declared field name is valid");
        return Err(problems::role_not_embeddable(&name, field.kind));
    }
    Ok(())
}

/// A node field declaration: `entity_key` is std's too (ws008 E1), with a fixed kind, so no graph declares it.
fn node_field_decl(spec: &str, fix: &dyn Fn(&ValidName) -> Command) -> Result<FieldDecl, Problem> {
    if spec.split(':').next() == Some(umx_core::entity::ENTITY_KEY) {
        return Err(problems::reserved_name(umx_core::entity::ENTITY_KEY));
    }
    field_decl(spec, RESERVED_NODE_FIELDS, fix)
}

fn std_kind(
    o: &Ontology,
    graph: &ValidName,
    on: (&str, bool),
    field: &FieldDecl,
) -> Result<(), Problem> {
    let placed = if on.1 {
        std_relation_decl(o, on.0, &field.name)
    } else {
        std_decl(o, on.0, &field.name)
    };
    match placed {
        Some(f) if f.kind != field.kind && f.values.is_empty() => {
            Err(problems::std_field_conflict(
                graph,
                &ValidName::new(&field.name).expect("checked by field_decl"),
                f.kind,
            ))
        }
        _ => Ok(()),
    }
}

/// ws008 T2c (req-100): std's declaration of `field` on node type `kind` in a std graph, placed there or allowed on
/// any node; none in a graph without std. A graph's own declaration of it is an override, never an extension.
fn std_decl(o: &Ontology, kind: &str, field: &str) -> Option<FieldDecl> {
    o.std_version()?;
    standard::fields_of_type(kind)
        .find(|f| f.name == field)
        .map(|f| std_field_as_decl(&f))
}

/// [`std_decl`] on relation `relation`'s edges.
fn std_relation_decl(o: &Ontology, relation: &str, field: &str) -> Option<FieldDecl> {
    o.std_version()?;
    standard::fields_of_relation(relation)
        .find(|f| f.name == field)
        .map(|f| std_field_as_decl(&f))
}

/// Whether std places `field` on node type `kind` in a std graph, so the type's effective declaration carries it
/// whatever graph.json says (not a field std allows on any node, which a type has only once it declares it).
fn std_placed(o: &Ontology, kind: &str, field: &str) -> bool {
    o.std_version().is_some()
        && standard::fields().any(|f| f.name == field && f.types.contains(&kind))
}

/// [`std_placed`] on relation `relation`'s edges.
fn std_placed_on_relation(o: &Ontology, relation: &str, field: &str) -> bool {
    o.std_version().is_some()
        && standard::fields().any(|f| f.name == field && f.relations.contains(&relation))
}

/// The nodes of `kind` whose embedded text a role change of `field` alters: those holding non-blank text in it.
fn role_affected(graph: &Graph, kind: &str, field: &str) -> Vec<Id> {
    graph
        .nodes()
        .map(|n| n.to_node())
        .filter(|n| {
            n.kind == kind
                && n.fields
                    .get(field)
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| !v.trim().is_empty())
        })
        .map(|n| n.id)
        .collect()
}

fn std_field_as_decl(f: &standard::StdField) -> FieldDecl {
    let mut decl = FieldDecl::new(f.name, f.kind);
    decl.values = f.values.iter().map(|v| v.to_string()).collect();
    if f.role != f.kind.default_role() {
        decl.role = Some(f.role);
    }
    decl
}

fn check_values(field: &FieldDecl, values: &[String], retry: Command) -> Result<(), Problem> {
    if values.is_empty()
        || (field.kind == FieldKind::String && umx_core::ontology::valid_field_values(values))
    {
        return Ok(());
    }
    let mut suggested = Vec::new();
    for value in values.iter().take(umx_core::ontology::MAX_FIELD_VALUES) {
        let label = ValidName::alias(value)
            .map(|v| v.as_str().to_string())
            .or_else(|| umx_core::name::suggest_name(value));
        if let Some(label) = label
            && !suggested.contains(&label)
        {
            suggested.push(label);
        }
    }
    let fix = if suggested.is_empty() {
        retry.word("--values").word("<values>")
    } else {
        retry.field_values(&suggested)
    };
    Err(problems::field_values_invalid(
        &ValidName::new(&field.name).expect("checked field name"),
        fix,
    ))
}

fn set_values_command(graph: &ValidName, kind: &ValidName, field: &ValidName) -> Command {
    Command::new("ontology set field")
        .name(kind)
        .name(field)
        .word("--graph")
        .name(graph)
}

fn revalues(
    o: &Ontology,
    graph: &ValidName,
    kind: &ValidName,
    field: &ValidName,
    have: &FieldDecl,
    given: &FieldDecl,
) -> Result<Vec<String>, Problem> {
    let std = std_decl(o, kind.as_str(), field.as_str()).filter(|f| f.kind == have.kind);
    let mut merged = have.clone();
    merged.values = std.as_ref().map_or_else(Vec::new, |f| f.values.clone());
    for value in &have.values {
        if !merged.values.contains(value) {
            merged.values.push(value.clone());
        }
    }
    for value in &given.values {
        if !merged.values.contains(value) {
            merged.values.push(value.clone());
        }
    }
    if merged.values != have.values {
        check_values(
            &merged,
            &merged.values,
            set_values_command(graph, kind, field),
        )?;
    }
    if let Some(std) = std {
        merged.values.retain(|v| !std.values.contains(v));
    }
    Ok(merged.values)
}

pub(crate) fn value_warnings<'a>(
    graph: &ValidName,
    o: &Ontology,
    node: &umx_core::Node,
    fields: impl IntoIterator<Item = &'a str>,
) -> Vec<Problem> {
    let Some(kind) = ValidName::new(&node.kind) else {
        return Vec::new();
    };
    fields
        .into_iter()
        .filter_map(|name| {
            let value = node.fields.get(name)?;
            let field = o.field_decl(&node.kind, name)?;
            let value = value.as_str()?;
            if field.kind != FieldKind::String
                || field.values.is_empty()
                || field.values.iter().any(|v| v == value)
            {
                return None;
            }
            let name = ValidName::new(name)?;
            let fix = if umx_core::name::is_snake_case(value) {
                let mut c = Command::new("ontology add field")
                    .name(&kind)
                    .name_with(&name, ':', "string");
                if field.effective_role() == Role::Content {
                    c = c.word("--role").word("content");
                }
                c.word("--graph")
                    .name(graph)
                    .field_values(&[value.to_string()])
            } else {
                Command::new("node edit")
                    .id(node.id)
                    .word("--set")
                    .name_with(&name, '=', "<value>")
                    .word("--graph")
                    .name(graph)
            };
            Some(
                problems::field_value_off_vocabulary(&name, &field.values, fix)
                    .subject(node.id.to_text()),
            )
        })
        .collect()
}

pub(crate) fn soft_kind_warnings(
    o: &Ontology,
    before: Option<&Ontology>,
    graph: &ValidName,
) -> Vec<Problem> {
    if o.std_version().is_none() {
        return Vec::new();
    }
    umx_core::adopt::kind_clashes(o)
        .into_iter()
        .filter(|c| c.keeps_own_kind())
        .filter(|c| {
            before
                .and_then(|b| b.field_decl(&c.at.on, &c.at.field))
                .is_none_or(|f| f.kind != c.kind)
        })
        .filter_map(|c| {
            let field = ValidName::new(&c.at.field)?;
            Some(problems::std_field_conflict_stored(
                graph,
                &c.at.on,
                &field,
                (c.kind, c.std_kind),
                c.kind,
                Command::new("ontology show").word("--graph").name(graph),
            ))
        })
        .collect()
}

/// A type name umx keeps (ws008 T2b): `std` in every graph; in a std graph also the provenance types only umx writes
/// (a graph without std may use `model` or `agent` as its own words, as before).
fn reserved_type_name(o: &Ontology, name: &str) -> Result<(), Problem> {
    if name == standard::STD {
        return Err(problems::std_reserved());
    }
    if o.std_version().is_some() && PROVENANCE_TYPES.contains(&name) {
        return Err(problems::provenance_type_reserved(name));
    }
    Ok(())
}

/// A relation name umx keeps in every graph (ws008 T2b, dogfood): `std`, the stored provenance relations
/// (`reserved_relation`) and the derived ones (`derived_relation`; similar_to has its own refusal).
fn reserved_relation_name(name: &ValidName) -> Result<(), Problem> {
    if name.as_str() == standard::STD {
        return Err(problems::std_reserved());
    }
    if PROVENANCE_RELATIONS.contains(&name.as_str()) {
        return Err(problems::reserved_relation(name));
    }
    if PROVENANCE_DERIVED.contains(&name.as_str()) {
        return Err(problems::derived_relation(name));
    }
    Ok(())
}

/// `--from` and `--to` types (ws008 T2b), checked as node type names; `std` is never one.
fn end_types(
    types: &[String],
    fix: &dyn Fn(&ValidName) -> Command,
) -> Result<Vec<String>, Problem> {
    let mut out: Vec<String> = Vec::new();
    for t in types {
        check_name(t, "node type", fix)?;
        if t == standard::STD {
            return Err(problems::std_reserved());
        }
        if !out.contains(t) {
            out.push(t.clone());
        }
    }
    Ok(out)
}

/// A symmetric relation has no from and to end (D49), so it takes no expected end types.
fn symmetric_ends() -> Problem {
    problems::usage_invalid(
        "a symmetric relation takes no --from or --to (its ends have no direction)",
        "ontology",
    )
}

/// Adds `from` and `to` to the relation's expected end types; true when one was new.
fn widen(o: &mut Ontology, relation: &str, from: &[String], to: &[String]) -> bool {
    if from.is_empty() && to.is_empty() {
        return false;
    }
    let ends = o
        .relation_endpoints
        .entry(relation.to_string())
        .or_default();
    let mut changed = false;
    for (side, given) in [(&mut ends.from, from), (&mut ends.to, to)] {
        for t in given {
            if !side.contains(t) {
                side.push(t.clone());
                changed = true;
            }
        }
    }
    changed
}

/// A `--description` as stored, checked before anything changes; an empty one is none.
fn described(description: Option<&str>) -> Result<Option<String>, Problem> {
    Ok(match description {
        Some(d) => check_description(d)?,
        None => None,
    })
}

/// A re-declare (F71: unchanged) that gives a description other than the one stored is `already_declared`, whose
/// next sets it with `graph set --description` (plan 079): never silently dropped, and `ontology add` never edits.
fn redescribed(
    graph: &ValidName,
    kind: Described,
    name: &ValidName,
    field: Option<&ValidName>,
    given: &Option<String>,
    stored: Option<&String>,
) -> Result<(), Problem> {
    match given {
        Some(d) if stored != Some(d) => Err(problems::already_declared_describe(
            graph, kind, name, field,
        )),
        _ => Ok(()),
    }
}

/// Security F1: a `--description` that looks like a secret is warned once written (graph.json is tracked), naming
/// the setting and rule, never the text. Nothing written (unchanged), nothing warned.
fn warn_secret(
    done: Outcome<OntologyAdded>,
    text: Option<&str>,
    kind: Described,
    name: &ValidName,
    field: Option<&ValidName>,
) -> Outcome<OntologyAdded> {
    let mut done = done?;
    if done.value.unchanged {
        return Ok(done);
    }
    let graph = alias_name(&done.value.alias);
    let fix = Command::new("graph set")
        .name(&graph)
        .word("--description")
        .described(kind, name, field);
    done.notes
        .extend(settings::secret_warnings(&graph, "description", text, fix));
    Ok(done)
}

impl GraphStore {
    /// The ontology umx acts on ([`super::Loaded::ontology`]): a std graph's shows std v1 first, with the hint
    /// `std_entries_shown` (D69). `umx ontology show --std` is [`Ontology::std_base`], no graph needed.
    pub fn ontology_show(&self, ctx: &Ctx) -> Outcome<(String, Ontology)> {
        let alias = self.select(ctx, &[])?;
        let ontology = self.get(&alias).ontology();
        let mut notes: Vec<Problem> = self.stale_note(&alias).into_iter().collect();
        if let Some(std) = ontology.std_version() {
            notes.push(problems::std_entries_shown(&alias_name(&alias), std));
        }
        Ok(Done {
            notes,
            value: (alias.clone(), ontology.clone()),
        })
    }

    /// `fields` are `<name>:<kind>` specs. `description` (plan 079) is one line on what the type means.
    pub fn ontology_add_type(
        &mut self,
        ctx: &Ctx,
        name: &str,
        fields: &[String],
        description: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        let name = check_name(name, "node type", &|g| {
            ctx.same_graph(Command::new("ontology add type").name(g))
        })?;
        if name.as_str() == standard::STD {
            return Err(problems::std_reserved());
        }
        let fix = |g: &ValidName| {
            ctx.same_graph(
                Command::new("ontology add type")
                    .name(&name)
                    .word("--field")
                    .name_with(g, ':', "<kind>"),
            )
        };
        let fields = fields
            .iter()
            .map(|f| node_field_decl(f, &fix))
            .collect::<Result<Vec<_>, _>>()?;
        let description = described(description)?;
        let text = description.clone();
        let done = self.change_ontology(ctx, "type", &name, |o, graph| {
            reserved_type_name(o, name.as_str())?;
            for f in &fields {
                std_kind(o, graph, (name.as_str(), false), f)?;
            }
            // ws008 T2b: a std graph has std's types already; the name alone adds nothing.
            if o.std_version().is_some()
                && standard::std_type(name.as_str()).is_some()
                && o.type_decl(name.as_str()).is_none()
                && fields.is_empty()
                && description.is_none()
            {
                return Ok(false);
            }
            // Declared already: unchanged if it has every given field with the same kind (F71), else refused whole.
            if let Some(t) = o.type_decl(name.as_str()) {
                for f in &fields {
                    let field = ValidName::new(&f.name).expect("checked by field_decl");
                    match t.fields.iter().find(|have| have.name == f.name) {
                        Some(have) if have.kind == f.kind => {}
                        Some(have) => {
                            return Err(problems::field_conflict(graph, &name, &field, have.kind));
                        }
                        None => {
                            let fix = Command::new("ontology add field")
                                .name(&name)
                                .name_with(&field, ':', f.kind.as_str())
                                .word("--graph")
                                .name(graph);
                            return Err(problems::field_missing(graph, &name, &field, fix));
                        }
                    }
                }
                redescribed(
                    graph,
                    Described::Type,
                    &name,
                    None,
                    &description,
                    t.description.as_ref(),
                )?;
                return Ok(false);
            }
            let mut t = TypeDecl::new(name.as_str(), fields);
            t.description = description;
            o.types.push(t);
            Ok(true)
        });
        warn_secret(done, text.as_deref(), Described::Type, &name, None)
    }

    /// F96: sets a declared type's default `node list --type` columns, or removes them (`None`). Each column must be
    /// a field every node carries or one the type declares (`field_missing`), so a list never names nothing. The
    /// same list again (or a reset with none set) is unchanged and writes nothing.
    pub fn ontology_set_type(
        &mut self,
        ctx: &Ctx,
        name: &str,
        columns: Option<&[String]>,
    ) -> Outcome<OntologyAdded> {
        let name = check_name(name, "node type", &|g| {
            ctx.same_graph(Command::new("ontology set type").name(g).word("--reset"))
        })?;
        let mut wanted: Vec<ValidName> = Vec::new();
        for c in columns.unwrap_or_default() {
            let c = check_name(c, "field name", &|g| {
                ctx.same_graph(
                    Command::new("ontology set type")
                        .name(&name)
                        .word("--columns")
                        .name(g),
                )
            })?;
            if !wanted.contains(&c) {
                wanted.push(c);
            }
        }
        // Security N1 and N2: provenance stays hidden, and a type's list stays short.
        if let Some(key) = wanted.iter().find_map(|c| provenance_column(c.as_str())) {
            return Err(problems::usage_invalid(key, "ontology"));
        }
        if wanted.len() > MAX_TYPE_COLUMNS {
            return Err(problems::usage_invalid(
                "a type takes at most 32 default columns",
                "ontology",
            ));
        }
        self.change_ontology(ctx, "columns", &name, |o, graph| {
            // In a std graph a provenance type is umx's: never the graph's own declaration (ws008 T2b).
            reserved_type_name(o, name.as_str())?;
            // ws008 T2b: a std graph has std's types and the fields std places on them without graph.json declaring
            // them (`ontology add type person` writes nothing there). A column is known by the effective declaration,
            // as node list and the type_column_unknown load rule know it.
            let effective = o.effective();
            let Some(decl) = effective.type_decl(name.as_str()) else {
                return Err(problems::type_unknown(graph, &name));
            };
            if let Some(c) = wanted.iter().find(|c| !decl.column_known(c.as_str())) {
                let fix = Command::new("ontology add field")
                    .name(&name)
                    .name_with(c, ':', "<kind>")
                    .word("--graph")
                    .name(graph);
                return Err(problems::field_missing(graph, &name, c, fix));
            }
            if o.type_decl(name.as_str()).is_none() {
                if wanted.is_empty() {
                    return Ok(false);
                }
                o.types.push(TypeDecl::new(name.as_str(), Vec::new()));
            }
            let t = o
                .types
                .iter_mut()
                .find(|t| t.name == name.as_str())
                .expect("declared above");
            let columns: Vec<String> = wanted.iter().map(|c| c.as_str().to_string()).collect();
            if t.columns == columns {
                return Ok(false);
            }
            t.columns = columns;
            Ok(true)
        })
    }

    /// F96: the default columns `node list --type <kind>` shows when no `--columns` is given: `id`, then the type's
    /// list, when every graph in scope that declares the type gives it the same list. Columns the type no longer
    /// declares (a hand edit, warned about on load as `type_column_unknown`) are skipped. None means today's
    /// defaults; a selection error is left to the list itself to report.
    pub fn node_type_columns(
        &self,
        ctx: &Ctx,
        kind: &str,
        within: Option<&str>,
    ) -> Option<Vec<String>> {
        let aliases =
            if ctx.scope.is_none() && !ctx.graph.as_deref().is_some_and(|g| g.contains(',')) {
                vec![self.choose(ctx, &[within]).ok()?]
            } else {
                self.graphs_in_scope(ctx, None, "node").ok()?.aliases
            };
        let mut lists = aliases.iter().filter_map(|a| {
            let t = self.graphs.get(a)?.ontology().type_decl(kind)?;
            Some(
                t.columns
                    .iter()
                    .filter(|c| t.column_known(c))
                    // Security: a pulled graph.json cannot pass the set-time cap at read either.
                    .take(MAX_TYPE_COLUMNS)
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        });
        let first = lists.next()?;
        if first.is_empty() || lists.any(|l| l != first) {
            return None;
        }
        let mut out = vec!["id".to_string()];
        out.extend(first.into_iter().filter(|c| c != "id"));
        Some(out)
    }

    pub fn ontology_add_status(
        &mut self,
        ctx: &Ctx,
        name: &str,
        description: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        let name = check_name(name, "status", &|g| {
            ctx.same_graph(Command::new("ontology add status").name(g))
        })?;
        let description = described(description)?;
        let text = description.clone();
        let done = self.change_ontology(ctx, "status", &name, |o, graph| {
            if o.has_status(name.as_str()) {
                let stored = o.status_descriptions.get(name.as_str());
                redescribed(graph, Described::Status, &name, None, &description, stored)?;
                return Ok(false);
            }
            o.statuses.push(name.as_str().to_string());
            if let Some(d) = description {
                o.status_descriptions.insert(name.as_str().to_string(), d);
            }
            Ok(true)
        });
        warn_secret(done, text.as_deref(), Described::Status, &name, None)
    }

    /// `ontology add tag <facet>:<value>|<facet>:*` (ws008 t2b, req-117): declares a std tag, or every value of a
    /// facet, in stored form. A std graph only (`tag_needs_std`); one declared already is `unchanged`.
    pub fn ontology_add_tag(&mut self, ctx: &Ctx, tag: &str) -> Outcome<OntologyAdded> {
        let alias = self.select_write(ctx, &[])?;
        if !super::tags::is_std(self.get(&alias).ontology()) {
            return Err(problems::tag_needs_std(&alias_name(&alias)));
        }
        let given = [tag.to_string()];
        let retry = super::tags::TagRetry {
            base: ctx.same_graph(Command::new("ontology add tag")),
            flags: alloc::vec![("", &given[..], super::tags::Reads::Patterns)],
        };
        let decl = umx_core::tags::parse_declaration(tag).map_err(|r| retry.refused(&r))?;
        let name = decl.clone();
        self.change_ontology_named(ctx, "tag", &name, |o, _| {
            if o.tags.contains(&decl) {
                return Ok(false);
            }
            o.tags.push(decl);
            o.tags.sort_unstable();
            Ok(true)
        })
    }

    /// Declares a relation. With `symmetric` (D49) it marks the relation symmetric, declaring it first if it is new;
    /// `part_of`, `similar_to` and `instance_of`, which are directed by what they mean, are refused (`symmetric_refused`). A relation
    /// declared already with the same symmetric flag is unchanged (F71); a plain add of a symmetric relation (one of
    /// the built-in ones included) is `already_declared` as a symmetric relation.
    pub fn ontology_add_relation(
        &mut self,
        ctx: &Ctx,
        name: &str,
        symmetric: bool,
        description: Option<&str>,
    ) -> Outcome<OntologyAdded> {
        self.ontology_add_relation_with_ends(ctx, name, symmetric, description, (&[], &[]))
    }

    /// [`Self::ontology_add_relation`] with `--from` and `--to` end types (ws008 T2b, req-99): soft expectations kept
    /// in graph.json's `relation_endpoints`; on a std relation they widen std's. An add that only widens (the relation
    /// declared, or std's in a std graph) writes the new types alone; all of them there already is unchanged.
    /// The names umx keeps are refused in every graph: `std` (`reserved_name`), the stored provenance relations
    /// (`reserved_relation`) and the derived ones (`derived_relation`).
    pub fn ontology_add_relation_with_ends(
        &mut self,
        ctx: &Ctx,
        name: &str,
        symmetric: bool,
        description: Option<&str>,
        ends: (&[String], &[String]),
    ) -> Outcome<OntologyAdded> {
        let name = check_name(name, "relation", &|g| {
            ctx.same_graph(Command::new("ontology add relation").name(g))
        })?;
        reserved_relation_name(&name)?;
        let relation = &name;
        let end_fix = |flag: &'static str| {
            move |g: &ValidName| {
                ctx.same_graph(
                    Command::new("ontology add relation")
                        .name(relation)
                        .word(flag)
                        .name(g),
                )
            }
        };
        let from = end_types(ends.0, &end_fix("--from"))?;
        let to = end_types(ends.1, &end_fix("--to"))?;
        let description = described(description)?;
        let text = description.clone();
        // `--symmetric` first: similar_to then gets `symmetric_refused`, which names what was asked.
        if symmetric {
            if !from.is_empty() || !to.is_empty() {
                return Err(symmetric_ends());
            }
            let done = self.mark_symmetric(ctx, &name, description);
            return warn_secret(done, text.as_deref(), Described::Relation, &name, None);
        }
        if name.as_str() == SIMILAR_TO {
            return Err(problems::similar_to_reserved());
        }
        let done = self.change_ontology(ctx, "relation", &name, |o, graph| {
            let symmetric = o.is_symmetric(name.as_str());
            let widening = !from.is_empty() || !to.is_empty();
            if symmetric && widening {
                return Err(symmetric_ends());
            }
            if symmetric {
                return Err(problems::already_declared(
                    graph,
                    "symmetric relation",
                    &name,
                ));
            }
            // ws008 T2b (req-99): a std graph has std's relations already, so the name alone adds nothing.
            let std_relation = o.std_version().is_some()
                && standard::std_relation(name.as_str()).is_some()
                && description.is_none();
            let declared = symmetric || o.has_relation(name.as_str()) || std_relation;
            if declared {
                let stored = o.relation_descriptions.get(name.as_str());
                redescribed(
                    graph,
                    Described::Relation,
                    &name,
                    None,
                    &description,
                    stored,
                )?;
            }
            // `--from`/`--to` add expected end types (soft), widening a std relation's. Nothing above fails after this.
            let widened = widen(o, name.as_str(), &from, &to);
            if declared {
                return Ok(widened);
            }
            o.relations.push(name.as_str().to_string());
            if let Some(d) = description {
                o.relation_descriptions.insert(name.as_str().to_string(), d);
            }
            Ok(true)
        });
        warn_secret(done, text.as_deref(), Described::Relation, &name, None)
    }

    /// `ontology add relation <r> --symmetric`. Edges the graph already holds both ways become one edge at once, as at
    /// load (R1): the lower-id-from edge stays and the other's line is kept as it is in edges.jsonl
    /// (`symmetric_duplicate`); the views' change log reports the other as removed. That line is found before
    /// anything changes, so a read failure changes nothing; a failed graph.json write puts the ontology back and
    /// leaves the edges as they were.
    /// A `description` is set with the mark, in the same graph.json write.
    fn mark_symmetric(
        &mut self,
        ctx: &Ctx,
        name: &ValidName,
        description: Option<String>,
    ) -> Outcome<OntologyAdded> {
        let n = name.as_str();
        if n == PART_OF || n == SIMILAR_TO || n == INSTANCE_OF {
            return Err(problems::symmetric_refused(name));
        }
        let alias = self.select_write(ctx, &[])?;
        let graph = alias_name(&alias);
        let files = self.ports.files.clone();
        let roots = self.roots.clone();
        let l = self
            .graphs
            .get_mut(&alias)
            .expect("selected graph is loaded");
        if l.meta.ontology.is_symmetric(n) {
            let stored = l.meta.ontology.relation_descriptions.get(n);
            redescribed(
                &graph,
                Described::Relation,
                name,
                None,
                &description,
                stored,
            )?;
            return Ok(Done::ok(OntologyAdded {
                alias,
                added: "symmetric relation",
                name: n.to_string(),
                unchanged: true,
            }));
        }
        // Expected end types given earlier have a direction a symmetric relation has not.
        if l.meta.ontology.relation_endpoints.contains_key(n) {
            return Err(symmetric_ends());
        }
        let before = l.meta.ontology.clone();
        l.meta.ontology.symmetric.push(n.to_string());
        if let Some(d) = description {
            l.meta
                .ontology
                .relation_descriptions
                .insert(n.to_string(), d);
        }
        if !l.meta.ontology.has_relation(n) {
            l.meta.ontology.relations.push(n.to_string());
        }
        let pairs = super::external::reversed_pairs(&l.graph, l.meta.id, &[n]);
        let lines = if pairs.is_empty() {
            Ok(Vec::new())
        } else {
            files
                .read(&l.dir, GraphFile::Edges)
                .map_err(|e| files_problem(&l.dir, &roots, GraphFile::Edges, e))
                .and_then(|bytes| {
                    let (bytes, _) =
                        super::graph::normalize_own_ends(bytes.unwrap_or_default(), l.meta.id)?;
                    edge_lines(&bytes, &pairs)
                        .map_err(|e| problems::decode_failed(&e, bytes.len() as u64))
                })
        };
        let lines = match lines {
            Ok(lines) => lines,
            Err(p) => {
                l.meta.ontology = before;
                return Err(p);
            }
        };
        // The edges that stop being edges, for the views' change log. Committed while they are still in the graph,
        // so the feed's one filter can still see their ends (and leaves out any between provenance nodes).
        let mut touched = Touched::default();
        touched.edges.removed = pairs.iter().map(|&(dropped, _)| dropped).collect();
        let mut warnings = persist(files.as_ref(), &alias, l, &[GraphFile::Graph], touched)
            .inspect_err(|_| l.meta.ontology = before)?;
        warnings.extend(
            l.graph
                .keep_reversed(&pairs, lines)
                .iter()
                .map(|r| problems::symmetric_duplicate(&graph, r.edge, r.in_use)),
        );
        if !pairs.is_empty() {
            self.incoming.remove_source(l.instance);
            self.incoming.add_graph(l.instance, &l.graph);
        }
        Ok(Done {
            value: OntologyAdded {
                alias,
                added: "symmetric relation",
                name: n.to_string(),
                unchanged: false,
            },
            notes: warnings,
        })
    }

    /// Declares a field on a type, declaring the type too if it is not yet (soft ontology: the warning's `next`
    /// must always work). The same field with the same kind and role is unchanged (F71); with another kind,
    /// `field_conflict`; with another role, `field_role_conflict` (a role changes only by `ontology set field`).
    pub fn ontology_add_field(
        &mut self,
        ctx: &Ctx,
        type_name: &str,
        spec: &str,
        description: Option<&str>,
        role: Option<&str>,
        values: &[String],
    ) -> Outcome<OntologyAdded> {
        let role = role.map(parse_role).transpose()?;
        let type_name = check_name(type_name, "node type", &|g| {
            ctx.same_graph(
                Command::new("ontology add field")
                    .name(g)
                    .word("<name>:<kind>"),
            )
        })?;
        if type_name.as_str() == standard::STD {
            return Err(problems::std_reserved());
        }
        let fix = |g: &ValidName| {
            ctx.same_graph(
                Command::new("ontology add field")
                    .name(&type_name)
                    .name_with(g, ':', "<kind>"),
            )
        };
        let mut field = node_field_decl(spec, &fix)?;
        check_values(
            &field,
            values,
            ctx.same_graph(
                Command::new("ontology add field")
                    .name(&type_name)
                    .name_with(
                        &ValidName::new(&field.name).expect("checked name"),
                        ':',
                        "string",
                    ),
            ),
        )?;
        field.values = values.to_vec();
        if let Some(role) = role {
            check_role(&field, role)?;
            field.role = Some(role);
        }
        field.description = described(description)?;
        let text = field.description.clone();
        let name = ValidName::new(&field.name).expect("checked by field_decl");
        let done = self.change_ontology(ctx, "field", &name.clone(), |o, graph| {
            std_kind(o, graph, (type_name.as_str(), false), &field)?;
            // The effective declaration (req-100): a field std already gives the type counts as declared, so the
            // same field again writes nothing and graph.json holds only the graph's own extensions.
            let own = o.field_decl(type_name.as_str(), &field.name).cloned();
            let from_std = own.is_none();
            let have = own.or_else(|| std_decl(o, type_name.as_str(), &field.name));
            match have {
                Some(f) if f.kind == field.kind && f.effective_role() == field.effective_role() => {
                    let merged_values = revalues(o, graph, &type_name, &name, &f, &field)?;
                    let changed_values = if from_std {
                        !merged_values.is_empty()
                    } else {
                        merged_values != f.values
                    };
                    // A description for a std field is the graph's own declaration of it, with std's kind (as
                    // `ontology add type person --description` is); std's entry has no description to clash with.
                    if !(from_std && field.description.is_some()) {
                        redescribed(
                            graph,
                            Described::Field,
                            &type_name,
                            Some(&name),
                            &field.description,
                            f.description.as_ref(),
                        )?;
                        if !changed_values {
                            return Ok(false);
                        }
                    }
                    field.values = merged_values;
                    if !from_std {
                        o.types
                            .iter_mut()
                            .find(|t| t.name == type_name.as_str())
                            .expect("declared type")
                            .fields
                            .iter_mut()
                            .find(|f| f.name == field.name)
                            .expect("declared field")
                            .values = field.values;
                        return Ok(true);
                    }
                }
                Some(f) if f.kind == field.kind => {
                    return Err(problems::field_role_conflict(
                        graph,
                        &type_name,
                        &name,
                        f.effective_role(),
                    ));
                }
                Some(f) if from_std && !f.values.is_empty() => {}
                Some(f) => return Err(problems::field_conflict(graph, &type_name, &name, f.kind)),
                None => {}
            }
            if o.type_decl(type_name.as_str()).is_none() {
                o.types.push(TypeDecl::new(type_name.as_str(), Vec::new()));
            }
            o.types
                .iter_mut()
                .find(|t| t.name == type_name.as_str())
                .expect("declared above")
                .fields
                .push(field);
            Ok(true)
        });
        warn_secret(
            done,
            text.as_deref(),
            Described::Field,
            &type_name,
            Some(&name),
        )
    }

    /// `ontology add field --relation <r> <name>:<kind>` (F92): declares a custom field on the relation's edges, and
    /// the relation too if it is new (soft ontology, as `ontology add field <type>` declares the type). The same field
    /// with the same kind is unchanged and writes nothing; another kind is `relation_field_conflict`. The name follows
    /// the edge rules: an edge's own keys are `reserved_name`. A derived relation (`similar_to`, `authored_by`, `edited_by`) is refused.
    pub fn ontology_add_relation_field(
        &mut self,
        ctx: &Ctx,
        relation: &str,
        spec: &str,
    ) -> Outcome<OntologyAdded> {
        // Derived relations (similar_to, authored_by, edited_by) are never stored, so they take no fields (as edge add).
        let rel = super::edge::check_relation_for(relation, &|g| {
            ctx.same_graph(
                Command::new("ontology add field")
                    .word("--relation")
                    .name(g)
                    .word("<name>:<kind>"),
            )
        })?;
        let fix = |g: &ValidName| {
            ctx.same_graph(
                Command::new("ontology add field")
                    .word("--relation")
                    .name(&rel)
                    .name_with(g, ':', "<kind>"),
            )
        };
        let field = field_decl(spec, RESERVED_EDGE_FIELDS, &fix)?;
        let name = ValidName::new(&field.name).expect("checked by field_decl");
        self.change_ontology(ctx, "relation field", &name.clone(), |o, graph| {
            std_kind(o, graph, (rel.as_str(), true), &field)?;
            let declared = o
                .relation_fields
                .get(rel.as_str())
                .and_then(|fields| fields.iter().find(|f| f.name == field.name))
                .cloned()
                .or_else(|| std_relation_decl(o, rel.as_str(), &field.name));
            match declared {
                Some(f) if f.kind == field.kind => return Ok(false),
                Some(f) => {
                    return Err(problems::relation_field_conflict(
                        graph, &rel, &name, f.kind,
                    ));
                }
                None => {}
            }
            if !o.has_relation(rel.as_str()) {
                o.relations.push(rel.as_str().to_string());
            }
            o.relation_fields
                .entry(rel.as_str().to_string())
                .or_default()
                .push(field);
            Ok(true)
        })
    }

    /// `ontology add field` on several types, or every declared type with `all` (F71): one graph.json write, all or
    /// nothing. Every type must be declared already, std's counting in a std graph (`type_unknown`); one that has
    /// the field (std's own counting, req-100) with another kind is `field_conflict`, or with another role
    /// `field_role_conflict`; one that has it with the same kind and role is left as it is and listed as `unchanged`
    /// (all of them unchanged: no write). One error per culprit, at most [`MAX_BATCH_ERRORS`]. The single-type form is
    /// [`Self::ontology_add_field`].
    pub fn ontology_add_field_many(
        &mut self,
        ctx: &Ctx,
        types: &[String],
        all: bool,
        spec: &str,
        options: (Option<&str>, Option<&str>, &[String]),
    ) -> Result<Done<FieldsAdded>, Vec<Problem>> {
        let (description, role, values) = options;
        let role = role.map(parse_role).transpose().map_err(|p| vec![p])?;
        if all == !types.is_empty() {
            return Err(vec![problems::usage_invalid(
                "name the types or give --all-types (one of the two)",
                "ontology",
            )]);
        }
        let type_name = |t: &str| {
            check_name(t, "node type", &|g| {
                ctx.same_graph(
                    Command::new("ontology add field")
                        .name(g)
                        .word("<name>:<kind>"),
                )
            })
        };
        let mut names: Vec<ValidName> = Vec::new();
        let mut errors = Vec::new();
        for t in types {
            match type_name(t) {
                Ok(n) if !names.contains(&n) => names.push(n),
                Ok(_) => {}
                Err(p) => errors.push(p),
            }
        }
        cap(&mut errors)?;
        let fix = |g: &ValidName| {
            let c = Command::new("ontology add field");
            let c = if all {
                c.word("--all-types")
            } else if names.len() <= 64 {
                // The types as typed, a comma list (F73).
                c.names(&names, ",")
            } else {
                c.word("<types>")
            };
            ctx.same_graph(c.name_with(g, ':', "<kind>"))
        };
        let mut field = node_field_decl(spec, &fix).map_err(|p| vec![p])?;
        let retry = if all {
            Command::new("ontology add field").word("--all-types")
        } else if names.len() <= 64 {
            Command::new("ontology add field").names(&names, ",")
        } else {
            Command::new("ontology add field").word("<types>")
        };
        check_values(
            &field,
            values,
            ctx.same_graph(retry.name_with(
                &ValidName::new(&field.name).expect("checked name"),
                ':',
                "string",
            )),
        )
        .map_err(|p| vec![p])?;
        field.values = values.to_vec();
        if let Some(role) = role {
            check_role(&field, role).map_err(|p| vec![p])?;
            field.role = Some(role);
        }
        field.description = described(description).map_err(|p| vec![p])?;
        let fname = ValidName::new(&field.name).expect("checked by field_decl");
        let alias = self.select_write(ctx, &[]).map_err(|p| vec![p])?;
        let graph = alias_name(&alias);
        let files = self.ports.files.clone();
        let l = self
            .graphs
            .get_mut(&alias)
            .expect("selected graph is loaded");
        let o = &l.meta.ontology;
        if all {
            // A loaded graph.json can declare a name the explicit list would refuse: refuse it the same way, whole.
            for t in &o.types {
                match type_name(&t.name) {
                    Ok(n) => names.push(n),
                    Err(p) => errors.push(p),
                }
            }
            cap(&mut errors)?;
            if names.is_empty() {
                return Err(vec![problems::usage_invalid(
                    "the graph declares no types (add one with ontology add type)",
                    "ontology",
                )]);
            }
        }
        // A std type counts as declared in a std graph (req-100), as it does for the single-type form and node list.
        let effective = o.effective();
        let (mut added, mut unchanged) = (Vec::new(), Vec::new());
        let mut updated_values = alloc::collections::BTreeMap::new();
        for n in &names {
            if let Err(p) = std_kind(o, &graph, (n.as_str(), false), &field) {
                errors.push(p);
                continue;
            }
            if effective.type_decl(n.as_str()).is_none() {
                errors.push(problems::type_unknown(&graph, n));
                continue;
            }
            let own = o.field_decl(n.as_str(), &field.name).cloned();
            let from_std = own.is_none();
            let have = own.or_else(|| std_decl(o, n.as_str(), &field.name));
            let same = |f: &FieldDecl| {
                f.kind == field.kind && f.effective_role() == field.effective_role()
            };
            match have {
                None => added.push(n.clone()),
                // As the single-type form: a description for a std field is the graph's own declaration of it.
                Some(f) if same(&f) => {
                    let merged_values = match revalues(o, &graph, n, &fname, &f, &field) {
                        Ok(values) => values,
                        Err(p) => {
                            errors.push(p);
                            continue;
                        }
                    };
                    let changed_values = if from_std {
                        !merged_values.is_empty()
                    } else {
                        merged_values != f.values
                    };
                    if from_std && field.description.is_some() {
                        updated_values.insert(n.as_str().to_string(), merged_values);
                        added.push(n.clone());
                        continue;
                    }
                    match redescribed(
                        &graph,
                        Described::Field,
                        n,
                        Some(&fname),
                        &field.description,
                        f.description.as_ref(),
                    ) {
                        Ok(()) if changed_values => {
                            updated_values.insert(n.as_str().to_string(), merged_values);
                            added.push(n.clone());
                        }
                        Ok(()) => unchanged.push(n.as_str().to_string()),
                        Err(p) => errors.push(p),
                    }
                }
                Some(f) if f.kind == field.kind => errors.push(problems::field_role_conflict(
                    &graph,
                    n,
                    &fname,
                    f.effective_role(),
                )),
                Some(f) if from_std && !f.values.is_empty() => added.push(n.clone()),
                Some(f) => errors.push(problems::field_conflict(&graph, n, &fname, f.kind)),
            }
        }
        cap(&mut errors)?;
        if added.is_empty() {
            return Ok(Done::ok(FieldsAdded {
                alias,
                name: field.name,
                types: Vec::new(),
                unchanged,
            }));
        }
        let before = l.meta.ontology.clone();
        for n in &added {
            let types = &mut l.meta.ontology.types;
            if !types.iter().any(|t| t.name == n.as_str()) {
                types.push(TypeDecl::new(n.as_str(), Vec::new()));
            }
        }
        for t in l.meta.ontology.types.iter_mut() {
            if added.iter().any(|n| n.as_str() == t.name) {
                let mut added_field = field.clone();
                if let Some(values) = updated_values.get(&t.name) {
                    added_field.values = values.clone();
                }
                if let Some(existing) = t.fields.iter_mut().find(|f| f.name == field.name) {
                    existing.values = added_field.values;
                } else {
                    t.fields.push(added_field);
                }
            }
        }
        let mut warnings = persist(
            files.as_ref(),
            &alias,
            l,
            &[GraphFile::Graph],
            Touched::default(),
        )
        .inspect_err(|_| l.meta.ontology = before.clone())
        .map_err(|p| vec![p])?;
        warnings.extend(soft_kind_warnings(&l.meta.ontology, Some(&before), &graph));
        let fix = Command::new("graph set")
            .name(&graph)
            .word("--description")
            .described(Described::Field, &added[0], Some(&fname));
        warnings.extend(settings::secret_warnings(
            &graph,
            "description",
            field.description.as_deref(),
            fix,
        ));
        Ok(Done {
            value: FieldsAdded {
                alias,
                name: field.name,
                types: added.iter().map(|n| n.as_str().to_string()).collect(),
                unchanged,
            },
            notes: warnings,
        })
    }

    /// `ontology set field <type> <field> --role data|content` (D56): states the field's role. The nodes of that type
    /// whose text in the field the change adds to or takes from what is embedded are returned, for the services to
    /// mark pending and queue: a role change is an explicit act, so the worker remakes them, never silently
    /// (req-97). Stating the role the field already states writes nothing.
    pub fn ontology_set_field(
        &mut self,
        ctx: &Ctx,
        type_name: &str,
        field: &str,
        role: Option<&str>,
        values: Option<&[String]>,
    ) -> Outcome<RoleSet> {
        let role = role.map(parse_role).transpose()?;
        if role.is_none() && values.is_none() {
            return Err(problems::usage_invalid(
                "give --role, --values or --clear-values",
                "ontology",
            ));
        }
        let options = |mut c: Command| {
            if let Some(role) = role {
                c = c.word("--role").word(role.as_str());
            }
            if let Some(values) = values {
                c = c.field_values(values);
            }
            c
        };
        let type_name = check_name(type_name, "node type", &|g| {
            ctx.same_graph(options(
                Command::new("ontology set field").name(g).word("<field>"),
            ))
        })?;
        let field = check_name(field, "field name", &|g| {
            ctx.same_graph(options(
                Command::new("ontology set field").name(&type_name).name(g),
            ))
        })?;
        let alias = self.select_write(ctx, &[])?;
        let graph = alias_name(&alias);
        let files = self.ports.files.clone();
        let l = self
            .graphs
            .get_mut(&alias)
            .expect("selected graph is loaded");
        // A std graph's fields count (req-98): a std field graph.json does not declare gets the graph's own
        // declaration of it here, with std's kind; std's entries are never written.
        let Some(effective) = l
            .ontology()
            .field_decl(type_name.as_str(), field.as_str())
            .cloned()
        else {
            return Err(problems::field_not_declared(&graph, &type_name, &field));
        };
        let std = std_decl(&l.meta.ontology, type_name.as_str(), field.as_str())
            .filter(|f| f.kind == effective.kind);
        let decl = l
            .meta
            .ontology
            .field_decl(type_name.as_str(), field.as_str())
            .cloned()
            .unwrap_or_else(|| {
                let mut own = effective.clone();
                own.values.clear();
                own
            });
        if let Some(role) = role {
            check_role(&decl, role)?;
        }
        let mut updated = decl.clone();
        if let Some(std) = &std {
            updated.values.retain(|v| !std.values.contains(v));
        }
        if let Some(role) = role {
            updated.role = Some(role);
        }
        if let Some(values) = values {
            let retry = set_values_command(&graph, &type_name, &field);
            if decl.kind != FieldKind::String {
                return Err(problems::field_values_invalid(
                    &field,
                    Command::new("ontology show").word("--graph").name(&graph),
                ));
            }
            check_values(&decl, values, retry.clone())?;
            updated.values.clear();
            for value in values {
                if !std.as_ref().is_some_and(|s| s.values.contains(value)) {
                    updated.values.push(value.clone());
                }
            }
            let mut effective_values = std.as_ref().map_or_else(Vec::new, |s| s.values.clone());
            effective_values.extend(updated.values.clone());
            check_values(&updated, &effective_values, retry)?;
        }
        let mut shown_values = std.map_or_else(Vec::new, |s| s.values);
        shown_values.extend(updated.values.clone());
        let mut out = RoleSet {
            alias: alias.clone(),
            type_name: type_name.as_str().to_string(),
            field: field.as_str().to_string(),
            role: role.unwrap_or_else(|| updated.effective_role()),
            values: shown_values,
            changed: false,
            affected: Vec::new(),
        };
        if decl == updated {
            return Ok(Done::ok(out));
        }
        let was = decl.effective_role();
        let before = l.meta.ontology.clone();
        let types = &mut l.meta.ontology.types;
        if !types.iter().any(|t| t.name == type_name.as_str()) {
            types.push(TypeDecl::new(type_name.as_str(), Vec::new()));
        }
        let t = types
            .iter_mut()
            .find(|t| t.name == type_name.as_str())
            .expect("declared above");
        if !t.fields.iter().any(|f| f.name == field.as_str()) {
            t.fields.push(decl);
        }
        let decl = t
            .fields
            .iter_mut()
            .find(|f| f.name == field.as_str())
            .expect("declared above");
        *decl = updated;
        let now = decl.effective_role();
        let warnings = persist(
            files.as_ref(),
            &alias,
            l,
            &[GraphFile::Graph],
            Touched::default(),
        )
        .inspect_err(|_| l.meta.ontology = before)?;
        out.changed = true;
        if was != now {
            out.affected = role_affected(&l.graph, type_name.as_str(), field.as_str());
        }
        Ok(Done {
            value: out,
            notes: warnings,
        })
    }

    /// `ontology remove type <name>` (F76): refused while a node has the type; its field declarations and description
    /// go with it.
    pub fn ontology_remove_type(&mut self, ctx: &Ctx, name: &str) -> Outcome<OntologyRemoved> {
        let name = removable(ctx, "type", name)?;
        self.remove_from_ontology(ctx, "type", &name, |o, g, graph| {
            let Some(i) = o.types.iter().position(|t| t.name == name.as_str()) else {
                return Ok(false);
            };
            in_use(
                graph,
                "type",
                &name,
                uses(&g.census().types, name.as_str()),
                "nodes",
                || Command::new("node list").word("--type").name(&name),
            )?;
            o.types.remove(i);
            Ok(true)
        })
    }

    /// `ontology remove status <name>` (F76): refused while a node is in it now (history does not count).
    pub fn ontology_remove_status(&mut self, ctx: &Ctx, name: &str) -> Outcome<OntologyRemoved> {
        let name = removable(ctx, "status", name)?;
        self.remove_from_ontology(ctx, "status", &name, |o, g, graph| {
            let n = name.as_str();
            if !o.has_status(n) && !o.status_descriptions.contains_key(n) {
                return Ok(false);
            }
            in_use(
                graph,
                "status",
                &name,
                uses(&g.census().statuses, n),
                "nodes",
                || Command::new("node list").word("--status").name(&name),
            )?;
            o.statuses.retain(|s| s != n);
            o.status_descriptions.remove(n);
            Ok(true)
        })
    }

    /// `ontology remove relation <name>` (F76): refused while an edge has it; its symmetric mark, description and
    /// declared fields (F92) go with it.
    pub fn ontology_remove_relation(&mut self, ctx: &Ctx, name: &str) -> Outcome<OntologyRemoved> {
        let name = removable(ctx, "relation", name)?;
        self.remove_from_ontology(ctx, "relation", &name, |o, g, graph| {
            let n = name.as_str();
            let declared = o.has_relation(n)
                || o.symmetric.iter().any(|s| s == n)
                || o.relation_descriptions.contains_key(n)
                || o.relation_fields.contains_key(n);
            if !declared {
                return Ok(false);
            }
            in_use(
                graph,
                "relation",
                &name,
                uses(&g.census().relations, n),
                "edges",
                || Command::new("edge list").word("--type").name(&name),
            )?;
            o.relations.retain(|r| r != n);
            o.symmetric.retain(|r| r != n);
            o.relation_descriptions.remove(n);
            o.relation_fields.remove(n);
            Ok(true)
        })
    }

    /// `ontology remove field <type> <name>` (F76): refused while a node of the type holds the field (one pass over
    /// the nodes) or the type's default columns (F96) name it. Another type declaring the same field keeps it. In a
    /// std graph a field std gives the type is refused (`reserved_name`, req-100); the graph's own override of a field
    /// std places on the type is removed whatever uses it, and std's declaration comes back (`role` and `affected`
    /// say when its role changed).
    pub fn ontology_remove_field(
        &mut self,
        ctx: &Ctx,
        type_name: &str,
        field: &str,
    ) -> Outcome<OntologyRemoved> {
        let kind = check_name(type_name, "node type", &|g| {
            ctx.same_graph(Command::new("ontology remove field").name(g).word("<name>"))
        })?;
        if let Some(p) = builtin("field", field) {
            return Err(p);
        }
        let name = check_name(field, "field name", &|g| {
            ctx.same_graph(Command::new("ontology remove field").name(&kind).name(g))
        })?;
        let mut flipped = None;
        let mut done = self.remove_from_ontology(ctx, "field", &name, |o, g, graph| {
            let std = std_decl(o, kind.as_str(), name.as_str()).is_some();
            let placed = std_placed(o, kind.as_str(), name.as_str())
                .then(|| std_decl(o, kind.as_str(), name.as_str()))
                .flatten();
            let own = o
                .types
                .iter_mut()
                .find(|t| t.name == kind.as_str())
                .and_then(|t| {
                    let i = t.fields.iter().position(|f| f.name == name.as_str())?;
                    Some((t, i))
                });
            let Some((t, i)) = own else {
                // req-100: std's own fields stay; a graph only adds to a std type.
                return if std {
                    Err(problems::std_field_kept(&name))
                } else {
                    Ok(false)
                };
            };
            if let Some(placed) = placed {
                // The graph's own override of a field std places on the type (a role from `ontology set field`):
                // std's declaration still backs the nodes and columns that use it, and std's role comes back.
                let was = t.fields.remove(i).effective_role();
                let now = placed.effective_role();
                if was != now {
                    flipped = Some(now);
                }
                return Ok(true);
            }
            let holders = g
                .nodes()
                .filter(|n| n.kind() == kind.as_str())
                .filter(|n| n.custom_field_names().any(|f| f == name.as_str()))
                .count();
            in_use(graph, "field", &name, holders, "nodes", || {
                Command::new("node list")
                    .word("--type")
                    .name(&kind)
                    .word("--columns")
                    .name(&name)
            })?;
            // F96: the type's default columns name it, so node list would show a column no declaration backs.
            let column = usize::from(t.columns.iter().any(|c| c == name.as_str()));
            in_use(graph, "field", &name, column, "type column lists", || {
                Command::new("ontology set type")
                    .name(&kind)
                    .word("--reset")
            })?;
            t.fields.remove(i);
            Ok(true)
        })?;
        if let Some(role) = flipped {
            let l = self
                .graphs
                .get(&done.value.alias)
                .expect("selected graph is loaded");
            done.value.role = Some(role);
            done.value.affected = role_affected(&l.graph, kind.as_str(), name.as_str());
        }
        Ok(done)
    }

    /// `ontology remove field --relation <r> <name>` (F76 on F92): refused while an edge of the relation holds the
    /// field (one pass over the edges).
    pub fn ontology_remove_relation_field(
        &mut self,
        ctx: &Ctx,
        relation: &str,
        field: &str,
    ) -> Outcome<OntologyRemoved> {
        let rel = check_name(relation, "relation", &|g| {
            ctx.same_graph(
                Command::new("ontology remove field")
                    .word("--relation")
                    .name(g)
                    .word("<name>"),
            )
        })?;
        if let Some(p) = builtin("relation field", field) {
            return Err(p);
        }
        let name = check_name(field, "field name", &|g| {
            ctx.same_graph(
                Command::new("ontology remove field")
                    .word("--relation")
                    .name(&rel)
                    .name(g),
            )
        })?;
        self.remove_from_ontology(ctx, "relation field", &name, |o, g, graph| {
            let std = std_relation_decl(o, rel.as_str(), name.as_str()).is_some();
            let placed = std_placed_on_relation(o, rel.as_str(), name.as_str());
            let own = o.relation_fields.get_mut(rel.as_str()).and_then(|fields| {
                let i = fields.iter().position(|f| f.name == name.as_str())?;
                Some((fields, i))
            });
            let Some((fields, i)) = own else {
                return if std {
                    Err(problems::std_field_kept(&name))
                } else {
                    Ok(false)
                };
            };
            if !placed {
                let holders = g
                    .edges()
                    .filter(|e| e.kind() == rel.as_str())
                    .filter(|e| e.field(name.as_str()).is_some())
                    .count();
                in_use(graph, "relation field", &name, holders, "edges", || {
                    Command::new("edge list")
                        .word("--type")
                        .name(&rel)
                        .word("--columns")
                        .name(&name)
                })?;
            }
            fields.remove(i);
            if fields.is_empty() {
                o.relation_fields.remove(rel.as_str());
            }
            Ok(true)
        })
    }

    /// One `ontology remove`: `change` sees the ontology and the graph, and gives `false` when the entry is not
    /// declared (nothing written). A refusal or a failed write leaves the ontology as it was.
    fn remove_from_ontology(
        &mut self,
        ctx: &Ctx,
        removed: &'static str,
        name: &ValidName,
        change: impl FnOnce(&mut Ontology, &Graph, &ValidName) -> Result<bool, Problem>,
    ) -> Outcome<OntologyRemoved> {
        let alias = self.select_write(ctx, &[])?;
        let files = self.ports.files.clone();
        let l = self
            .graphs
            .get_mut(&alias)
            .expect("selected graph is loaded");
        let before = l.meta.ontology.clone();
        let changed = change(&mut l.meta.ontology, &l.graph, &alias_name(&alias))
            .inspect_err(|_| l.meta.ontology = before.clone())?;
        let value = |unchanged| OntologyRemoved {
            alias: alias.clone(),
            removed,
            name: name.as_str().to_string(),
            unchanged,
            role: None,
            affected: Vec::new(),
        };
        if !changed {
            return Ok(Done::ok(value(true)));
        }
        let warnings = persist(
            files.as_ref(),
            &alias,
            l,
            &[GraphFile::Graph],
            Touched::default(),
        )
        .inspect_err(|_| {
            l.meta.ontology = before;
        })?;
        Ok(Done {
            value: value(false),
            notes: warnings,
        })
    }

    fn change_ontology(
        &mut self,
        ctx: &Ctx,
        added: &'static str,
        name: &ValidName,
        change: impl FnOnce(&mut Ontology, &ValidName) -> Result<bool, Problem>,
    ) -> Outcome<OntologyAdded> {
        self.change_ontology_named(ctx, added, name.as_str(), change)
    }

    /// [`Self::change_ontology`] for an entry whose name is not a plain name (a tag, `origin:*`).
    fn change_ontology_named(
        &mut self,
        ctx: &Ctx,
        added: &'static str,
        name: &str,
        change: impl FnOnce(&mut Ontology, &ValidName) -> Result<bool, Problem>,
    ) -> Outcome<OntologyAdded> {
        let alias = self.select_write(ctx, &[])?;
        let files = self.ports.files.clone();
        let l = self
            .graphs
            .get_mut(&alias)
            .expect("selected graph is loaded");
        let before = l.meta.ontology.clone();
        // `false`: the same declaration is there already, so nothing is written (F71).
        if !change(&mut l.meta.ontology, &alias_name(&alias))? {
            return Ok(Done::ok(OntologyAdded {
                alias,
                added,
                name: name.to_string(),
                unchanged: true,
            }));
        }
        let mut warnings = persist(
            files.as_ref(),
            &alias,
            l,
            &[GraphFile::Graph],
            Touched::default(),
        )
        .inspect_err(|_| {
            l.meta.ontology = before.clone();
        })?;
        warnings.extend(soft_kind_warnings(
            &l.meta.ontology,
            Some(&before),
            &alias_name(&alias),
        ));
        Ok(Done {
            value: OntologyAdded {
                alias,
                added,
                name: name.to_string(),
                unchanged: false,
            },
            notes: warnings,
        })
    }
}

/// Fails with `errors` if there are any, keeping the first [`MAX_BATCH_ERRORS`] and a count of the rest.
pub(super) fn cap(errors: &mut Vec<Problem>) -> Result<(), Vec<Problem>> {
    if errors.is_empty() {
        return Ok(());
    }
    let failed = errors.len();
    let mut errors = core::mem::take(errors);
    if failed > MAX_BATCH_ERRORS {
        errors.truncate(MAX_BATCH_ERRORS);
        errors.push(problems::ontology_errors_truncated(
            failed,
            MAX_BATCH_ERRORS,
        ));
    }
    Err(errors)
}

/// `ontology remove <what> <name>`'s first checks: a built-in entry is refused, then the name rules.
fn removable(ctx: &Ctx, what: &'static str, name: &str) -> Result<ValidName, Problem> {
    if let Some(p) = builtin(what, name) {
        return Err(p);
    }
    let kind_of_name = if what == "type" { "node type" } else { what };
    check_name(name, kind_of_name, &|g| {
        ctx.same_graph(Command::new("ontology remove").word(what).name(g))
    })
}

/// Refuses with `ontology_in_use` when `count` records use the entry; `list` is the command that shows them.
fn in_use(
    graph: &ValidName,
    what: &'static str,
    name: &ValidName,
    count: usize,
    users: &'static str,
    list: impl FnOnce() -> Command,
) -> Result<(), Problem> {
    if count == 0 {
        return Ok(());
    }
    let fix = list().word("--graph").name(graph);
    Err(problems::ontology_in_use(
        graph, what, name, count, users, fix,
    ))
}
