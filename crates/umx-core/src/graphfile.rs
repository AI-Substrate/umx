//! The Graphfile (ws002 s17, D66, req-128): a plain TOML file named `Graphfile` at a git repo's top that lists the
//! repo's graphs, by path (relative to the top, the base `root:repo` resolves to), suggested alias and graph id, with
//! optional named sets of aliases to load together. It is committed and people may edit it; umx writes it sorted and
//! byte-stable with its own small encoder ([`Graphfile::encode`]; toml only parses), so rewriting an unchanged file gives the same
//! bytes. It holds no graph data, so it is never a central graph (D63); the machine index (D61) records what this
//! machine loaded, the Graphfile what the repo holds.
//!
//! Pure: parse, validate, encode and resolve sets. Every string in a Graphfile is untrusted: an error names line
//! numbers and fields, never the text it found. Any duplicate (one path, alias or id twice, or one alias twice in a
//! set) or a set member naming no graph makes the whole file invalid, as does a cap (graph-addressing-expert,
//! graph-storage-expert and security, s17).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::Id;
use crate::locator::repo_path;
use crate::problem::ValidName;

/// The file's name, at the repo top.
pub const GRAPHFILE: &str = "Graphfile";
/// The version this umx reads and writes. A higher one is refused on read and never written.
pub const VERSION: i64 = 1;
/// Read and write cap: umx never writes a file it would refuse to read.
pub const MAX_BYTES: usize = 1 << 20;
pub const MAX_ENTRIES: usize = 1_000;
pub const MAX_SETS: usize = 100;
/// The first line umx writes.
pub const HEADER: &str =
    "# Graphfile: the umx graphs in this repo. umx keeps it sorted; see `umx docs graphfile`.\n";

/// One graph the repo holds. The id is its identity, the path its place, the alias only a suggested load name.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    pub alias: String,
    pub id: Id,
    /// Keys umx does not know, kept and written after `id`, sorted by name.
    pub extra: BTreeMap<String, toml::Value>,
}

impl Entry {
    pub fn new(path: String, alias: String, id: Id) -> Entry {
        Entry {
            path,
            alias,
            id,
            extra: BTreeMap::new(),
        }
    }
}

/// A valid Graphfile: entries sorted by path (byte order), each path, alias and id once, and sets whose members name
/// entries' aliases, each set's members sorted and once.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graphfile {
    entries: Vec<Entry>,
    sets: BTreeMap<String, Vec<String>>,
}

/// Which cap a file or a write went over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    Bytes,
    Entries,
    Sets,
}

impl Cap {
    pub fn describe(self) -> String {
        match self {
            Cap::Bytes => alloc::format!("over {} bytes", MAX_BYTES),
            Cap::Entries => alloc::format!("over {MAX_ENTRIES} graphs"),
            Cap::Sets => alloc::format!("over {MAX_SETS} sets"),
        }
    }
}

/// Why a file is not a Graphfile umx can use. Never holds text from the file: `lines` (1-based) and a fixed `what`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid {
        lines: Vec<usize>,
        what: &'static str,
    },
    /// A version newer than [`VERSION`].
    Newer(i64),
    TooLarge(Cap),
}

impl Error {
    fn at(line: usize, what: &'static str) -> Error {
        Error::Invalid {
            lines: vec![line],
            what,
        }
    }

    fn twice(first: usize, again: usize, what: &'static str) -> Error {
        Error::Invalid {
            lines: vec![first, again],
            what,
        }
    }

    /// The reason in words, for a problem's slot: lines and fields only.
    pub fn reason(&self) -> String {
        match self {
            Error::Invalid { lines, what } => match lines.as_slice() {
                [] => (*what).to_string(),
                [one] => alloc::format!("line {one}: {what}"),
                [first, rest @ ..] => {
                    let rest: Vec<String> = rest.iter().map(ToString::to_string).collect();
                    alloc::format!("lines {first} and {}: {what}", rest.join(" and "))
                }
            },
            Error::Newer(v) => {
                alloc::format!("it is version {v} and this umx reads version {VERSION}")
            }
            Error::TooLarge(cap) => alloc::format!("it is {}", cap.describe()),
        }
    }
}

