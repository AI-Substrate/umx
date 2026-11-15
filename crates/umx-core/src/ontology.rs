//! A graph's vocabulary: node types (with declared fields), statuses and relations. Soft: it informs, never refuses.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    String,
    Int,
    Float,
    Bool,
    Date,
    List,
}

impl FieldKind {
    pub const ALL: [FieldKind; 6] = [
        FieldKind::String,
        FieldKind::Int,
        FieldKind::Float,
        FieldKind::Bool,
        FieldKind::Date,
        FieldKind::List,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FieldKind::String => "string",
            FieldKind::Int => "int",
            FieldKind::Float => "float",
            FieldKind::Bool => "bool",
            FieldKind::Date => "date",
            FieldKind::List => "list",
        }
    }

    pub fn parse(s: &str) -> Option<FieldKind> {
        // `number` is accepted as an alias for float (PRD req-08 example).
        if s == "number" {
            return Some(FieldKind::Float);
        }
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The role a field of this kind has when none is stated (D56): `string` is content, every other kind data.
    pub fn default_role(self) -> Role {
        if self.may_be_content() {
            Role::Content
        } else {
            Role::Data
        }
    }

    /// Whether a field of this kind can be embedded at all: only `string` (req-34, req-97).
    pub fn may_be_content(self) -> bool {
        self == FieldKind::String
    }
}

/// What a declared field is for (D56, req-97). Content fields are embedded; data fields (file names, ids, agent ids,
/// seats, hashes, locators, provenance) never are: no field vector, and left out of the whole-node text. Word search
/// and exact match are the same for both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Content,
    Data,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Content => "content",
            Role::Data => "data",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        [Role::Content, Role::Data]
            .into_iter()
            .find(|r| r.as_str() == s)
    }
}

/// Legacy identity keys from before D63 (one `entity_key` replaced them): older graphs may still carry their values,
/// so they stay never embedded. Not provenance, so not hidden with it.
pub const IDENTITY_KEYS: &[&str] = &["person_key", "org_key", "team_key"];

/// Whether field `name` is never embedded whatever its declaration: [`PROVENANCE_KEYS`], the legacy
/// [`IDENTITY_KEYS`], [`crate::entity::ENTITY_KEY`] and the removed-evidence trail
/// ([`crate::evidence::EVIDENCE_REMOVED`], req-126: removal reasons are never sent to an embedder).
pub fn never_embedded_field(name: &str) -> bool {
    PROVENANCE_KEYS.contains(&name)
        || IDENTITY_KEYS.contains(&name)
        || name == crate::entity::ENTITY_KEY
        || name == crate::evidence::EVIDENCE_REMOVED
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDecl {
    pub name: String,
    pub kind: FieldKind,
    /// The role stated with `--role` or `ontology set field` (D56); absent means the kind's default
    /// ([`FieldKind::default_role`]). Not written when absent, so older graph.json files are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// What the field means, one line (`umx graph guide`). Older builds keep it among their unknown keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Keys this build does not know (a newer umx or a hand edit), kept through load and save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDecl {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    /// The columns `umx node list --type <name>` shows when no `--columns` is given (F96), set with `umx ontology
    /// set type`. Not written when empty, so older graph.json files are unchanged (and older umx keeps the key).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// What the type means, one line (`umx graph guide`). Older builds keep it among their unknown keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Keys this build does not know, kept through load and save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ontology {
    pub types: Vec<TypeDecl>,
    pub statuses: Vec<String>,
    pub relations: Vec<String>,
    /// Relations marked symmetric with `umx ontology add relation <r> --symmetric` (D49), besides the
    /// [`SYMMETRIC_RELATIONS`] that always are. Not written when empty, so older graph.json files are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symmetric: Vec<String>,
    /// Custom fields declared on a relation's edges (F92), by relation, each list in declaration order. Not written
    /// when empty, so older graph.json files are unchanged; an older umx keeps the key as an unknown one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub relation_fields: BTreeMap<String, Vec<FieldDecl>>,
    /// What the graph is for, set by `umx graph set --purpose`. The guide keys live here, not at graph.json's top
    /// level, because these objects keep unknown keys in older builds: no format bump (plan 079, graph-storage-expert).
    /// Each is skipped when absent or empty, so a graph without them is written byte for byte as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// How the graph is used (naming, what goes in a note, when to add an edge), `umx graph set --conventions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conventions: Option<String>,
    /// One line per status, by name. A name not in `statuses` may have one (soft vocabulary).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub status_descriptions: BTreeMap<String, String>,
    /// One line per relation, by name. A name not in `relations` may have one (soft vocabulary).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub relation_descriptions: BTreeMap<String, String>,
    /// Keys this build does not know, kept through load and save.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The longest description of a type, field, status or relation, in characters (plan 079).
pub const MAX_DESCRIPTION_CHARS: usize = 500;
/// The longest purpose or conventions text, in bytes: graph.json is capped at 1 MiB, so guidance text cannot fill it.
pub const MAX_GUIDE_TEXT_BYTES: usize = 4096;

/// What a description is of: an ontology entry's kind, as `umx graph set --description <kind>:<name>=<text>` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Described {
    Type,
    Status,
    Relation,
    Field,
}

impl Described {
    pub const ALL: [Described; 4] = [
        Described::Type,
        Described::Status,
        Described::Relation,
        Described::Field,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Described::Type => "type",
            Described::Status => "status",
            Described::Relation => "relation",
            Described::Field => "field",
        }
    }

    pub fn parse(s: &str) -> Option<Described> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Which custom fields word search indexes (req-68), by the rule a field's kind follows (`field_kind` in services): a
/// field of a node is text when the node's type declares it `string`; when its type does not declare it, when the
/// first type that declares it does. A field no type declares is never text, so a date or number stays an exact-match
/// column even when another type has a string field of the same name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextFields {
    by_type: BTreeMap<String, BTreeMap<String, bool>>,
    any: BTreeMap<String, bool>,
}

