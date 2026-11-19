//! `umx ontology adopt-std` (ws008 T3, req-101), the pure part: what adopting std v1 does to a graph's stored
//! ontology. [`fold`] sets `std` and drops what std now supplies, keeping the graph's own extensions and overrides
//! and every custom name; [`rename`] applies one `--map <from>=<to>` to the ontology and the spine declarations;
//! [`kind_clashes`] finds the fields a graph declares with another kind than std's (F285); [`suggestions`] offers
//! renames for `--dry-run`, never applied. The node and edge lines are the services' part.

use crate::ontology::{
    FieldDecl, FieldKind, Ontology, SYMMETRIC_RELATIONS, TypeDecl, edit_distance,
};
use crate::spine::SpineDecl;
use crate::standard::{self, STD_VERSION, StdField};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a `--map` renames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renaming {
    Type,
    Relation,
}

impl Renaming {
    pub fn as_str(self) -> &'static str {
        match self {
            Renaming::Type => "type",
            Renaming::Relation => "relation",
        }
    }
}

/// Where a field is declared: on node type `on`, or on the edges of relation `on`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldAt {
    pub on: String,
    pub relation: bool,
    pub field: String,
}

/// A field the graph declares with another kind than std v1's field of that name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindClash {
    pub at: FieldAt,
    pub kind: FieldKind,
    pub std_kind: FieldKind,
}

/// Type, relation and status names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    pub types: Vec<String>,
    pub relations: Vec<String>,
    pub statuses: Vec<String>,
}

/// The graph's ontology once it adopts std.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    /// The stored ontology to write: `std` set, the folded entries gone.
    pub ontology: Ontology,
    /// The graph's own entries std now supplies, dropped.
    pub folded: Names,
    /// The graph's own field declarations std places with the same kind, role and no description, dropped.
    pub folded_fields: Vec<FieldAt>,
    /// The graph's own fields kept on a std type or relation: extensions, and overrides of std's role or description.
    pub extended: Vec<FieldAt>,
    /// The graph's own names std does not have, kept as they are.
    pub custom: Names,
    /// Fields declared with another kind than std's: adopting is refused while any is left.
    pub clashes: Vec<KindClash>,
}

/// Every field `o` declares, on a type or a relation, with another kind than std v1's field of that name (F285): std's
/// kind would win in a std graph (`Ontology::effective`).
pub fn kind_clashes(o: &Ontology) -> Vec<KindClash> {
    let mut out = Vec::new();
    let mut check = |on: &str, relation: bool, f: &FieldDecl| {
        if let Some(s) = standard::std_field(&f.name).filter(|s| s.kind != f.kind) {
            out.push(KindClash {
                at: FieldAt {
                    on: on.to_string(),
                    relation,
                    field: f.name.clone(),
                },
                kind: f.kind,
                std_kind: s.kind,
            });
        }
    };
    for t in &o.types {
        for f in &t.fields {
            check(&t.name, false, f);
        }
    }
    for (r, fields) in &o.relation_fields {
        for f in fields {
            check(r, true, f);
        }
    }
    out
}

/// Whether `f` says only what std's placed field `s` says: its kind and role, no description and no unknown keys.
fn same_as_std(s: Option<StdField>, f: &FieldDecl) -> bool {
    s.is_some_and(|s| {
        s.kind == f.kind
            && f.effective_role() == s.role
            && f.description.is_none()
            && f.extra.is_empty()
    })
}

