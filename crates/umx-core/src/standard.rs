//! std v1, the standard vocabulary built into umx (ws008 T1, D57; D71's time rows; the lifecycle statuses). Pure
//! data, no I/O. Names are stored plain (`person`, never `std:person`); `std:` is a qualifier the CLI accepts and
//! shows ([`qualified`]). A graph opts in with `"std": 1` in its ontology, and what that changes (the effective
//! ontology, the reserved names, the soft endpoint warnings) is T2's: this module is only the table they read.
//!
//! One source (architect, ws-008-placement): names other modules already own are referenced, never restated. The
//! provenance types, keys and relations come from [`crate::ontology`], the derived relations from
//! [`DERIVED_RELATIONS`], `part_of` and `same_as` from their consts and `entity_key` from [`crate::entity`], so a
//! change there is a change here.

use crate::ontology::{
    DERIVED_RELATIONS, FieldKind, INSTANCE_OF, NOT_SAME_AS, PART_OF, PROVENANCE_DERIVED,
    PROVENANCE_KEYS, PROVENANCE_RELATIONS, PROVENANCE_TYPES, Role, SAME_AS, SYMMETRIC_RELATIONS,
};

/// The highest std this build knows. std versions only add names.
pub const STD_VERSION: u32 = 1;
/// The qualifier the CLI accepts before a std name: `std:person` means `person`.
pub const QUALIFIER: &str = "std:";

/// A gathering with attendees.
pub const MEETING: &str = "meeting";
/// A happening that is not a gathering (a release, an incident, a deadline).
pub const EVENT: &str = "event";
/// A part of a meeting, event or other segment (D71 R5); segments nest, `part_of` their parent.
pub const SEGMENT: &str = "segment";
/// A recording or its text, as a source.
pub const TRANSCRIPT: &str = "transcript";
/// The types a time spine anchors on (D71, ws008 TM3): one added with no [`STARTS_ON`] warns `anchor_undated`.
pub const ANCHOR_TYPES: &[&str] = &[MEETING, EVENT, SEGMENT, TRANSCRIPT];
/// An order number inside a parent, counting from 1 (D71 R5). It compares only between siblings with the same
/// `part_of` parent; two siblings may share one (parallel tracks).
pub const STEP: &str = "step";
/// The least value of [`STEP`].
pub const STEP_MIN: i64 = 1;
/// A comes before b, where there is no clock or to say there are tracks (D71 R5). Same type on both ends.
pub const PRECEDES: &str = "precedes";
/// The thing that came up, to the finest anchor it came up in (D71).
pub const RAISED_IN: &str = "raised_in";
/// The one span pair (D71 R1): a node's or an edge's span in the world, allowed on any node or edge.
pub const STARTS_ON: &str = "starts_on";
pub const ENDS_ON: &str = "ends_on";
/// When something is due (the dates addendum).
pub const DUE_ON: &str = "due_on";
/// A meeting or event to where it was held (a place; an online meeting may point at a system).
pub const HELD_AT: &str = "held_at";
/// The std list field on every type (req-117).
pub const TAGS: &str = "tags";

/// What a std name may be used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags {
    /// Only umx writes it: a user node of a reserved type warns, a user edge of a reserved relation is refused.
    pub reserved: bool,
    /// umx derives it and never stores it (edge add refused).
    pub derived: bool,
    /// Provenance (ws007): umx-made, hidden with provenance, never embedded.
    pub provenance: bool,
}

const USER: Flags = Flags {
    reserved: false,
    derived: false,
    provenance: false,
};
const PROVENANCE: Flags = Flags {
    reserved: true,
    derived: false,
    provenance: true,
};

/// Where a std type sits in the std docs table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    People,
    Work,
    Sources,
    PlacesAndThings,
    Resources,
    Provenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StdType {
    pub name: &'static str,
    pub group: Group,
    pub flags: Flags,
    /// The types its `part_of` parent is expected to be (soft): a [`SEGMENT`] with none hints
    /// `segment_without_parent`, never refused. Empty: no expected parent.
    pub parents: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StdRelation {
    pub name: &'static str,
    pub flags: Flags,
    /// The expected types at each end (soft: another type is accepted with relation_endpoint_unexpected). Empty: any.
    pub from_types: &'static [&'static str],
    pub to_types: &'static [&'static str],
    /// Both ends are expected to be of one type ([`PRECEDES`]); a mixed pair warns relation_endpoint_unexpected.
    pub same_type_ends: bool,
}