impl TextFields {
    /// Fields that are text whatever the node's type (no type declares them otherwise).
    pub fn strings<S: Into<String>>(names: impl IntoIterator<Item = S>) -> TextFields {
        TextFields {
            by_type: BTreeMap::new(),
            any: names.into_iter().map(|n| (n.into(), true)).collect(),
        }
    }

    /// Whether field `field` of a node of type `kind` is indexed.
    pub fn is_text(&self, kind: &str, field: &str) -> bool {
        if PROVENANCE_KEYS.contains(&field) {
            return false;
        }
        match self.by_type.get(kind).and_then(|m| m.get(field)) {
            Some(&text) => text,
            None => self.any.get(field).copied().unwrap_or(false),
        }
    }

    /// Heap bytes, roughly: the names and a map entry each.
    pub fn heap_bytes(&self) -> usize {
        const ENTRY: usize = 64;
        self.by_type
            .iter()
            .map(|(t, m)| {
                t.capacity() + ENTRY + m.keys().map(|f| f.capacity() + ENTRY).sum::<usize>()
            })
            .sum::<usize>()
            + self.any.keys().map(|f| f.capacity() + ENTRY).sum::<usize>()
    }
}

/// Starter vocabulary for `umx graph new` (PRD q-07 in snake_case, workshop 007 section 7).
pub const STARTER_TYPES: &[&str] = &[
    "problem",
    "actor",
    "option",
    "mechanism",
    "criterion",
    "question",
    "decision",
    "tension",
    "precedent",
    "capability",
];
pub const STARTER_STATUSES: &[&str] = &[
    "open",
    "proposed",
    "leaning",
    "agreed",
    "acknowledged",
    "contested",
    "ruled_out",
    "context",
];
pub const STARTER_RELATIONS: &[&str] = &[
    "supports",
    "undercuts",
    "depends_on",
    "blocks",
    "conflicts_with",
    "answers",
    "part_of",
    "alternative_to",
    "raises",
    "owned_by",
    "tests",
    "possibly_same_as",
    "criterion_for",
];

