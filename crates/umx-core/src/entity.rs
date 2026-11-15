//! Entity keys (ws008 E1, req-127, D63): one std field, `entity_key`, a list of `scheme:value` keys on a node of
//! any type. Two nodes in different graphs are the same entity when they have the same type and share a key. This
//! module is the one place a key is checked and put in its stored form; ws008 T1's std table names
//! [`ENTITY_KEY`] rather than repeating it.

use crate::id::Id;
use crate::name::is_snake_case;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::Value;

/// The std field that holds a node's entity keys: a list of strings, role data (never embedded or word indexed).
pub const ENTITY_KEY: &str = "entity_key";

/// Schemes whose value is compared without case: an email address and a user principal name (D63c Q2).
const CASELESS: &[&str] = &["email", "upn"];

/// Schemes whose value never holds whitespace: an id, an email address, a user principal name.
const NO_SPACES: &[&str] = &["id", "email", "upn"];

/// The longest value a key may hold, in bytes (graph-addressing-expert F1, security N1).
pub const MAX_VALUE_BYTES: usize = 256;

/// The most keys one node holds (graph-addressing-expert F1, security N3): more is refused at write and ignored past
/// this at load.
pub const MAX_KEYS: usize = 32;

/// Characters that hide or reorder text: bidi controls and marks, zero-width characters, the soft hyphen. A key
/// holding one could look like another key while comparing different (security N1).
pub(crate) fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Why a key was refused, with the fixed key when one can be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyRefused {
    /// The key as given.
    pub given: String,
    pub reason: &'static str,
    /// The key in a form that would be accepted, when there is one.
    pub fixed: Option<String>,
}

/// One key in its stored form: the scheme as given (snake_case), then the value trimmed, lowercased for `email:`,
/// `upn:` and `domain:` (which also loses its trailing dots), with the scheme and host lowercased for `url:`, and in
/// canonical id text for `id:`.
pub fn normalise_key(given: &str) -> Result<String, KeyRefused> {
    let refused = |reason, fixed| KeyRefused {
        given: given.to_string(),
        reason,
        fixed,
    };
    let key = given.trim();
    let Some((scheme, value)) = key.split_once(':') else {
        return Err(refused("no scheme (write scheme:value)", None));
    };
    let value = value.trim();
    if key.chars().any(char::is_control) {
        return Err(refused(
            "a control character (such as a tab or newline)",
            None,
        ));
    }
    if key.chars().any(hidden) {
        return Err(refused("a bidi or zero-width character", None));
    }
    if !is_snake_case(scheme) {
        let fixed = snake(scheme).map(|s| alloc::format!("{s}:{value}"));
        let fixed = fixed.and_then(|f| normalise_key(&f).ok());
        return Err(refused("the scheme is not snake_case", fixed));
    }
    if value.is_empty() {
        return Err(refused("no value after the scheme", None));
    }
    if value.len() > MAX_VALUE_BYTES {
        return Err(refused("the value is over 256 bytes", None));
    }
    if NO_SPACES.contains(&scheme) && value.chars().any(char::is_whitespace) {
        return Err(refused("whitespace inside an id, email or upn value", None));
    }
    if scheme == "id" {
        return match value.parse::<Id>() {
            Ok(id) => Ok(alloc::format!("id:{}", id.to_text())),
            Err(_) => Err(refused("an id: key takes a 26-character umx id", None)),
        };
    }
    if CASELESS.contains(&scheme) {
        return Ok(alloc::format!("{scheme}:{}", value.to_lowercase()));
    }
    if scheme == "domain" {
        let value = value.to_lowercase();
        // R8: every trailing root dot goes, so a stored form normalises to itself.
        let value = value.trim_end_matches('.');
        if value.is_empty() {
            return Err(refused("no value after the scheme", None));
        }
        return Ok(alloc::format!("domain:{value}"));
    }
    if scheme == "url" {
        return Ok(alloc::format!("url:{}", url_form(value)));
    }
    Ok(alloc::format!("{scheme}:{value}"))
}