/// `own` (a graph without std) as a std graph's stored ontology. An entry std supplies is dropped: a type, relation
/// or status std has, a field std places on that type or relation with the same kind and role and no description,
/// and a std type left with nothing of its own. Everything else is kept: a field std does not place there (an
/// extension), one with another role or a description (an override), and every name std does not have. The effective
/// ontology then reads the same for every name the graph declared.
pub fn fold(own: &Ontology) -> Fold {
    let clashes = kind_clashes(own);
    let mut o = own.clone();
    o.std = Some(STD_VERSION);
    let (mut folded, mut custom) = (Names::default(), Names::default());
    let (mut folded_fields, mut extended) = (Vec::new(), Vec::new());
    let mut types: Vec<TypeDecl> = Vec::new();
    for mut t in core::mem::take(&mut o.types) {
        let name = t.name.clone();
        let std = standard::std_type(&name).is_some();
        t.fields.retain(|f| {
            let at = FieldAt {
                on: name.clone(),
                relation: false,
                field: f.name.clone(),
            };
            let placed =
                standard::fields().find(|s| s.name == f.name && s.types.contains(&name.as_str()));
            if same_as_std(placed, f) {
                folded_fields.push(at);
                false
            } else {
                if std {
                    extended.push(at);
                }
                true
            }
        });
        if !std {
            custom.types.push(name);
            types.push(t);
        } else if t.fields.is_empty()
            && t.columns.is_empty()
            && t.description.is_none()
            && t.extra.is_empty()
        {
            folded.types.push(name);
        } else {
            types.push(t);
        }
    }
    o.types = types;
    for r in core::mem::take(&mut o.relations) {
        if standard::std_relation(&r).is_some() {
            folded.relations.push(r);
        } else {
            custom.relations.push(r.clone());
            o.relations.push(r);
        }
    }
    for s in core::mem::take(&mut o.statuses) {
        if standard::std_status(&s).is_some() {
            folded.statuses.push(s);
        } else {
            custom.statuses.push(s.clone());
            o.statuses.push(s);
        }
    }
    for (r, mut fields) in core::mem::take(&mut o.relation_fields) {
        let std = standard::std_relation(&r).is_some();
        fields.retain(|f| {
            let at = FieldAt {
                on: r.clone(),
                relation: true,
                field: f.name.clone(),
            };
            let placed =
                standard::fields().find(|s| s.name == f.name && s.relations.contains(&r.as_str()));
            if same_as_std(placed, f) {
                folded_fields.push(at);
                false
            } else {
                if std {
                    extended.push(at);
                }
                true
            }
        });
        if !fields.is_empty() {
            o.relation_fields.insert(r, fields);
        }
    }
    Fold {
        ontology: o,
        folded,
        folded_fields,
        extended,
        custom,
        clashes,
    }
}

/// Why a `--map` cannot apply to the ontology.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameClash {
    /// Both names are declared, and declare `item` (a field name, `columns`, `description` or an unknown key)
    /// differently.
    Merge(String),
    /// One relation is symmetric and the other is not, so the stored ends would mean something else.
    Symmetry,
}

/// Applies `--map <from>=<to>` to `o` and the spine declarations: the declaration of `from` becomes `to`'s (merged
/// into `to`'s when the graph declares both), and every place that names it follows. `used(name)` says whether the
/// graph's lines use relation `name`, so a relation nothing declares still counts as known.
pub fn rename(
    o: &mut Ontology,
    spines: &mut [SpineDecl],
    what: Renaming,
    from: &str,
    to: &str,
    used: &dyn Fn(&str) -> bool,
) -> Result<(), RenameClash> {
    match what {
        Renaming::Type => rename_type(o, spines, from, to),
        Renaming::Relation => rename_relation(o, spines, from, to, used),
    }
}

fn rename_type(
    o: &mut Ontology,
    spines: &mut [SpineDecl],
    from: &str,
    to: &str,
) -> Result<(), RenameClash> {
    if let Some(i) = o.types.iter().position(|t| t.name == from) {
        let mut a = o.types.remove(i);
        match o.types.iter_mut().find(|t| t.name == to) {
            Some(b) => merge_type(b, a)?,
            None => {
                a.name = to.to_string();
                o.types.insert(i, a);
            }
        }
    }
    for ends in o.relation_endpoints.values_mut() {
        swap_name(&mut ends.from, from, to);
        swap_name(&mut ends.to, from, to);
    }
    for s in spines {
        if s.anchors.kind == from {
            s.anchors.kind = to.to_string();
        }
    }
    Ok(())
}