/// Relations that are symmetric in every graph (D49): `a r b` means `b r a`. Such an edge is stored once, with the
/// lower id as `from`, and reads follow it from either end.
pub const SYMMETRIC_RELATIONS: &[&str] = &[
    "conflicts_with",
    "alternative_to",
    POSSIBLY_SAME_AS,
    SAME_AS,
    NOT_SAME_AS,
];

/// The "is" link (ws008 E2, D63c Q9): two nodes, usually in two graphs, are one entity. Stored once like any
/// symmetric edge, written by `umx entity link`.
pub const SAME_AS: &str = "same_as";
/// A proposed match (ws008): the two nodes may be one entity; reads keep them apart.
pub const POSSIBLY_SAME_AS: &str = "possibly_same_as";
/// A denial (ws008 E2c, req-137): the two nodes are NOT one entity, whatever keys or links say. Written only by
/// `umx entity deny`, with a required `reason` field ([`DENY_REASON`]).
pub const NOT_SAME_AS: &str = "not_same_as";
/// A thing is one instance of another (ws008 T6, req-138; answer 6 on the entity review): "this deployment
/// is an instance of that product". Directed, one way, allowed across graphs, and never a join: an entity read
/// follows `same_as` only, so two nodes `instance_of` each other stay two entities.
pub const INSTANCE_OF: &str = "instance_of";
/// The field a `not_same_as` edge holds its reason in: role data, reserved on edges so `--set` cannot forge it.
pub const DENY_REASON: &str = "reason";

/// std relations every graph knows without declaring them (ws008 E2, T6), so writing one never warns
/// `off_vocabulary_relation` and `ontology remove` refuses them. Not in [`STARTER_RELATIONS`], so new graph.json
/// files are unchanged.
pub const STD_RELATIONS: &[&str] = &[SAME_AS, NOT_SAME_AS, INSTANCE_OF];

/// The status a node gets when none is given.
pub const DEFAULT_STATUS: &str = "open";
/// The relation `--parent` creates and `--within` walks.
pub const PART_OF: &str = "part_of";

/// umx's own relation for similarity (workshop 008 section 9): kept from the vectors, reserved, never added by hand.
pub const SIMILAR_TO: &str = "similar_to";

/// The node types that hold provenance (ws007, D55). A node of one of these types is never embedded, whatever its
/// graph.json declares (req-97).
pub const PROVENANCE_TYPES: &[&str] = &["platform", "agent", "model", "harness", "session"];
/// Fields only umx sets, hidden with provenance (ws007). Never embedded, whatever graph.json declares.
pub const PROVENANCE_KEYS: &[&str] = &["prov_key", "authored_by_session", "edited_by_session"];

/// The most default columns `umx ontology set type` gives a type (F96, security N2).
pub const MAX_TYPE_COLUMNS: usize = 32;
pub const PROVENANCE_RELATIONS: &[&str] = &["session_of", "runs_on", "uses_model", "registered_in"];
pub const PROVENANCE_DERIVED: &[&str] = &["authored_by", "edited_by"];

/// Relations umx derives itself and never lets a write add, change or remove by hand (F75: `edge delete` and
/// `edge edit` refuse them). The one shared list: a workstream that adds a derived relation adds it here.
pub const DERIVED_RELATIONS: &[&str] = &[SIMILAR_TO, PROVENANCE_DERIVED[0], PROVENANCE_DERIVED[1]];

/// Derived relations cannot be added as stored edges.
pub fn is_derived_relation(kind: &str) -> bool {
    DERIVED_RELATIONS.contains(&kind)
}

/// Relations directed by what they mean, never symmetric (D49, ws004 s09).
fn directed(kind: &str) -> bool {
    kind == PART_OF || kind == SIMILAR_TO || kind == INSTANCE_OF
}

impl FieldDecl {
    pub fn new(name: impl Into<String>, kind: FieldKind) -> FieldDecl {
        FieldDecl {
            name: name.into(),
            kind,
            role: None,
            description: None,
            extra: BTreeMap::new(),
        }
    }