impl StdRelation {
    /// Whether it is one edge both ways: read from [`SYMMETRIC_RELATIONS`], so never a second list.
    pub fn symmetric(&self) -> bool {
        SYMMETRIC_RELATIONS.contains(&self.name)
    }

    /// Whether `from` and `to` are the ends this relation expects (soft: false only warns).
    pub fn expects(&self, from: &str, to: &str) -> bool {
        let fits = |types: &[&str], t: &str| types.is_empty() || types.contains(&t);
        fits(self.from_types, from)
            && fits(self.to_types, to)
            && (!self.same_type_ends || from == to)
    }
}

/// Where a std field may be used without a declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// On the types and relations it is placed on (a graph may add it elsewhere as a custom field of its kind).
    Placed,
    /// On any node, never off_vocabulary.
    AnyNode,
    /// On any node or edge, never off_vocabulary (D71 R1's span pair; provenance keys).
    AnyNodeOrEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StdField {
    pub name: &'static str,
    pub kind: FieldKind,
    /// Its s22 role (D56): content is embedded, data never is.
    pub role: Role,
    pub flags: Flags,
    pub scope: Scope,
    /// The types std places it on (shown with them); may name types a graph declares itself (`phase`, `method`).
    pub types: &'static [&'static str],
    /// The relations std places it on, as relation fields (F92's `relation_fields`).
    pub relations: &'static [&'static str],
    /// The least value of an int field.
    pub min: Option<i64>,
}

/// What a status says about the node's work (D69's standings, as the lifecycle statuses use them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    Unsettled,
    Settled,
    OutOfPlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StdStatus {
    pub name: &'static str,
    pub standing: Standing,
    /// The status move should say why (`umx node status <node> <status> --reason ...`).
    pub reason_expected: bool,
    /// One line for the std docs table.
    pub description: &'static str,
}

const fn ty(name: &'static str, group: Group) -> StdType {
    StdType {
        name,
        group,
        flags: USER,
        parents: &[],
    }
}

const fn rel(
    name: &'static str,
    from_types: &'static [&'static str],
    to_types: &'static [&'static str],
) -> StdRelation {
    StdRelation {
        name,
        flags: USER,
        from_types,
        to_types,
        same_type_ends: false,
    }
}

const fn field(
    name: &'static str,
    kind: FieldKind,
    role: Role,
    scope: Scope,
    types: &'static [&'static str],
) -> StdField {
    StdField {
        name,
        kind,
        role,
        flags: USER,
        scope,
        types,
        relations: &[],
        min: None,
    }
}

const OWNERS: &[&str] = &["person", "team", "organisation"];
const DATED: &[&str] = &["meeting", "event", "project", SEGMENT];
const TENURE: &[&str] = &["works_for", "member_of"];

/// The user types, in the std docs order. The provenance types follow from [`PROVENANCE_TYPES`].
const USER_TYPES: &[StdType] = &[
    ty("person", Group::People),
    ty("organisation", Group::People),
    ty("team", Group::People),
    ty("role", Group::People),
    ty("project", Group::Work),
    ty(MEETING, Group::Work),
    ty(EVENT, Group::Work),
    StdType {
        parents: &["meeting", "event", SEGMENT],
        ..ty(SEGMENT, Group::Work)
    },
    ty("decision", Group::Work),
    ty("action", Group::Work),
    ty("question", Group::Work),
    ty("risk", Group::Work),
    ty("topic", Group::Work),
    ty("document", Group::Sources),
    ty(TRANSCRIPT, Group::Sources),
    ty("place", Group::PlacesAndThings),
    ty("system", Group::PlacesAndThings),
    ty("file", Group::Resources),
    ty("url", Group::Resources),
    ty("folder", Group::Resources),
    ty("excerpt", Group::Resources),
];