/// Why an entry cannot be added: the Graphfile already lists that path, alias or id (adding it would make the whole
/// file invalid).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clash {
    Path,
    Alias,
    Id,
}

/// The 1-based line holding byte `at` of `text`.
fn line_of(text: &str, at: usize) -> usize {
    text.as_bytes()[..at.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

fn not_toml(text: &str, e: &toml::de::Error) -> Error {
    Error::Invalid {
        lines: e
            .span()
            .map(|s| vec![line_of(text, s.start)])
            .unwrap_or_default(),
        what: "it is not valid TOML",
    }
}

impl Graphfile {
    pub fn new() -> Graphfile {
        Graphfile::default()
    }

    /// The entries, sorted by path.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn sets(&self) -> &BTreeMap<String, Vec<String>> {
        &self.sets
    }

    /// The entries of set `name`, in its members' order; None when there is no such set.
    pub fn set(&self, name: &str) -> Option<Vec<&Entry>> {
        let members = self.sets.get(name)?;
        Some(
            members
                .iter()
                .filter_map(|a| self.entries.iter().find(|e| &e.alias == a))
                .collect(),
        )
    }

    pub fn by_id(&self, id: Id) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn by_path(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// Adds `entry` in its sorted place, or says which of its path, alias or id is listed already. The path must be in
    /// normal form ([`repo_path`]) and the alias valid; the caller made both.
    pub fn add(&mut self, entry: Entry) -> Result<(), Clash> {
        if self.entries.iter().any(|e| e.path == entry.path) {
            return Err(Clash::Path);
        }
        if self.entries.iter().any(|e| e.alias == entry.alias) {
            return Err(Clash::Alias);
        }
        if self.entries.iter().any(|e| e.id == entry.id) {
            return Err(Clash::Id);
        }
        let at = self.entries.partition_point(|e| e.path < entry.path);
        self.entries.insert(at, entry);
        Ok(())
    }

    /// Moves the entry with `id` to `path` (a folder found moved), keeping its sorted place; false when no entry has
    /// that id or another entry has that path.
    pub fn move_to(&mut self, id: Id, path: String) -> bool {
        if self.entries.iter().any(|e| e.path == path) {
            return false;
        }
        let Some(at) = self.entries.iter().position(|e| e.id == id) else {
            return false;
        };
        let mut entry = self.entries.remove(at);
        entry.path = path;
        let at = self.entries.partition_point(|e| e.path < entry.path);
        self.entries.insert(at, entry);
        true
    }

    /// Renames the entry with `id` to `alias`, in every set too (`umx graph rename`, ws006 S23): Ok(true) when it
    /// changed, Ok(false) when no entry has that id or it already has that alias, `Clash::Alias` when another entry
    /// has the alias. The alias must be valid; the caller made it.
    pub fn rename(&mut self, id: Id, alias: &str) -> Result<bool, Clash> {
        let Some(at) = self.entries.iter().position(|e| e.id == id) else {
            return Ok(false);
        };
        if self.entries[at].alias == alias {
            return Ok(false);
        }
        if self.entries.iter().any(|e| e.alias == alias) {
            return Err(Clash::Alias);
        }
        let old = core::mem::replace(&mut self.entries[at].alias, alias.to_string());
        for members in self.sets.values_mut() {
            if let Some(m) = members.iter_mut().find(|m| **m == old) {
                *m = alias.to_string();
                members.sort();
            }
        }
        Ok(true)
    }

    /// Removes the entry with `alias`, and that alias from every set (a set left empty goes); false when no entry
    /// has it (`graph scan --drop`, the undo of `graph new`'s add).
    pub fn remove(&mut self, alias: &str) -> bool {
        let Some(at) = self.entries.iter().position(|e| e.alias == alias) else {
            return false;
        };
        self.entries.remove(at);
        for members in self.sets.values_mut() {
            members.retain(|m| m != alias);
        }
        self.sets.retain(|_, members| !members.is_empty());
        true
    }

    /// The Graphfile in `bytes`, whole or not at all.
    pub fn decode(bytes: &[u8]) -> Result<Graphfile, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge(Cap::Bytes));
        }
        let text = core::str::from_utf8(bytes).map_err(|_| Error::Invalid {
            lines: Vec::new(),
            what: "it is not UTF-8 text",
        })?;
        // Twice: once keeping each key's place (for line numbers), once into values.
        let doc = toml::de::DeTable::parse(text).map_err(|e| not_toml(text, &e))?;
        let mut table: toml::Table = toml::from_str(text).map_err(|e| not_toml(text, &e))?;
        let doc = doc.get_ref();
        let line = |key: &str| {
            doc.iter()
                .find(|(k, _)| k.get_ref().as_ref() == key)
                .map_or(1, |(k, _)| line_of(text, k.span().start))
        };
        for (key, _) in doc.iter() {
            if !matches!(key.get_ref().as_ref(), "graphfile" | "graph" | "sets") {
                return Err(Error::at(
                    line_of(text, key.span().start),
                    "it has a top-level key besides graphfile and graph and sets",
                ));
            }
        }
        match table.remove("graphfile") {
            Some(toml::Value::Integer(v)) if v > VERSION => return Err(Error::Newer(v)),
            Some(toml::Value::Integer(VERSION)) => {}
            Some(_) => return Err(Error::at(line("graphfile"), "graphfile is not 1")),
            None => {
                return Err(Error::at(1, "it has no graphfile version line"));
            }
        }
        let graphs = match table.remove("graph") {
            None => Vec::new(),
            Some(toml::Value::Array(rows)) => rows,
            Some(_) => return Err(Error::at(line("graph"), "graph is not a list of tables")),
        };
        if graphs.len() > MAX_ENTRIES {
            return Err(Error::TooLarge(Cap::Entries));
        }
        // Each entry's line: where its table starts in the file.
        let entry_lines: Vec<usize> = match doc
            .iter()
            .find(|(k, _)| k.get_ref().as_ref() == "graph")
            .map(|(_, v)| v.get_ref())
        {
            Some(toml::de::DeValue::Array(rows)) => {
                rows.iter().map(|r| line_of(text, r.span().start)).collect()
            }
            _ => Vec::new(),
        };
        let mut gf = Graphfile::new();
        let mut seen_path: BTreeMap<String, usize> = BTreeMap::new();
        let mut seen_alias: BTreeMap<String, usize> = BTreeMap::new();
        let mut seen_id: BTreeMap<Id, usize> = BTreeMap::new();
        for (n, row) in graphs.into_iter().enumerate() {
            let at = entry_lines.get(n).copied().unwrap_or(1);
            let toml::Value::Table(mut row) = row else {
                return Err(Error::at(at, "a graph is not a table"));
            };
            let mut text_field = |key: &str, missing: &'static str| match row.remove(key) {
                Some(toml::Value::String(s)) => Ok(s),
                _ => Err(Error::at(at, missing)),
            };
            let path = text_field("path", "a graph has no path string")?;
            let alias = text_field("alias", "a graph has no alias string")?;
            let id = text_field("id", "a graph has no id string")?;
            if repo_path(&path).ok().as_deref() != Some(path.as_str()) {
                return Err(Error::at(
                    at,
                    "a path is not in Graphfile form: see umx docs get graphfile",
                ));
            }
            if ValidName::alias(&alias).is_none() {
                return Err(Error::at(at, "an alias is not a valid alias"));
            }
            let id: Id = id
                .parse()
                .map_err(|_| Error::at(at, "an id is not a graph id"))?;
            if let Some(first) = seen_path.insert(path.clone(), at) {
                return Err(Error::twice(first, at, "a path is listed twice"));
            }
            if let Some(first) = seen_alias.insert(alias.clone(), at) {
                return Err(Error::twice(first, at, "an alias is listed twice"));
            }
            if let Some(first) = seen_id.insert(id, at) {
                return Err(Error::twice(first, at, "an id is listed twice"));
            }
            gf.entries.push(Entry {
                path,
                alias,
                id,
                extra: row.into_iter().collect(),
            });
        }
        gf.entries.sort_by(|a, b| a.path.cmp(&b.path));
        let sets = match table.remove("sets") {
            None => toml::Table::new(),
            Some(toml::Value::Table(sets)) => sets,
            Some(_) => return Err(Error::at(line("sets"), "sets is not a table")),
        };
        if sets.len() > MAX_SETS {
            return Err(Error::TooLarge(Cap::Sets));
        }
        let set_line = |name: &str| match doc
            .iter()
            .find(|(k, _)| k.get_ref().as_ref() == "sets")
            .map(|(_, v)| v.get_ref())
        {
            Some(toml::de::DeValue::Table(t)) => t
                .iter()
                .find(|(k, _)| k.get_ref().as_ref() == name)
                .map_or(1, |(k, _)| line_of(text, k.span().start)),
            _ => line("sets"),
        };
        for (name, members) in sets {
            let at = set_line(&name);
            if ValidName::alias(&name).is_none() {
                return Err(Error::at(at, "a set name is not a valid name"));
            }
            let toml::Value::Array(members) = members else {
                return Err(Error::at(at, "a set is not a list of aliases"));
            };
            let mut list = Vec::new();
            for m in members {
                let toml::Value::String(m) = m else {
                    return Err(Error::at(at, "a set is not a list of aliases"));
                };
                if !seen_alias.contains_key(&m) {
                    return Err(Error::at(at, "a set names an alias no graph has"));
                }
                if list.contains(&m) {
                    return Err(Error::at(at, "a set names an alias twice"));
                }
                list.push(m);
            }
            list.sort();
            gf.sets.insert(name, list);
        }
        Ok(gf)
    }

    /// Whether `bytes` are exactly what umx writes for them. A file that is not (comments, another order) is never
    /// rewritten by `graph new` or `graph scan --add`, only by an explicit `--rewrite`.
    pub fn is_canonical(bytes: &[u8]) -> bool {
        Graphfile::decode(bytes)
            .and_then(|g| g.encode())
            .is_ok_and(|again| again == bytes)
    }

    /// Sets set `name` to `members` (sorted, once each), or removes it when `members` is empty. Every member must be an
    /// entry's alias; false (and nothing changes) otherwise, or when the name is not valid.
    pub fn put_set(&mut self, name: &str, members: &[String]) -> bool {
        if ValidName::alias(name).is_none()
            || !members
                .iter()
                .all(|m| self.entries.iter().any(|e| &e.alias == m))
        {
            return false;
        }
        let mut list = members.to_vec();
        list.sort();
        list.dedup();
        if list.is_empty() {
            self.sets.remove(name);
        } else {
            self.sets.insert(name.to_string(), list);
        }
        true
    }

    /// The file's bytes: the header, `graphfile = 1`, each entry (path, alias, id, then unknown keys by name) in path
    /// order, then `[sets]` when there are any; one blank line between tables, LF only, a final newline. Strings are
    /// written by [`quoted`] and the layout is umx's own, so the bytes never change under a toml upgrade (toml only parses). Refused over a cap, so
    /// umx never writes a file it would refuse to read.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(Error::TooLarge(Cap::Entries));
        }
        if self.sets.len() > MAX_SETS {
            return Err(Error::TooLarge(Cap::Sets));
        }
        let mut out = String::from(HEADER);
        out.push_str("graphfile = 1\n");
        for e in &self.entries {
            out.push_str("\n[[graph]]\n");
            out.push_str(&alloc::format!("path = {}\n", quoted(&e.path)));
            out.push_str(&alloc::format!("alias = {}\n", quoted(&e.alias)));
            out.push_str(&alloc::format!("id = {}\n", quoted(&e.id.to_text())));
            for (k, v) in &e.extra {
                out.push_str(&alloc::format!("{} = {}\n", key(k), value(v)));
            }
        }
        if !self.sets.is_empty() {
            out.push_str("\n[sets]\n");
            for (name, members) in &self.sets {
                let list: Vec<String> = members.iter().map(|m| quoted(m)).collect();
                out.push_str(&alloc::format!("{} = [{}]\n", key(name), list.join(", ")));
            }
        }
        if out.len() > MAX_BYTES {
            return Err(Error::TooLarge(Cap::Bytes));
        }
        Ok(out.into_bytes())
    }
}

