//! umx's own config (req-72): the typed catalogue of settings, the deterministic file codec and how the layers
//! merge. Pure: bytes in, values out. The files are read and written through the ConfigFiles port, secrets
//! through the Secrets port; neither is ever read from another tool.
//!
//! Layers, lowest first: default, machine (`$UMX_HOME/config.toml`), repo (`<repo>/.umx/config.toml`, only the
//! keys marked `repo`, and only to narrow), env, flag. A file is flat TOML written in a fixed order; a hand-written
//! `[table]` parses the same.

use crate::model::EmbedderRule;
use crate::name::is_snake_case;
use crate::ports::{GraphDir, Locality};
use crate::problem::{Candidate, Command, Problem, SafeText, Slot};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::Serialize;

/// The config file's name, in `$UMX_HOME` and in a repo's `.umx/` folder.
pub const FILE_NAME: &str = "config.toml";
/// The secrets file umx owns, in `$UMX_HOME`: `NAME=value` lines, mode 0600.
pub const SECRETS_FILE_NAME: &str = "secrets.env";
/// The folder in a repository that holds its config file.
pub const REPO_DIR: &str = ".umx";
/// The table of named embedder instances: `embedders.<name>.<key>`.
pub const INSTANCES: &str = "embedders";
/// The table of source roots evidence may cite as `root:<name>/<path>` (req-47, ws006 S1): `source_roots.<name> =
/// <folder>`, machine layer only, since a repo must not widen what umx reads on a machine.
pub const SOURCE_ROOTS: &str = "source_roots";
/// The built-in source root, `root:repo/<path>`: the git top level holding the graph folder. Never configured.
pub const REPO_ROOT: &str = "repo";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// 1 to 256 characters, no control characters.
    Text,
    /// An `https://` URL with a host and no user info.
    Https,
    /// The name of an environment variable: `[A-Z_][A-Z0-9_]{0,63}`. Never a value.
    EnvName,
    /// One of `now`; the `later` values are known but not supported yet.
    Choice {
        now: &'static [&'static str],
        later: &'static [&'static str],
    },
    Int {
        min: i64,
        max: i64,
    },
    /// `0` (never) or a duration such as `30m`, between the bounds in ms.
    Duration {
        min_ms: u64,
        max_ms: u64,
    },
    /// Absolute folders; on the command line and in env, colon-separated.
    Folders,
    /// `none` or the name of an embedder instance.
    Instance,
    /// A decimal number from `min` to `max` ten-thousandths; a whole number in range is one too.
    Decimal {
        min: i32,
        max: i32,
    },
    /// One absolute path, no control characters.
    Path,
    /// One absolute folder.
    Folder,
}

/// When a changed setting takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    Now,
    #[serde(rename = "daemon restart")]
    Restart,
}

/// One catalogue row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySpec {
    /// The whole key, or for an instance key the part after `embedders.<name>.`.
    pub key: &'static str,
    pub kind: ValueKind,
    pub doc: &'static str,
    /// What a valid value looks like, for the `config_value_invalid` message.
    pub expected: &'static str,
    /// The default as it would be written on the command line; None when unset by default or set by the host.
    pub default: Option<&'static str>,
    /// May the repo layer set it (to narrow only)?
    pub repo: bool,
    /// The env var that overrides the files.
    pub env: Option<&'static str>,
    /// The flag that overrides everything.
    pub flag: Option<&'static str>,
    pub applies: Applies,
    /// A value for a runnable `umx config set` next.
    pub example: &'static str,
    /// An invalid value makes the setting empty rather than falling back to the wider default.
    pub fail_closed: bool,
}

const fn row(
    key: &'static str,
    kind: ValueKind,
    doc: &'static str,
    expected: &'static str,
) -> KeySpec {
    KeySpec {
        key,
        kind,
        doc,
        expected,
        default: None,
        repo: false,
        env: None,
        flag: None,
        applies: Applies::Now,
        example: "<value>",
        fail_closed: false,
    }
}

/// The longest idle unload: 30 days.
pub const MAX_IDLE_UNLOAD_MS: u64 = 720 * 3_600_000;

/// The settings that are not in a table, in file order.
pub const SETTINGS: &[KeySpec] = &[
    KeySpec {
        env: Some("UMX_ROOTS"),
        applies: Applies::Restart,
        example: "/Users/me/work:/Users/me/notes",
        fail_closed: true,
        ..row(
            "roots",
            ValueKind::Folders,
            "Folders graph files may live in. Default: your home folder and the temp folder.",
            "absolute folders separated by colons",
        )
    },
    KeySpec {
        default: Some("30m"),
        flag: Some("--idle-unload"),
        applies: Applies::Restart,
        example: "2h",
        ..row(
            "daemon.idle_unload",
            ValueKind::Duration {
                min_ms: 1_000,
                max_ms: MAX_IDLE_UNLOAD_MS,
            },
            "A loaded graph unused this long unloads itself; 0 means never.",
            "0 or a duration from 1s to 720h such as 30m",
        )
    },
    KeySpec {
        default: Some("none"),
        repo: true,
        applies: Applies::Restart,
        example: "none",
        ..row(
            "embedder.active",
            ValueKind::Instance,
            "The embedder instance graphs use, or none. A repo may only choose none or a local instance. The daemon wires it at start.",
            "none or the name of an embedders instance",
        )
    },
    // req-123 (D80): the machine's default for the related check on node add and batch. A graph's `related` setting
    // and the call's --related or --no-related win over it.
    KeySpec {
        default: Some("on"),
        example: "off",
        ..row(
            "related",
            ValueKind::Choice {
                now: &["on", "off"],
                later: &[],
            },
            "Whether node add and batch embed each new node to report related existing nodes: on or off. A graph's own related setting, then the call's --related or --no-related, win over it.",
            "on or off",
        )
    },
    // `umx ui` (ws009 s2): what it runs. Machine only, never the repo layer: a repo must not choose a program.
    KeySpec {
        env: Some("UMX_UI_DIR"),
        example: "/Users/me/unimatrix/apps/ui",
        ..row(
            "ui.dir",
            ValueKind::Path,
            "The built graph view (apps/ui) umx ui serves. Default: apps/ui beside the umx source umx was built from.",
            "an absolute path",
        )
    },
    KeySpec {
        example: "/opt/homebrew/bin/node",
        ..row(
            "ui.node",
            ValueKind::Path,
            "The node umx ui runs the view with. Default: the first node on PATH.",
            "an absolute path",
        )
    },
];