/// The stored user relations. The reserved provenance relations follow from [`PROVENANCE_RELATIONS`] and the
/// derived ones from [`DERIVED_RELATIONS`].
const USER_RELATIONS: &[StdRelation] = &[
    rel("member_of", &["person", "team"], &["team", "organisation"]),
    rel("works_for", &["person"], &["organisation"]),
    rel(PART_OF, &[], &[]),
    rel("attended", &[], &["meeting"]),
    rel("owns", OWNERS, &[]),
    rel("decides", &[], &[]),
    rel("answers", &[], &[]),
    rel(
        RAISED_IN,
        &[],
        &[SEGMENT, "meeting", "event", "transcript", "phase"],
    ),
    rel(
        "raised_by",
        &["question", "risk", "decision", "action"],
        OWNERS,
    ),
    rel(HELD_AT, &["meeting", "event"], &["place", "system"]),
    rel("mentions", &[], &[]),
    rel("sourced_from", &[], &[]),
    rel("supersedes", &[], &[]),
    StdRelation {
        same_type_ends: true,
        ..rel(PRECEDES, &[], &[])
    },
    rel(SAME_AS, &[], &[]),
    // Written only by `umx entity deny` (ws008 E2c): edge writes of it are refused there.
    rel(NOT_SAME_AS, &[], &[]),
    // ws008 T6 (req-138): one way, any types at either end, across graphs; related, never the same entity.
    rel(INSTANCE_OF, &[], &[]),
];

/// The user fields. The provenance keys follow from [`PROVENANCE_KEYS`].
const USER_FIELDS: &[StdField] = &[
    field(
        "aliases",
        FieldKind::List,
        Role::Data,
        Scope::Placed,
        &["person", "organisation"],
    ),
    field(
        "job_title",
        FieldKind::String,
        Role::Content,
        Scope::Placed,
        &["person"],
    ),
    field(
        "org",
        FieldKind::String,
        Role::Content,
        Scope::Placed,
        &["person"],
    ),
    field(TAGS, FieldKind::List, Role::Data, Scope::AnyNode, &[]),
    field(
        crate::entity::ENTITY_KEY,
        FieldKind::List,
        Role::Data,
        Scope::AnyNode,
        &[],
    ),
    StdField {
        relations: TENURE,
        ..field(
            STARTS_ON,
            FieldKind::Date,
            Role::Data,
            Scope::AnyNodeOrEdge,
            DATED,
        )
    },
    StdField {
        relations: TENURE,
        ..field(
            ENDS_ON,
            FieldKind::Date,
            Role::Data,
            Scope::AnyNodeOrEdge,
            DATED,
        )
    },
    field(
        DUE_ON,
        FieldKind::Date,
        Role::Data,
        Scope::Placed,
        &["project", "action", "decision", "risk"],
    ),
    StdField {
        min: Some(STEP_MIN),
        ..field(
            STEP,
            FieldKind::Int,
            Role::Data,
            Scope::Placed,
            &[SEGMENT, "phase", "method"],
        )
    },
];

/// The lifecycle statuses (graph-addressing-expert, topic:ws008-std-statuses): any type may use them; moving between
/// them is free (no transition table); one name each, no synonyms (`delivered` is not `done`; the built-in `open`
/// stays).
pub const STATUSES: &[StdStatus] = &[
    StdStatus {
        name: "active",
        standing: Standing::Unsettled,
        reason_expected: false,
        description: "work on it is under way",
    },
    StdStatus {
        name: "stalled",
        standing: Standing::Unsettled,
        reason_expected: true,
        description: "started, but not moving and not given up (blocked, waiting or parked)",
    },
    StdStatus {
        name: "delivered",
        standing: Standing::Settled,
        reason_expected: false,
        description: "done, with its outcome handed over",
    },
    StdStatus {
        name: "withdrawn",
        standing: Standing::OutOfPlay,
        reason_expected: true,
        description: "given up or cancelled before delivery, by either side",
    },
];

/// Every std type: the user types, then the reserved provenance types.
pub fn types() -> impl Iterator<Item = StdType> {
    USER_TYPES
        .iter()
        .copied()
        .chain(PROVENANCE_TYPES.iter().map(|&name| StdType {
            name,
            group: Group::Provenance,
            flags: PROVENANCE,
            parents: &[],
        }))
}

/// Every std relation: the stored user ones, the reserved provenance ones, then the derived ones.
pub fn relations() -> impl Iterator<Item = StdRelation> {
    let reserved = PROVENANCE_RELATIONS.iter().map(|&name| StdRelation {
        flags: PROVENANCE,
        ..rel(name, &[], &[])
    });
    let derived = DERIVED_RELATIONS.iter().map(|&name| StdRelation {
        flags: Flags {
            reserved: true,
            derived: true,
            provenance: PROVENANCE_DERIVED.contains(&name),
        },
        ..rel(name, &[], &[])
    });
    USER_RELATIONS
        .iter()
        .copied()
        .chain(reserved)
        .chain(derived)
}