/// A TOML basic string holding `s`: `"` and `\` escaped, the short escapes for tab, LF, CR, backspace and form
/// feed, and every other control character as `\uXXXX`. One form for every string, so the bytes never depend on a
/// library's choice between string styles.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_control() => out.push_str(&alloc::format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A kept value, inline: strings as [`quoted`], arrays and tables inline with `, ` between items (tables' keys in
/// order), other scalars as TOML writes them.
fn value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => quoted(s),
        toml::Value::Array(items) => {
            let items: Vec<String> = items.iter().map(value).collect();
            alloc::format!("[{}]", items.join(", "))
        }
        toml::Value::Table(t) if t.is_empty() => "{}".to_string(),
        toml::Value::Table(t) => {
            let items: Vec<String> = t
                .iter()
                .map(|(k, v)| alloc::format!("{} = {}", key(k), value(v)))
                .collect();
            alloc::format!("{{ {} }}", items.join(", "))
        }
        other => other.to_string(),
    }
}

/// A key: bare when TOML allows it bare, else quoted.
fn key(k: &str) -> String {
    let bare = !k.is_empty()
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare { k.to_string() } else { quoted(k) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01JDCYPHER00000000000000A1";
    const B: &str = "01JDCYPHER00000000000000B1";
    const C: &str = "01JDCYPHER00000000000000C1";

    /// The canonical form, byte for byte (graph-storage-expert, s17).
    const GOLDEN: &str =
        "# Graphfile: the umx graphs in this repo. umx keeps it sorted; see `umx docs graphfile`.
graphfile = 1

[[graph]]
path = \".\"
alias = \"top\"
id = \"01JDCYPHER00000000000000C1\"

[[graph]]
path = \"docs/graphs/acme\"
alias = \"acme\"
id = \"01JDCYPHER00000000000000A1\"
note = \"kept\"
owner = \"risk team\"

[[graph]]
path = \"docs/graphs/acme_risks\"
alias = \"acme_risks\"
id = \"01JDCYPHER00000000000000B1\"

[sets]
acme = [\"acme\", \"acme_risks\"]
";

    fn id(s: &str) -> Id {
        s.parse().unwrap()
    }

    fn sample() -> Graphfile {
        let mut g = Graphfile::new();
        g.add(Entry::new(
            "docs/graphs/acme_risks".into(),
            "acme_risks".into(),
            id(B),
        ))
        .unwrap();
        let mut acme = Entry::new("docs/graphs/acme".into(), "acme".into(), id(A));
        acme.extra.insert("owner".into(), "risk team".into());
        acme.extra.insert("note".into(), "kept".into());
        g.add(acme).unwrap();
        g.add(Entry::new(".".into(), "top".into(), id(C))).unwrap();
        assert!(g.put_set("acme", &["acme_risks".into(), "acme".into(), "acme".into()]));
        g
    }

    fn invalid(text: &str) -> (Vec<usize>, &'static str) {
        match Graphfile::decode(text.as_bytes()) {
            Err(Error::Invalid { lines, what }) => (lines, what),
            other => panic!("not invalid: {other:?}\n{text}"),
        }
    }

    #[test]
    fn the_encoding_is_the_golden_bytes_and_round_trips() {
        let bytes = sample().encode().unwrap();
        assert_eq!(String::from_utf8(bytes.clone()).unwrap(), GOLDEN);
        assert_eq!(Graphfile::decode(&bytes).unwrap(), sample());
        assert_eq!(Graphfile::decode(&bytes).unwrap().encode().unwrap(), bytes);
        assert!(Graphfile::is_canonical(&bytes));
    }

    #[test]
    fn an_empty_graphfile_is_two_lines() {
        let bytes = Graphfile::new().encode().unwrap();
        assert_eq!(
            bytes,
            alloc::format!("{HEADER}graphfile = 1\n").into_bytes()
        );
        assert!(Graphfile::is_canonical(&bytes));
    }

    #[test]
    fn strings_are_escaped_and_odd_keys_quoted() {
        let mut g = Graphfile::new();
        let mut e = Entry::new("a b/\u{e9}t\u{e9}".into(), "a".into(), id(A));
        e.extra.insert("odd key".into(), "say \"hi\"\n".into());
        g.add(e).unwrap();
        let bytes = g.encode().unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.contains("\"odd key\" = \"say \\\"hi\\\"\\n\"\n"),
            "{text}"
        );
        assert_eq!(Graphfile::decode(&bytes).unwrap(), g);
        assert!(Graphfile::is_canonical(&bytes));
    }

    #[test]
    fn every_character_toml_must_escape_round_trips_and_kept_values_too() {
        let all: String = (0u32..0x2000).filter_map(char::from_u32).collect();
        let mut g = Graphfile::new();
        let mut e = Entry::new("p".into(), "a".into(), id(A));
        e.extra.insert("all".into(), all.into());
        let mut inner = toml::Table::new();
        inner.insert(
            "z key".into(),
            toml::Value::Array(vec![1.into(), "two\"".into(), true.into()]),
        );
        inner.insert("a".into(), toml::Value::Table(toml::Table::new()));
        e.extra.insert("table".into(), toml::Value::Table(inner));
        e.extra.insert("float".into(), 1.5.into());
        g.add(e).unwrap();
        let bytes = g.encode().unwrap();
        assert!(
            !bytes
                .iter()
                .any(|b| *b == b'\r' || (*b < 0x20 && *b != b'\n')),
            "no raw control bytes"
        );
        assert_eq!(Graphfile::decode(&bytes).unwrap(), g);
        assert!(Graphfile::is_canonical(&bytes));
    }

    #[test]
    fn a_file_in_another_order_or_with_comments_reads_but_is_not_canonical() {
        let commented = GOLDEN.replace("graphfile = 1\n", "graphfile = 1 # hand edited\n");
        assert_eq!(Graphfile::decode(commented.as_bytes()).unwrap(), sample());
        assert!(!Graphfile::is_canonical(commented.as_bytes()));
        let reordered = GOLDEN.replace(
            "alias = \"acme\"\nid = \"01JDCYPHER00000000000000A1\"",
            "id = \"01JDCYPHER00000000000000A1\"\nalias = \"acme\"",
        );
        assert_eq!(Graphfile::decode(reordered.as_bytes()).unwrap(), sample());
        assert!(!Graphfile::is_canonical(reordered.as_bytes()));
    }

    fn one(path: &str) -> String {
        alloc::format!(
            "graphfile = 1\n\n[[graph]]\npath = {}\nalias = \"a\"\nid = \"{A}\"\n",
            quoted(path)
        )
    }

    #[test]
    fn each_path_outside_the_normal_form_is_refused() {
        assert!(
            Graphfile::decode(one("docs/g").as_bytes()).is_ok(),
            "positive control"
        );
        assert!(Graphfile::decode(one(".").as_bytes()).is_ok(), "the top");
        let mut accepted = Vec::new();
        for path in [
            "/abs/g",
            "../g",
            "docs/../g",
            "~/g",
            "~user/g",
            "docs\\g",
            "c:/g",
            "root:repo/g",
            "docs/g/",
            "./docs/g",
            "docs//g",
            "docs/./g",
            "",
            "docs/\u{202E}g",
            "docs/\u{200B}g",
            "docs/\u{7}g",
            "docs/g\n",
        ] {
            // Every path checked before failing, so one run shows each refusal a plant removes.
            match Graphfile::decode(one(path).as_bytes()) {
                Err(Error::Invalid { lines, what })
                    if lines == [3] && what.starts_with("a path is not in Graphfile form") => {}
                other => accepted.push(alloc::format!("{path:?}: {other:?}")),
            }
        }
        assert!(accepted.is_empty(), "not refused:\n{}", accepted.join("\n"));
    }

    #[test]
    fn each_duplicate_makes_the_whole_file_invalid_naming_both_lines() {
        let two = |p2: &str, a2: &str, i2: &str| {
            alloc::format!(
                "graphfile = 1\n\n[[graph]]\npath = \"p1\"\nalias = \"a1\"\nid = \"{A}\"\n\n[[graph]]\npath = \"{p2}\"\nalias = \"{a2}\"\nid = \"{i2}\"\n"
            )
        };
        assert!(
            Graphfile::decode(two("p2", "a2", B).as_bytes()).is_ok(),
            "positive control"
        );
        let got = |text: String| match Graphfile::decode(text.as_bytes()) {
            Err(Error::Invalid { lines, what }) => alloc::format!("{lines:?} {what}"),
            other => alloc::format!("{other:?}"),
        };
        let found = [
            got(two("p1", "a2", B)),
            got(two("p2", "a1", B)),
            got(two("p2", "a2", A)),
            got(two("p1", "a1", A)),
        ];
        assert_eq!(
            found,
            [
                "[3, 8] a path is listed twice",
                "[3, 8] an alias is listed twice",
                "[3, 8] an id is listed twice",
                "[3, 8] a path is listed twice",
            ],
            "each duplicate kind, and an exact repeat"
        );
    }

    #[test]
    fn a_set_naming_an_unknown_or_repeated_alias_is_invalid() {
        let with = |set: &str| {
            alloc::format!(
                "graphfile = 1\n\n[[graph]]\npath = \"p\"\nalias = \"a\"\nid = \"{A}\"\n\n[sets]\n{set}\n"
            )
        };
        assert!(
            Graphfile::decode(with("s = [\"a\"]").as_bytes()).is_ok(),
            "positive control"
        );
        let got = |text: String| match Graphfile::decode(text.as_bytes()) {
            Err(Error::Invalid { lines, what }) => alloc::format!("{lines:?} {what}"),
            other => alloc::format!("{other:?}"),
        };
        assert_eq!(
            [
                got(with("s = [\"a\", \"b\"]")),
                got(with("s = [\"a\", \"a\"]"))
            ],
            [
                "[9] a set names an alias no graph has",
                "[9] a set names an alias twice"
            ]
        );
        assert_eq!(
            invalid(&with("Bad = [\"a\"]")).1,
            "a set name is not a valid name"
        );
        assert_eq!(
            invalid(&with("s = \"a\"")).1,
            "a set is not a list of aliases"
        );
    }

    #[test]
    fn versions_fields_and_keys_are_checked() {
        let ok = alloc::format!(
            "graphfile = 1\n\n[[graph]]\npath = \"p\"\nalias = \"a\"\nid = \"{A}\"\n"
        );
        assert!(Graphfile::decode(ok.as_bytes()).is_ok(), "positive control");
        assert_eq!(
            Graphfile::decode(ok.replace("graphfile = 1", "graphfile = 2").as_bytes()),
            Err(Error::Newer(2))
        );
        assert_eq!(
            invalid(&ok.replace("graphfile = 1", "graphfile = 0")).0,
            [1]
        );
        assert_eq!(
            invalid(&ok.replace("graphfile = 1\n", "")).1,
            "it has no graphfile version line"
        );
        assert_eq!(
            invalid(&ok.replace("alias = \"a\"", "alias = \"Not-Alias\"")).1,
            "an alias is not a valid alias"
        );
        assert_eq!(invalid(&ok.replace(A, "nope")).1, "an id is not a graph id");
        assert_eq!(
            invalid(&ok.replace("path = \"p\"\n", "")).1,
            "a graph has no path string"
        );
        assert_eq!(
            invalid(&ok.replace("graphfile = 1\n", "graphfile = 1\nextra = 1\n")),
            (
                vec![2],
                "it has a top-level key besides graphfile and graph and sets"
            )
        );
        assert_eq!(
            invalid(&(ok.clone() + "path = \"twice\"\n")).1,
            "it is not valid TOML",
            "strict TOML"
        );
        let (lines, what) = invalid("graphfile = 1\n\n[[graph]\n");
        assert_eq!((lines, what), (vec![3], "it is not valid TOML"));
    }

    #[test]
    fn the_byte_cap_holds_on_read() {
        let mut big = String::from("graphfile = 1\n");
        while big.len() <= MAX_BYTES {
            big.push_str("# padding padding padding padding padding padding padding padding\n");
        }
        assert_eq!(
            Graphfile::decode(big.as_bytes()),
            Err(Error::TooLarge(Cap::Bytes))
        );
    }

    #[test]
    fn the_entry_cap_holds_on_read_and_on_write() {
        let mut many = String::from("graphfile = 1\n");
        for n in 0..=MAX_ENTRIES {
            many.push_str(&alloc::format!(
                "[[graph]]\npath = \"g{n}\"\nalias = \"g{n}\"\nid = \"{}\"\n",
                Id(n as u128 + 1).to_text()
            ));
        }
        assert_eq!(
            Graphfile::decode(many.as_bytes()),
            Err(Error::TooLarge(Cap::Entries))
        );

        let mut g = Graphfile::new();
        for n in 0..MAX_ENTRIES {
            g.add(Entry::new(
                alloc::format!("g{n}"),
                alloc::format!("g{n}"),
                Id(n as u128 + 1),
            ))
            .unwrap();
        }
        let full = g.encode().expect("exactly the cap is written");
        assert!(Graphfile::decode(&full).is_ok(), "and read");
        g.entries
            .push(Entry::new("zz".into(), "zz".into(), Id(9_999)));
        assert_eq!(g.encode(), Err(Error::TooLarge(Cap::Entries)));
    }

    #[test]
    fn the_set_cap_holds_on_read_and_on_write() {
        let mut sets = Graphfile::new();
        sets.add(Entry::new("p".into(), "a".into(), id(A))).unwrap();
        for n in 0..MAX_SETS {
            assert!(sets.put_set(&alloc::format!("s{n}"), &["a".into()]));
        }
        let at_cap = sets.encode().unwrap();
        assert!(Graphfile::decode(&at_cap).is_ok());
        assert!(sets.put_set("one_more", &["a".into()]));
        assert_eq!(sets.encode(), Err(Error::TooLarge(Cap::Sets)));
        let text = String::from_utf8(at_cap).unwrap() + "one_more = [\"a\"]\n";
        assert_eq!(
            Graphfile::decode(text.as_bytes()),
            Err(Error::TooLarge(Cap::Sets))
        );
    }

    #[test]
    fn adds_refuse_a_listed_path_alias_or_id_and_moves_keep_order() {
        let mut g = sample();
        assert_eq!(
            g.add(Entry::new("docs/graphs/acme".into(), "x".into(), Id(7))),
            Err(Clash::Path)
        );
        assert_eq!(
            g.add(Entry::new("x".into(), "acme".into(), Id(7))),
            Err(Clash::Alias)
        );
        assert_eq!(
            g.add(Entry::new("x".into(), "x".into(), id(A))),
            Err(Clash::Id)
        );
        assert!(g.move_to(id(C), "zz/top".into()));
        let paths: Vec<&str> = g.entries().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            ["docs/graphs/acme", "docs/graphs/acme_risks", "zz/top"]
        );
        assert!(!g.move_to(id(A), "zz/top".into()), "another entry's path");
        let acme: Vec<&str> = g
            .set("acme")
            .unwrap()
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(acme, ["docs/graphs/acme", "docs/graphs/acme_risks"]);
        assert!(g.set("nope").is_none());
        assert!(!g.put_set("s", &["unknown".into()]));
    }

    #[test]
    fn a_removed_alias_leaves_its_sets_and_an_emptied_set_goes() {
        let mut g = sample();
        assert!(!g.remove("nope"), "an unknown alias changes nothing");
        assert_eq!(g, sample());
        assert!(g.remove("acme"));
        assert_eq!(g.set("acme").unwrap().len(), 1);
        assert!(g.remove("acme_risks"));
        assert!(g.sets().is_empty(), "the emptied set went");
        assert!(Graphfile::decode(&g.encode().unwrap()).is_ok());
    }

    #[test]
    fn an_error_names_lines_and_fields_never_the_text() {
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let text = alloc::format!(
            "graphfile = 1\n\n[[graph]]\npath = \"../{secret}\"\nalias = \"a\"\nid = \"{A}\"\n"
        );
        let e = Graphfile::decode(text.as_bytes()).unwrap_err();
        assert!(!e.reason().contains("ghp_"), "{}", e.reason());
        assert!(e.reason().starts_with("line 3:"), "{}", e.reason());
    }
}
