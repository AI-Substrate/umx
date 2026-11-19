//! Graph settings: the values in graph.json besides the vocabulary that `umx graph set` changes (PRD 001
//! req-71). This table is the one list: the CLI, the daemon, the docs and `graph new`'s next commands read it,
//! so a later setting (similar_k, ...) is one row here.

use crate::ontology::{Described, MAX_DESCRIPTION_CHARS, MAX_GUIDE_TEXT_BYTES};
use crate::problem::Slot;
use crate::{
    Command, EmbedderRule, GraphEmbedding, GraphMeta, Policy, Problem, ValidName, Visibility,
    secrets,
};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub struct Setting {
    /// snake_case (007 rule 2); the flag is exactly `--<name>`.
    pub name: &'static str,
    /// `--<name>`, spelled out so commands built from it use only umx's own words.
    pub flag: &'static str,
    /// The value placeholder in a `next` that must not repeat what was typed, for example `<text>`.
    pub placeholder: &'static str,
    /// A value that works as printed, shell-quoted, for `graph new`'s next commands.
    pub example: &'static str,
    pub summary: &'static str,
    /// Whether `graph new` suggests it in its next commands (the D53 pair is set on purpose, never by default).
    pub suggest: bool,
    /// Changes the setting. Ok carries warnings; Err refuses and changes nothing. The name is the alias the session
    /// selected, for diagnostics only: graph.json's stored alias can differ (`graph load --as`, review B2).
    pub apply: fn(&mut GraphMeta, &ValidName, &str) -> Result<Vec<Problem>, Problem>,
}

pub const SETTINGS: &[Setting] = &[
    Setting {
        name: "title",
        flag: "--title",
        placeholder: "<text>",
        example: "\"Launch planning\"",
        summary: "the graph's title, shown by umx graph list",
        suggest: true,
        apply: set_title,
    },
    Setting {
        name: "policy",
        flag: "--policy",
        placeholder: "<policy>",
        example: "secured",
        summary: "who may embed the graph's text: open, or secured (only an embedder that keeps text on this machine); lowering it takes effect in the same call, recorded on this machine and warned (policy_lowered)",
        suggest: true,
        apply: set_policy,
    },
    Setting {
        name: "visibility",
        flag: "--visibility",
        placeholder: "<visibility>",
        example: "named",
        summary: "whether reads that do not name the graph show it: open, or named (left out of unscoped search and multi-graph reads); a looser value takes effect in the same call, recorded on this machine and warned (policy_lowered)",
        suggest: false,
        apply: set_visibility,
    },
    Setting {
        name: "embedder",
        flag: "--embedder",
        placeholder: "<rule>",
        example: "trusted",
        summary: "which embedder instances may get the graph's text: any, trusted (local instances, or hosted ones defined in the machine config), or local; a looser value takes effect in the same call, recorded on this machine and warned (policy_lowered)",
        suggest: false,
        apply: set_embedder,
    },
    Setting {
        name: "related",
        flag: "--related",
        placeholder: "<on|off>",
        example: "off",
        summary: "whether node add and batch embed each new node to report up to 3 related existing nodes: on or off, over the machine's related setting; the --related and --no-related flags win for one call",
        suggest: false,
        apply: set_related,
    },
    Setting {
        name: "purpose",
        flag: "--purpose",
        placeholder: "<text>",
        example: "\"Decide how to launch in spring\"",
        summary: "what the graph is for, shown first by umx graph guide; an empty value clears it",
        suggest: true,
        apply: set_purpose,
    },
    Setting {
        name: "conventions",
        flag: "--conventions",
        placeholder: "<text>",
        example: "\"Titles are short noun phrases\"",
        summary: "how the graph is used (naming, notes, when to add an edge), shown by umx graph guide; an empty value clears it",
        suggest: true,
        apply: set_conventions,
    },
    Setting {
        name: "description",
        flag: "--description",
        placeholder: "<kind>:<name>=<text>",
        example: "\"status:open=Not worked on yet\"",
        summary: "one line on what a type, status, relation or field means: <kind>:<name>=<text>, a field named <type>.<field>; an empty text clears it",
        suggest: true,
        apply: set_description,
    },
];

pub fn find(name: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.name == name)
}

/// The setting names, for the `setting_unknown` message.
pub fn names() -> Slot {
    Slot::Words(SETTINGS.iter().map(|s| s.name).collect())
}

fn set_title(
    meta: &mut GraphMeta,
    _graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    let title = value.trim();
    if title.is_empty() {
        return Err(Problem::new("usage_invalid")
            .with("what", Slot::Word("the title is empty"))
            .with("noun", Slot::Word("graph")));
    }
    meta.title = title.to_string();
    Ok(Vec::new())
}