/// A `url:` value with its scheme and host lowercased and the rest (path, query, fragment) kept as given (ws008 E2b
/// F8). A value with no `://` is kept as it is.
fn url_form(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    // R7: user info before the last `@` keeps its case; only the host (and port) is lowercased.
    let (user, host) = match authority.rfind('@') {
        Some(at) => authority.split_at(at + 1),
        None => ("", authority),
    };
    alloc::format!(
        "{}://{user}{}{tail}",
        scheme.to_ascii_lowercase(),
        host.to_lowercase()
    )
}

/// A field value as a list of keys in stored form: each normalised, repeats dropped, first use kept. A string is one
/// key (or several, comma separated); a list must hold only strings.
pub fn normalise_keys(value: &Value) -> Result<Vec<String>, KeyRefused> {
    let given: Vec<&str> = match value {
        Value::String(s) => s.split(',').collect(),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let Some(s) = item.as_str() else {
                    return Err(KeyRefused {
                        given: item.to_string(),
                        reason: "a key must be a string",
                        fixed: None,
                    });
                };
                out.push(s);
            }
            out
        }
        other => {
            return Err(KeyRefused {
                given: other.to_string(),
                reason: "entity_key is a list of scheme:value strings",
                fixed: None,
            });
        }
    };
    let mut keys: Vec<String> = Vec::with_capacity(given.len());
    for g in given {
        if g.trim().is_empty() {
            continue;
        }
        let key = normalise_key(g)?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    if keys.len() > MAX_KEYS {
        return Err(KeyRefused {
            given: String::new(),
            reason: "a node holds at most 32 keys",
            fixed: None,
        });
    }
    Ok(keys)
}

/// The keys of a stored value that are well formed, in stored form, at most [`MAX_KEYS`]; anything else is skipped
/// (a hand edit), so a key refused at write never counts at load either.
pub fn stored_keys(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => {
            let mut keys: Vec<String> = Vec::new();
            for key in items.iter().filter_map(Value::as_str) {
                if keys.len() == MAX_KEYS {
                    break;
                }
                if let Ok(key) = normalise_key(key)
                    && !keys.contains(&key)
                {
                    keys.push(key);
                }
            }
            keys
        }
        _ => Vec::new(),
    }
}

/// What a stored `entity_key` value holds besides keys in stored form (a hand edit, a merge, an older umx): keys that
/// parse but are not written in stored form, and entries that cannot be used at all (not a string, not a key, past
/// [`MAX_KEYS`], or a value that is not a list). Both are reported on load; neither stops the load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoredFindings {
    pub not_canonical: bool,
    pub unusable: bool,
}

pub fn stored_findings(value: &Value) -> StoredFindings {
    let mut f = StoredFindings::default();
    let Value::Array(items) = value else {
        f.unusable = true;
        return f;
    };
    let mut usable = 0;
    for item in items {
        match item.as_str().map(normalise_key) {
            Some(Ok(key)) => {
                usable += 1;
                if item.as_str() != Some(key.as_str()) {
                    f.not_canonical = true;
                }
            }
            _ => f.unusable = true,
        }
    }
    if usable > MAX_KEYS {
        f.unusable = true;
    }
    f
}

/// A stored value with every usable key put in stored form (repeats dropped, first use kept) and every other entry
/// kept as it is, in order: what a node's next write stores, so a hand-edited key becomes canonical without losing
/// anything umx cannot read. A value that is not a list is kept as it is.
pub fn canonical(value: &Value) -> Value {
    let Value::Array(items) = value else {
        return value.clone();
    };
    let mut out: Vec<Value> = Vec::with_capacity(items.len());
    for item in items {
        let v = match item.as_str().map(normalise_key) {
            Some(Ok(key)) => Value::String(key),
            _ => item.clone(),
        };
        if !out.contains(&v) {
            out.push(v);
        }
    }
    Value::Array(out)
}