    /// The role umx acts on: data for a never-embedded name or a kind that cannot be content (a pulled graph.json
    /// saying otherwise changes nothing), else the stated role, else the kind's default.
    pub fn effective_role(&self) -> Role {
        if never_embedded_field(&self.name) || !self.kind.may_be_content() {
            return Role::Data;
        }
        self.role.unwrap_or(self.kind.default_role())
    }
}

impl TypeDecl {
    pub fn new(name: impl Into<String>, fields: Vec<FieldDecl>) -> TypeDecl {
        TypeDecl {
            name: name.into(),
            fields,
            columns: Vec::new(),
            description: None,
            extra: BTreeMap::new(),
        }
    }

    /// True when `column` can be one of this type's default columns (F96): a field every node carries, or one this
    /// type declares. Never a provenance key (security N1), so a pulled graph.json cannot unhide one.
    pub fn column_known(&self, column: &str) -> bool {
        !PROVENANCE_KEYS.contains(&column)
            && (crate::name::RESERVED_NODE_FIELDS.contains(&column)
                || self.fields.iter().any(|f| f.name == column))
    }
}

impl Ontology {
    pub fn starter() -> Ontology {
        Ontology {
            types: STARTER_TYPES
                .iter()
                .map(|t| TypeDecl::new(*t, Vec::new()))
                .collect(),
            statuses: STARTER_STATUSES.iter().map(|s| s.to_string()).collect(),
            relations: STARTER_RELATIONS.iter().map(|s| s.to_string()).collect(),
            symmetric: Vec::new(),
            relation_fields: BTreeMap::new(),
            purpose: None,
            conventions: None,
            status_descriptions: BTreeMap::new(),
            relation_descriptions: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Sets (or with `None`, clears) one entry's description. `name` is `<type>.<field>` for a field. A type or field
    /// must be declared, as its description lives on its declaration; a status or relation need not be (soft
    /// vocabulary). Err names what is missing: `(Described::Type, type)` or `(Described::Field, field)`.
    pub fn describe(
        &mut self,
        kind: Described,
        name: &str,
        text: Option<String>,
    ) -> Result<(), (Described, String)> {
        match kind {
            Described::Type => {
                let t = self
                    .types
                    .iter_mut()
                    .find(|t| t.name == name)
                    .ok_or((Described::Type, name.to_string()))?;
                t.description = text;
            }
            Described::Field => {
                let (type_name, field) = name.split_once('.').unwrap_or((name, ""));
                let t = self
                    .types
                    .iter_mut()
                    .find(|t| t.name == type_name)
                    .ok_or((Described::Type, type_name.to_string()))?;
                let f = t
                    .fields
                    .iter_mut()
                    .find(|f| f.name == field)
                    .ok_or((Described::Field, field.to_string()))?;
                f.description = text;
            }
            Described::Status | Described::Relation => {
                let map = if kind == Described::Status {
                    &mut self.status_descriptions
                } else {
                    &mut self.relation_descriptions
                };
                match text {
                    Some(t) => {
                        map.insert(name.to_string(), t);
                    }
                    None => {
                        map.remove(name);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn type_decl(&self, name: &str) -> Option<&TypeDecl> {
        self.types.iter().find(|t| t.name == name)
    }

    pub fn has_status(&self, name: &str) -> bool {
        self.statuses.iter().any(|s| s == name)
    }

    pub fn has_relation(&self, name: &str) -> bool {
        self.relations.iter().any(|s| s == name)
    }

    /// Whether relation `kind` is symmetric in this graph (D49): one of [`SYMMETRIC_RELATIONS`], or marked in
    /// `symmetric`. The query verbs and the Cypher executor (ws005) read it; see also [`Ontology::symmetric_kinds`].
    /// `part_of` and `similar_to` never are, even if a hand-edited graph.json lists them: they are directed by what
    /// they mean (`symmetric_refused`), and a similar_to list is one node's nearest, not a pair (ws004 s09).
    pub fn is_symmetric(&self, kind: &str) -> bool {
        !directed(kind)
            && (SYMMETRIC_RELATIONS.contains(&kind) || self.symmetric.iter().any(|s| s == kind))
    }

    /// Every symmetric relation of this graph, sorted and once each: [`SYMMETRIC_RELATIONS`] plus the marked ones.
    /// Cheap enough to take once per query.
    pub fn symmetric_kinds(&self) -> Vec<&str> {
        let mut kinds: Vec<&str> = SYMMETRIC_RELATIONS
            .iter()
            .copied()
            .chain(self.symmetric.iter().map(String::as_str))
            .filter(|k| !directed(k))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }

    pub fn field_kind(&self, type_name: &str, field: &str) -> Option<FieldKind> {
        self.type_decl(type_name)?
            .fields
            .iter()
            .find(|f| f.name == field)
            .map(|f| f.kind)
    }

    pub fn field_decl(&self, type_name: &str, field: &str) -> Option<&FieldDecl> {
        self.type_decl(type_name)?
            .fields
            .iter()
            .find(|f| f.name == field)
    }

    /// Whether any field states a role, which needs graph.json format [`crate::model::FORMAT_FIELD_ROLES`].
    pub fn has_roles(&self) -> bool {
        self.types
            .iter()
            .any(|t| t.fields.iter().any(|f| f.role.is_some()))
    }

    /// The declared kind of custom field `field` on edges of `relation` (F92).
    pub fn relation_field_kind(&self, relation: &str, field: &str) -> Option<FieldKind> {
        self.relation_fields
            .get(relation)?
            .iter()
            .find(|f| f.name == field)
            .map(|f| f.kind)
    }

    /// The custom fields word search indexes besides title and note (req-68), per node type.
    pub fn text_fields(&self) -> TextFields {
        let mut out = TextFields::default();
        for t in &self.types {
            for f in &t.fields {
                let text = f.kind == FieldKind::String;
                out.by_type
                    .entry(t.name.clone())
                    .or_default()
                    .entry(f.name.clone())
                    .or_insert(text);
                out.any.entry(f.name.clone()).or_insert(text);
            }
        }
        out
    }

    pub fn type_names(&self) -> impl Iterator<Item = &str> {
        self.types.iter().map(|t| t.name.as_str())
    }
}

/// Up to three declared names closest to `name`, closest first (for off-vocabulary candidates).
pub fn nearest<'a>(name: &str, declared: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut scored: Vec<(usize, &str)> = declared.map(|d| (edit_distance(name, d), d)).collect();
    scored.sort();
    let limit = name.len().max(3) / 2 + 1;
    scored
        .into_iter()
        .filter(|(d, _)| *d <= limit)
        .take(3)
        .map(|(_, n)| n.to_string())
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = alloc::vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::is_valid_name;

    #[test]
    fn starter_names_follow_rule_2() {
        let o = Ontology::starter();
        assert!(
            o.type_names()
                .chain(o.statuses.iter().map(String::as_str))
                .all(is_valid_name)
        );
        assert!(o.relations.iter().all(|r| is_valid_name(r)));
        assert!(o.has_relation(PART_OF) && o.has_status(DEFAULT_STATUS));
    }

    #[test]
    fn a_directed_relation_is_never_symmetric_even_if_listed() {
        let mut o = Ontology::starter();
        o.symmetric = ["similar_to", "part_of", "instance_of", "blocks"]
            .map(String::from)
            .to_vec();
        assert!(o.is_symmetric("blocks") && o.is_symmetric("conflicts_with"));
        assert!(!o.is_symmetric(SIMILAR_TO) && !o.is_symmetric(PART_OF));
        // ws008 T6: an instance points to what it is one of.
        assert!(!o.is_symmetric(INSTANCE_OF));
        let kinds = o.symmetric_kinds();
        assert!(kinds.contains(&"blocks"));
        assert!(!kinds.contains(&SIMILAR_TO) && !kinds.contains(&PART_OF));
    }

    #[test]
    fn nearest_names() {
        let o = Ontology::starter();
        assert_eq!(
            nearest("problme", o.type_names())
                .first()
                .map(String::as_str),
            Some("problem")
        );
        assert!(nearest("zzzzzzzzzz", o.type_names()).is_empty());
    }
}
