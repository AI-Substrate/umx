//! `umx config import --from flowspace3` (ws004 s27, D27: umx copies config, never shares it): maps the embedder
//! flowspace3 uses, read from its `config.toml` (decoded to dotted entries), to one umx embedder instance. Pure:
//! it reads only the named keys of the provider flowspace3's `embedder.active` names, never a secret's value, and
//! writes nothing. Auth is always `cli` (az login); a provider that uses a key is reported by that fact only.

use crate::config::{
    ConfigValue, Entries, INSTANCE_KEYS, INSTANCES, is_instance_name, looks_like_secret,
    parse_value,
};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The tools umx can import from.
pub const SOURCES: &[&str] = &["flowspace3"];

/// The embedding models whose name a deployment name may start with, longest first, and their full width.
const MODELS: &[(&str, i64)] = &[
    ("text-embedding-3-large", 3072),
    ("text-embedding-3-small", 1536),
    ("text-embedding-ada-002", 1536),
];

/// What an import would write: one instance's machine keys, in catalogue order.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    /// The umx instance name.
    pub instance: String,
    /// flowspace3's provider name.
    pub provider: String,
    /// `embedders.<instance>.<leaf>` and its value.
    pub entries: Vec<(String, ConfigValue)>,
    /// flowspace3 signs in to this provider with a key: umx does not copy it and uses az login.
    pub source_uses_key: bool,
    /// The model came from the deployment's name, not from a model key.
    pub model_inferred: bool,
}

/// Why nothing can be imported, in umx's own words (a catalogue slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    NoActive,
    NoProvider,
    NotAzure,
    Missing(&'static str),
    Invalid(&'static str),
    NoModel,
    BadName,
}

impl Refused {
    pub fn why(self) -> &'static str {
        match self {
            Refused::NoActive => "flowspace3 names no embedder (embedder.active is not set)",
            Refused::NoProvider => "the flowspace3 embedder.active names no provider in its config",
            Refused::NotAzure => {
                "the flowspace3 embedder is not kind azure_openai (umx imports only Azure OpenAI)"
            }
            Refused::Missing("endpoint") => "the flowspace3 embedder has no endpoint",
            Refused::Missing(_) => "the flowspace3 embedder has no deployment",
            Refused::Invalid("endpoint") => {
                "the flowspace3 embedder endpoint is not an https URL umx accepts"
            }
            Refused::Invalid("dims") => {
                "the flowspace3 embedder dimensions is not a whole number from 1 to 4096"
            }
            Refused::Invalid(_) => {
                "a flowspace3 embedder value is not one umx accepts (or it looks like a secret)"
            }
            Refused::NoModel => {
                "the model cannot be told from the flowspace3 deployment name (pass --model)"
            }
            Refused::BadName => {
                "the instance name is not snake_case of at most 32 characters (pass --as)"
            }
        }
    }
}

fn text<'a>(entries: &'a Entries, key: &str) -> Option<&'a str> {
    entries.get(key).and_then(ConfigValue::as_text)
}

fn spec(leaf: &str) -> &'static crate::config::KeySpec {
    INSTANCE_KEYS
        .iter()
        .find(|s| s.key == leaf)
        .expect("an instance key")
}