fn rename_relation(
    o: &mut Ontology,
    spines: &mut [SpineDecl],
    from: &str,
    to: &str,
    used: &dyn Fn(&str) -> bool,
) -> Result<(), RenameClash> {
    let symmetric = |o: &Ontology, r: &str| {
        SYMMETRIC_RELATIONS.contains(&r) || o.symmetric.iter().any(|s| s == r)
    };
    let known = standard::std_relation(to).is_some()
        || o.relations.iter().any(|r| r == to)
        || o.symmetric.iter().any(|r| r == to)
        || o.relation_fields.contains_key(to)
        || used(to);
    // A mark of the graph's own moves to a relation nothing has yet.
    let carried = !known && o.symmetric.iter().any(|s| s == from);
    if symmetric(o, from) != (symmetric(o, to) || carried) {
        return Err(RenameClash::Symmetry);
    }
    swap_name(&mut o.relations, from, to);
    swap_name(&mut o.symmetric, from, to);
    if let Some(a) = o.relation_fields.remove(from) {
        merge_fields(o.relation_fields.entry(to.to_string()).or_default(), a)?;
    }
    if let Some(a) = o.relation_endpoints.remove(from) {
        let b = o.relation_endpoints.entry(to.to_string()).or_default();
        for t in a.from {
            if !b.from.contains(&t) {
                b.from.push(t);
            }
        }
        for t in a.to {
            if !b.to.contains(&t) {
                b.to.push(t);
            }
        }
        for (k, v) in a.extra {
            match b.extra.get(&k) {
                Some(have) if *have != v => return Err(RenameClash::Merge(k)),
                Some(_) => {}
                None => {
                    b.extra.insert(k, v);
                }
            }
        }
    }
    if let Some(d) = o.relation_descriptions.remove(from) {
        match o.relation_descriptions.get(to) {
            Some(have) if *have != d => return Err(RenameClash::Merge("description".into())),
            Some(_) => {}
            None => {
                o.relation_descriptions.insert(to.to_string(), d);
            }
        }
    }
    for s in spines {
        if s.anchors.inside.as_deref() == Some(from) {
            s.anchors.inside = Some(to.to_string());
        }
        for p in &mut s.place {
            if p == from {
                *p = to.to_string();
            }
        }
        // An order entry is a field or a chain relation: a field of the anchor type keeps its name.
        let field = o
            .type_decl(&s.anchors.kind)
            .is_some_and(|t| t.fields.iter().any(|f| f.name == from));
        if !field {
            for e in &mut s.order {
                if e == from {
                    *e = to.to_string();
                }
            }
        }
    }
    Ok(())
}

/// `from` in `names` becomes `to`, once, where it first stood.
fn swap_name(names: &mut Vec<String>, from: &str, to: &str) {
    if !names.iter().any(|n| n == from) {
        return;
    }
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for n in names.drain(..) {
        let n = if n == from { to.to_string() } else { n };
        if !out.contains(&n) {
            out.push(n);
        }
    }
    *names = out;
}

fn merge_type(b: &mut TypeDecl, a: TypeDecl) -> Result<(), RenameClash> {
    merge_fields(&mut b.fields, a.fields)?;
    if !a.columns.is_empty() {
        if b.columns.is_empty() {
            b.columns = a.columns;
        } else if b.columns != a.columns {
            return Err(RenameClash::Merge("columns".into()));
        }
    }
    if let Some(d) = a.description {
        match &b.description {
            Some(have) if *have != d => return Err(RenameClash::Merge("description".into())),
            Some(_) => {}
            None => b.description = Some(d),
        }
    }
    for (k, v) in a.extra {
        match b.extra.get(&k) {
            Some(have) if *have != v => return Err(RenameClash::Merge(k)),
            Some(_) => {}
            None => {
                b.extra.insert(k, v);
            }
        }
    }
    Ok(())
}

