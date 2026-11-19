//! Embedder port: turns a batch of texts into L2-normalised vectors for one named model and width (blueprint 001,
//! c-port-embedder). Synchronous like every port: a network adapter blocks inside its own call.
//!
//! The two required declarations, `concurrency_ceiling` and `max_input_tokens`, follow Flowspace's embedder port
//! (workshop 009 section 5).

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// What a text is for. Some models (local ones especially) embed documents and queries with different prefixes,
/// so the same words give different vectors depending on the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextKind {
    /// Text stored in the graph (a field or a whole node).
    Document,
    /// Text a user searches with.
    Query,
}

/// Where an adapter sends the texts it embeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Locality {
    /// Texts never leave this machine (a local model, or the fake).
    Local,
    /// Texts go to a provider over the network.
    Hosted,
}

/// The identity of a vector space: model, width and, for models that add a prompt prefix, the prompt version.
/// Displays as `model@dims` or `model@dims+pN`, and that text names folders (`embeddings/<key>/`), so the model
/// is a slug: `[a-z0-9._-]`, starting with a letter or digit, at most [`ModelKey::MAX_MODEL_LEN`] bytes. Keyed by model, never by deployment or endpoint: two
/// deployments of one model give comparable vectors, two models never do.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelKey {
    model: String,
    dims: usize,
    prompt_version: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelKeyError {
    /// No `@`, or the model is empty, longer than `MAX_MODEL_LEN`, has a character outside `[a-z0-9._-]` or starts
    /// with `.`, `_` or `-`.
    BadModel,
    /// The width is missing, not a number, zero, or above `MAX_DIMS`.
    BadDims,
    /// Text after `+` is not `p<number>`.
    BadPromptVersion,
}

impl fmt::Display for ModelKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ModelKeyError::BadModel => {
                "the model must be lower-case letters, digits, '.', '_' or '-', starting with a letter or digit"
            }
            ModelKeyError::BadDims => "dims must be a whole number from 1 to 8192",
            ModelKeyError::BadPromptVersion => "the prompt version must be written p<number>",
        })
    }
}

impl ModelKey {
    /// Longest model name. umx-protocol's `Fill::model_key` accepts exactly the keys `parse` does.
    pub const MAX_MODEL_LEN: usize = 128;

    /// Widest vector a key may name (every current model fits; 3,072 is the widest hosted one). graph.json arrives by
    /// `git pull` and sizes the in-memory blocks, so a key above this is refused as invalid data, never allocated
    /// (security). umx-protocol's `Fill::model_key` keeps the same bound.
    pub const MAX_DIMS: usize = 8192;

    pub fn new(model: &str, dims: usize) -> Result<ModelKey, ModelKeyError> {
        let first_ok = model
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let all_ok = model.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
        if !first_ok || !all_ok || model.len() > ModelKey::MAX_MODEL_LEN {
            return Err(ModelKeyError::BadModel);
        }
        if dims == 0 || dims > ModelKey::MAX_DIMS {
            return Err(ModelKeyError::BadDims);
        }
        Ok(ModelKey {
            model: model.to_string(),
            dims,
            prompt_version: None,
        })
    }

    /// Set when the adapter adds a prompt prefix; bump it when the prefix changes, because that changes the space.
    pub fn with_prompt_version(mut self, version: u32) -> ModelKey {
        self.prompt_version = Some(version);
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn dims(&self) -> usize {
        self.dims
    }

    pub fn prompt_version(&self) -> Option<u32> {
        self.prompt_version
    }

    /// Parses what `Display` writes: `model@dims` or `model@dims+pN`.
    pub fn parse(text: &str) -> Result<ModelKey, ModelKeyError> {
        let (model, rest) = text.split_once('@').ok_or(ModelKeyError::BadModel)?;
        let (dims, version) = match rest.split_once('+') {
            Some((dims, version)) => (dims, Some(version)),
            None => (rest, None),
        };
        let dims = parse_digits(dims).ok_or(ModelKeyError::BadDims)?;
        let key = ModelKey::new(model, dims)?;
        match version {
            Some(v) => Ok(key.with_prompt_version(
                v.strip_prefix('p')
                    .and_then(parse_digits)
                    .ok_or(ModelKeyError::BadPromptVersion)?,
            )),
            None => Ok(key),
        }
    }
}

/// Stored as its text, `model@dims[+pN]`; text that does not parse is refused.
impl serde::Serialize for ModelKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for ModelKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<ModelKey, D::Error> {
        let text = String::deserialize(d)?;
        ModelKey::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl core::str::FromStr for ModelKey {
    type Err = ModelKeyError;

    fn from_str(text: &str) -> Result<ModelKey, ModelKeyError> {
        ModelKey::parse(text)
    }
}

/// Plain ASCII digits only (no sign, no spaces), so a parsed key prints back to the same text.
fn parse_digits<T: core::str::FromStr>(text: &str) -> Option<T> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for ModelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.model, self.dims)?;
        if let Some(v) = self.prompt_version {
            write!(f, "+p{v}")?;
        }
        Ok(())
    }
}

