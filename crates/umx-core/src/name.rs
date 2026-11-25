//! Name rules shared by node types, statuses, relations, field names and graph aliases (workshop 007 rule 2).

use crate::evidence::EVIDENCE_REMOVED;
use crate::ontology::PROVENANCE_KEYS;

/// Fields every node carries; refused as custom field names (rule 4).
pub const RESERVED_NODE_FIELDS: &[&str] = &[
    "id",
    "address",
    "graph",
    "type",
    "title",
    "status",
    "note",
    "evidence",
    "evidence_kinds",
    // Read-only in `umx query`: each pointer's source, so a custom field of that name never shadows it.
    "evidence_sources",
    // req-126: the trail of removed evidence (`umx node evidence remove`), kept among the custom fields in name
    // order like edge F75's stamps, so an older umx keeps it byte for byte.
    EVIDENCE_REMOVED,
    "status_history",
    "created",
    "updated",
    "created_by",
    PROVENANCE_KEYS[0],
    PROVENANCE_KEYS[1],
    PROVENANCE_KEYS[2],
    // ws006 S3 (req-77): the locator and the flat names read from it.
    "locator",
    "locator_kind",
    "locator_source",
    "locator_format",
    "locator_hash",
    "locator_length",
    "locator_seq_max",
    "locator_of",
    "locator_selector",
    // F188: the read-side secret flags umx computes on every node object; a stored field of that name is never
    // passed through as if it were them (R324-1).
    "secret_suspected",
];

/// Fields every edge carries, or will (F92, graph-addressing-expert: selector ends, edge evidence and its removal, the
/// std namespace; F75's edit stamps below, so a declared custom field of that name can never make a later edit
/// refuse); refused as custom field names (rule 4). Checked on write only, so a graph that
/// already holds one of these as a custom field still loads.
pub const RESERVED_EDGE_FIELDS: &[&str] = &[
    "id",
    "address",
    "graph",
    "type",
    "from",
    "from_graph",
    "from_selector",
    "to",
    "to_graph",
    "to_selector",
    "evidence",
    "evidence_kinds",
    "evidence_sources",
    EVIDENCE_REMOVED,
    "std",
    "created",
    "created_by",
    // ws006 S5b (req-124): the evidence slot and its derived kinds.
    "evidence",
    "evidence_kinds",
    PROVENANCE_KEYS[0],
    PROVENANCE_KEYS[1],
    PROVENANCE_KEYS[2],
    // F75: the edit stamp `edge edit` writes. Kept among the custom fields in name order (no fixed key place), so
    // an older umx keeps them byte for byte.
    "updated",
    "updated_by",
    // ws008 E2c: a `not_same_as` edge's reason, written only by `umx entity deny` (security).
    crate::ontology::DENY_REASON,
    // F190: the read-side secret flags edge show and edge list compute, as on a node (R324-1).
    "secret_suspected",
];

/// openCypher and LadybugDB keywords, refused as names (workshop 007 rule 2). Fixed now: tightening it later
/// would break existing graphs. Owned by the cypher-expert.
pub const RESERVED_WORDS: &[&str] = &[
    "add",
    "all",
    "alter",
    "and",
    "any",
    "as",
    "asc",
    "ascending",
    "attach",
    "by",
    "call",
    "case",
    "cast",
    "column",
    "contains",
    "copy",
    "count",
    "create",
    "default",
    "delete",
    "desc",
    "descending",
    "detach",
    "distinct",
    "drop",
    "else",
    "end",
    "ends",
    "exists",
    "export",
    "false",
    "from",
    "glob",
    "group",
    "import",
    "in",
    "install",
    "is",
    "key",
    "limit",
    "load",
    "macro",
    "match",
    "merge",
    "node",
    "none",
    "not",
    "null",
    "on",
    "optional",
    "or",
    "order",
    "primary",
    "profile",
    "recursive",
    "rel",
    "remove",
    "rename",
    "return",
    "set",
    "shortest",
    "single",
    "skip",
    "starts",
    "table",
    "then",
    "to",
    "true",
    "union",
    "unwind",
    "use",
    "when",
    "where",
    "with",
    "xor",
    "yield",
];

/// True if `[a-z][a-z0-9_]*` (at most 64 characters) and not a reserved word.
pub fn is_valid_name(name: &str) -> bool {
    is_snake_case(name) && !is_reserved_word(name)
}

/// `[a-z][a-z0-9_]*`, at most 64 characters. Graph aliases use this alone (they are not Cypher names).
pub fn is_snake_case(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 64
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

pub fn is_reserved_word(name: &str) -> bool {
    RESERVED_WORDS.iter().any(|w| w.eq_ignore_ascii_case(name))
}

/// The snake_case form of a name, for the `next` of a `name_invalid` error. None if nothing usable remains.
pub fn suggest_name(name: &str) -> Option<alloc::string::String> {
    let mut out = alloc::string::String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    while out.starts_with(|c: char| c.is_ascii_digit()) {
        out.remove(0);
    }
    if is_reserved_word(&out) {
        out.push_str("_item");
    }
    is_valid_name(&out).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_valid_name("depends_on"));
        assert!(is_valid_name("q2"));
        assert!(!is_valid_name("depends-on"));
        assert!(!is_valid_name("Problem"));
        assert!(!is_valid_name("2x"));
        assert!(!is_valid_name(""));
        assert_eq!(suggest_name("Depends-On").as_deref(), Some("depends_on"));
        assert_eq!(suggest_name("--").as_deref(), None);
        assert!(!is_valid_name("group") && !is_valid_name("Match"));
        assert!(is_snake_case("group"));
        assert_eq!(suggest_name("Group").as_deref(), Some("group_item"));
        assert_eq!(suggest_name("group").as_deref(), Some("group_item"));
    }
}