fn merge_fields(b: &mut Vec<FieldDecl>, a: Vec<FieldDecl>) -> Result<(), RenameClash> {
    for f in a {
        match b.iter().find(|g| g.name == f.name) {
            Some(g)
                if g.kind != f.kind
                    || g.effective_role() != f.effective_role()
                    || g.description != f.description
                    || g.extra != f.extra =>
            {
                return Err(RenameClash::Merge(f.name));
            }
            Some(_) => {}
            None => b.push(f),
        }
    }
    Ok(())
}

/// For each of `names` std v1 does not have, the std user type or relation it is closest to, when only a small slip
/// apart (a quarter of its length, at least one edit): `organization` to `organisation`. Offered by `--dry-run` and
/// never applied (req-101: renames are never inferred).
pub fn suggestions<'a>(
    what: Renaming,
    names: impl Iterator<Item = &'a str>,
) -> Vec<(String, &'static str)> {
    let std_names: Vec<&'static str> = match what {
        Renaming::Type => standard::types()
            .filter(|t| !t.flags.reserved)
            .map(|t| t.name)
            .collect(),
        Renaming::Relation => standard::relations()
            .filter(|r| !r.flags.reserved)
            .map(|r| r.name)
            .collect(),
    };
    let mut out: Vec<(String, &'static str)> = Vec::new();
    for name in names {
        let have = match what {
            Renaming::Type => standard::std_type(name).is_some(),
            Renaming::Relation => standard::std_relation(name).is_some(),
        };
        if have || out.iter().any(|(n, _)| n == name) {
            continue;
        }
        let limit = (name.chars().count() / 4).max(1);
        let best = std_names
            .iter()
            .map(|s| (edit_distance(name, s), *s))
            .filter(|(d, _)| *d <= limit)
            .min_by_key(|(d, _)| *d);
        if let Some((_, s)) = best {
            out.push((name.to_string(), s));
        }
    }
    out
}

/// Why a value of a `--field-to-tag` field stays in its field (ws008 t2b, req-117).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unmovable {
    /// Text with no snake_case form (`123`, `---`, empty).
    NoTag,
    /// Not text: a number, a bool, an object or a nested list.
    NotText,
    /// Text that looks like a secret (as stored or upper cased): never moved, never shown.
    Secret,
    /// Text whose snake_case form is longer than a tag allows: it would be cut, so it stays whole.
    TooLong,
    /// The node would hold more than [`crate::tags::MAX_TAGS`] tags: none of its values move.
    TagLimit,
}

impl Unmovable {
    pub fn as_str(self) -> &'static str {
        match self {
            Unmovable::NoTag => "no snake_case tag can be made from it",
            Unmovable::NotText => "it is not text",
            Unmovable::Secret => "it looks like a secret",
            Unmovable::TooLong => "it is longer than a tag allows",
            Unmovable::TagLimit => "the node would hold more than 64 tags",
        }
    }
}

/// What `--field-to-tag <field>` does to one node's value of `field`.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldTags {
    /// Each text moved, with the tag it became (`<field>:<value>` in stored form).
    pub moved: Vec<(String, String)>,
    /// The new tags, in stored form, sorted: the ones the node did not hold yet.
    pub added: Vec<String>,
    /// What stays in the field: None when every value moved (the field leaves the node).
    pub kept: Option<crate::Value>,
    /// Each value (or list item) that stays, and why.
    pub unmovable: Vec<(crate::Value, Unmovable)>,
}

/// The std tag text `value` of field `field` becomes: `<field>:<value>`, through the one tag normaliser, its snake_case
/// fix taken (`Double Diamond` becomes `origin:double_diamond`). Refused when no tag with facet `field` can be made, or
/// when the fix would cut the value to fit (a tag never holds part of a value).
pub fn field_tag(field: &str, value: &str) -> Result<String, Unmovable> {
    if crate::tags::snake_cut(value) {
        return Err(Unmovable::TooLong);
    }
    let tag = match crate::tags::normalise_tag(&alloc::format!("{field}:{value}")) {
        Ok(t) => t,
        Err(r) => match r.fixed {
            Some((Some(f), v)) => alloc::format!("{f}:{v}"),
            _ => return Err(Unmovable::NoTag),
        },
    };
    if crate::tags::split(&tag).0 == Some(field) {
        Ok(tag)
    } else {
        Err(Unmovable::NoTag)
    }
}

