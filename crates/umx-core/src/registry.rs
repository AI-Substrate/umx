//! This machine's graph registry (req-56, first slice: ws002 F72, plan 081): `$UMX_HOME/registry/graphs.toml`, one
//! row per graph folder umx has loaded: its id, alias, folder and whether it is loaded, in load order (the earliest
//! row of a graph id is its primary copy, d-21). The daemon reloads the loaded rows when it starts. Machine state,
//! never in a graph or repo folder; it holds no policy (security, F72). Read and written through a LocalState port
//! instance of its own, in its own owner-only folder. Unknown keys are kept, so alias redirects can come later as
//! more keys with no migration (architect, F72).
//!
//! It is also this machine's graph index (ws002 s16, req-116): rows outlive an unload, so `graph list --all` shows
//! every graph loaded here (up to [`MAX_ROWS`]; when full, the oldest unloaded row goes). A row's optional
//! `last_loaded`, `nodes`, `edges` and `model` are written only for a graph that was open when it last loaded, and
//! `policy` is a display hint only, never read as a policy (security, plan 098). `no_primary` lists the graph ids
//! loaded as two or more copies whose primary had unloaded (F80, d-21): a restart keeps them without a primary.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::model::{EmbedderRule, Policy, Visibility};
use crate::ports::embedder::ModelKey;
use crate::{Id, name::is_snake_case, ports::GraphDir};

/// The owner-only folder under `$UMX_HOME` that holds the registry.
pub const REGISTRY_DIR: &str = "registry";
pub const REGISTRY_FILE: &str = "graphs.toml";
pub const REGISTRY_VERSION: u32 = 1;
/// At most this many rows (architect, F72).
pub const MAX_ROWS: usize = 64;
/// The file never grows past this: 4 KiB under the small-file reader's cap, so a full file still reads.
pub const MAX_BYTES: usize = crate::ports::config_files::SMALL_FILE_CAP as usize - 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: Id,
    pub alias: String,
    pub folder: String,
    /// Loaded when the daemon last wrote the row: an explicit unload clears it; idle unload keeps it.
    pub loaded: bool,
    /// When it last loaded here (ISO 8601 UTC); open graphs only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_loaded: Option<String>,
    /// Its node and edge counts when it last loaded; open graphs only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edges: Option<u64>,
    /// graph.json's embedding model key when it last loaded; open graphs only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The policy it had when it last loaded ([`Policy::name`]): shown, never acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Row {
    /// A row with no index details.
    pub fn new(id: Id, alias: String, folder: String, loaded: bool) -> Row {
        Row {
            id,
            alias,
            folder,
            loaded,
            last_loaded: None,
            nodes: None,
            edges: None,
            model: None,
            policy: None,
            extra: BTreeMap::new(),
        }
    }

    fn valid(&self) -> bool {
        is_snake_case(&self.alias) && GraphDir::parse(&self.folder).is_ok()
    }

    /// Drops index details that do not parse (the file is hand-editable): the row itself stays.
    fn tidy(&mut self) {
        if self.last_loaded.as_deref().is_some_and(|t| !iso_like(t)) {
            self.last_loaded = None;
        }
        if self
            .model
            .as_deref()
            .is_some_and(|m| ModelKey::parse(m).is_err())
        {
            self.model = None;
        }
        if self
            .policy
            .as_deref()
            .is_some_and(|p| policy_named(p).is_none())
        {
            self.policy = None;
        }
    }

    /// The policy hint, if it names one.
    pub fn policy_hint(&self) -> Option<Policy> {
        self.policy.as_deref().and_then(policy_named)
    }
}

/// The policy [`Policy::name`] spells `name`.
pub fn policy_named(name: &str) -> Option<Policy> {
    [Visibility::Open, Visibility::Named]
        .into_iter()
        .flat_map(|visibility| {
            [
                EmbedderRule::Any,
                EmbedderRule::Trusted,
                EmbedderRule::Local,
            ]
            .map(|embedder| Policy {
                visibility,
                embedder,
            })
        })
        .find(|p| p.name() == name)
}