/// Every std field: the user ones, then the provenance keys.
pub fn fields() -> impl Iterator<Item = StdField> {
    USER_FIELDS
        .iter()
        .copied()
        .chain(PROVENANCE_KEYS.iter().map(|&name| StdField {
            flags: PROVENANCE,
            ..field(
                name,
                FieldKind::String,
                Role::Data,
                Scope::AnyNodeOrEdge,
                &[],
            )
        }))
}

pub fn std_type(name: &str) -> Option<StdType> {
    types().find(|t| t.name == name)
}

pub fn std_relation(name: &str) -> Option<StdRelation> {
    relations().find(|r| r.name == name)
}

pub fn std_field(name: &str) -> Option<StdField> {
    fields().find(|f| f.name == name)
}

pub fn std_status(name: &str) -> Option<StdStatus> {
    STATUSES.iter().copied().find(|s| s.name == name)
}

/// The std fields a node of type `kind` has without declaring them: those placed on it and those allowed anywhere.
pub fn fields_of_type(kind: &str) -> impl Iterator<Item = StdField> + '_ {
    fields().filter(move |f| f.scope != Scope::Placed || f.types.contains(&kind))
}

/// The std fields an edge of `relation` has without declaring them: those placed on it and those allowed on any
/// edge.
pub fn fields_of_relation(relation: &str) -> impl Iterator<Item = StdField> + '_ {
    fields().filter(move |f| f.scope == Scope::AnyNodeOrEdge || f.relations.contains(&relation))
}

/// What a qualified name is looked up as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Type,
    Relation,
    Field,
    Status,
}

/// `std:<name>` where `<name>` is not a std name of that kind (the CLI's `not_std`). `name` is the text after the
/// qualifier, unchecked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotStd<'a> {
    pub kind: NameKind,
    pub name: &'a str,
}