/// The umx instance name for a flowspace3 provider: lower case, `-` and other separators as `_`.
pub fn instance_name(provider: &str) -> String {
    provider
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Maps flowspace3's active embedder to a umx instance. `name` (`--as`), `model` (`--model`) and `dims` (`--dims`)
/// override.
pub fn flowspace3(
    entries: &Entries,
    name: Option<&str>,
    model: Option<&str>,
    dims: Option<u32>,
) -> Result<Import, Refused> {
    let provider = text(entries, "embedder.active").ok_or(Refused::NoActive)?;
    let at = |leaf: &str| format!("providers.{provider}.{leaf}");
    let prefix = at("");
    if !entries.keys().any(|k| k.starts_with(&prefix)) {
        return Err(Refused::NoProvider);
    }
    if text(entries, &at("kind")) != Some("azure_openai") {
        return Err(Refused::NotAzure);
    }
    let instance = name.map_or_else(|| instance_name(provider), str::to_string);
    if !is_instance_name(&instance) {
        return Err(Refused::BadName);
    }
    let endpoint = text(entries, &at("endpoint")).ok_or(Refused::Missing("endpoint"))?;
    let deployment = text(entries, &at("deployment")).ok_or(Refused::Missing("deployment"))?;
    let known = MODELS.iter().find(|(m, _)| deployment.starts_with(m));
    let (model, model_inferred) = match (model, text(entries, &at("model"))) {
        (Some(m), _) | (None, Some(m)) => (m, false),
        (None, None) => (known.ok_or(Refused::NoModel)?.0, true),
    };
    let dims = match dims
        .map(i64::from)
        .map(ConfigValue::Int)
        .as_ref()
        .or_else(|| {
            entries
                .get(&at("dimensions"))
                .or_else(|| entries.get(&at("dims")))
        }) {
        Some(ConfigValue::Int(n)) => *n,
        Some(_) => return Err(Refused::Invalid("dims")),
        None => MODELS
            .iter()
            .find(|(m, _)| *m == model)
            .map(|(_, d)| *d)
            .ok_or(Refused::NoModel)?,
    };
    let mut values: Vec<(&str, String)> = alloc::vec![
        ("kind", "azure_openai".to_string()),
        ("endpoint", endpoint.to_string()),
        ("deployment", deployment.to_string()),
        ("model", model.to_string()),
        ("dims", dims.to_string()),
    ];
    if let Some(v) = text(entries, &at("api_version")) {
        values.push(("api_version", v.to_string()));
    }
    values.push(("auth", "cli".to_string()));
    let mut out = Vec::new();
    for (leaf, v) in values {
        if looks_like_secret(&v) {
            return Err(Refused::Invalid(leaf));
        }
        let parsed = parse_value(spec(leaf), &v).map_err(|_| Refused::Invalid(leaf))?;
        out.push((format!("{INSTANCES}.{instance}.{leaf}"), parsed));
    }
    // Names only: whether a key-ish setting is there, never what it holds.
    let source_uses_key = entries.keys().any(|k| {
        k.strip_prefix(&prefix).is_some_and(|leaf| {
            let leaf = leaf.to_ascii_lowercase();
            leaf.contains("key") || leaf.contains("secret") || leaf.contains("token")
        })
    }) || text(entries, &at("auth")).is_some_and(|a| a != "cli" && a != "az");
    Ok(Import {
        instance,
        provider: provider.to_string(),
        entries: out,
        source_uses_key,
        model_inferred,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::decode;

    const FS3: &str = r#"
[providers.azure-embed]
kind = "azure_openai"
endpoint = "https://example-resource.openai.azure.com"
deployment = "text-embedding-3-small-01"
api_version = "2024-02-01"
dimensions = 1024
api_key_env = "FAKE_FS3_KEY_NAME"

[providers.chat]
kind = "azure_openai"
endpoint = "https://other.openai.azure.com"
deployment = "gpt-x"

[embedder]
active = "azure-embed"

[summarizer]
active = "chat"
"#;

    fn import(text: &str) -> Result<Import, Refused> {
        flowspace3(&decode(text.as_bytes()).unwrap(), None, None, None)
    }

    #[test]
    fn the_active_embedder_maps_to_exactly_the_instance_keys_with_cli_auth() {
        let i = import(FS3).unwrap();
        assert_eq!(i.instance, "azure_embed");
        assert_eq!(i.provider, "azure-embed");
        let keys: Vec<(&str, String)> = i
            .entries
            .iter()
            .map(|(k, v)| (k.as_str(), format!("{v:?}")))
            .collect();
        assert_eq!(
            keys,
            [
                (
                    "embedders.azure_embed.kind",
                    "Text(\"azure_openai\")".into()
                ),
                (
                    "embedders.azure_embed.endpoint",
                    "Text(\"https://example-resource.openai.azure.com\")".into()
                ),
                (
                    "embedders.azure_embed.deployment",
                    "Text(\"text-embedding-3-small-01\")".into()
                ),
                (
                    "embedders.azure_embed.model",
                    "Text(\"text-embedding-3-small\")".into()
                ),
                ("embedders.azure_embed.dims", "Int(1024)".into()),
                (
                    "embedders.azure_embed.api_version",
                    "Text(\"2024-02-01\")".into()
                ),
                ("embedders.azure_embed.auth", "Text(\"cli\")".into()),
            ]
        );
        assert!(i.source_uses_key && i.model_inferred);
        assert!(!format!("{i:?}").contains("FAKE_FS3_KEY_NAME"));
    }

    #[test]
    fn overrides_and_refusals() {
        let e = decode(FS3.as_bytes()).unwrap();
        let i = flowspace3(&e, Some("azure"), Some("text-embedding-3-large"), Some(256)).unwrap();
        assert_eq!(i.instance, "azure");
        assert!(!i.model_inferred);
        assert_eq!(i.entries[4].1, ConfigValue::Int(256));
        assert_eq!(
            flowspace3(&e, Some("Bad-Name"), None, None),
            Err(Refused::BadName)
        );
        assert_eq!(
            flowspace3(&e, None, None, Some(5000)),
            Err(Refused::Invalid("dims"))
        );
        let no_model = FS3.replace("text-embedding-3-small-01", "my-deployment");
        assert_eq!(import(&no_model), Err(Refused::NoModel));
        let chat = FS3.replace("active = \"azure-embed\"", "active = \"nope\"");
        assert_eq!(import(&chat), Err(Refused::NoProvider));
        assert_eq!(import("[embedder]\n"), Err(Refused::NoActive));
        let http = FS3.replace("https://example-resource", "http://example-resource");
        assert_eq!(import(&http), Err(Refused::Invalid("endpoint")));
        let local = FS3.replacen("kind = \"azure_openai\"", "kind = \"ollama\"", 1);
        assert_eq!(import(&local), Err(Refused::NotAzure));
    }

    #[test]
    fn a_provider_with_no_key_setting_is_not_reported_as_using_one() {
        let i = import(&FS3.replace("api_key_env = \"FAKE_FS3_KEY_NAME\"\n", "")).unwrap();
        assert!(!i.source_uses_key);
        let no_dims = FS3.replace("dimensions = 1024\n", "");
        assert_eq!(
            import(&no_dims).unwrap().entries[4].1,
            ConfigValue::Int(1536)
        );
    }
}
