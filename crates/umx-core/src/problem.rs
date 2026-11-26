//! Warnings and errors as values (workshop 007 section 2). A problem is a catalogue code plus named slots; the
//! daemon renders `message` and `next` from umx-protocol's catalogue templates. Every slot value is one of the safe
//! kinds below, each built only through a checking constructor, so graph text cannot reach `message` or `next`
//! (req-41). Graph text goes only in `candidates`, which the envelope marks untrusted.

use crate::codec::{DecodeReason, GraphFile};
use crate::id::{ID_LEN, Id, normalise_id_text};
use crate::name::{is_snake_case, is_valid_name};
use crate::ports::{FilesError, GraphDir};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use serde::Serialize;

/// A name that passed workshop 007 rule 2 (snake_case, not a reserved word). Only `ValidName::new` builds one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidName(String);

impl ValidName {
    pub fn new(name: &str) -> Option<ValidName> {
        is_valid_name(name).then(|| ValidName(name.to_string()))
    }

    /// A graph alias: snake_case, reserved words allowed (aliases are not Cypher names).
    pub fn alias(alias: &str) -> Option<ValidName> {
        is_snake_case(alias).then(|| ValidName(alias.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Id text an agent gave (a full id or a prefix): Crockford characters only, upper case, at most 26.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdText(String);

impl IdText {
    pub fn new(text: &str) -> Option<IdText> {
        let t = normalise_id_text(text.trim())?;
        (!t.is_empty() && t.len() <= ID_LEN).then_some(IdText(t))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Text safe to show in a message or next: a path or OS error made only of `[A-Za-z0-9._/+~:-]` and single
/// spaces, at most 256 characters. Folder names are agent-chosen and may hold anything, so they pass through here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeText(String);

const SAFE_PUNCT: &str = "._/+~:-";

impl SafeText {
    /// The path as is if every character is safe, including ASCII spaces, else None.
    pub fn path(dir: &GraphDir) -> Option<SafeText> {
        let p = dir.as_str();
        (p.len() <= 256
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ' ' || SAFE_PUNCT.contains(c)))
        .then(|| SafeText(p.to_string()))
    }

    /// A config key or env var name (`embedders.azure_small.kind`, `AZURE_OPENAI_API_KEY`): letters, digits, `_`
    /// and `.` only, at most 128 characters, else None.
    pub fn key(key: &str) -> Option<SafeText> {
        (!key.is_empty()
            && key.len() <= 128
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .then(|| SafeText(key.to_string()))
    }

    /// A git repo URL from an address's `?repo=` hint, safe to put after `git clone` in a `next`: `https://` then
    /// `[A-Za-z0-9._/~:+-]` starting with a letter or digit, or `git@<host>:<path>` with host `[A-Za-z0-9.-]` and
    /// path `[A-Za-z0-9._/~+-]`, neither starting with `-`. At most 256 characters, no spaces, so a hint can never
    /// add a git option or a second command. Anything else is None.
    pub fn repo_url(url: &str) -> Option<SafeText> {
        repo_url_ok(url).then(|| SafeText(url.to_string()))
    }

    /// Error text with every unsafe character replaced by `?`, truncated to 256 characters.
    pub fn lossy(text: &str) -> SafeText {
        SafeText(
            text.chars()
                .take(256)
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == ' ' || SAFE_PUNCT.contains(c) {
                        c
                    } else {
                        '?'
                    }
                })
                .collect(),
        )
    }

    /// Text umx built from its own words and checked parts (a batch ref's reason, plan 205): [`SafeText::lossy`]'s
    /// rule, keeping `_` as well.
    pub fn words(text: &str) -> SafeText {
        Self::prose(text, false)
    }

    /// Generated rule prose keeps its own commas; graph text still uses the ordinary sanitizer.
    pub(crate) fn rule_sentence(text: &str) -> SafeText {
        Self::prose(text, true)
    }

    fn prose(text: &str, commas: bool) -> SafeText {
        SafeText(
            text.chars()
                .take(256)
                .map(|c| {
                    if c.is_ascii_alphanumeric()
                        || c == ' '
                        || c == '_'
                        || SAFE_PUNCT.contains(c)
                        || (commas && c == ',')
                    {
                        c
                    } else {
                        '?'
                    }
                })
                .collect(),
        )
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The rule behind [`SafeText::repo_url`]; umx-protocol's `Fill::repo_url` applies the same rule again.
pub fn repo_url_ok(url: &str) -> bool {
    let run = |s: &str, punct: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || punct.contains(c))
    };
    if url.len() > 256 {
        return false;
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return rest.starts_with(|c: char| c.is_ascii_alphanumeric()) && run(rest, "._/~:+-");
    }
    match url.strip_prefix("git@").and_then(|r| r.split_once(':')) {
        Some((host, path)) => run(host, ".-") && run(path, "._/~+-"),
        None => false,
    }
}

/// An embedder endpoint's host, `host[:port]` only (D83, security W2): never its scheme, user info, path or query,
/// where a credential or an `api-key=` may sit. Built only by [`Host::from_endpoint`], which shows nothing rather than
/// guess when the URL is not plain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host(String);

impl Host {
    /// The host and port of `url`: the scheme, `user:pass@` and everything from the first `/`, `?` or `#` dropped;
    /// the host `[A-Za-z0-9.-]` not starting with `.` or `-`, the port 1 to 5 digits. None when an `@` sits after
    /// the host (user info holding a `/`) or anything else is not plain, so no part of a credential is ever shown.
    pub fn from_endpoint(url: &str) -> Option<Host> {
        let rest = url.split_once("://").map_or(url, |(_, r)| r);
        let before_query = rest.split(['?', '#']).next().unwrap_or("");
        let authority = before_query.split('/').next().unwrap_or("");
        if before_query[authority.len()..].contains('@') {
            return None;
        }
        let shown = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host, port) = match shown.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (shown, None),
        };
        let host_ok = !host.is_empty()
            && host.len() <= 253
            && !host.starts_with(['.', '-'])
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        let port_ok = port
            .is_none_or(|p| (1..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()));
        (host_ok && port_ok).then(|| Host(shown.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where an embedder instance sends a graph's text: kept on this machine, or to a hosted endpoint (its host when
/// one could be shown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    Local,
    Hosted(Option<Host>),
}

/// An embedder instance that may now get a graph's text: its name (config instance names are snake_case) and reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    pub name: ValidName,
    pub reach: Reach,
}

impl fmt::Display for Receiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reach {
            Reach::Local => write!(f, "{} (local)", self.name.as_str()),
            Reach::Hosted(Some(h)) => {
                write!(f, "{} (hosted at {})", self.name.as_str(), h.as_str())
            }
            Reach::Hosted(None) => write!(
                f,
                "{} (hosted at an endpoint not shown)",
                self.name.as_str()
            ),
        }
    }
}

/// A runnable `umx ...` command for a `next`, built from umx's own words (verb path, flags, placeholders such as
/// `<title>`) and checked values only. Never a typed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command(String);

impl Command {
    /// The longest command a `next` keeps as written: umx-protocol's `Fill::COMMAND_MAX`, which umx-core cannot
    /// depend on (umx-daemon's test holds the two equal). A longer one would reach the caller cleaned.
    pub const MAX: usize = 300;

    /// `path` is the verb path, for example `"node add"`.
    pub fn new(path: &'static str) -> Command {
        Command(format!("umx {path}"))
    }

    fn push(mut self, part: &str) -> Command {
        self.0.push(' ');
        self.0.push_str(part);
        self
    }

    /// A flag or placeholder: `--type`, `<title>`.
    pub fn word(self, word: &'static str) -> Command {
        self.push(word)
    }

    pub fn name(self, name: &ValidName) -> Command {
        self.push(name.as_str())
    }

    /// `<name><sep><placeholder>`, for example `likelihood=<value>` or `size:<kind>`.
    pub fn name_with(self, name: &ValidName, sep: char, placeholder: &'static str) -> Command {
        self.push(&format!("{}{sep}{placeholder}", name.as_str()))
    }

    /// `<prefix><name>`, for example `date_field:due_on`: a checked name after a fixed prefix.
    pub fn qualified(self, prefix: &'static str, name: &ValidName) -> Command {
        self.push(&format!("{prefix}{}", name.as_str()))
    }

    /// `<kind>:<name>=<text>`, or `field:<type>.<field>=<text>`: what `graph set --description` takes, with the text
    /// a placeholder.
    pub fn described(
        self,
        kind: crate::ontology::Described,
        name: &ValidName,
        field: Option<&ValidName>,
    ) -> Command {
        let name = match field {
            Some(f) => format!("{}.{}", name.as_str(), f.as_str()),
            None => name.as_str().to_string(),
        };
        self.push(&format!("{}:{name}=<text>", kind.as_str()))
    }

    /// Checked names joined by `sep`, for example `id,title` after `--columns`.
    pub fn names(self, names: &[ValidName], sep: &'static str) -> Command {
        let joined: Vec<&str> = names.iter().map(ValidName::as_str).collect();
        self.push(&joined.join(sep))
    }

    /// A field's ordered label list, never arbitrary stored text.
    pub fn field_values(self, values: &[String]) -> Command {
        if values.is_empty() {
            return self.word("--clear-values");
        }
        let next = self.word("--values");
        if !crate::ontology::valid_field_values(values) {
            return next.word("<values>");
        }
        let list = values.join(",");
        if next.as_str().len() + 1 + list.len() > Self::MAX {
            return next.word("<values>");
        }
        next.push(&list)
    }

    /// Std tags (or `<facet>:*`) in stored form, joined by commas (ws008 t2b): None when there are none or one is not
    /// in stored form, so a typed tag never reaches a command. A list holding `*` is single-quoted, so a shell never
    /// expands it.
    pub fn tags(self, tags: &[String]) -> Option<Command> {
        let stored = |t: &String| crate::tags::Pattern::parse(t).is_ok_and(|p| p.text() == *t);
        if tags.is_empty() || tags.len() > 64 || !tags.iter().all(stored) {
            return None;
        }
        let list = tags.join(",");
        Some(match list.contains('*') {
            true => self.push(&format!("'{list}'")),
            false => self.push(&list),
        })
    }

    /// A number umx parsed, such as a `--limit`.
    pub fn count(self, n: u64) -> Command {
        self.push(&n.to_string())
    }

    pub fn id(self, id: Id) -> Command {
        self.push(&id.to_text())
    }

    /// `umx:<alias>/<id>`.
    pub fn address(self, alias: &ValidName, id: Id) -> Command {
        self.push(&format!("umx:{}/{}", alias.as_str(), id))
    }

    pub fn id_text(self, id: &IdText) -> Command {
        self.push(id.as_str())
    }

    /// A config key ([`SafeText::key`]), else the placeholder `<key>`.
    pub fn key(self, key: &str) -> Command {
        match SafeText::key(key) {
            Some(k) => self.push(k.as_str()),
            None => self.push("<key>"),
        }
    }

    /// A model name as a model key takes it (`ModelKey` rules), else the placeholder `<model>`.
    pub fn model(self, model: &str) -> Command {
        match crate::ports::ModelKey::parse(&format!("{model}@1")) {
            Ok(k) => self.push(k.model()),
            Err(_) => self.push("<model>"),
        }
    }

    /// A checked model key, `model@dims[+pN]`.
    pub fn model_key(self, key: &crate::ports::ModelKey) -> Command {
        self.push(&format!("{key}"))
    }

    /// The path if it is safe text, else the placeholder `<path>`.
    pub fn path(self, dir: &GraphDir) -> Command {
        match SafeText::path(dir) {
            Some(p) if p.as_str().contains(' ') => self.push(&format!("'{}'", p.as_str())),
            Some(p) => self.push(p.as_str()),
            None => self.push("<path>"),
        }
    }

    /// A second command, run when this one succeeds: `&& umx <path>`; `path` is its verb path, as in [`Self::new`].
    pub fn then(self, path: &'static str) -> Command {
        self.push(&format!("&& umx {path}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One slot value. Each variant holds a checked type, so building a slot from graph text does not compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    Id(Id),
    IdText(IdText),
    Name(ValidName),
    /// Checked field names, shown separated by spaces.
    Names(Vec<ValidName>),
    Path(SafeText),
    Count(u64),
    /// umx's own words.
    Word(&'static str),
    /// A list of umx's own words, shown joined by ` or `.
    Words(Vec<&'static str>),
    /// A fixed rule sentence built from checked rule parts, never its description.
    Sentence(SafeText),
    Command(Command),
    /// An OS or adapter error, made safe.
    Os(SafeText),
    /// A config key or env var name ([`SafeText::key`]).
    Key(SafeText),
    /// Why a graph file does not load: the codec's own words and line numbers.
    Decode(DecodeReason),
    /// A parsed embedding model key, `model@dims[+pN]`.
    ModelKey(crate::ports::ModelKey),
    /// A graph file or vector shard, as its path relative to the graph folder (only checked parts).
    File(crate::ports::FileRef),
    /// An address's `?repo=` hint for graph_not_loaded's `<hint>`: Some when it passed [`SafeText::repo_url`]. The
    /// daemon renders it through umx-protocol's `Fill::repo_hint`, which checks it again.
    RepoHint(Option<SafeText>),
    /// Embedder instances and where each sends text (D83), shown joined by ` and `; a host only as [`Host`].
    Receivers(Vec<Receiver>),
}

impl Slot {
    /// The name if it passes rule 2 (or is a valid alias), else `fallback`.
    pub fn name_or(name: &str, fallback: &'static str) -> Slot {
        ValidName::alias(name).map_or(Slot::Word(fallback), Slot::Name)
    }

    pub fn id_text_or(text: &str, fallback: &'static str) -> Slot {
        IdText::new(text).map_or(Slot::Word(fallback), Slot::IdText)
    }

    pub fn file(file: GraphFile) -> Slot {
        Slot::Word(file.file_name())
    }

    pub fn os(e: &FilesError) -> Slot {
        Slot::Os(SafeText::lossy(&e.to_string()))
    }

    /// The key if it is safe text, else `fallback`.
    pub fn key_or(key: &str, fallback: &'static str) -> Slot {
        SafeText::key(key).map_or(Slot::Word(fallback), Slot::Key)
    }

    /// The path if it is safe text, else the word `<path>`.
    pub fn path(dir: &GraphDir) -> Slot {
        SafeText::path(dir).map_or(Slot::Word("<path>"), Slot::Path)
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Slot::Id(id) => write!(f, "{id}"),
            Slot::IdText(t) => f.write_str(t.as_str()),
            Slot::Name(n) => f.write_str(n.as_str()),
            Slot::Names(names) => {
                let names: Vec<_> = names.iter().map(ValidName::as_str).collect();
                f.write_str(&names.join(" "))
            }
            Slot::Path(p) => f.write_str(p.as_str()),
            Slot::Count(n) => write!(f, "{n}"),
            Slot::Word(w) => f.write_str(w),
            Slot::Words(ws) => f.write_str(&ws.join(" or ")),
            Slot::Command(c) => f.write_str(c.as_str()),
            Slot::Os(e) | Slot::Key(e) | Slot::Sentence(e) => f.write_str(e.as_str()),
            Slot::RepoHint(Some(url)) => write!(f, " Its address hints repo {}.", url.as_str()),
            Slot::RepoHint(None) => f.write_str(" Its address has a repo hint that is not shown."),
            Slot::Decode(r) => write!(f, "{r}"),
            Slot::ModelKey(k) => write!(f, "{k}"),
            Slot::File(file) => write!(f, "{file}"),
            Slot::Receivers(rs) => {
                for (i, r) in rs.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" and ")?;
                    }
                    write!(f, "{r}")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Candidate {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleContext {
    pub name: String,
    pub count: u64,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub code: &'static str,
    /// Slot name (as in the catalogue template, without `<>`) to value.
    pub slots: Vec<(&'static str, Slot)>,
    /// An id, address or path umx chose.
    pub subject: Option<String>,
    /// May hold graph text; untrusted in the envelope.
    pub candidates: Vec<Candidate>,
    /// The line of a `umx batch` file the problem is about (req-88); None for every other command.
    pub line: Option<u32>,
    pub rule: Option<Box<RuleContext>>,
}

impl Problem {
    pub fn new(code: &'static str) -> Problem {
        Problem {
            code,
            slots: Vec::new(),
            subject: None,
            candidates: Vec::new(),
            line: None,
            rule: None,
        }
    }

    pub fn with(mut self, slot: &'static str, value: Slot) -> Problem {
        self.slots.push((slot, value));
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Problem {
        self.subject = Some(subject.into());
        self
    }

    /// The batch file line this problem is about.
    pub fn at_line(mut self, line: u32) -> Problem {
        self.line = Some(line);
        self
    }

    pub fn candidates(mut self, candidates: Vec<Candidate>) -> Problem {
        self.candidates = candidates;
        self
    }

    /// A slot's value as text.
    pub fn slot(&self, name: &str) -> Option<String> {
        self.slots
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_only_host_and_port_and_never_a_credential() {
        let shown = |u: &str| Host::from_endpoint(u).map(|h| h.0);
        assert_eq!(
            shown("https://u:secret@h.example/p?api-key=x").as_deref(),
            Some("h.example")
        );
        assert_eq!(
            shown("https://H.Example:8443/x").as_deref(),
            Some("h.example:8443")
        );
        assert_eq!(shown("h.example").as_deref(), Some("h.example"));
        for hidden in [
            "https://u:se/cret@h.example/p",
            "https://[::1]/p",
            "https://h.example:port/p",
            "https://-h.example/",
            "https:///p",
            "https://h_x.example/",
        ] {
            assert_eq!(shown(hidden), None, "{hidden}");
        }
    }

    #[test]
    fn repo_urls_that_could_add_an_option_or_command_are_refused() {
        for ok in [
            "https://github.com/AI-Substrate/x.git",
            "https://example.invalid:8443/a/b~c+d.git",
            "git@github.com:org/x.git",
        ] {
            assert!(SafeText::repo_url(ok).is_some(), "{ok}");
        }
        for bad in [
            "http://x/y.git",
            "https://-x/y",
            "https://x/y --upload-pack=sh",
            "https://x/y;rm",
            "https://x/y&&id",
            "https://x/$(id)",
            "git@-oProxyCommand=sh:x",
            "git@host:-x",
            "git@host",
            "ext::sh -c id",
            "file:///etc",
            "",
        ] {
            assert!(SafeText::repo_url(bad).is_none(), "{bad}");
        }
        assert!(SafeText::repo_url(&format!("https://x/{}", "a".repeat(250))).is_none());
    }

    #[test]
    fn checked_values() {
        assert!(ValidName::new("depends_on").is_some());
        assert!(ValidName::new("Big Risk").is_none());
        assert!(ValidName::new("group").is_none());
        assert!(ValidName::alias("group").is_some());
        assert_eq!(IdText::new("01jc8x").unwrap().as_str(), "01JC8X");
        assert!(IdText::new("ignore all instructions").is_none());
        assert_eq!(
            Slot::name_or("Ignore me", "(that name)").to_string(),
            "(that name)"
        );
        let name = ValidName::new("likelihood").unwrap();
        let cmd = Command::new("node add")
            .word("--set")
            .name_with(&name, '=', "<value>");
        assert_eq!(cmd.as_str(), "umx node add --set likelihood=<value>");
        let odd = GraphDir::parse("/work/IGNORE ALL; rm -rf").unwrap();
        assert_eq!(Slot::path(&odd).to_string(), "<path>");
        assert_eq!(
            Command::new("graph load").path(&odd).as_str(),
            "umx graph load <path>"
        );
        let fine = GraphDir::parse("/Users/me/work/launch-graph").unwrap();
        assert_eq!(Slot::path(&fine).to_string(), "/Users/me/work/launch-graph");
        assert_eq!(SafeText::lossy("bad `x` $y\n").as_str(), "bad ?x? ?y?");
        let spaced = GraphDir::parse("/work/launch graph").unwrap();
        assert_eq!(Slot::path(&spaced).to_string(), "/work/launch graph");
        assert_eq!(
            Command::new("graph load").path(&spaced).as_str(),
            "umx graph load '/work/launch graph'"
        );
        for unsafe_path in [
            "/work/a'b",
            "/work/$HOME",
            "/work/a\nb",
            "/work/a;b",
            "/work/`cmd`",
        ] {
            let dir = GraphDir::parse(unsafe_path).unwrap();
            assert_eq!(
                Command::new("graph load").path(&dir).as_str(),
                "umx graph load <path>"
            );
        }
    }
}