/// The keys of one embedder instance, `embedders.<name>.<key>`, in file order. Every one is machine only, and applies
/// at daemon restart: the daemon builds every instance once when it starts (ws004 s05).
pub const INSTANCE_KEYS: &[KeySpec] = &[
    KeySpec {
        applies: Applies::Restart,
        example: "local",
        ..row(
            "kind",
            ValueKind::Choice {
                now: &["azure_openai", "local"],
                later: &[],
            },
            "Which adapter serves the instance.",
            "azure_openai or local",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "https://example.openai.azure.com",
        ..row(
            "endpoint",
            ValueKind::Https,
            "The Azure OpenAI resource URL.",
            "an https:// URL with no user name or password",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "text-embedding-3-small",
        ..row(
            "deployment",
            ValueKind::Text,
            "The Azure deployment name (not the model name).",
            "1 to 256 characters with no control characters",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "text-embedding-3-small",
        ..row(
            "model",
            ValueKind::Text,
            "The model; with dims it names the vector space (vectors are keyed by model@dims, never by instance).",
            "1 to 256 characters with no control characters",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "256",
        ..row(
            "dims",
            ValueKind::Int { min: 1, max: 4096 },
            "The width of the vectors the instance produces.",
            "a whole number from 1 to 4096",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        default: Some("2024-02-01"),
        example: "2024-02-01",
        ..row(
            "api_version",
            ValueKind::Text,
            "The Azure OpenAI API version.",
            "1 to 256 characters with no control characters",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        default: Some("cli"),
        example: "cli",
        ..row(
            "auth",
            ValueKind::Choice {
                now: &["cli", "api_key"],
                later: &["managed_identity", "workload_identity"],
            },
            "cli signs in through az login (Entra ID); api_key reads the key named by api_key_env.",
            "cli or api_key",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "AZURE_OPENAI_API_KEY",
        ..row(
            "api_key_env",
            ValueKind::EnvName,
            "The NAME of the env var (or secrets.env line) that holds the key; never the key itself.",
            "an env var name such as AZURE_OPENAI_API_KEY",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "1500000",
        ..row(
            "tokens_per_minute",
            ValueKind::Int {
                min: 1,
                max: 100_000_000,
            },
            "How fast background embedding may send to this instance, in estimated tokens a minute (ws004 s25). Default 1500000 for azure_openai (umx shares its deployment's quota with other tools); local instances are unpaced unless set. Two instances on one Azure resource are paced apart, so their sum can pass its cap. A 429 halves it until the provider recovers.",
            "a whole number from 1 to 100000000",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "4",
        ..row(
            "threads",
            ValueKind::Int { min: 1, max: 64 },
            "local: CPU threads per session. Default 4.",
            "a whole number from 1 to 64",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "1",
        ..row(
            "sessions",
            ValueKind::Int { min: 1, max: 16 },
            "local: sessions in the pool, each loading the model once. Default 1 (no pool).",
            "a whole number from 1 to 16",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "16",
        ..row(
            "batch_size",
            ValueKind::Int { min: 1, max: 256 },
            "local: texts per model run. Default 16.",
            "a whole number from 1 to 256",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "gpu",
        ..row(
            "execution",
            ValueKind::Choice {
                now: &["gpu", "cpu"],
                later: &[],
            },
            "local: gpu (the default where umx has a GPU backend: Metal on macOS, CUDA in a cuda build) or cpu (about 3 texts a second).",
            "gpu or cpu",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "0.25",
        ..row(
            "search_floor",
            ValueKind::Decimal {
                min: -10_000,
                max: 10_000,
            },
            "The cosine a vector row needs to take part in umx search for this instance's model, replacing the \
             calibrated floor (results then say floor_source config, floor_calibrated false). Machine only.",
            "a number from -1 to 1",
        )
    },
    KeySpec {
        applies: Applies::Restart,
        example: "0.5",
        ..row(
            "similar_floor",
            ValueKind::Decimal {
                min: -10_000,
                max: 10_000,
            },
            "The score a similar_to edge needs for this instance's model, replacing the calibrated floor; a graph's \
             own embedding.similarity.floor still wins. Changing it rebuilds stored similar_to lists. Machine only.",
            "a number from -1 to 1",
        )
    },
];

/// The one key of a source root, `source_roots.<name>`. Machine only; applies at once (each evidence write reads it).
pub const ROOT_KEY: KeySpec = KeySpec {
    example: "/Users/me/transcripts",
    ..row(
        "",
        ValueKind::Folder,
        "A folder evidence may cite as root:<name>/<path>. It must be inside roots. Add one with umx root add.",
        "an absolute folder",
    )
};

/// A value in a config file or on the command line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Text(String),
    Int(i64),
    List(Vec<String>),
    Decimal(Decimal),
    /// A TOML value of a type no setting takes (a boolean, float, date, table or mixed array).
    Other(Unread),
}

/// A value umx cannot use, kept as written so rewriting the file never drops it: an invalid `roots` stays invalid
/// (and closed) until it is changed on purpose. It serialises as its kind only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unread {
    pub kind: &'static str,
    /// The value as TOML.
    toml: String,
}

impl Unread {
    fn value(kind: &'static str, value: &toml::Value) -> ConfigValue {
        ConfigValue::Other(Unread {
            kind,
            toml: value.to_string(),
        })
    }
}

impl Serialize for Unread {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.kind)
    }
}

/// A TOML decimal number, kept bit for bit so a rewrite never changes it. Serialises as the number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decimal(u64);

impl Decimal {
    pub fn new(value: f64) -> Decimal {
        Decimal(value.to_bits())
    }

    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl Serialize for Decimal {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(self.get())
    }
}

impl ConfigValue {
    /// A number: a decimal, or a whole number.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            ConfigValue::Decimal(d) => Some(d.get()),
            ConfigValue::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ConfigValue::Text(t) => Some(t),
            _ => None,
        }
    }
}

/// One file layer: full dotted key to value.
pub type Entries = BTreeMap<String, ConfigValue>;

/// A file layer umx writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileLayer {
    Machine,
    Repo,
}

impl FileLayer {
    pub fn word(self) -> &'static str {
        match self {
            FileLayer::Machine => "machine",
            FileLayer::Repo => "repo",
        }
    }
}

/// A parsed key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Setting(&'static KeySpec),
    Instance {
        name: String,
        spec: &'static KeySpec,
    },
    /// `source_roots.<name>`.
    Root(String),
}

impl Key {
    pub fn spec(&self) -> &'static KeySpec {
        match self {
            Key::Setting(spec) | Key::Instance { spec, .. } => spec,
            Key::Root(_) => &ROOT_KEY,
        }
    }

    pub fn full(&self) -> String {
        match self {
            Key::Setting(spec) => spec.key.to_string(),
            Key::Instance { name, spec } => format!("{INSTANCES}.{name}.{}", spec.key),
            Key::Root(name) => format!("{SOURCE_ROOTS}.{name}"),
        }
    }
}

/// What `config unset` removes: one key, or a whole instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Key(Key),
    Instance(String),
}

/// An instance name: snake_case, at most 32 characters, and never `none`, which `embedder.active` reserves for "no
/// embedder" (so a repo's `none` always allows nothing, N1).
pub fn is_instance_name(name: &str) -> bool {
    name != "none" && name.len() <= 32 && is_snake_case(name)
}

/// A source root name: snake_case, at most 32 characters, and never `repo`, which is built in.
pub fn is_root_name(name: &str) -> bool {
    name != REPO_ROOT && name.len() <= 32 && is_snake_case(name)
}

pub fn key(text: &str) -> Option<Key> {
    if let Some(spec) = SETTINGS.iter().find(|s| s.key == text) {
        return Some(Key::Setting(spec));
    }
    if let Some(name) = text
        .strip_prefix(SOURCE_ROOTS)
        .and_then(|r| r.strip_prefix('.'))
    {
        return is_root_name(name).then(|| Key::Root(name.to_string()));
    }
    let rest = text.strip_prefix(INSTANCES)?.strip_prefix('.')?;
    let (name, leaf) = rest.split_once('.')?;
    let spec = INSTANCE_KEYS.iter().find(|s| s.key == leaf)?;
    is_instance_name(name).then(|| Key::Instance {
        name: name.to_string(),
        spec,
    })
}

pub fn target(text: &str) -> Option<Target> {
    if let Some(k) = key(text) {
        return Some(Target::Key(k));
    }
    let name = text.strip_prefix(INSTANCES)?.strip_prefix('.')?;
    is_instance_name(name).then(|| Target::Instance(name.to_string()))
}

/// Every key umx knows, instance keys written with `<name>`, for an unknown key's candidates.
pub fn known_keys() -> Vec<String> {
    SETTINGS
        .iter()
        .map(|s| s.key.to_string())
        .chain(
            INSTANCE_KEYS
                .iter()
                .map(|s| format!("{INSTANCES}.<name>.{}", s.key)),
        )
        .chain([format!("{SOURCE_ROOTS}.<name>")])
        .collect()
}

/// `0`, or a whole number followed by ms, s, m or h.
pub fn duration_ms(text: &str) -> Option<u64> {
    if text == "0" {
        return Some(0);
    }
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (n, unit) = text.split_at(split);
    let scale = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    };
    n.parse::<u64>().ok()?.checked_mul(scale)
}

const NOT_YET: &str = "a value umx supports now (that one is not supported yet)";
const LOOKS_SECRET: &str =
    "a value that is not a key or token (secrets go in the environment or secrets.env)";

/// True for text that looks like a key or token: a known token prefix, or 32 or more letters and digits in a row.
pub fn looks_like_secret(text: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "xox",
        "AKIA",
        "eyJ",
    ];
    if PREFIXES.iter().any(|p| text.starts_with(p)) {
        return true;
    }
    let mut run = 0;
    for c in text.chars() {
        run = if c.is_ascii_alphanumeric() {
            run + 1
        } else {
            0
        };
        if run >= 32 {
            return true;
        }
    }
    false
}

/// True for a key name that looks like it holds a secret.
pub fn secret_like_key(key: &str) -> bool {
    // An instance's pace in tokens is a rate, not a credential (ws004 s25): it is refused in a repo layer as
    // machine only.
    let key = key.strip_suffix("tokens_per_minute").unwrap_or(key);
    ["key", "token", "secret", "password"]
        .iter()
        .any(|w| key.contains(w))
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('A'..='Z' | '_'))
        && name.len() <= 64
        && chars.all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'))
}

fn is_text(text: &str) -> bool {
    (1..=256).contains(&text.chars().count()) && !text.chars().any(char::is_control)
}