/// The stored name `text` means: `std:<name>` is `<name>` when it is a std name of `kind`, else [`NotStd`]; a name
/// without the qualifier is itself, unchecked (a plain name may be custom).
pub fn qualified(kind: NameKind, text: &str) -> Result<&str, NotStd<'_>> {
    let Some(name) = text.strip_prefix(QUALIFIER) else {
        return Ok(text);
    };
    let known = match kind {
        NameKind::Type => std_type(name).is_some(),
        NameKind::Relation => std_relation(name).is_some(),
        NameKind::Field => std_field(name).is_some(),
        NameKind::Status => std_status(name).is_some(),
    };
    if known {
        Ok(name)
    } else {
        Err(NotStd { kind, name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::{RESERVED_EDGE_FIELDS, RESERVED_NODE_FIELDS, RESERVED_WORDS};
    use crate::problem::ValidName;
    use alloc::collections::BTreeSet;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    fn every_name() -> Vec<(&'static str, &'static str)> {
        let mut all: Vec<(&str, &str)> = Vec::new();
        all.extend(types().map(|t| ("type", t.name)));
        all.extend(relations().map(|r| ("relation", r.name)));
        all.extend(fields().map(|f| ("field", f.name)));
        all.extend(STATUSES.iter().map(|s| ("status", s.name)));
        all
    }

    #[test]
    fn every_std_name_is_a_valid_lower_snake_case_name_and_no_reserved_word() {
        for (kind, name) in every_name() {
            assert!(ValidName::new(name).is_some(), "{kind} {name}");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    && name.starts_with(|c: char| c.is_ascii_lowercase()),
                "{kind} {name} is lower snake case"
            );
            assert!(!RESERVED_WORDS.contains(&name), "{kind} {name}");
            assert_ne!(name, "std", "std itself is reserved, never a std name");
        }
    }

    #[test]
    fn no_name_is_listed_twice_in_one_kind() {
        for kind in ["type", "relation", "field", "status"] {
            let names: Vec<&str> = every_name()
                .into_iter()
                .filter(|n| n.0 == kind)
                .map(|n| n.1)
                .collect();
            let unique: BTreeSet<&str> = names.iter().copied().collect();
            assert_eq!(unique.len(), names.len(), "{kind}: {names:?}");
        }
    }

    #[test]
    fn no_user_field_collides_with_a_standard_field_or_a_provenance_key() {
        for f in fields() {
            let standard =
                RESERVED_NODE_FIELDS.contains(&f.name) || RESERVED_EDGE_FIELDS.contains(&f.name);
            if f.flags.provenance {
                assert!(PROVENANCE_KEYS.contains(&f.name), "{}", f.name);
                continue;
            }
            assert!(!standard, "{} is a standard field name", f.name);
            assert!(
                !PROVENANCE_KEYS.contains(&f.name),
                "{} is a provenance key",
                f.name
            );
        }
        // One source: every provenance key is a std field, flagged.
        for key in PROVENANCE_KEYS {
            assert!(
                std_field(key).is_some_and(|f| f.flags.provenance && f.flags.reserved),
                "{key}"
            );
        }
    }

    #[test]
    fn the_provenance_and_derived_names_are_the_ontology_s_own() {
        let prov_types: Vec<&str> = types()
            .filter(|t| t.flags.provenance)
            .map(|t| t.name)
            .collect();
        assert_eq!(prov_types, PROVENANCE_TYPES);
        let reserved: Vec<&str> = relations()
            .filter(|r| r.flags.reserved && !r.flags.derived)
            .map(|r| r.name)
            .collect();
        assert_eq!(reserved, PROVENANCE_RELATIONS);
        let derived: Vec<&str> = relations()
            .filter(|r| r.flags.derived)
            .map(|r| r.name)
            .collect();
        assert_eq!(derived, DERIVED_RELATIONS);
        assert!(std_relation("similar_to").is_some_and(|r| !r.flags.provenance));
        assert!(std_relation("authored_by").is_some_and(|r| r.flags.provenance));
        // No user type, relation or field is reserved, derived or provenance.
        assert!(USER_TYPES.iter().all(|t| t.flags == USER));
        assert!(USER_RELATIONS.iter().all(|r| r.flags == USER));
        assert!(USER_FIELDS.iter().all(|f| f.flags == USER));
    }

    #[test]
    fn every_role_is_one_its_kind_can_have_and_dates_are_data() {
        for f in fields() {
            assert!(
                f.role == Role::Data || f.kind.may_be_content(),
                "{} {:?} {:?}",
                f.name,
                f.kind,
                f.role
            );
            if f.kind == FieldKind::Date {
                assert_eq!(f.role, Role::Data, "{}", f.name);
            }
            assert!(f.min.is_none() || f.kind == FieldKind::Int, "{}", f.name);
        }
    }

    #[test]
    fn endpoints_and_placements_name_std_types_or_the_anchor_types_d71_names() {
        // phase and method are types a graph declares itself; std places step on them and raised_in may point at a
        // phase (D71).
        let known = |t: &str| std_type(t).is_some() || t == "phase" || t == "method";
        for r in relations() {
            for t in r.from_types.iter().chain(r.to_types) {
                assert!(known(t), "{} names {t}", r.name);
            }
        }
        for f in fields() {
            for t in f.types {
                assert!(known(t), "{} placed on {t}", f.name);
            }
            for r in f.relations {
                assert!(std_relation(r).is_some(), "{} placed on {r}", f.name);
            }
        }
        for t in types() {
            for p in t.parents {
                assert!(std_type(p).is_some(), "{} parent {p}", t.name);
            }
        }
    }

    #[test]
    fn the_d71_rows_hold() {
        let segment = std_type(SEGMENT).unwrap();
        assert_eq!(segment.parents, ["meeting", "event", SEGMENT]);
        let on_segment: Vec<&str> = fields_of_type(SEGMENT).map(|f| f.name).collect();
        for f in [STARTS_ON, ENDS_ON, STEP] {
            assert!(on_segment.contains(&f), "{f}");
        }
        let step = std_field(STEP).unwrap();
        assert_eq!(
            (step.kind, step.min, step.scope),
            (FieldKind::Int, Some(1), Scope::Placed)
        );
        assert_eq!(step.types, [SEGMENT, "phase", "method"]);
        // The span pair: kind date, on any node or edge, placed on the dated types and tenure edges.
        for name in [STARTS_ON, ENDS_ON] {
            let f = std_field(name).unwrap();
            assert_eq!(
                (f.kind, f.scope),
                (FieldKind::Date, Scope::AnyNodeOrEdge),
                "{name}"
            );
            assert!(
                fields_of_type("widget").any(|g| g.name == name),
                "{name} anywhere"
            );
            assert!(
                fields_of_relation("blocks").any(|g| g.name == name),
                "{name} on any edge"
            );
            assert_eq!(f.types, DATED);
            assert_eq!(f.relations, TENURE);
        }
        assert!(std_field("valid_from").is_none() && std_field("valid_to").is_none());
        // due_on only where placed.
        assert!(!fields_of_type("widget").any(|f| f.name == DUE_ON));
        let precedes = std_relation(PRECEDES).unwrap();
        assert!(precedes.same_type_ends && !precedes.symmetric());
        assert!(precedes.expects(SEGMENT, SEGMENT) && precedes.expects("phase", "phase"));
        assert!(!precedes.expects(SEGMENT, "meeting"));
        let raised_in = std_relation(RAISED_IN).unwrap();
        assert!(raised_in.expects("widget", SEGMENT) && raised_in.expects("risk", "transcript"));
        assert!(!raised_in.expects("risk", "person") && !raised_in.symmetric());
        let held_at = std_relation(HELD_AT).unwrap();
        assert!(held_at.expects("meeting", "place") && held_at.expects("meeting", "system"));
        assert!(!held_at.expects("meeting", "person"));
        assert!(std_relation("works_for").is_some_and(|r| !r.expects("person", "team")));
        assert!(std_relation(SAME_AS).is_some_and(|r| r.symmetric()));
        assert!(std_relation(NOT_SAME_AS).is_some_and(|r| r.symmetric()));
    }

    #[test]
    fn the_anchor_types_are_std_types() {
        for t in ANCHOR_TYPES {
            assert!(std_type(t).is_some(), "{t}");
        }
        assert!(fields_of_type(SEGMENT).any(|f| f.name == STARTS_ON));
    }

    #[test]
    fn the_lifecycle_statuses_and_their_standings() {
        let got: Vec<(&str, Standing, bool)> = STATUSES
            .iter()
            .map(|s| (s.name, s.standing, s.reason_expected))
            .collect();
        assert_eq!(
            got,
            [
                ("active", Standing::Unsettled, false),
                ("stalled", Standing::Unsettled, true),
                ("delivered", Standing::Settled, false),
                ("withdrawn", Standing::OutOfPlay, true),
            ]
        );
        // No synonyms of the built-in default, and none of its own.
        for s in ["open", "done", "complete", "cancelled"] {
            assert!(std_status(s).is_none(), "{s}");
        }
        assert!(std_status(crate::ontology::DEFAULT_STATUS).is_none());
    }

    #[test]
    fn the_qualifier_means_the_plain_name_or_not_std() {
        assert_eq!(qualified(NameKind::Type, "std:person"), Ok("person"));
        assert_eq!(qualified(NameKind::Type, "person"), Ok("person"));
        assert_eq!(qualified(NameKind::Type, "widget"), Ok("widget"));
        assert_eq!(
            qualified(NameKind::Type, "std:widget"),
            Err(NotStd {
                kind: NameKind::Type,
                name: "widget"
            })
        );
        assert_eq!(qualified(NameKind::Relation, "std:precedes"), Ok(PRECEDES));
        assert_eq!(qualified(NameKind::Field, "std:step"), Ok(STEP));
        assert_eq!(qualified(NameKind::Status, "std:stalled"), Ok("stalled"));
        // A name is std only as its own kind.
        assert_eq!(
            qualified(NameKind::Relation, "std:person"),
            Err(NotStd {
                kind: NameKind::Relation,
                name: "person"
            })
        );
        assert_eq!(
            qualified(NameKind::Type, "std:"),
            Err(NotStd {
                kind: NameKind::Type,
                name: ""
            })
        );
        assert_eq!(
            qualified(NameKind::Type, "std:std:person"),
            Err(NotStd {
                kind: NameKind::Type,
                name: "std:person"
            })
        );
    }

    /// The interim std sheet (D57's interim sheet) as a fixture: every name in it is in std
    /// v1, except the two person fields std renamed (`title` became `job_title`) or dropped (`name`).
    #[test]
    fn every_interim_sheet_name_is_std_but_the_two_renamed_person_fields() {
        let sheet: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/std/interim-sheet.json")).unwrap();
        let names = |key: &str| -> Vec<String> {
            sheet[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        for t in names("types") {
            assert!(std_type(&t).is_some(), "type {t}");
        }
        for r in names("relations") {
            assert!(std_relation(&r).is_some(), "relation {r}");
        }
        let mut not_std: Vec<String> = Vec::new();
        for (kind, fields) in sheet["fields"].as_object().unwrap() {
            assert!(std_type(kind).is_some(), "{kind}");
            for f in fields.as_array().unwrap() {
                let f = f.as_str().unwrap();
                if !fields_of_type(kind).any(|g| g.name == f) {
                    not_std.push(alloc::format!("{kind}.{f}"));
                }
            }
        }
        assert_eq!(not_std, ["person.name", "person.title"]);
        assert_eq!(names("renamed"), not_std);
    }
}