/// Whether any text in `v`, a string, an object's key or a value at any depth, is one `secret` flags.
fn holds_secret(v: &crate::Value, secret: &dyn Fn(&str) -> bool) -> bool {
    match v {
        crate::Value::String(s) => secret(s),
        crate::Value::Array(items) => items.iter().any(|x| holds_secret(x, secret)),
        crate::Value::Object(map) => map
            .iter()
            .any(|(k, x)| secret(k) || holds_secret(x, secret)),
        _ => false,
    }
}

/// How a value that stays is shown: text as itself, its first 80 characters then `…` when longer; anything else by
/// its shape only (`<object>`, `<list>`, `<not text>`), never its content; a secret is never shown at all.
pub fn shown_value(v: &crate::Value, why: Unmovable) -> String {
    match (why, v) {
        (Unmovable::Secret, _) => "<masked>".to_string(),
        (_, crate::Value::String(s)) if s.chars().count() > 80 => {
            s.chars().take(80).chain(core::iter::once('…')).collect()
        }
        (_, crate::Value::String(s)) => s.clone(),
        (_, crate::Value::Object(_)) => "<object>".to_string(),
        (_, crate::Value::Array(_)) => "<list>".to_string(),
        _ => "<not text>".to_string(),
    }
}

/// `--field-to-tag` on one node (ws008 t2b, req-117): each text of `value` (a string, or each string of a list)
/// becomes a `<field>:<value>` tag, to merge into `current` (the node's tags in stored form). A value that is not text,
/// has no snake_case form or one longer than a tag, or that holds anything `secret` flags (a key or a string at any
/// depth) stays in the field. The cap is the caller's, once every field is planned ([`merged_tags`]).
pub fn field_to_tags(
    field: &str,
    value: &crate::Value,
    current: &[String],
    secret: &dyn Fn(&str) -> bool,
) -> FieldTags {
    use crate::Value;
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => alloc::vec![other],
    };
    let mut moved = Vec::new();
    let mut unmovable = Vec::new();
    let mut stays = Vec::new();
    for item in items {
        let why = match item {
            _ if holds_secret(item, secret) => Some(Unmovable::Secret),
            Value::String(s) => match field_tag(field, s) {
                Ok(tag) => {
                    moved.push((s.clone(), tag));
                    None
                }
                Err(why) => Some(why),
            },
            _ => Some(Unmovable::NotText),
        };
        if let Some(why) = why {
            unmovable.push((item.clone(), why));
            stays.push(item.clone());
        }
    }
    let mut added: Vec<String> = moved
        .iter()
        .map(|(_, t)| t.clone())
        .filter(|t| !current.contains(t))
        .collect();
    added.sort();
    added.dedup();
    let kept = match (value, stays.len()) {
        (_, 0) => None,
        (Value::Array(_), _) => Some(Value::Array(stays)),
        (_, _) => Some(value.clone()),
    };
    FieldTags {
        moved,
        added,
        kept,
        unmovable,
    }
}