/// `2026-11-11T05:06:07Z` or with milliseconds: what [`crate::time::iso_from_ms`] writes.
fn iso_like(text: &str) -> bool {
    (20..=24).contains(&text.len())
        && text.ends_with('Z')
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'T' | b'Z' | b'.'))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Registry {
    pub rows: Vec<Row>,
    /// 1-based positions of rows that did not parse or validate: skipped, reported, and dropped on the next write.
    pub skipped: Vec<usize>,
    /// Graph ids loaded as two or more copies whose primary had unloaded (F80, d-21), kept across a restart.
    pub no_primary: Vec<Id>,
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Limit,
    Newer(u64),
}

impl Registry {
    /// The registry in `bytes`. The whole file fails (and nothing reloads) when it does not parse, has no or a newer
    /// version, or is over its limits; a single bad row is skipped and listed in `skipped` (security, F72).
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        let text = core::str::from_utf8(bytes).map_err(|_| Error::Invalid)?;
        let toml::Value::Table(mut table) =
            toml::from_str::<toml::Value>(text).map_err(|_| Error::Invalid)?
        else {
            return Err(Error::Invalid);
        };
        match table.remove("umx_registry").and_then(|v| v.as_integer()) {
            Some(v) if v > i64::from(REGISTRY_VERSION) => return Err(Error::Newer(v as u64)),
            Some(v) if v == i64::from(REGISTRY_VERSION) => {}
            _ => return Err(Error::Invalid),
        }
        let raw = match table.remove("graphs") {
            None => Vec::new(),
            Some(toml::Value::Array(rows)) => rows,
            Some(_) => return Err(Error::Invalid),
        };
        if raw.len() > MAX_ROWS {
            return Err(Error::Limit);
        }
        let no_primary = match table.remove("no_primary") {
            None => Vec::new(),
            Some(toml::Value::Array(ids)) if ids.len() > MAX_ROWS => return Err(Error::Limit),
            Some(toml::Value::Array(ids)) => ids
                .iter()
                .filter_map(|v| v.as_str()?.parse::<Id>().ok())
                .collect(),
            Some(_) => return Err(Error::Invalid),
        };
        let mut registry = Registry {
            no_primary,
            extra: table.into_iter().collect(),
            ..Registry::default()
        };
        for (n, value) in raw.into_iter().enumerate() {
            match value.try_into::<Row>() {
                Ok(mut row) if row.valid() => {
                    row.tidy();
                    registry.rows.push(row);
                }
                _ => registry.skipped.push(n + 1),
            }
        }
        Ok(registry)
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        // Both limits as decode checks them, so umx never writes a file it would refuse (review R193-B3).
        if self.rows.len() > MAX_ROWS || self.no_primary.len() > MAX_ROWS {
            return Err(Error::Limit);
        }
        if !self.rows.iter().all(Row::valid) {
            return Err(Error::Invalid);
        }
        let mut table: toml::Table = self.extra.clone().into_iter().collect();
        table.insert(
            "umx_registry".into(),
            toml::Value::Integer(REGISTRY_VERSION.into()),
        );
        let rows = self
            .rows
            .iter()
            .map(|r| toml::Value::try_from(r).map_err(|_| Error::Invalid))
            .collect::<Result<Vec<_>, _>>()?;
        table.insert("graphs".into(), toml::Value::Array(rows));
        if !self.no_primary.is_empty() {
            let ids = self
                .no_primary
                .iter()
                .map(|id| toml::Value::String(id.to_text()))
                .collect();
            table.insert("no_primary".into(), toml::Value::Array(ids));
        }
        let bytes = toml::to_string(&table)
            .map_err(|_| Error::Invalid)?
            .into_bytes();
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        Ok(bytes)
    }

    /// Records `row` as loaded. A folder already loaded in this daemon (`in_memory`) keeps its place in load order;
    /// any other row for the folder is moved to the end, as its load order is now the latest. The alias now names
    /// this folder, so another folder's row with the same alias is no longer loaded (an alias names one graph; a
    /// reused alias never leaves two loaded rows). False when the registry is full: the row is not recorded.
    pub fn loaded(&mut self, row: Row, in_memory: bool) -> bool {
        for other in &mut self.rows {
            if other.alias == row.alias && other.folder != row.folder {
                other.loaded = false;
            }
        }
        let at = self.rows.iter().position(|r| r.folder == row.folder);
        match at {
            Some(i) if in_memory => {
                let extra = core::mem::take(&mut self.rows[i].extra);
                self.rows[i] = Row { extra, ..row };
            }
            Some(i) => {
                let old = self.rows.remove(i);
                self.rows.push(Row {
                    extra: old.extra,
                    ..row
                });
            }
            None => {
                // Full: make room by forgetting the oldest unloaded row, never a loaded one.
                if self.rows.len() >= MAX_ROWS {
                    match self.rows.iter().position(|r| !r.loaded) {
                        Some(i) => {
                            self.rows.remove(i);
                        }
                        None => return false,
                    }
                }
                self.rows.push(row);
            }
        }
        true
    }

    /// Removes every row with `alias` and returns how many went (`graph forget`, req-116).
    pub fn forget(&mut self, alias: &str) -> usize {
        let before = self.rows.len();
        self.rows.retain(|r| r.alias != alias);
        before - self.rows.len()
    }

    /// Records whether graph id `id` is without a primary copy (F80); Ok(true) when that changed. At most
    /// [`MAX_ROWS`] ids are kept: one more is refused (`Limit`) and nothing changes.
    pub fn set_no_primary(&mut self, id: Id, none: bool) -> Result<bool, Error> {
        let at = self.no_primary.iter().position(|i| *i == id);
        match (at, none) {
            (None, true) if self.no_primary.len() >= MAX_ROWS => return Err(Error::Limit),
            (None, true) => self.no_primary.push(id),
            (Some(i), false) => {
                self.no_primary.remove(i);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// `graph primary` (F98): moves the row for `folder` ahead of every other row with its graph id, so the next start
    /// loads it first and it is the primary again. False when there is no such row or it is already first.
    pub fn put_first(&mut self, folder: &str) -> bool {
        let Some(at) = self.rows.iter().position(|r| r.folder == folder) else {
            return false;
        };
        let id = self.rows[at].id;
        match self.rows[..at].iter().position(|r| r.id == id) {
            Some(first) => {
                let row = self.rows.remove(at);
                self.rows.insert(first, row);
                true
            }
            None => false,
        }
    }

    /// Clears the loaded flag of the row for `folder`; false when there is none or it was already clear.
    pub fn unloaded(&mut self, folder: &str) -> bool {
        match self
            .rows
            .iter_mut()
            .find(|r| r.folder == folder && r.loaded)
        {
            Some(r) => {
                r.loaded = false;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: u128, alias: &str, loaded: bool) -> Row {
        Row::new(Id(n), alias.into(), alloc::format!("/g/{alias}"), loaded)
    }

    #[test]
    fn a_registry_round_trips_keeping_unknowns_and_skips_only_bad_rows() {
        let text = alloc::format!(
            "umx_registry = 1\nfuture = 'keep'\n[[graphs]]\nid = '{}'\nalias = 'a'\nfolder = '/g/a'\nloaded = true\nlater = 7\n[[graphs]]\nid = '{}'\nalias = 'Bad Alias'\nfolder = '/g/b'\nloaded = true\n[[graphs]]\nid = '{}'\nalias = 'c'\nfolder = 'relative'\nloaded = true\n[[graphs]]\nalias = 'd'\n[[graphs]]\nid = '{}'\nalias = 'e'\nfolder = '/g/e'\nloaded = false\n",
            Id(1),
            Id(2),
            Id(3),
            Id(5)
        );
        let r = Registry::decode(text.as_bytes()).unwrap();
        assert_eq!(
            r.rows.iter().map(|r| r.alias.as_str()).collect::<Vec<_>>(),
            ["a", "e"]
        );
        assert_eq!(r.skipped, [2, 3, 4]);
        assert_eq!(r.rows[0].extra["later"], toml::Value::Integer(7));
        let again = Registry::decode(&r.encode().unwrap()).unwrap();
        assert_eq!((again.rows, again.extra), (r.rows, r.extra));
    }

    #[test]
    fn a_newer_invalid_or_oversized_registry_is_refused_whole() {
        let newer = Registry::decode(b"umx_registry = 2\n[[graphs]]\nfuture = 1\n");
        assert_eq!(newer, Err(Error::Newer(2)));
        for bad in [
            &b"umx_registry = 0\n"[..],
            b"graphs = []\n",
            b"not toml [",
            b"umx_registry = 1\ngraphs = 3\n",
        ] {
            assert_eq!(Registry::decode(bad), Err(Error::Invalid), "{bad:?}");
        }
        assert_eq!(
            Registry::decode(&alloc::vec![b' '; MAX_BYTES + 1]),
            Err(Error::Limit)
        );
        let full = Registry {
            rows: (0..=MAX_ROWS as u128).map(|n| row(n, "a", true)).collect(),
            ..Registry::default()
        };
        assert_eq!(full.encode(), Err(Error::Limit));
        let text = String::from_utf8(full.encode_unchecked()).unwrap();
        assert_eq!(Registry::decode(text.as_bytes()), Err(Error::Limit));
        assert_eq!(
            Registry::decode(b"umx_registry = 1\n").unwrap(),
            Registry::default()
        );
    }

    #[test]
    fn put_first_moves_a_row_ahead_of_its_graph_ids_other_rows_only() {
        let mut r = Registry::default();
        for (n, alias) in [(1, "a"), (2, "x"), (1, "b"), (1, "c")] {
            assert!(r.loaded(row(n, alias, true), false));
        }
        let order = |r: &Registry| r.rows.iter().map(|r| r.alias.clone()).collect::<Vec<_>>();
        assert!(r.put_first("/g/c"));
        assert_eq!(order(&r), ["c", "a", "x", "b"]);
        assert!(!r.put_first("/g/c"), "already first of its id");
        assert!(!r.put_first("/g/x"), "the only row of its id");
        assert!(!r.put_first("/g/none"));
        let again = Registry::decode(&r.encode().unwrap()).unwrap();
        assert_eq!(
            order(&again),
            ["c", "a", "x", "b"],
            "the order is what a restart reads"
        );
    }

    #[test]
    fn rows_keep_load_order_and_a_full_registry_forgets_only_unloaded_rows() {
        let mut r = Registry::default();
        assert!(r.loaded(row(1, "a", true), false));
        assert!(r.loaded(row(2, "b", true), false));
        // A reload of a folder in memory keeps its place; a new load of a folder not in memory moves to the end.
        let renamed = Row {
            alias: "a2".into(),
            ..row(1, "a", true)
        };
        assert!(r.loaded(renamed.clone(), true));
        assert_eq!((r.rows[0].alias.as_str(), r.rows.len()), ("a2", 2));
        assert!(r.unloaded("/g/a"));
        assert!(!r.unloaded("/g/a"), "already clear");
        assert!(r.loaded(renamed, false));
        assert_eq!(
            r.rows.iter().map(|r| r.alias.as_str()).collect::<Vec<_>>(),
            ["b", "a2"]
        );
        let mut full = Registry::default();
        for n in 0..MAX_ROWS as u128 {
            full.rows.push(row(n, &alloc::format!("g{n}"), n != 3));
        }
        assert!(full.loaded(row(99, "new", true), false));
        assert!(!full.rows.iter().any(|r| r.alias == "g3"));
        assert!(!full.loaded(row(100, "more", true), false));
        assert_eq!(full.rows.len(), MAX_ROWS);
    }

    #[test]
    fn a_reused_alias_leaves_one_loaded_row() {
        let mut r = Registry::default();
        assert!(r.loaded(row(1, "demo", true), false));
        r.unloaded("/g/demo");
        let new = Row {
            folder: "/g/new".into(),
            ..row(2, "demo", true)
        };
        assert!(r.loaded(new.clone(), false));
        // Reused while the old one was still remembered as loaded (it had unloaded when idle).
        let mut idle = Registry::default();
        assert!(idle.loaded(row(1, "demo", true), false));
        assert!(idle.loaded(new, false));
        for r in [r, idle] {
            let loaded: Vec<&str> = r
                .rows
                .iter()
                .filter(|r| r.loaded)
                .map(|r| r.folder.as_str())
                .collect();
            assert_eq!(loaded, ["/g/new"]);
        }
    }

    #[test]
    fn index_details_and_no_primary_round_trip_and_bad_details_drop_alone() {
        let mut r = Registry::default();
        let mut a = row(1, "a", true);
        a.last_loaded = Some("2026-11-11T05:06:07.123Z".into());
        a.nodes = Some(3);
        a.edges = Some(2);
        a.model = Some("m@8".into());
        a.policy = Some("open".into());
        assert!(r.loaded(a.clone(), false));
        assert!(r.loaded(row(2, "b", false), false));
        assert_eq!(r.set_no_primary(Id(7), true), Ok(true));
        assert_eq!(r.set_no_primary(Id(7), true), Ok(false), "already set");
        let again = Registry::decode(&r.encode().unwrap()).unwrap();
        assert_eq!(
            (&again.rows, &again.no_primary),
            (&r.rows, &alloc::vec![Id(7)])
        );
        assert_eq!(again.rows[0].policy_hint(), Some(Policy::OPEN));

        let text = alloc::format!(
            "umx_registry = 1\nno_primary = ['{}', 'not an id']\n[[graphs]]\nid = '{}'\nalias = 'a'\nfolder = '/g/a'\nloaded = false\nlast_loaded = 'yesterday'\nmodel = 'not a key'\npolicy = 'wide open'\nnodes = 4\n",
            Id(7),
            Id(1)
        );
        let bad = Registry::decode(text.as_bytes()).unwrap();
        assert_eq!(bad.no_primary, [Id(7)]);
        let row = &bad.rows[0];
        assert_eq!(
            (&row.last_loaded, &row.model, &row.policy, row.nodes),
            (&None, &None, &None, Some(4))
        );

        assert_eq!(r.set_no_primary(Id(7), false), Ok(true));
        assert_eq!(r.forget("a"), 1);
        assert_eq!(r.forget("a"), 0);
        assert_eq!(r.rows.len(), 1);
    }

    /// Review R193-B3: the no-primary marks are bounded as decode bounds them, so a written file always reads back.
    #[test]
    fn no_primary_marks_stop_at_the_limit_the_decoder_reads() {
        let mut r = Registry::default();
        for n in 0..MAX_ROWS as u128 {
            assert_eq!(r.set_no_primary(Id(n), true), Ok(true));
        }
        let full = Registry::decode(&r.encode().unwrap()).unwrap();
        assert_eq!(full.no_primary.len(), MAX_ROWS, "64 marks round-trip");
        assert_eq!(r.set_no_primary(Id(999), true), Err(Error::Limit));
        assert_eq!(r.no_primary.len(), MAX_ROWS, "the 65th changed nothing");
        assert_eq!(
            r.set_no_primary(Id(3), true),
            Ok(false),
            "a known id is fine"
        );
        r.no_primary.push(Id(999));
        assert_eq!(r.encode(), Err(Error::Limit), "65 marks are never written");
        let ids: Vec<String> = (0..=MAX_ROWS as u128)
            .map(|n| alloc::format!("'{}'", Id(n)))
            .collect();
        let text = alloc::format!("umx_registry = 1\nno_primary = [{}]\n", ids.join(", "));
        assert_eq!(Registry::decode(text.as_bytes()), Err(Error::Limit));
    }

    impl Registry {
        fn encode_unchecked(&self) -> Vec<u8> {
            let rows: Vec<_> = self
                .rows
                .iter()
                .map(|r| toml::Value::try_from(r).unwrap())
                .collect();
            let mut t = toml::Table::new();
            t.insert("umx_registry".into(), toml::Value::Integer(1));
            t.insert("graphs".into(), toml::Value::Array(rows));
            toml::to_string(&t).unwrap().into_bytes()
        }
    }
}