fn is_https(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    url.len() <= 2048
        && !host.is_empty()
        && !host.contains('@')
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Checks a value against its row. The error is the reason, for the message.
pub fn validate(spec: &KeySpec, value: &ConfigValue) -> Result<(), &'static str> {
    if let ConfigValue::Text(t) = value
        && looks_like_secret(t)
        && spec.kind != ValueKind::EnvName
    {
        return Err(LOOKS_SECRET);
    }
    let ok = match (spec.kind, value) {
        (ValueKind::Text, ConfigValue::Text(t)) => is_text(t),
        (ValueKind::Https, ConfigValue::Text(t)) => is_https(t),
        (ValueKind::EnvName, ConfigValue::Text(t)) => is_env_name(t),
        (ValueKind::Choice { now, later }, ConfigValue::Text(t)) => {
            if later.contains(&t.as_str()) {
                return Err(NOT_YET);
            }
            now.contains(&t.as_str())
        }
        (ValueKind::Int { min, max }, ConfigValue::Int(n)) => (min..=max).contains(n),
        (ValueKind::Duration { min_ms, max_ms }, ConfigValue::Text(t)) => {
            duration_ms(t).is_some_and(|ms| ms == 0 || (min_ms..=max_ms).contains(&ms))
        }
        (ValueKind::Folders, ConfigValue::List(dirs)) => {
            !dirs.is_empty() && dirs.iter().all(|d| GraphDir::parse(d).is_ok())
        }
        (ValueKind::Instance, ConfigValue::Text(t)) => t == "none" || is_instance_name(t),
        (ValueKind::Decimal { min, max }, ConfigValue::Decimal(_) | ConfigValue::Int(_)) => {
            value.as_number().is_some_and(|n| {
                n.is_finite() && (f64::from(min) / 1e4..=f64::from(max) / 1e4).contains(&n)
            })
        }
        (ValueKind::Path, ConfigValue::Text(t)) => is_text(t) && t.starts_with('/'),
        (ValueKind::Folder, ConfigValue::Text(t)) => {
            t.len() <= 1024 && !t.chars().any(char::is_control) && GraphDir::parse(t).is_ok()
        }
        _ => false,
    };
    if ok { Ok(()) } else { Err(spec.expected) }
}

/// A value typed on the command line, checked.
pub fn parse_value(spec: &KeySpec, text: &str) -> Result<ConfigValue, &'static str> {
    let value = match spec.kind {
        ValueKind::Int { .. } => ConfigValue::Int(text.parse().map_err(|_| spec.expected)?),
        ValueKind::Decimal { .. } => {
            ConfigValue::Decimal(Decimal::new(text.parse().map_err(|_| spec.expected)?))
        }
        ValueKind::Folders => ConfigValue::List(
            text.split(':')
                .filter(|d| !d.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => ConfigValue::Text(text.to_string()),
    };
    validate(spec, &value)?;
    Ok(value)
}

/// A config file that does not parse: line and column (1-based) and why. Never quotes the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub line: usize,
    pub column: usize,
    pub reason: &'static str,
}