/// The tags a node holds once `added` merge into `current` (both in stored form), or None past
/// [`crate::tags::MAX_TAGS`]: then none of the node's values move (all or nothing per node).
pub fn merged_tags(current: &[String], added: &[String]) -> Option<Vec<String>> {
    crate::tags::finish(current.iter().chain(added).cloned().collect()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ontology::{RelationEnds, Role};
    use alloc::vec;

    fn field(name: &str, kind: FieldKind) -> FieldDecl {
        FieldDecl::new(name, kind)
    }

    /// A graph built to the interim std sheet (the fixture beside the std table): every name std, and the person
    /// fields the sheet had, `name` among them.
    fn interim() -> Ontology {
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
        let mut types: Vec<TypeDecl> = names("types")
            .into_iter()
            .map(|t| TypeDecl::new(t, vec![]))
            .collect();
        for (kind, fields) in sheet["fields"].as_object().unwrap() {
            let t = types.iter_mut().find(|t| t.name == *kind).unwrap();
            for f in fields.as_array().unwrap() {
                let f = f.as_str().unwrap();
                // `title` is a reserved key in every graph: no graph can declare it.
                if f == "title" {
                    continue;
                }
                let kind = standard::std_field(f).map_or(FieldKind::String, |s| s.kind);
                t.fields.push(field(f, kind));
            }
        }
        Ontology {
            types,
            relations: names("relations"),
            statuses: vec!["open".into(), "active".into()],
            ..Ontology::default()
        }
    }

    #[test]
    fn an_interim_sheet_graph_folds_into_std_keeping_only_person_name() {
        let f = fold(&interim());
        assert!(f.clashes.is_empty(), "{:?}", f.clashes);
        assert_eq!(f.ontology.std, Some(STD_VERSION));
        let kept: Vec<(&str, Vec<&str>)> = f
            .ontology
            .types
            .iter()
            .map(|t| {
                (
                    t.name.as_str(),
                    t.fields.iter().map(|f| f.name.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(kept, [("person", vec!["name"])]);
        assert!(f.ontology.relations.is_empty());
        assert_eq!(f.ontology.statuses, ["open"]);
        assert_eq!(f.custom.statuses, ["open"]);
        assert!(f.custom.types.is_empty() && f.custom.relations.is_empty());
        assert_eq!(
            f.extended,
            [FieldAt {
                on: "person".into(),
                relation: false,
                field: "name".into()
            }]
        );
        assert!(f.folded.relations.contains(&"authored_by".to_string()));
    }

    /// The effective ontology reads the same for every field the graph declared: no role moves, so no embedded
    /// text changes.
    #[test]
    fn folding_keeps_every_declared_fields_effective_kind_and_role() {
        let mut own = interim();
        let person = own.types.iter_mut().find(|t| t.name == "person").unwrap();
        // An override of std's role, a description, and an extension.
        person
            .fields
            .iter_mut()
            .find(|f| f.name == "org")
            .unwrap()
            .role = Some(Role::Content);
        person
            .fields
            .iter_mut()
            .find(|f| f.name == "job_title")
            .unwrap()
            .description = Some("What they do".into());
        // A key this build does not know (a newer umx, a hand edit) is the graph's own too.
        person
            .fields
            .iter_mut()
            .find(|f| f.name == "aliases")
            .unwrap()
            .extra
            .insert("note".into(), serde_json::json!("kept"));
        own.types.push(TypeDecl::new(
            "widget",
            vec![
                field("tags", FieldKind::List),
                field("size", FieldKind::Int),
            ],
        ));
        own.relation_fields.insert(
            "works_for".into(),
            vec![
                field("starts_on", FieldKind::Date),
                field("grade", FieldKind::String),
            ],
        );
        let before = own.effective();
        let f = fold(&own);
        let after = f.ontology.effective();
        for t in &own.types {
            for d in &t.fields {
                let (a, b) = (
                    before.field_decl(&t.name, &d.name).unwrap(),
                    after.field_decl(&t.name, &d.name).unwrap(),
                );
                assert_eq!(
                    (a.kind, a.effective_role(), &a.description),
                    (b.kind, b.effective_role(), &b.description),
                    "{}.{}",
                    t.name,
                    d.name
                );
            }
        }
        let grade = |o: &Ontology| o.relation_field_kind("works_for", "grade");
        assert_eq!(grade(&before), grade(&after));
        // The override, the described field and the extension stay; std's own copies go.
        let kept: Vec<&str> = f.extended.iter().map(|a| a.field.as_str()).collect();
        assert_eq!(kept, ["aliases", "job_title", "org", "name", "grade"]);
        let aliases = f.ontology.type_decl("person").unwrap().fields[0].clone();
        assert_eq!(aliases.extra["note"], serde_json::json!("kept"));
        assert!(
            f.folded_fields
                .iter()
                .any(|a| a.relation && a.field == "starts_on")
        );
        // `tags` is allowed on any node but placed on no type: a custom type keeps its own declaration.
        assert!(f.ontology.type_decl("widget").unwrap().fields.len() == 2);
        assert_eq!(f.custom.types, ["widget"]);
    }

    #[test]
    fn a_field_with_another_kind_than_std_s_clashes_on_any_type_or_relation() {
        let own = Ontology {
            types: vec![
                TypeDecl::new("person", vec![field("aliases", FieldKind::String)]),
                TypeDecl::new("widget", vec![field("due_on", FieldKind::String)]),
            ],
            relation_fields: [(
                "works_for".to_string(),
                vec![field("ends_on", FieldKind::Int)],
            )]
            .into_iter()
            .collect(),
            ..Ontology::default()
        };
        let folded = fold(&own);
        let clashes: Vec<(&str, bool, &str, FieldKind)> = folded
            .clashes
            .iter()
            .map(|c| {
                (
                    c.at.on.as_str(),
                    c.at.relation,
                    c.at.field.as_str(),
                    c.std_kind,
                )
            })
            .collect();
        assert_eq!(
            clashes,
            [
                ("person", false, "aliases", FieldKind::List),
                ("widget", false, "due_on", FieldKind::Date),
                ("works_for", true, "ends_on", FieldKind::Date),
            ]
        );
    }

    #[test]
    fn a_type_rename_moves_its_declaration_and_every_place_that_names_it() {
        let mut o = Ontology {
            types: vec![TypeDecl::new(
                "individual",
                vec![field("bio", FieldKind::String)],
            )],
            relations: vec!["mentors".into()],
            relation_endpoints: [(
                "mentors".to_string(),
                RelationEnds {
                    from: vec!["individual".into()],
                    to: vec!["individual".into(), "team".into()],
                    ..RelationEnds::default()
                },
            )]
            .into_iter()
            .collect(),
            ..Ontology::default()
        };
        let mut spines: Vec<SpineDecl> = vec![
            serde_json::from_value(serde_json::json!({
                "name": "s", "title": "S", "main": false,
                "anchors": { "type": "individual", "inside": null }, "order": [], "place": []
            }))
            .unwrap(),
        ];
        rename(
            &mut o,
            &mut spines,
            Renaming::Type,
            "individual",
            "person",
            &|_| false,
        )
        .unwrap();
        assert_eq!(o.types[0].name, "person");
        assert_eq!(o.relation_endpoints["mentors"].from, ["person"]);
        assert_eq!(o.relation_endpoints["mentors"].to, ["person", "team"]);
        assert_eq!(spines[0].anchors.kind, "person");
    }

    #[test]
    fn a_rename_onto_a_declared_name_merges_and_refuses_a_difference() {
        let base = Ontology {
            types: vec![
                TypeDecl::new("individual", vec![field("bio", FieldKind::String)]),
                TypeDecl::new("person", vec![field("desk", FieldKind::String)]),
            ],
            ..Ontology::default()
        };
        let mut o = base.clone();
        rename(
            &mut o,
            &mut [],
            Renaming::Type,
            "individual",
            "person",
            &|_| false,
        )
        .unwrap();
        let fields: Vec<&str> = o.types[0].fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!((o.types.len(), fields), (1, vec!["desk", "bio"]));
        let mut o = base;
        o.types[0].fields.push(field("desk", FieldKind::Int));
        assert_eq!(
            rename(
                &mut o,
                &mut [],
                Renaming::Type,
                "individual",
                "person",
                &|_| false
            ),
            Err(RenameClash::Merge("desk".into()))
        );
    }

    #[test]
    fn a_relation_rename_keeps_symmetry_or_is_refused() {
        let mut o = Ontology {
            relations: vec!["knows".into(), "likes".into()],
            symmetric: vec!["knows".into()],
            relation_descriptions: [("knows".to_string(), "Met".to_string())]
                .into_iter()
                .collect(),
            ..Ontology::default()
        };
        assert_eq!(
            rename(
                &mut o.clone(),
                &mut [],
                Renaming::Relation,
                "knows",
                "likes",
                &|_| false
            ),
            Err(RenameClash::Symmetry)
        );
        assert_eq!(
            rename(
                &mut o.clone(),
                &mut [],
                Renaming::Relation,
                "likes",
                "same_as",
                &|_| false
            ),
            Err(RenameClash::Symmetry)
        );
        rename(
            &mut o,
            &mut [],
            Renaming::Relation,
            "knows",
            "acquainted",
            &|_| false,
        )
        .unwrap();
        assert_eq!(o.relations, ["acquainted", "likes"]);
        assert_eq!(o.symmetric, ["acquainted"]);
        assert_eq!(o.relation_descriptions["acquainted"], "Met");
    }

    #[test]
    fn suggestions_are_small_slips_from_a_std_user_name_only() {
        let types = suggestions(
            Renaming::Type,
            ["organization", "individual", "plan", "person", "meetings"].into_iter(),
        );
        assert_eq!(
            types,
            [
                ("organization".to_string(), "organisation"),
                ("meetings".to_string(), "meeting")
            ]
        );
        // Reserved and derived names are never offered.
        assert!(suggestions(Renaming::Relation, ["authored_byy"].into_iter()).is_empty());
        assert_eq!(
            suggestions(Renaming::Relation, ["member_off"].into_iter()),
            [("member_off".to_string(), "member_of")]
        );
    }
    #[test]
    fn field_to_tags_moves_text_and_keeps_the_rest_unshown() {
        use crate::Value;
        let none = |_: &str| false;
        let got = field_to_tags(
            "origin",
            &Value::String("Double Diamond".into()),
            &[],
            &none,
        );
        assert_eq!(got.added, ["origin:double_diamond"]);
        assert_eq!(got.kept, None);
        let secret = |s: &str| s == "sec";
        let list =
            serde_json::json!(["A B", "a-b", "123", 7, "sec", {"sec": 1}, [["sec"]], {"a": 1}]);
        let got = field_to_tags("origin", &list, &["origin:a_b".into()], &secret);
        assert_eq!(got.moved.len(), 2, "two texts, one tag");
        assert!(got.added.is_empty(), "the node already held it");
        let why: Vec<Unmovable> = got.unmovable.iter().map(|(_, w)| *w).collect();
        assert_eq!(
            why,
            [
                Unmovable::NoTag,
                Unmovable::NotText,
                Unmovable::Secret,
                Unmovable::Secret,
                Unmovable::Secret,
                Unmovable::NotText
            ]
        );
        let shown: Vec<String> = got
            .unmovable
            .iter()
            .map(|(v, w)| shown_value(v, *w))
            .collect();
        assert_eq!(
            shown,
            [
                "123",
                "<not text>",
                "<masked>",
                "<masked>",
                "<masked>",
                "<object>"
            ]
        );
        let long = "word ".repeat(600);
        assert_eq!(field_tag("origin", &long), Err(Unmovable::TooLong));
        assert_eq!(field_tag("origin", "a:b"), Ok("origin:a_b".into()));
        assert_eq!(field_tag("origin", ""), Err(Unmovable::NoTag));
        let empty = field_to_tags("origin", &serde_json::json!([]), &[], &none);
        assert!(empty.moved.is_empty() && empty.kept.is_none());
        let full: Vec<String> = (0..64).map(|i| alloc::format!("t{i}")).collect();
        assert_eq!(merged_tags(&full, &["origin:x".into()]), None);
        assert!(merged_tags(&full[1..], &["origin:x".into()]).is_some());
    }
}