/// Why a batch was not embedded. No variant carries input text, keys or tokens: messages are safe to log and show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbedError {
    /// The provider cannot be reached or refused our credentials (offline, auth, model not downloaded). `next` is
    /// a hint for the user, such as the command that fixes it.
    Unavailable { reason: String, next: String },
    /// The input at `index` is empty or only whitespace. Such inputs have no meaning to embed, and some providers
    /// fail the whole batch on them, so adapters refuse them before calling out.
    EmptyInput { index: usize },
    /// The input at `index` is longer than `max_input_tokens` and this adapter refuses rather than truncates.
    InputTooLong { index: usize },
    /// The provider answered with something unusable: wrong count, wrong width, not a number, unknown model.
    BadResponse(String),
    /// The provider asked us to slow down. `retry_after_ms` is its hint when it gave one.
    RateLimited { retry_after_ms: Option<u64> },
    /// The adapter's configuration is wrong (missing model, bad endpoint, dims the model cannot give).
    Config(String),
}

impl fmt::Display for EmbedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbedError::Unavailable { reason, next } => {
                write!(f, "embedder unavailable: {reason} (next: {next})")
            }
            EmbedError::EmptyInput { index } => {
                write!(f, "input {index} is empty or only whitespace")
            }
            EmbedError::InputTooLong { index } => {
                write!(f, "input {index} is longer than the embedder accepts")
            }
            EmbedError::BadResponse(m) => write!(f, "embedder gave a bad response: {m}"),
            EmbedError::RateLimited {
                retry_after_ms: Some(ms),
            } => write!(f, "embedder rate limited; retry after {ms} ms"),
            EmbedError::RateLimited {
                retry_after_ms: None,
            } => f.write_str("embedder rate limited"),
            EmbedError::Config(m) => write!(f, "embedder configuration: {m}"),
        }
    }
}

/// How an adapter signed in: `kind` is umx's own word (`az`, `azd`, `api_key`); `from` is the program as run
/// (an absolute path) or the env var NAME the key was read from. Never a token or key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub kind: &'static str,
    pub from: String,
}

/// The result of one `embed` call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Embedded {
    /// One per input, in input order, each `key().dims()` wide and L2-normalised.
    pub vectors: Vec<Vec<f32>>,
    /// Indexes of inputs the provider cut to `max_input_tokens` before embedding, ascending. Their vectors cover
    /// only the start of the text, so callers record it.
    pub truncated: Vec<usize>,
    /// The tokens the provider says it counted for the call, when it says (Azure's `usage.prompt_tokens`): pacing
    /// corrects its estimate by it (ws004 s25).
    pub tokens: Option<u64>,
}

pub trait Embedder: Send + Sync {
    /// One vector per input, in input order, each `key().dims()` wide and L2-normalised (length 1 within float
    /// error), plus the inputs that were truncated. An empty batch gives an empty result and must not call the
    /// provider. An empty or whitespace-only input fails the batch with `EmptyInput`. Fails as a whole: no partial
    /// results.
    fn embed(&self, texts: &[String], kind: TextKind) -> Result<Embedded, EmbedError>;

    /// The vector space this adapter writes into. Stored next to every vector; vectors with different keys are
    /// never compared.
    fn key(&self) -> ModelKey;