fn line_col(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

fn flatten(prefix: &str, value: &toml::Value, out: &mut Entries) {
    let key = |k: &str| {
        if prefix.is_empty() {
            k.to_string()
        } else {
            format!("{prefix}.{k}")
        }
    };
    match value {
        toml::Value::Table(t) => {
            for (k, v) in t {
                flatten(&key(k), v, out);
            }
        }
        toml::Value::String(s) => {
            out.insert(prefix.to_string(), ConfigValue::Text(s.clone()));
        }
        toml::Value::Integer(n) => {
            out.insert(prefix.to_string(), ConfigValue::Int(*n));
        }
        toml::Value::Array(items) => {
            let texts: Option<Vec<String>> = items
                .iter()
                .map(|i| i.as_str().map(str::to_string))
                .collect();
            let v = texts.map_or_else(
                || Unread::value("a list that is not all text", value),
                ConfigValue::List,
            );
            out.insert(prefix.to_string(), v);
        }
        toml::Value::Boolean(_) => {
            out.insert(prefix.to_string(), Unread::value("true or false", value));
        }
        toml::Value::Float(f) if f.is_finite() => {
            out.insert(prefix.to_string(), ConfigValue::Decimal(Decimal::new(*f)));
        }
        toml::Value::Float(_) => {
            out.insert(prefix.to_string(), Unread::value("a decimal number", value));
        }
        toml::Value::Datetime(_) => {
            out.insert(prefix.to_string(), Unread::value("a date", value));
        }
    }
}

/// Reads a config file. Nested tables flatten to dotted keys.
pub fn decode(bytes: &[u8]) -> Result<Entries, FileError> {
    let text = core::str::from_utf8(bytes).map_err(|e| {
        let (line, column) = line_col(
            core::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
            e.valid_up_to(),
        );
        FileError {
            line,
            column,
            reason: "the file is not UTF-8",
        }
    })?;
    let table: toml::Table = toml::from_str(text).map_err(|e| {
        let (line, column) = e.span().map_or((1, 1), |s| line_col(text, s.start));
        FileError {
            line,
            column,
            reason: "this is not valid TOML",
        }
    })?;
    let mut out = Entries::new();
    for (k, v) in &table {
        flatten(k, v, &mut out);
    }
    Ok(out)
}

/// File order: settings in catalogue order, then instances by name with their keys in catalogue order, then source
/// roots by name, then any other key by name.
fn rank(key: &str) -> (u8, String, usize) {
    if let Some(i) = SETTINGS.iter().position(|s| s.key == key) {
        return (0, String::new(), i);
    }
    match self::key(key) {
        Some(Key::Instance { name, spec }) => {
            let i = INSTANCE_KEYS.iter().position(|s| s == spec).unwrap_or(0);
            (1, name, i)
        }
        Some(Key::Root(name)) => (2, name, 0),
        _ => (3, key.to_string(), 0),
    }
}

fn bare(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn toml_key(key: &str) -> String {
    key.split('.')
        .map(|seg| {
            if bare(seg) {
                seg.to_string()
            } else {
                toml::Value::String(seg.to_string()).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn toml_value(value: &ConfigValue) -> String {
    let v = match value {
        ConfigValue::Text(t) => toml::Value::String(t.clone()),
        ConfigValue::Int(n) => toml::Value::Integer(*n),
        ConfigValue::Decimal(d) => toml::Value::Float(d.get()),
        ConfigValue::List(items) => toml::Value::Array(
            items
                .iter()
                .map(|i| toml::Value::String(i.clone()))
                .collect(),
        ),
        ConfigValue::Other(u) => return u.toml.clone(),
    };
    v.to_string()
}

const HEADER: &str = "# umx config. Written by `umx config set` and `umx config unset`; see `umx docs get config`.\n";

/// Writes a config file: a fixed header, then one `key = value` line per entry in file order. The same entries
/// always give the same bytes. Every entry is written, a value umx cannot use as it was read, so a rewrite never
/// drops or repairs a setting it was not asked to change.
pub fn encode(entries: &Entries) -> Vec<u8> {
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort_by_cached_key(|k| rank(k));
    let mut out = String::from(HEADER);
    for k in keys {
        out.push_str(&toml_key(k));
        out.push_str(" = ");
        out.push_str(&toml_value(&entries[k]));
        out.push('\n');
    }
    out.into_bytes()
}

/// Where a resolved value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Default,
    Machine,
    Repo,
    Env,
    Flag,
    /// No layer sets it and it has no default.
    Unset,
    /// A fail-closed setting whose value is invalid: it is empty until fixed.
    Invalid,
}

/// What `resolve` reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub machine: Entries,
    pub repo: Option<Entries>,
    /// Env var name to raw value, only for the catalogue's env names that are set and not blank.
    pub env: BTreeMap<String, String>,
    /// Key to the value a flag gave.
    pub flags: Entries,
    /// Key to a default the host knows (roots: home and the temp folder).
    pub defaults: Entries,
}

/// One resolved setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setting {
    pub key: String,
    pub spec: &'static KeySpec,
    pub value: Option<ConfigValue>,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Resolved {
    /// Settings in file order, then each instance's keys.
    pub settings: Vec<Setting>,
    /// Everything `config check` reports, in a fixed order.
    pub findings: Vec<Problem>,
    /// Fail-closed keys whose value is invalid and that no env or flag overrides.
    pub closed: Vec<String>,
}

impl Resolved {
    pub fn get(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }

    pub fn value(&self, key: &str) -> Option<&ConfigValue> {
        self.get(key).and_then(|s| s.value.as_ref())
    }

    /// The defined instance names, sorted.
    pub fn instances(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .settings
            .iter()
            .filter_map(|s| match key(&s.key) {
                Some(Key::Instance { name, .. }) => Some(name),
                _ => None,
            })
            .collect();
        names.dedup();
        names
    }

    /// The source roots with a valid folder, by name: (name, folder as written).
    pub fn source_roots(&self) -> Vec<(String, String)> {
        self.settings
            .iter()
            .filter_map(|s| match (key(&s.key), &s.value) {
                (Some(Key::Root(name)), Some(ConfigValue::Text(folder))) => {
                    Some((name, folder.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The configured roots, or empty when invalid (fail closed).
    pub fn roots(&self) -> Vec<String> {
        match self.value("roots") {
            Some(ConfigValue::List(dirs)) => dirs.clone(),
            _ => Vec::new(),
        }
    }

    /// The idle unload in ms.
    pub fn idle_unload_ms(&self) -> u64 {
        self.value("daemon.idle_unload")
            .and_then(ConfigValue::as_text)
            .and_then(duration_ms)
            .unwrap_or(30 * 60_000)
    }
}

fn instance_names(entries: &Entries) -> Vec<String> {
    let mut names: Vec<String> = entries
        .keys()
        .filter_map(|k| match key(k) {
            Some(Key::Instance { name, .. }) => Some(name),
            _ => None,
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

fn root_names(entries: &Entries) -> Vec<String> {
    entries
        .keys()
        .filter_map(|k| match key(k) {
            Some(Key::Root(name)) => Some(name),
            _ => None,
        })
        .collect()
}

fn default_value(spec: &KeySpec, full: &str, defaults: &Entries) -> Option<ConfigValue> {
    defaults
        .get(full)
        .cloned()
        .or_else(|| spec.default.and_then(|d| parse_value(spec, d).ok()))
}

/// An env value for a setting: folders keep only absolute entries (UMX_ROOTS as before), others must be valid.
fn env_value(spec: &KeySpec, raw: &str) -> Option<ConfigValue> {
    match spec.kind {
        ValueKind::Folders => {
            let dirs: Vec<String> = raw
                .split(':')
                .map(str::trim)
                .filter(|d| GraphDir::parse(d).is_ok())
                .map(str::to_string)
                .collect();
            Some(ConfigValue::List(dirs))
        }
        _ => parse_value(spec, raw.trim()).ok(),
    }
}

/// Merges the layers and lists what is wrong. Never fails: a bad value is skipped (or, for a fail-closed key,
/// empties the setting) and reported.
pub fn resolve(inputs: &Inputs) -> Resolved {
    let mut findings = Vec::new();
    let mut closed = Vec::new();
    let machine = &inputs.machine;
    let empty = Entries::new();
    let repo = inputs.repo.as_ref().unwrap_or(&empty);
    let defined = instance_names(machine);
    let source_roots = root_names(machine);

    // Unknown keys first, machine then repo, by name.
    for (layer, entries) in [(FileLayer::Machine, machine), (FileLayer::Repo, repo)] {
        for k in entries.keys().filter(|k| key(k).is_none()) {
            findings.push(match deprecated(k) {
                Some(why) => problems::key_deprecated(k, why, layer),
                None => problems::key_unknown(k, Some(layer)),
            });
        }
    }

    // The repo layer: only keys marked repo, never anything that looks like a secret, and only to narrow.
    let mut repo_ok = Entries::new();
    for (k, v) in repo {
        let Some(parsed) = key(k) else { continue };
        let spec = parsed.spec();
        let looks_secret = secret_like_key(k) || v.as_text().is_some_and(looks_like_secret);
        if looks_secret {
            findings.push(problems::repo_ignored(k, problems::WHY_SECRET));
        } else if !spec.repo {
            findings.push(problems::repo_ignored(k, problems::WHY_MACHINE_ONLY));
        } else if let Err(why) = validate(spec, v) {
            findings.push(problems::value_invalid(k, FileLayer::Repo, why, spec));
        } else if k == "embedder.active" && !narrows(v, machine) {
            findings.push(problems::repo_ignored(k, problems::WHY_WIDENS));
        } else {
            repo_ok.insert(k.clone(), v.clone());
        }
    }

    let mut settings = Vec::new();
    // Instance keys are machine only: no repo layer, env var or flag may define an instance or change one (D62: an
    // instance is trusted because it is defined here).
    let keys = SETTINGS
        .iter()
        .map(|s| (s.key.to_string(), s, false))
        .chain(defined.iter().flat_map(|name| {
            INSTANCE_KEYS
                .iter()
                .map(move |s| (format!("{INSTANCES}.{name}.{}", s.key), s, true))
        }))
        .chain(
            source_roots
                .iter()
                .map(|name| (format!("{SOURCE_ROOTS}.{name}"), &ROOT_KEY, true)),
        );
    for (full, spec, machine_only) in keys {
        let mut value = default_value(spec, &full, &inputs.defaults);
        let mut source = if value.is_some() {
            Source::Default
        } else {
            Source::Unset
        };
        if let Some(v) = machine.get(&full) {
            match validate(spec, v) {
                Ok(()) => (value, source) = (Some(v.clone()), Source::Machine),
                Err(why) => {
                    findings.push(problems::value_invalid(
                        &full,
                        FileLayer::Machine,
                        why,
                        spec,
                    ));
                    if spec.fail_closed {
                        (value, source) = (None, Source::Invalid);
                    }
                }
            }
        }
        if let Some(v) = repo_ok.get(&full) {
            (value, source) = (Some(v.clone()), Source::Repo);
        }
        if let Some(v) = spec
            .env
            .and_then(|e| inputs.env.get(e))
            .and_then(|raw| env_value(spec, raw))
        {
            (value, source) = (Some(v), Source::Env);
        }
        if let Some(v) = inputs.flags.get(&full).filter(|_| !machine_only) {
            (value, source) = (Some(v.clone()), Source::Flag);
        }
        if source == Source::Invalid {
            closed.push(full.clone());
        }
        settings.push(Setting {
            key: full,
            spec,
            value,
            source,
        });
    }
    let mut resolved = Resolved {
        settings,
        findings,
        closed,
    };
    let extra = required(&resolved, &defined);
    resolved.findings.extend(extra);
    resolved
}

/// A repo's embedder.active narrows if it is none or names a machine instance of kind local.
fn narrows(value: &ConfigValue, machine: &Entries) -> bool {
    match value.as_text() {
        Some("none") => true,
        Some(name) => {
            machine.get(&format!("{INSTANCES}.{name}.kind"))
                == Some(&ConfigValue::Text("local".into()))
        }
        None => false,
    }
}

/// Missing keys an instance needs, and an active instance that is not defined.
fn required(resolved: &Resolved, defined: &[String]) -> Vec<Problem> {
    let mut out = Vec::new();
    if let Some(Setting {
        value: Some(ConfigValue::Text(active)),
        ..
    }) = resolved.get("embedder.active")
        && active != "none"
        && !defined.contains(active)
    {
        out.push(problems::instance_unknown("embedder.active"));
    }
    for name in defined {
        let get = |leaf: &str| resolved.value(&format!("{INSTANCES}.{name}.{leaf}"));
        let need = |leaf: &'static str, why: &'static str, out: &mut Vec<Problem>| {
            if get(leaf).is_none() {
                out.push(problems::missing(name, leaf, why));
            }
        };
        need("kind", "every instance needs a kind", &mut out);
        need("model", "vectors are keyed by model and dims", &mut out);
        need("dims", "vectors are keyed by model and dims", &mut out);
        if get("kind").and_then(ConfigValue::as_text) == Some("azure_openai") {
            need("endpoint", "an azure_openai instance needs it", &mut out);
            need("deployment", "an azure_openai instance needs it", &mut out);
        }
        if get("auth").and_then(ConfigValue::as_text) == Some("api_key") {
            need("api_key_env", "auth api_key needs it", &mut out);
        }
    }
    out
}

/// Whether instance `name` is trusted (D62, the operator: "If I configure an embedding then I trust it"): it is defined
/// in the machine config. Only the machine layer can define an instance or set its keys (no repo layer, env var or flag
/// can), so every defined instance is trusted. A graph whose embedder setting is `trusted` may use any of them; `local`
/// still gets only a local one.
pub fn trusted(resolved: &Resolved, name: &str) -> bool {
    resolved.instances().iter().any(|n| n == name)
}

/// The instances whose hosted shape (kind, endpoint, deployment, model, dims) differs between `before` and `after`:
/// added, removed, repointed, or made unbuildable by a partial unset (R202-2), on either side hosted. Which graphs
/// that moves is the caller's to work out with the selection rule (D62, security s20b); a change here is a heads-up,
/// never a gate.
pub fn route_changes(before: &Resolved, after: &Resolved) -> Vec<String> {
    let get = |r: &Resolved, name: &str, leaf: &str| {
        r.value(&format!("{INSTANCES}.{name}.{leaf}")).cloned()
    };
    // Every key that says where a hosted instance sends and what it makes; None for a local one or none at all.
    let hosted = |r: &Resolved, name: &str| {
        let leaves =
            ["kind", "endpoint", "deployment", "model", "dims"].map(|leaf| get(r, name, leaf));
        let local = leaves[0].as_ref().and_then(ConfigValue::as_text) == Some("local");
        let any = leaves.iter().any(Option::is_some);
        (any && !local).then_some(leaves)
    };
    let mut names = before.instances();
    names.extend(after.instances());
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|n| {
            let (was, now) = (hosted(before, n), hosted(after, n));
            (was.is_some() || now.is_some()) && was != now
        })
        .collect()
}

/// Keys umx once read and now ignores, with why: one left in a config file warns `config_key_deprecated`, never
/// `config_key_unknown`, so an older config keeps working.
pub fn deprecated(text: &str) -> Option<&'static str> {
    let rest = text.strip_prefix(INSTANCES)?.strip_prefix('.')?;
    let (name, leaf) = rest.split_once('.')?;
    (is_instance_name(name) && leaf == "trusted_endpoint")
        .then_some("every instance in the machine config is trusted (D62)")
}

/// The instance chosen for a graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Embedder {
    pub name: String,
    pub kind: String,
}

/// Why no embedder may be used. `key` is `embedder.active` or `--embedder`.
#[derive(Clone, Debug, PartialEq)]
pub enum EmbedderRefused {
    /// The name is not a defined instance (`config_instance_unknown`).
    Unknown(Problem),
    /// The graph's embedder setting does not admit the instance (req-46, req-91).
    NotAdmitted { key: &'static str, instance: String },
}

/// The embedder a graph uses: `requested` (a flag) else `embedder.active`. The graph's embedder setting beats every
/// layer: `local` gets only a local instance and `trusted` a local one or one defined in the machine config
/// ([`trusted`]), whatever env, flag or config ask for. The repo layer in `resolved` must come from the graph's folder,
/// not the caller's working folder.
pub fn embedder_for(
    resolved: &Resolved,
    rule: EmbedderRule,
    requested: Option<&str>,
) -> Result<Option<Embedder>, EmbedderRefused> {
    let (name, key) = match requested {
        Some(r) => (r.to_string(), "--embedder"),
        None => match resolved
            .value("embedder.active")
            .and_then(ConfigValue::as_text)
        {
            Some(a) => (a.to_string(), "embedder.active"),
            None => return Ok(None),
        },
    };
    if name == "none" {
        return Ok(None);
    }
    if !resolved.instances().contains(&name) {
        return Err(EmbedderRefused::Unknown(problems::instance_unknown(key)));
    }
    let kind = resolved
        .value(&format!("{INSTANCES}.{name}.kind"))
        .and_then(ConfigValue::as_text)
        .unwrap_or("")
        .to_string();
    let locality = if kind == "local" {
        Locality::Local
    } else {
        Locality::Hosted
    };
    if !rule.admits(locality, trusted(resolved, &name)) {
        return Err(EmbedderRefused::NotAdmitted {
            key,
            instance: name,
        });
    }
    Ok(Some(Embedder { name, kind }))
}

/// The config codes: constructors whose slot names match umx-protocol's catalogue templates.
pub mod problems {
    use super::*;

    /// Every code this module returns. The daemon checks each has a row in umx-protocol's catalogue.
    pub const CODES: &[&str] = &[
        "config_key_unknown",
        "config_key_deprecated",
        "config_value_invalid",
        "config_repo_ignored",
        "config_repo_refused",
        "config_repo_none",
        "config_missing",
        "config_instance_unknown",
        "config_file_invalid",
        "config_file_refused",
        "config_invalid",
        "config_restart_needed",
        "secret_missing",
        "secrets_line_invalid",
    ];

    pub const WHY_MACHINE_ONLY: &str = "it is machine only (set it in the machine config)";
    pub const WHY_SECRET: &str =
        "it looks like a secret (secrets go in the environment or secrets.env)";
    pub const WHY_WIDENS: &str = "a repo may only choose none or an instance whose kind is local";

    /// The fix that sets a key to its example. Never for a fail-closed key: a roots value is the human's choice, and
    /// umx never suggests widening its own file access (usage note 031), so those and source roots point to the
    /// guidance.
    fn set(key: &str, spec: &KeySpec, layer: FileLayer) -> Command {
        if spec.fail_closed || spec.kind == ValueKind::Folder {
            return Command::new("docs get config");
        }
        let cmd = Command::new("config set").key(key).word(spec.example);
        match layer {
            FileLayer::Machine => cmd,
            FileLayer::Repo => cmd.word("--repo"),
        }
    }

    fn unset(key: &str, layer: FileLayer) -> Command {
        let cmd = Command::new("config unset").key(key);
        match layer {
            FileLayer::Machine => cmd,
            FileLayer::Repo => cmd.word("--repo"),
        }
    }

    /// `layer` is the file the key was found in, or None when it was typed.
    pub fn key_unknown(key: &str, layer: Option<FileLayer>) -> Problem {
        let fix = match layer {
            Some(l) if SafeText::key(key).is_some() => unset(key, l),
            _ => Command::new("config show"),
        };
        Problem::new("config_key_unknown")
            .with("key", Slot::key_or(key, "(that key)"))
            .with("fix", Slot::Command(fix))
            .candidates(
                known_keys()
                    .into_iter()
                    .map(|id| Candidate {
                        id,
                        title: None,
                        score: None,
                    })
                    .collect(),
            )
    }

    pub fn value_invalid(
        key: &str,
        layer: FileLayer,
        why: &'static str,
        spec: &KeySpec,
    ) -> Problem {
        let fix = if layer == FileLayer::Repo && !spec.repo {
            unset(key, layer)
        } else {
            set(key, spec, layer)
        };
        Problem::new("config_value_invalid")
            .with("key", Slot::key_or(key, "(that key)"))
            .with("layer", Slot::Word(layer.word()))
            .with("expected", Slot::Word(why))
            .with("fix", Slot::Command(fix))
            .subject(key)
    }

    /// A key found in the repo file that umx ignores.
    pub fn repo_ignored(key: &str, why: &'static str) -> Problem {
        Problem::new("config_repo_ignored")
            .with("key", Slot::key_or(key, "(that key)"))
            .with("why", Slot::Word(why))
            .with("fix", Slot::Command(unset(key, FileLayer::Repo)))
            .subject(key)
    }

    /// `config set --repo` on a key the repo may not set.
    pub fn repo_refused(key: &str, why: &'static str, spec: &KeySpec) -> Problem {
        Problem::new("config_repo_refused")
            .with("key", Slot::key_or(key, "(that key)"))
            .with("why", Slot::Word(why))
            .with("fix", Slot::Command(set(key, spec, FileLayer::Machine)))
            .subject(key)
    }

    pub fn repo_none() -> Problem {
        Problem::new("config_repo_none")
    }

    pub fn missing(instance: &str, leaf: &'static str, why: &'static str) -> Problem {
        let full = format!("{INSTANCES}.{instance}.{leaf}");
        let spec = INSTANCE_KEYS
            .iter()
            .find(|s| s.key == leaf)
            .expect("an instance key");
        Problem::new("config_missing")
            .with("key", Slot::key_or(&full, "(that key)"))
            .with("why", Slot::Word(why))
            .with("fix", Slot::Command(set(&full, spec, FileLayer::Machine)))
            .subject(full)
    }

    /// A key umx no longer reads, found in a config file: ignored, with why and the command that removes it.
    pub fn key_deprecated(key: &str, why: &'static str, layer: FileLayer) -> Problem {
        Problem::new("config_key_deprecated")
            .with("key", Slot::key_or(key, "(that key)"))
            .with("why", Slot::Word(why))
            .with("fix", Slot::Command(unset(key, layer)))
            .subject(key.to_string())
    }

    /// `key` (embedder.active or a flag) names an instance no `embedders.<name>` defines.
    pub fn instance_unknown(key: &'static str) -> Problem {
        Problem::new("config_instance_unknown")
            .with("key", Slot::Word(key))
            .with(
                "fix",
                Slot::Command(Command::new("config set embedder.active none")),
            )
    }

    /// `next` is `umx config check` when a command refuses to rewrite the file, else the docs.
    pub fn file_invalid(file: &GraphDir, e: &FileError, from_check: bool) -> Problem {
        let fix = if from_check {
            Command::new("docs get config")
        } else {
            Command::new("config check")
        };
        Problem::new("config_file_invalid")
            .with("file", Slot::path(file))
            .with("line", Slot::Count(e.line as u64))
            .with("column", Slot::Count(e.column as u64))
            .with("reason", Slot::Word(e.reason))
            .with("fix", Slot::Command(fix))
            .subject(file.as_str())
    }

    /// A config or secrets file umx will not trust; `reason` says how to fix it.
    pub fn file_refused(file: &GraphDir, reason: &'static str) -> Problem {
        Problem::new("config_file_refused")
            .with("file", Slot::path(file))
            .with("reason", Slot::Word(reason))
            .subject(file.as_str())
    }

    /// Graph verbs while a fail-closed setting is invalid.
    pub fn config_invalid(key: &str) -> Problem {
        Problem::new("config_invalid").with("key", Slot::key_or(key, "(a setting)"))
    }

    pub fn restart_needed(key: &str) -> Problem {
        Problem::new("config_restart_needed").with("key", Slot::key_or(key, "(that key)"))
    }

    pub fn secret_missing(key: &str, env: &str) -> Problem {
        Problem::new("secret_missing")
            .with("key", Slot::key_or(key, "(that key)"))
            .with("env", Slot::key_or(env, "(that name)"))
            .subject(key)
    }

    pub fn secrets_line_invalid(file: &GraphDir, line: u64) -> Problem {
        Problem::new("secrets_line_invalid")
            .with("file", Slot::path(file))
            .with("line", Slot::Count(line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn entries(pairs: &[(&str, ConfigValue)]) -> Entries {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn text(t: &str) -> ConfigValue {
        ConfigValue::Text(t.to_string())
    }

    fn codes(r: &Resolved) -> Vec<&'static str> {
        r.findings.iter().map(|p| p.code).collect()
    }

    #[test]
    fn keys_parse_settings_and_instance_keys_only() {
        assert_eq!(key("roots").unwrap().full(), "roots");
        assert_eq!(
            key("embedders.azure_small.kind").unwrap().full(),
            "embedders.azure_small.kind"
        );
        assert!(
            key("embedders.azure-small.kind").is_none(),
            "snake_case names only"
        );
        assert!(key("embedders.azure_small.colour").is_none());
        assert!(key("embedder.provider").is_none());
        assert_eq!(
            target("embedders.azure_small"),
            Some(Target::Instance("azure_small".into()))
        );
    }

    #[test]
    fn source_roots_are_a_machine_only_table() {
        assert_eq!(key("source_roots.acme"), Some(Key::Root("acme".into())));
        for bad in [
            "source_roots.repo",
            "source_roots.Acme",
            "source_roots.a.b",
            "source_roots.",
        ] {
            assert!(key(bad).is_none(), "{bad}");
        }
        let spec = key("source_roots.acme").unwrap().spec();
        assert!(parse_value(spec, "relative/folder").is_err());
        assert!(parse_value(spec, "/Users/me/transcripts").is_ok());
        let r = resolve(&Inputs {
            machine: entries(&[("source_roots.acme", text("/work/acme"))]),
            repo: Some(entries(&[("source_roots.evil", text("/etc"))])),
            ..Inputs::default()
        });
        assert_eq!(
            r.source_roots(),
            vec![("acme".to_string(), "/work/acme".to_string())]
        );
        assert_eq!(codes(&r), vec!["config_repo_ignored"]);
        let e = entries(&[
            ("source_roots.acme", text("/a")),
            ("embedders.x.kind", text("local")),
            ("roots", ConfigValue::List(vec!["/w".into()])),
        ]);
        let written = String::from_utf8(encode(&e)).unwrap();
        let order: Vec<&str> = written
            .lines()
            .skip(1)
            .map(|l| l.split(' ').next().unwrap())
            .collect();
        assert_eq!(
            order,
            vec!["roots", "embedders.x.kind", "source_roots.acme"]
        );
    }

    #[test]
    fn values_are_typed() {
        let spec = |k: &str| key(k).unwrap().spec();
        assert!(parse_value(spec("embedders.a.endpoint"), "http://x.openai.azure.com").is_err());
        assert!(parse_value(spec("embedders.a.endpoint"), "https://user:pw@x.example").is_err());
        assert!(parse_value(spec("embedders.a.endpoint"), "https://x.openai.azure.com").is_ok());
        assert_eq!(
            parse_value(spec("embedders.a.dims"), "256"),
            Ok(ConfigValue::Int(256))
        );
        assert!(parse_value(spec("embedders.a.dims"), "0").is_err());
        assert_eq!(
            parse_value(spec("embedders.a.auth"), "managed_identity"),
            Err(NOT_YET)
        );
        assert!(parse_value(spec("embedders.a.kind"), "fake").is_err());
        assert!(parse_value(spec("embedders.a.api_key_env"), "AZURE_OPENAI_API_KEY").is_ok());
        assert!(parse_value(spec("embedders.a.api_key_env"), "lower").is_err());
        assert!(parse_value(spec("daemon.idle_unload"), "0").is_ok());
        assert!(parse_value(spec("daemon.idle_unload"), "721h").is_err());
        assert_eq!(
            parse_value(spec("roots"), "/a:/b"),
            Ok(ConfigValue::List(vec!["/a".into(), "/b".into()]))
        );
        assert!(parse_value(spec("roots"), "relative").is_err());
        assert_eq!(
            parse_value(
                spec("embedders.a.deployment"),
                "0123456789abcdef0123456789abcdef"
            ),
            Err(LOOKS_SECRET),
            "an Azure key pasted as a value"
        );
    }

    #[test]
    fn encode_is_deterministic_and_round_trips() {
        let e = entries(&[
            ("embedders.zed.kind", text("local")),
            ("embedder.active", text("azure_small")),
            ("embedders.azure_small.dims", ConfigValue::Int(256)),
            ("embedders.azure_small.kind", text("azure_openai")),
            ("roots", ConfigValue::List(vec!["/a".into(), "/b c".into()])),
            ("zz.unknown", text("kept \"quoted\"")),
        ]);
        let bytes = encode(&e);
        let text = core::str::from_utf8(&bytes).unwrap();
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(
            lines,
            [
                "roots = [\"/a\", \"/b c\"]",
                "embedder.active = \"azure_small\"",
                "embedders.azure_small.kind = \"azure_openai\"",
                "embedders.azure_small.dims = 256",
                "embedders.zed.kind = \"local\"",
                "zz.unknown = 'kept \"quoted\"'",
            ]
        );
        assert_eq!(decode(&bytes).unwrap(), e);
        assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
    }

    #[test]
    fn values_umx_cannot_use_survive_a_rewrite() {
        let file = b"roots = false\nembedders.az.dims = 1.5\nembedders.az.kind = [\"local\", 1]\nwhen = 2026-11-10\n";
        let mut e = decode(file).unwrap();
        assert_eq!(
            e["roots"],
            ConfigValue::Other(Unread {
                kind: "true or false",
                toml: "false".into()
            })
        );
        e.insert("daemon.idle_unload".into(), text("1h"));
        let rewritten = encode(&e);
        let again = decode(&rewritten).unwrap();
        assert_eq!(again, e, "nothing dropped or changed");
        let r = resolve(&Inputs {
            machine: again,
            defaults: entries(&[("roots", ConfigValue::List(vec!["/home".into()]))]),
            ..Inputs::default()
        });
        assert!(
            r.roots().is_empty(),
            "still closed after an unrelated rewrite"
        );
        assert_eq!(
            umx_core_json(&e["roots"]),
            "\"true or false\"",
            "an unread value serialises as its kind only"
        );
    }

    fn umx_core_json(v: &ConfigValue) -> String {
        serde_json::to_string(v).unwrap()
    }

    #[test]
    fn a_fail_closed_key_never_suggests_a_value() {
        let roots = key("roots").unwrap().spec();
        for p in [
            problems::value_invalid("roots", FileLayer::Machine, "x", roots),
            problems::repo_refused("roots", problems::WHY_MACHINE_ONLY, roots),
        ] {
            assert_eq!(p.slot("fix").as_deref(), Some("umx docs get config"));
        }
        let dims = key("embedders.a.dims").unwrap().spec();
        assert_eq!(
            problems::value_invalid("embedders.a.dims", FileLayer::Machine, "x", dims)
                .slot("fix")
                .as_deref(),
            Some("umx config set embedders.a.dims 256")
        );
    }

    #[test]
    fn a_repo_cannot_choose_what_umx_ui_runs() {
        // ws009 s2 (R5): ui.dir and ui.node are machine only, so a repo's config never picks a program to run.
        let r = resolve(&Inputs {
            machine: entries(&[("ui.node", text("/usr/local/bin/node"))]),
            repo: Some(entries(&[
                ("ui.dir", text("/repo/evil")),
                ("ui.node", text("/repo/evil-node")),
            ])),
            ..Inputs::default()
        });
        assert_eq!(r.value("ui.dir"), None);
        assert_eq!(r.value("ui.node"), Some(&text("/usr/local/bin/node")));
        for k in ["ui.dir", "ui.node"] {
            assert!(
                r.findings.iter().any(|p| p.code == "config_repo_ignored"
                    && p.subject.as_deref() == Some(k)
                    && p.slot("why").as_deref() == Some(problems::WHY_MACHINE_ONLY)),
                "{k}: {:?}",
                r.findings
            );
        }
    }

    #[test]
    fn ui_paths_must_be_absolute() {
        let dir = key("ui.dir").unwrap().spec();
        assert!(parse_value(dir, "/Users/me/apps/ui").is_ok());
        for bad in ["apps/ui", "", "/a\nb"] {
            assert!(parse_value(dir, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn hand_written_tables_parse_the_same() {
        let hand = b"[embedders.azure_small]\nkind = \"azure_openai\"\ndims = 256\n\n[embedder]\nactive = \"azure_small\"\n";
        let flat = b"embedder.active = \"azure_small\"\nembedders.azure_small.kind = \"azure_openai\"\nembedders.azure_small.dims = 256\n";
        assert_eq!(decode(hand).unwrap(), decode(flat).unwrap());
    }

    #[test]
    fn a_file_that_does_not_parse_names_line_and_column() {
        let e = decode(b"roots = [\"/a\"]\nembedder.active = \n").unwrap_err();
        assert_eq!((e.line, e.column), (2, 19));
        let e = decode(b"ok = 1\n\xff\n").unwrap_err();
        assert_eq!((e.line, e.reason), (2, "the file is not UTF-8"));
    }

    #[test]
    fn layers_merge_in_order_and_say_where_from() {
        let inputs = Inputs {
            machine: entries(&[
                ("daemon.idle_unload", text("2h")),
                ("roots", ConfigValue::List(vec!["/m".into()])),
            ]),
            env: [("UMX_ROOTS".to_string(), "/e:relative".to_string())].into(),
            flags: entries(&[("daemon.idle_unload", text("5m"))]),
            defaults: entries(&[("roots", ConfigValue::List(vec!["/home".into()]))]),
            ..Inputs::default()
        };
        let r = resolve(&inputs);
        assert_eq!(r.get("roots").unwrap().source, Source::Env);
        assert_eq!(
            r.roots(),
            ["/e"],
            "env roots keep only absolute folders, as before"
        );
        assert_eq!(r.get("daemon.idle_unload").unwrap().source, Source::Flag);
        assert_eq!(r.idle_unload_ms(), 5 * 60_000);
        assert_eq!(r.get("embedder.active").unwrap().source, Source::Default);
        let r = resolve(&Inputs {
            env: BTreeMap::new(),
            flags: Entries::new(),
            ..inputs
        });
        assert_eq!(r.roots(), ["/m"]);
        assert_eq!(r.get("daemon.idle_unload").unwrap().source, Source::Machine);
        assert!(r.findings.is_empty());
    }

    #[test]
    fn invalid_roots_fail_closed_and_other_bad_values_fall_back() {
        let r = resolve(&Inputs {
            machine: entries(&[
                ("roots", ConfigValue::List(vec!["relative".into()])),
                ("daemon.idle_unload", text("forever")),
            ]),
            defaults: entries(&[("roots", ConfigValue::List(vec!["/home".into()]))]),
            ..Inputs::default()
        });
        assert!(r.roots().is_empty(), "never the wider default");
        assert_eq!(r.get("roots").unwrap().source, Source::Invalid);
        assert_eq!(r.closed, ["roots"]);
        assert_eq!(r.get("daemon.idle_unload").unwrap().source, Source::Default);
        assert_eq!(codes(&r), ["config_value_invalid", "config_value_invalid"]);
    }

    fn two_instances() -> Entries {
        entries(&[
            ("embedders.azure_small.kind", text("azure_openai")),
            (
                "embedders.azure_small.endpoint",
                text("https://x.openai.azure.com"),
            ),
            ("embedders.azure_small.deployment", text("emb")),
            (
                "embedders.azure_small.model",
                text("text-embedding-3-small"),
            ),
            ("embedders.azure_small.dims", ConfigValue::Int(256)),
            ("embedders.nomic.kind", text("local")),
            ("embedders.nomic.model", text("nomic-embed-text-v1.5")),
            ("embedders.nomic.dims", ConfigValue::Int(256)),
            ("embedder.active", text("azure_small")),
        ])
    }

    #[test]
    fn floors_are_machine_only_decimals_from_minus_one_to_one() {
        let spec = key("embedders.azure_small.search_floor").unwrap().spec();
        assert_eq!(
            parse_value(spec, "0.25"),
            Ok(ConfigValue::Decimal(Decimal::new(0.25)))
        );
        assert_eq!(
            parse_value(spec, "-1"),
            Ok(ConfigValue::Decimal(Decimal::new(-1.0)))
        );
        for bad in ["1.5", "-1.01", "NaN", "inf", "high"] {
            assert!(parse_value(spec, bad).is_err(), "{bad}");
        }
        // A file's decimal and whole numbers both work, and a rewrite keeps the decimal bit for bit.
        let file = b"embedders.azure_small.search_floor = 0.1234567\nembedders.azure_small.similar_floor = 1\n";
        let e = decode(file).unwrap();
        assert_eq!(decode(&encode(&e)).unwrap(), e);
        let mut machine = two_instances();
        machine.extend(e);
        let r = resolve(&Inputs {
            machine: machine.clone(),
            ..Inputs::default()
        });
        assert!(r.findings.is_empty(), "{:?}", codes(&r));
        assert_eq!(
            r.value("embedders.azure_small.search_floor")
                .and_then(ConfigValue::as_number),
            Some(0.1234567)
        );
        assert_eq!(
            r.value("embedders.azure_small.similar_floor")
                .and_then(ConfigValue::as_number),
            Some(1.0)
        );
        // A repo cannot set either.
        for k in [
            "embedders.azure_small.search_floor",
            "embedders.nomic.similar_floor",
        ] {
            let r = resolve(&Inputs {
                machine: two_instances(),
                repo: Some(entries(&[(k, ConfigValue::Decimal(Decimal::new(0.9)))])),
                ..Inputs::default()
            });
            assert_eq!(codes(&r), ["config_repo_ignored"], "{k}");
            assert_eq!(r.value(k), None, "{k}");
        }
    }

    #[test]
    fn the_repo_layer_only_narrows() {
        let resolve_repo = |repo: Entries| {
            resolve(&Inputs {
                machine: two_instances(),
                repo: Some(repo),
                ..Inputs::default()
            })
        };
        let r = resolve_repo(entries(&[("embedder.active", text("nomic"))]));
        assert_eq!(r.value("embedder.active"), Some(&text("nomic")));
        assert_eq!(r.get("embedder.active").unwrap().source, Source::Repo);
        assert!(r.findings.is_empty());

        let r = resolve_repo(entries(&[("embedder.active", text("none"))]));
        assert_eq!(r.value("embedder.active"), Some(&text("none")));

        // A cloned repo cannot switch to the hosted instance, redirect it, or widen roots.
        for (k, v) in [
            ("embedder.active", text("azure_small")),
            (
                "embedders.azure_small.endpoint",
                text("https://evil.example"),
            ),
            ("roots", ConfigValue::List(vec!["/".into()])),
            ("embedders.nomic.api_key_env", text("STOLEN")),
        ] {
            let r = resolve_repo(entries(&[(k, v)]));
            assert_eq!(codes(&r), ["config_repo_ignored"], "{k}");
            assert_eq!(r.findings[0].subject.as_deref(), Some(k));
            assert_ne!(r.get(k).map(|s| s.source), Some(Source::Repo), "{k}");
        }
        assert_eq!(
            resolve_repo(entries(&[("embedders.nomic.api_key_env", text("X"))])).findings[0]
                .slot("why")
                .as_deref(),
            Some(problems::WHY_SECRET)
        );
    }

    /// ws004 s25 (security): an instance's pace is machine only. One instance serves every graph on the machine,
    /// so a cloned repo can neither slow it for all of them nor drive it past a shared quota.
    #[test]
    fn a_repo_layer_never_sets_an_instances_tokens_per_minute() {
        let key = "embedders.azure_small.tokens_per_minute";
        let mut machine = two_instances();
        machine.insert(key.into(), ConfigValue::Int(2_000_000));
        for repo in [1, 1_000_000_000] {
            let r = resolve(&Inputs {
                machine: machine.clone(),
                repo: Some(entries(&[(key, ConfigValue::Int(repo))])),
                ..Inputs::default()
            });
            assert_eq!(codes(&r), ["config_repo_ignored"], "{repo}");
            assert_eq!(
                r.findings[0].slot("why").as_deref(),
                Some(problems::WHY_MACHINE_ONLY)
            );
            assert_eq!(r.value(key), Some(&ConfigValue::Int(2_000_000)), "{repo}");
            assert_eq!(r.get(key).unwrap().source, Source::Machine);
        }
    }

    #[test]
    fn check_names_missing_keys_and_unknown_instances() {
        let r = resolve(&Inputs {
            machine: entries(&[
                ("embedders.azure_small.kind", text("azure_openai")),
                ("embedders.azure_small.auth", text("api_key")),
                ("embedder.active", text("nope")),
                ("embedders.x.colour", text("red")),
            ]),
            ..Inputs::default()
        });
        let found: Vec<(&str, Option<String>)> =
            r.findings.iter().map(|p| (p.code, p.slot("key"))).collect();
        let k = |s: &str| Some(s.to_string());
        assert_eq!(
            found,
            [
                ("config_key_unknown", k("embedders.x.colour")),
                ("config_instance_unknown", k("embedder.active")),
                ("config_missing", k("embedders.azure_small.model")),
                ("config_missing", k("embedders.azure_small.dims")),
                ("config_missing", k("embedders.azure_small.endpoint")),
                ("config_missing", k("embedders.azure_small.deployment")),
                ("config_missing", k("embedders.azure_small.api_key_env")),
            ]
        );
        assert_eq!(
            r.findings[2].slot("fix").as_deref(),
            Some("umx config set embedders.azure_small.model text-embedding-3-small")
        );
    }

    #[test]
    fn only_the_machine_layer_defines_or_changes_an_instance_so_only_its_instances_are_trusted() {
        // Positive control (D62): an instance defined in the machine config is trusted, with nothing else to do.
        let r = resolve(&Inputs {
            machine: two_instances(),
            ..Inputs::default()
        });
        assert!(trusted(&r, "azure_small") && trusted(&r, "nomic"));
        assert_eq!(
            embedder_for(&r, EmbedderRule::Trusted, None)
                .unwrap()
                .unwrap()
                .name,
            "azure_small"
        );
        // The repo layer, a flag or an env var never defines an instance, nor moves a machine instance's endpoint.
        let rogue = [
            ("embedders.rogue_x.kind", text("azure_openai")),
            (
                "embedders.rogue_x.endpoint",
                text("https://evil.example.com"),
            ),
            ("embedders.rogue_x.deployment", text("emb")),
            ("embedders.rogue_x.model", text("text-embedding-3-small")),
            ("embedders.rogue_x.dims", ConfigValue::Int(256)),
            (
                "embedders.azure_small.endpoint",
                text("https://evil.example.com"),
            ),
        ];
        let repo = resolve(&Inputs {
            machine: two_instances(),
            repo: Some(entries(&rogue)),
            ..Inputs::default()
        });
        let flag = resolve(&Inputs {
            machine: two_instances(),
            flags: entries(&rogue),
            ..Inputs::default()
        });
        let env = resolve(&Inputs {
            machine: two_instances(),
            env: [
                ("UMX_EMBEDDERS_ROGUE_X_KIND", "azure_openai"),
                ("UMX_EMBEDDERS_ROGUE_X_ENDPOINT", "https://evil.example.com"),
                (
                    "UMX_EMBEDDERS_AZURE_SMALL_ENDPOINT",
                    "https://evil.example.com",
                ),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into(),
            ..Inputs::default()
        });
        for (layer, r) in [("repo", &repo), ("flag", &flag), ("env", &env)] {
            assert!(!trusted(r, "rogue_x"), "{layer}");
            assert!(
                matches!(
                    embedder_for(r, EmbedderRule::Trusted, Some("rogue_x")),
                    Err(EmbedderRefused::Unknown(_))
                ),
                "{layer}"
            );
            assert_eq!(
                r.value("embedders.azure_small.endpoint"),
                Some(&text("https://x.openai.azure.com")),
                "{layer}"
            );
        }
    }

    #[test]
    fn a_leftover_trusted_endpoint_warns_deprecated_and_changes_nothing() {
        let mut machine = two_instances();
        machine.insert(
            "embedders.azure_small.trusted_endpoint".into(),
            text("https://elsewhere.openai.azure.com"),
        );
        let r = resolve(&Inputs {
            machine,
            ..Inputs::default()
        });
        let found: Vec<_> = r
            .findings
            .iter()
            .filter(|p| p.slot("key").as_deref() == Some("embedders.azure_small.trusted_endpoint"))
            .map(|p| p.code)
            .collect();
        assert_eq!(found, ["config_key_deprecated"]);
        assert!(trusted(&r, "azure_small"));
        assert!(key("embedders.azure_small.trusted_endpoint").is_none());
    }

    #[test]
    fn a_hosted_instance_that_sends_somewhere_new_is_a_route_change_and_a_local_one_never_is() {
        let at = |machine: Entries| {
            resolve(&Inputs {
                machine,
                ..Inputs::default()
            })
        };
        let base = at(two_instances());
        assert!(
            route_changes(&base, &at(two_instances())).is_empty(),
            "no change"
        );
        let mut moved = two_instances();
        moved.insert(
            "embedders.azure_small.endpoint".into(),
            text("https://y.openai.azure.com"),
        );
        assert_eq!(route_changes(&base, &at(moved)), ["azure_small"]);
        let mut redeployed = two_instances();
        redeployed.insert("embedders.azure_small.deployment".into(), text("other"));
        assert_eq!(route_changes(&base, &at(redeployed)), ["azure_small"]);
        // A partial unset leaves the name but no buildable instance: still a change (R202-2).
        for leaf in ["kind", "model", "dims", "endpoint"] {
            let mut partial = two_instances();
            partial.remove(&alloc::format!("embedders.azure_small.{leaf}"));
            assert_eq!(
                route_changes(&base, &at(partial)),
                ["azure_small"],
                "{leaf}"
            );
        }
        let mut added = two_instances();
        for (k, v) in [
            ("embedders.az2.kind", text("azure_openai")),
            ("embedders.az2.endpoint", text("https://z.openai.azure.com")),
            ("embedders.az2.deployment", text("emb")),
            ("embedders.az2.model", text("text-embedding-3-small")),
            ("embedders.az2.dims", ConfigValue::Int(256)),
        ] {
            added.insert(k.into(), v);
        }
        assert_eq!(route_changes(&base, &at(added.clone())), ["az2"]);
        assert_eq!(route_changes(&at(added), &base), ["az2"], "removed");
        let mut local = two_instances();
        local.insert("embedders.nomic.dims".into(), ConfigValue::Int(512));
        assert!(
            route_changes(&base, &at(local)).is_empty(),
            "a local instance sends nothing anywhere"
        );
    }

    #[test]
    fn a_secured_graph_embeds_only_locally_whatever_asks() {
        let r = resolve(&Inputs {
            machine: two_instances(),
            ..Inputs::default()
        });
        let open = embedder_for(&r, EmbedderRule::Any, None).unwrap().unwrap();
        assert_eq!(open.name, "azure_small");
        let not_local = |key| EmbedderRefused::NotAdmitted {
            key,
            instance: "azure_small".into(),
        };
        assert_eq!(
            embedder_for(&r, EmbedderRule::Local, None),
            Err(not_local("embedder.active"))
        );
        assert_eq!(
            embedder_for(&r, EmbedderRule::Local, Some("azure_small")),
            Err(not_local("--embedder"))
        );
        let local = embedder_for(&r, EmbedderRule::Local, Some("nomic")).unwrap();
        assert_eq!(local.unwrap().kind, "local");
        assert!(matches!(
            embedder_for(&r, EmbedderRule::Local, Some("ghost")),
            Err(EmbedderRefused::Unknown(p)) if p.code == "config_instance_unknown"
        ));
    }

    #[test]
    fn secret_shapes() {
        assert!(looks_like_secret("sk-abc"));
        assert!(looks_like_secret("0123456789abcdef0123456789abcdef"));
        assert!(!looks_like_secret("text-embedding-3-small"));
        assert!(!looks_like_secret("https://my-resource.openai.azure.com"));
        assert!(secret_like_key("embedders.a.api_key_env"));
        assert!(!secret_like_key("embedder.active"));
        assert!(!secret_like_key("embedders.a.tokens_per_minute"));
        assert!(secret_like_key("embedders.a.token"));
    }

    #[test]
    fn every_code_returned_is_in_codes() {
        let src = include_str!("config.rs");
        let used: alloc::collections::BTreeSet<&str> = src
            .split("Problem::new(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        let listed: alloc::collections::BTreeSet<&str> = problems::CODES.iter().copied().collect();
        assert_eq!(used, listed);
    }
}