/// Writes the policy into graph.json. Raising it takes effect at once; the graph store records a lowering on this
/// machine after the write (D83), so it takes effect in the same `graph set`.
fn set_policy(
    meta: &mut GraphMeta,
    _graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    let policy = match value.trim() {
        "open" => Policy::OPEN,
        "secured" => Policy::SECURED,
        _ => return Err(invalid("the policy is open or secured")),
    };
    write_policy(meta, |_| policy);
    Ok(Vec::new())
}

/// One of the two D53 settings; the other keeps its value.
fn set_visibility(
    meta: &mut GraphMeta,
    _graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    let visibility = match value.trim() {
        "open" => Visibility::Open,
        "named" => Visibility::Named,
        _ => return Err(invalid("the visibility is open or named")),
    };
    write_policy(meta, |p| Policy { visibility, ..p });
    Ok(Vec::new())
}

fn set_embedder(
    meta: &mut GraphMeta,
    _graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    let embedder = match value.trim() {
        "any" => EmbedderRule::Any,
        "trusted" => EmbedderRule::Trusted,
        "local" => EmbedderRule::Local,
        _ => return Err(invalid("the embedder is any, trusted or local")),
    };
    write_policy(meta, |p| Policy { embedder, ..p });
    Ok(Vec::new())
}

fn invalid(what: &'static str) -> Problem {
    Problem::new("usage_invalid")
        .with("what", Slot::Word(what))
        .with("noun", Slot::Word("graph"))
}

fn write_policy(meta: &mut GraphMeta, change: impl FnOnce(Policy) -> Policy) {
    let policy = change(meta.file_policy());
    match &mut meta.embedding {
        Some(e) => e.policy = policy,
        None if policy == Policy::OPEN => {}
        None => meta.embedding = Some(GraphEmbedding::new(None, policy)),
    }
}

/// Records `related: true` or `false` in graph.json's embedding section (req-123, D80): the graph's word, over the
/// machine's `related` setting; a call's flag wins over both. A graph with no section gets one to hold it.
fn set_related(
    meta: &mut GraphMeta,
    _graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    let on = match value.trim() {
        "on" => true,
        "off" => false,
        _ => return Err(invalid("related is on or off")),
    };
    let policy = meta.file_policy();
    meta.embedding
        .get_or_insert_with(|| GraphEmbedding::new(None, policy))
        .related = Some(on);
    Ok(Vec::new())
}

/// A description as stored: trimmed, one line, at most [`MAX_DESCRIPTION_CHARS`] characters. Empty means none.
pub fn check_description(text: &str) -> Result<Option<String>, Problem> {
    let text = text.trim();
    if text.contains(['\n', '\r']) {
        return Err(Problem::new("usage_invalid")
            .with("what", Slot::Word("a description is one line"))
            .with("noun", Slot::Word("guide")));
    }
    let chars = text.chars().count();
    if chars > MAX_DESCRIPTION_CHARS {
        return Err(too_long(
            "a description",
            chars,
            "characters",
            MAX_DESCRIPTION_CHARS,
        ));
    }
    Ok((!text.is_empty()).then(|| text.to_string()))
}

/// A purpose or conventions text as stored: trimmed, at most [`MAX_GUIDE_TEXT_BYTES`] bytes. Empty means none.
fn check_guide_text(what: &'static str, text: &str) -> Result<Option<String>, Problem> {
    let text = text.trim();
    if text.len() > MAX_GUIDE_TEXT_BYTES {
        return Err(too_long(what, text.len(), "bytes", MAX_GUIDE_TEXT_BYTES));
    }
    Ok((!text.is_empty()).then(|| text.to_string()))
}

fn too_long(what: &'static str, length: usize, unit: &'static str, max: usize) -> Problem {
    Problem::new("description_too_long")
        .with("what", Slot::Word(what))
        .with("length", Slot::Count(length as u64))
        .with("unit", Slot::Word(unit))
        .with("max", Slot::Count(max as u64))
}

/// req-49 (security F1): guide text lands in graph.json, which is tracked, so a text that looks like a secret is
/// warned at write time. Names the graph, the setting and the rule, never the text; `fix` sets it again.
pub fn secret_warnings(
    graph: &ValidName,
    setting: &'static str,
    text: Option<&str>,
    fix: Command,
) -> Vec<Problem> {
    match text.and_then(|t| secrets::field_suspect(setting, t)) {
        Some(rule) => alloc::vec![
            Problem::new("setting_secret_suspected")
                .with("graph", Slot::Name(graph.clone()))
                .with("setting", Slot::Word(setting))
                .with("rule", Slot::Word(rule.name()))
                .with("fix", Slot::Command(fix))
        ],
        None => Vec::new(),
    }
}