    /// How many `embed` calls callers may run at once. Required, with no default, because any default is wrong
    /// both ways: too high and a hosted provider rate-limits or a local model thrashes the CPU; too low and a
    /// large graph embeds needlessly slowly. Each adapter knows its own limit (hosted: provider quota; local: 1).
    fn concurrency_ceiling(&self) -> usize;

    /// The most inputs one `embed` call takes; a caller never sends more (ws004). Past it a provider refuses the call
    /// (Azure: 2,048) or a local model runs out of room in one run. The default, 16, is the size every call had
    /// before ws004: safe for any adapter, and slow for a hosted one, so an adapter that takes more says so.
    fn max_batch_inputs(&self) -> usize {
        16
    }

    /// The model's real input window, in its own tokens. Longer inputs are either refused (`InputTooLong`) or cut
    /// and listed in `Embedded::truncated`, whichever the adapter does; never cut silently. Required, with no
    /// default, because a guessed cap either refuses text the model takes or hides text it cuts.
    fn max_input_tokens(&self) -> usize;

    /// Whether texts leave the machine. Required, with no default, because a secured graph may only use a local
    /// embedder: defaulting to `Local` would let a hosted adapter that forgot to say so receive secured text, and
    /// defaulting to `Hosted` would hide a real local adapter. Each adapter states what it does.
    fn locality(&self) -> Locality;

    /// Whether `embed` could run now, checked without sending any text: a hosted adapter gets its credentials (an
    /// az token), a local one checks its model files. Returns within a bounded time. `umx doctor` calls it.
    /// Required, with no default, because a default `Ok` would report a logged-out or missing embedder as ready.
    fn ready(&self) -> Result<(), EmbedError>;

    /// The credential the last successful `ready()` or `embed` used, or `None` before one worked and for adapters
    /// that need none (local, fake). Names the kind and where it came from (a program path, an env var NAME), never
    /// a token or key. Required, with no default, so a hosted adapter cannot forget to say how it signs in.
    fn credential(&self) -> Option<Credential>;

    /// This adapter's id in the adapter matrix (blueprint d-6), such as `fake.embedder`. describe() and
    /// `umx daemon status` read it from the wired object (E-1).
    fn adapter_id(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn key_displays_and_parses_back() {
        for text in [
            "text-embedding-3-small@256",
            "nomic-embed-text-v1.5@256+p1",
            "a_b@8",
        ] {
            let key: ModelKey = text.parse().unwrap();
            assert_eq!(format!("{key}"), text);
        }
        let key = ModelKey::parse("m@4+p2").unwrap();
        assert_eq!(key, ModelKey::new("m", 4).unwrap().with_prompt_version(2));
        assert_eq!(
            (key.model(), key.dims(), key.prompt_version()),
            ("m", 4, Some(2))
        );
    }

    #[test]
    fn bad_keys_are_refused() {
        for bad in [
            "m",
            "@4",
            "nomic-ai/nomic-embed-text-v1.5@256",
            "Model@4",
            "..@4",
            ".m@4",
            "-m@4",
            "m m@4",
        ] {
            assert_eq!(ModelKey::parse(bad), Err(ModelKeyError::BadModel), "{bad}");
        }
        for bad in [
            "m@", "m@0", "m@-4", "m@+4", "m@4x", "org@m@4", "m@8193", "x@100000",
        ] {
            assert_eq!(ModelKey::parse(bad), Err(ModelKeyError::BadDims), "{bad}");
        }
        for bad in ["m@4+", "m@4+1", "m@4+p", "m@4+p-1"] {
            assert_eq!(
                ModelKey::parse(bad),
                Err(ModelKeyError::BadPromptVersion),
                "{bad}"
            );
        }
        assert_eq!(ModelKey::new("m", 0), Err(ModelKeyError::BadDims));
        assert_eq!(
            ModelKey::new("m", ModelKey::MAX_DIMS + 1),
            Err(ModelKeyError::BadDims)
        );
        assert_eq!(
            ModelKey::parse("m@8192").map(|k| k.dims()),
            Ok(ModelKey::MAX_DIMS)
        );
        let longest = "a".repeat(ModelKey::MAX_MODEL_LEN);
        assert!(ModelKey::new(&longest, 8).is_ok());
        assert_eq!(
            ModelKey::new(&alloc::format!("{longest}a"), 8),
            Err(ModelKeyError::BadModel)
        );
    }
}