/// The index key for a node of type `kind` holding `key`: the match rule is the same type and a shared key.
pub fn index_key(kind: &str, key: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(kind.len() + 1 + key.len());
    out.extend_from_slice(kind.as_bytes());
    out.push(0);
    out.extend_from_slice(key.as_bytes());
    out
}

/// A scheme in snake_case, or None if nothing usable is left.
fn snake(scheme: &str) -> Option<String> {
    let mut out = String::new();
    for c in scheme.trim().chars() {
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
    is_snake_case(&out).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ws008 E2b F8: `domain:` is caseless and loses one trailing dot; `url:` lowercases its scheme and host only.
    #[test]
    fn domain_and_url_keys_have_one_stored_form() {
        for (given, stored) in [
            ("domain:Microsoft.com", "domain:microsoft.com"),
            ("domain: Example.COM. ", "domain:example.com"),
            (
                "url:HTTPS://Example.COM/Path",
                "url:https://example.com/Path",
            ),
            ("url:http://Host.Example?Q=A", "url:http://host.example?Q=A"),
            ("url:Example.COM/Path", "url:Example.COM/Path"),
            ("domain:Example.COM..", "domain:example.com"),
            (
                "url:HTTPS://User@Example.COM/Path",
                "url:https://User@example.com/Path",
            ),
            (
                "url:https://a:B@Host.Example:8080/X",
                "url:https://a:B@host.example:8080/X",
            ),
            ("name:Microsoft.com", "name:Microsoft.com"),
        ] {
            assert_eq!(normalise_key(given).unwrap(), stored, "{given}");
        }
        assert!(normalise_key("domain:.").is_err());
        assert!(
            normalise_key("domain:..").is_err(),
            "R8: nothing but dots is no domain"
        );
        // R8: a stored form normalises to itself, so a lookup by the given text finds it.
        for given in [
            "domain:Example.COM..",
            "domain:x.org.",
            "url:HTTPS://User@Example.COM/Path?Q",
            "email:Jo@X.com",
        ] {
            let once = normalise_key(given).unwrap();
            assert_eq!(normalise_key(&once).unwrap(), once, "{given}");
        }
        assert_ne!(
            normalise_key("url:https://Ann@x.com/").unwrap(),
            normalise_key("url:https://ann@x.com/").unwrap(),
            "R7: user info keeps its case"
        );
        let f = stored_findings(&json!(["domain:Microsoft.com"]));
        assert!(
            f.not_canonical && !f.unusable,
            "an old mixed-case domain loads and is rewritten"
        );
        assert_eq!(
            stored_keys(&json!(["domain:Microsoft.com"])),
            ["domain:microsoft.com"]
        );
    }

    #[test]
    fn keys_are_put_in_one_stored_form() {
        assert_eq!(
            normalise_key(" email: Foo@X.com ").unwrap(),
            "email:foo@x.com"
        );
        assert_eq!(
            normalise_key("upn:Jo.Smith@Corp.EXAMPLE").unwrap(),
            "upn:jo.smith@corp.example"
        );
        assert_eq!(
            normalise_key("name:Example.com").unwrap(),
            "name:Example.com"
        );
        assert_eq!(
            normalise_key("abn:51 824 753 556").unwrap(),
            "abn:51 824 753 556"
        );
        let id = "01M74CQV0000000000000000AB";
        assert_eq!(
            normalise_key(&alloc::format!("id:{}", id.to_lowercase())).unwrap(),
            alloc::format!("id:{id}")
        );
    }

    #[test]
    fn malformed_keys_are_refused_with_a_fix_where_one_exists() {
        assert_eq!(normalise_key("foo@x.com").unwrap_err().fixed, None);
        let bad = normalise_key("E-Mail:Foo@X.com").unwrap_err();
        assert_eq!(bad.reason, "the scheme is not snake_case");
        assert_eq!(bad.fixed.as_deref(), Some("e_mail:Foo@X.com"));
        let bad = normalise_key("Email:Foo@X.com").unwrap_err();
        assert_eq!(bad.fixed.as_deref(), Some("email:foo@x.com"));
        assert!(normalise_key("email:").is_err());
        assert!(normalise_key("id:not-an-id").is_err());
        assert!(normalise_key(":x").is_err());
    }

    #[test]
    fn a_list_is_normalised_and_deduplicated_in_order() {
        let v = json!(["email:B@x.com", "domain:x.com", "email:b@X.com"]);
        assert_eq!(
            normalise_keys(&v).unwrap(),
            ["email:b@x.com", "domain:x.com"]
        );
        assert_eq!(
            normalise_keys(&json!("email:a@x.com, domain:x.com")).unwrap(),
            ["email:a@x.com", "domain:x.com"]
        );
        assert!(normalise_keys(&json!([1])).is_err());
        assert!(normalise_keys(&json!(true)).is_err());
        assert_eq!(normalise_keys(&json!([])).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn values_are_bounded_and_clean() {
        let long = "x".repeat(MAX_VALUE_BYTES + 1);
        let at_limit = "x".repeat(MAX_VALUE_BYTES);
        for (key, why) in [
            (
                alloc::format!("domain:{long}"),
                "the value is over 256 bytes",
            ),
            (
                "email:a\tb@x.com".into(),
                "a control character (such as a tab or newline)",
            ),
            (
                "domain:x\ny.com".into(),
                "a control character (such as a tab or newline)",
            ),
            (
                "email:jo\u{200B}@x.com".into(),
                "a bidi or zero-width character",
            ),
            (
                "domain:ex\u{202E}moc.elpmaxe".into(),
                "a bidi or zero-width character",
            ),
            (
                "email:jo smith@x.com".into(),
                "whitespace inside an id, email or upn value",
            ),
            (
                "upn:jo smith".into(),
                "whitespace inside an id, email or upn value",
            ),
            (
                "id:01M74CQV0000 000000000AB".into(),
                "whitespace inside an id, email or upn value",
            ),
        ] {
            assert_eq!(normalise_key(&key).unwrap_err().reason, why, "{key:?}");
        }
        assert!(normalise_key(&alloc::format!("domain:{at_limit}")).is_ok());
        assert!(
            normalise_key("abn:51 824 753 556").is_ok(),
            "spaces are fine in other schemes"
        );
        let many: Vec<String> = (0..=MAX_KEYS).map(|i| alloc::format!("n:{i}")).collect();
        assert_eq!(
            normalise_keys(&json!(many)).unwrap_err().reason,
            "a node holds at most 32 keys"
        );
        assert_eq!(
            normalise_keys(&json!(many[..MAX_KEYS])).unwrap().len(),
            MAX_KEYS
        );
        assert_eq!(
            stored_keys(&json!(many)).len(),
            MAX_KEYS,
            "a hand edit past the cap counts 32"
        );
        let hidden = json!(["email:jo\u{200B}@x.com", "email:jo@x.com"]);
        assert_eq!(
            stored_keys(&hidden),
            ["email:jo@x.com"],
            "dropped at load too"
        );
    }

    #[test]
    fn a_stored_value_is_judged_and_made_canonical_keeping_what_cannot_be_read() {
        let v = json!(["email:Ana@X.com", "nope", "email:ana@x.com", 3]);
        assert_eq!(
            stored_findings(&v),
            StoredFindings {
                not_canonical: true,
                unusable: true
            }
        );
        assert_eq!(canonical(&v), json!(["email:ana@x.com", "nope", 3]));
        assert_eq!(
            stored_findings(&json!(["email:a@x.com"])),
            StoredFindings::default()
        );
        assert!(stored_findings(&json!("email:a@x.com")).unusable);
    }

    #[test]
    fn stored_keys_skip_what_a_hand_edit_broke() {
        let v = json!(["email:A@x.com", "nope", 3, "email:a@x.com"]);
        assert_eq!(stored_keys(&v), ["email:a@x.com"]);
        assert!(stored_keys(&json!("email:a@x.com")).is_empty());
    }
}