fn set_guide_text(
    graph: &ValidName,
    setting: &'static str,
    what: &'static str,
    value: &str,
    out: &mut Option<String>,
) -> Result<Vec<Problem>, Problem> {
    let text = check_guide_text(what, value)?;
    let fix = Command::new("graph set")
        .name(graph)
        .word(if setting == "purpose" {
            "--purpose"
        } else {
            "--conventions"
        })
        .word("<text>");
    let warnings = secret_warnings(graph, setting, text.as_deref(), fix);
    *out = text;
    Ok(warnings)
}

fn set_purpose(
    meta: &mut GraphMeta,
    graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    set_guide_text(
        graph,
        "purpose",
        "the purpose",
        value,
        &mut meta.ontology.purpose,
    )
}

fn set_conventions(
    meta: &mut GraphMeta,
    graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    set_guide_text(
        graph,
        "conventions",
        "the conventions",
        value,
        &mut meta.ontology.conventions,
    )
}

/// `<kind>:<name>=<text>`. A type or field must be declared (its description lives on its declaration): else
/// `description_undeclared`, naming the `ontology add` that declares it with the description. A status or relation
/// need not be (soft vocabulary): it is described and warned `off_vocabulary_*`.
fn set_description(
    meta: &mut GraphMeta,
    graph: &ValidName,
    value: &str,
) -> Result<Vec<Problem>, Problem> {
    let bad = || {
        Problem::new("usage_invalid")
            .with(
                "what",
                Slot::Word("--description takes <kind>:<name>=<text> with kind type or status or relation or field and a field named <type>.<field>"),
            )
            .with("noun", Slot::Word("guide"))
    };
    let (target, text) = value.split_once('=').ok_or_else(bad)?;
    let (kind, name) = target.trim().split_once(':').ok_or_else(bad)?;
    let kind = Described::parse(kind).ok_or_else(bad)?;
    let (name, field) = match kind {
        Described::Field => {
            let (t, f) = name.split_once('.').ok_or_else(bad)?;
            (t, Some(ValidName::new(f).ok_or_else(bad)?))
        }
        _ => (name, None),
    };
    let name = ValidName::new(name).ok_or_else(bad)?;
    let text = check_description(text)?;
    let secret = secret_warnings(
        graph,
        "description",
        text.as_deref(),
        Command::new("graph set")
            .name(graph)
            .word("--description")
            .described(kind, &name, field.as_ref()),
    );
    let full = match &field {
        Some(f) => alloc::format!("{}.{}", name.as_str(), f.as_str()),
        None => name.as_str().to_string(),
    };
    if let Err((missing, _)) = meta.ontology.describe(kind, &full, text) {
        return Err(undeclared(Some(graph), missing, &name, field.as_ref()));
    }
    let declared = match kind {
        Described::Status => meta.ontology.has_status(name.as_str()),
        Described::Relation => meta.ontology.has_relation(name.as_str()),
        _ => true,
    };
    let warning = |code: &'static str, slot: &'static str| {
        Problem::new(code)
            .with(slot, Slot::Name(name.clone()))
            .with("graph", Slot::Name(graph.clone()))
    };
    let mut out = match (declared, kind) {
        (false, Described::Status) => alloc::vec![warning("off_vocabulary_status", "status")],
        (false, Described::Relation) => alloc::vec![warning("off_vocabulary_relation", "relation")],
        _ => Vec::new(),
    };
    out.extend(secret);
    Ok(out)
}

/// A description given for a type or field the ontology does not declare. `missing` is what is not declared: the
/// type (then `name` is it), or the field of a declared type.
pub fn undeclared(
    graph: Option<&ValidName>,
    missing: Described,
    name: &ValidName,
    field: Option<&ValidName>,
) -> Problem {
    // A field's `ontology add field` declares its type too, so it is the fix whichever is missing.
    let fix = match field {
        Some(f) => Command::new("ontology add field")
            .name(name)
            .name_with(f, ':', "<kind>"),
        None => Command::new("ontology add type").name(name),
    };
    let fix = fix.word("--description").word("<text>");
    let fix = match graph {
        Some(g) => fix.word("--graph").name(g),
        None => fix,
    };
    let (kind, shown) = match (missing, field) {
        (Described::Field, Some(f)) => ("field", f),
        _ => ("type", name),
    };
    Problem::new("description_undeclared")
        .with("kind of name", Slot::Word(kind))
        .with("name", Slot::Name(shown.clone()))
        .with("fix", Slot::Command(fix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name::is_valid_name;

    #[test]
    fn every_setting_name_is_a_valid_name_and_its_own_flag() {
        for s in SETTINGS {
            assert!(is_valid_name(s.name), "{}", s.name);
            assert_eq!(s.flag.strip_prefix("--"), Some(s.name), "{}", s.name);
            // `--graph` and `--by` are every command's own flags.
            assert!(!["graph", "by"].contains(&s.name), "{}", s.name);
        }
    }
}
