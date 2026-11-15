// Copyright (c) 2026 AI-Substrate. MIT. Adapted from flowspace3 crates/providers/src/azure_openai.rs
// (URL, error hints, vector ordering), made sync and keyed by model instead of deployment.

//! The Azure OpenAI embedder (workstream 004 s02).
//!
//! Sync on purpose: the port is sync, so this uses ureq (no async runtime, no Azure SDK).
//! - Auth is `az login` by default: the token comes from `az` (then `azd`) run directly, found on
//!   PATH, and is cached until 5 minutes before it expires. An api key is allowed, never the
//!   default; its value is handed in by the caller (umx-compose reads env), never stored in config.
//! - Redirects are never followed and any 3xx is an error, so neither the bearer token nor the
//!   api key can be forwarded to another host. TLS uses the platform's roots.
//! - Vectors are keyed `<model>@<dims>`, never by deployment, so two Azure resources serving the
//!   same model share committed vectors.

mod retry;
mod token;

#[cfg(test)]
mod tests;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use umx_core::ports::{Credential, EmbedError, Embedded, Embedder, Locality, ModelKey, TextKind};

pub use retry::RetryPolicy;
pub use token::{
    CLI_TIMEOUT, COGNITIVE_SERVICES_SCOPE, CliTokenSource, TOKEN_REFRESH_MARGIN, Token, TokenError,
    TokenSource,
};

use token::TokenCache;

/// A tested `api-version` for the embeddings route.
pub const DEFAULT_API_VERSION: &str = "2024-02-01";

/// OpenAI's per-input cap for its embedding models; Azure answers 400 past it.
pub const MAX_INPUT_TOKENS: usize = 8192;

/// Calls at once the deployment takes (Flowspace's measured choice): its quota, not umx, is the limit.
const CONCURRENCY_CEILING: usize = 32;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const NEXT_LOGIN: &str = "run az login";

/// One Azure OpenAI embeddings deployment. Holds no secret: an api key is named, not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureOpenAiConfig {
    /// Resource root, `https://<resource>.openai.azure.com`; a trailing slash is fine.
    pub endpoint: String,
    /// The DEPLOYMENT name, which need not match the model name. Never part of the key.
    pub deployment: String,
    /// The model the deployment serves, e.g. `text-embedding-3-small`. Names the vector space.
    pub model: String,
    /// Width asked for (`dimensions`) and checked on every vector.
    pub dims: usize,
    pub api_version: String,
    pub auth: AzureAuth,
}

impl AzureOpenAiConfig {
    /// `az login` auth and [`DEFAULT_API_VERSION`].
    pub fn new(
        endpoint: impl Into<String>,
        deployment: impl Into<String>,
        model: impl Into<String>,
        dims: usize,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            deployment: deployment.into(),
            model: model.into(),
            dims,
            api_version: DEFAULT_API_VERSION.to_string(),
            auth: AzureAuth::Cli,
        }
    }
}

/// How the adapter proves who it is. v1 is `cli` or `api_key` only (D27a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AzureAuth {
    /// An Entra token from `az` (then `azd`) on PATH.
    #[default]
    Cli,
    /// The resource's `api-key`, read by the caller from the named environment variable.
    ApiKey { env_var_name: String },
}

/// The api key value, in memory only. `Debug` never prints it.
struct ApiKey(String);

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[derive(Debug)]
enum Auth {
    Key { key: ApiKey, env_var_name: String },
    Entra(TokenCache),
}

/// [`Embedder`] backed by one Azure OpenAI embeddings deployment.
pub struct AzureOpenAiEmbedder {
    agent: ureq::Agent,
    url: String,
    key: ModelKey,
    config: AzureOpenAiConfig,
    auth: Auth,
    retry: RetryPolicy,
}

impl std::fmt::Debug for AzureOpenAiEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureOpenAiEmbedder")
            .field("url", &self.url)
            .field("model", &self.config.model)
            .field("dims", &self.config.dims)
            .field("auth", &self.auth)
            .finish()
    }
}

impl AzureOpenAiEmbedder {
    /// Build from config. For `api_key`, `env` looks up the named variable (umx-compose passes
    /// its env reader); the value is read once, here. For `cli`, tokens come from `az`/`azd`.
    pub fn new(
        config: AzureOpenAiConfig,
        env: impl FnOnce(&str) -> Option<String>,
    ) -> Result<Self, EmbedError> {
        let auth = match &config.auth {
            AzureAuth::Cli => Auth::Entra(TokenCache::new(Box::new(CliTokenSource::default()))),
            AzureAuth::ApiKey { env_var_name } => {
                let value = env(env_var_name).filter(|v| !v.trim().is_empty()).ok_or_else(|| {
                    EmbedError::Config(format!(
                        "Azure OpenAI api_key auth: environment variable {env_var_name} is not set \
                         or is empty; export the resource's key, or use auth = cli (az login)"
                    ))
                })?;
                Auth::Key {
                    key: ApiKey(value),
                    env_var_name: env_var_name.clone(),
                }
            }
        };
        Self::build(config, auth)
    }

    /// Build with Entra tokens from `source` (a stub in tests; a different identity later).
    pub fn with_token_source(
        config: AzureOpenAiConfig,
        source: Box<dyn TokenSource>,
    ) -> Result<Self, EmbedError> {
        Self::build(config, Auth::Entra(TokenCache::new(source)))
    }

    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    fn build(config: AzureOpenAiConfig, auth: Auth) -> Result<Self, EmbedError> {
        let url = embeddings_url(&config)?;
        let key = ModelKey::new(&config.model, config.dims).map_err(|e| {
            EmbedError::Config(format!(
                "Azure OpenAI model `{}` @ {}: {e}",
                config.model, config.dims
            ))
        })?;
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            // ureq keeps 3 idle connections a host by default; background embedding runs up to the declared ceiling
            // of calls at once (ws004 s25), so keep that many warm rather than redo TLS for each.
            .max_idle_connections(CONCURRENCY_CEILING)
            .max_idle_connections_per_host(CONCURRENCY_CEILING)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .new_agent();
        Ok(Self {
            agent,
            url,
            key,
            config,
            auth,
            retry: RetryPolicy::default(),
        })
    }

    /// POST with retries on transient statuses only.
    /// POST with retries on transient statuses only, plus one immediate retry with a fresh token
    /// when an Entra token is rejected (it may have been revoked before its expiry).
    fn post(&self, body: &[u8]) -> Result<Vec<u8>, EmbedError> {
        let mut attempt = 0u32;
        let mut fresh_token_tried = false;
        loop {
            let rejection = match self.try_post(body)? {
                Ok(bytes) => return Ok(bytes),
                Err(rejection) => rejection,
            };
            if rejection.status == 401 && matches!(self.auth, Auth::Entra(_)) && !fresh_token_tried
            {
                fresh_token_tried = true;
                continue;
            }
            let spent = attempt + 1 >= self.retry.attempts;
            if !retry::is_transient(rejection.status) || spent {
                return Err(self.failure(&rejection));
            }
            std::thread::sleep(self.retry.wait(attempt, rejection.retry_after));
            attempt += 1;
        }
    }

    fn try_post(&self, body: &[u8]) -> Result<Result<Vec<u8>, Rejection>, EmbedError> {
        let request = self
            .agent
            .post(&self.url)
            .header("content-type", "application/json");
        let request = match &self.auth {
            Auth::Key { key, .. } => request.header("api-key", &key.0),
            Auth::Entra(cache) => {
                let bearer = cache.bearer().map_err(|TokenError(reason)| {
                    EmbedError::Unavailable {
                        reason: format!(
                            "Azure OpenAI: no Entra token for {COGNITIVE_SERVICES_SCOPE} ({reason})"
                        ),
                        next: NEXT_LOGIN.to_string(),
                    }
                })?;
                request.header("authorization", &format!("Bearer {bearer}"))
            }
        };
        let mut response = request.send(body).map_err(|e| self.transport(&e))?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let retry_after = retry::retry_after(
            header("retry-after-ms").as_deref(),
            header("retry-after").as_deref(),
            std::time::SystemTime::now(),
        );
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(|e| self.transport(&e))?;
        if (200..300).contains(&status) {
            return Ok(Ok(bytes));
        }
        if status == 401
            && let Auth::Entra(cache) = &self.auth
        {
            cache.forget();
        }
        Ok(Err(Rejection {
            status,
            retry_after,
            error: AzureErrorBody::parse(&bytes),
        }))
    }

    fn transport(&self, e: &ureq::Error) -> EmbedError {
        EmbedError::Unavailable {
            reason: format!(
                "Azure OpenAI: could not reach {}: {e}",
                self.config.endpoint
            ),
            next: "check the network and the endpoint".to_string(),
        }
    }

    /// Name the fix: Azure's common rejections look alike and have different remedies.
    fn failure(&self, r: &Rejection) -> EmbedError {
        let endpoint = &self.config.endpoint;
        let code = r.error.code.as_deref().unwrap_or("");
        let message = r.error.message.as_deref().unwrap_or("");
        let status = r.status;
        let over_cap = message.contains("maximum input length");
        let too_long = input_too_long(message);
        // The response's message is read for its shape only and never copied into the error:
        // it can echo input text.
        let with_code = |text: String| {
            if code.is_empty() {
                format!("Azure OpenAI {status}: {text}")
            } else {
                format!("Azure OpenAI {status} ({code}): {text}")
            }
        };
        match status {
            300..=399 => EmbedError::BadResponse(with_code(format!(
                "{endpoint} answered with a redirect, which is never followed; set the endpoint \
                 to the resource root https://<resource>.openai.azure.com"
            ))),
            401 => match &self.auth {
                Auth::Key { env_var_name, .. } => EmbedError::Unavailable {
                    reason: with_code(format!(
                        "the api key in {env_var_name} was rejected by {endpoint}"
                    )),
                    next: format!("check {env_var_name}, or use auth = cli and {NEXT_LOGIN}"),
                },
                Auth::Entra(_) => EmbedError::Unavailable {
                    reason: with_code(format!(
                        "the az login token was rejected by {endpoint}; it needs the Cognitive \
                         Services OpenAI User role there"
                    )),
                    next: NEXT_LOGIN.to_string(),
                },
            },
            403 if code.contains("AuthenticationTypeDisabled")
                || message.contains("AuthenticationTypeDisabled") =>
            {
                EmbedError::Config(with_code(format!(
                    "{endpoint} has key authentication disabled; use auth = cli (az login)"
                )))
            }
            403 => EmbedError::Unavailable {
                reason: with_code(format!(
                    "this identity may not use {endpoint}; grant it the Cognitive Services \
                     OpenAI User role on the resource"
                )),
                next: "ask the resource owner for the Cognitive Services OpenAI User role"
                    .to_string(),
            },
            404 => EmbedError::Config(with_code(format!(
                "deployment `{}` does not exist on {endpoint}; this is the DEPLOYMENT name, \
                 which need not match the model name",
                self.config.deployment
            ))),
            400 if over_cap => match too_long {
                Some(index) => EmbedError::InputTooLong { index },
                None => EmbedError::BadResponse(with_code(format!(
                    "an input over the cap of {MAX_INPUT_TOKENS} tokens (Azure named no index)"
                ))),
            },
            400 if message.contains("api-version") || code.contains("api-version") => {
                EmbedError::Config(with_code(format!(
                    "api-version `{}` is not accepted for embeddings; try {DEFAULT_API_VERSION}",
                    self.config.api_version
                )))
            }
            429 => EmbedError::RateLimited {
                retry_after_ms: r
                    .retry_after
                    .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            },
            _ => EmbedError::BadResponse(with_code(format!(
                "{endpoint} refused the embeddings request"
            ))),
        }
    }
}

impl Embedder for AzureOpenAiEmbedder {
    fn adapter_id(&self) -> &'static str {
        "azure-openai.embedder"
    }

    /// Azure's embedding models take no document/query prefix, so `kind` changes nothing.
    fn embed(&self, texts: &[String], _kind: TextKind) -> Result<Embedded, EmbedError> {
        if texts.is_empty() {
            return Ok(Embedded::default());
        }
        if let Some(index) = texts.iter().position(|t| t.trim().is_empty()) {
            return Err(EmbedError::EmptyInput { index });
        }
        let body = serde_json::to_vec(&EmbeddingRequest {
            input: texts,
            dimensions: self.config.dims,
        })
        .map_err(|e| EmbedError::BadResponse(format!("could not encode the request: {e}")))?;
        let bytes = self.post(&body)?;
        let response: EmbeddingResponse = serde_json::from_slice(&bytes).map_err(|e| {
            EmbedError::BadResponse(format!("Azure OpenAI: unreadable embeddings response: {e}"))
        })?;
        if let Some(model) = response.model.as_deref()
            && model != self.config.model
        {
            return Err(EmbedError::BadResponse(format!(
                "deployment `{}` served model `{model}`, but config says `{}`; fix the model \
                 in config so vectors are keyed by what made them",
                self.config.deployment, self.config.model
            )));
        }
        let mut vectors = order_embeddings(response.data, texts.len())?;
        for (index, vector) in vectors.iter_mut().enumerate() {
            if vector.len() != self.config.dims {
                return Err(EmbedError::BadResponse(format!(
                    "asked for {} dimensions but vector {index} has {}; the deployment ignored \
                     `dimensions`, so use a model that honours it",
                    self.config.dims,
                    vector.len()
                )));
            }
            normalise(vector).ok_or_else(|| {
                EmbedError::BadResponse(format!("vector {index} is zero or not finite"))
            })?;
        }
        Ok(Embedded {
            vectors,
            truncated: Vec::new(),
            tokens: response.usage.map(|u| u.prompt_tokens),
        })
    }

    /// `<model>@<dims>`: never the deployment, which differs per Azure resource.
    fn key(&self) -> ModelKey {
        self.key.clone()
    }

    /// Flowspace's measured choice: a deployment is sized by its own quota, 429s carry
    /// `Retry-After` and are absorbed by the retry; throttling here would leave quota unused.
    fn concurrency_ceiling(&self) -> usize {
        CONCURRENCY_CEILING
    }

    /// [`MAX_INPUT_TOKENS`]: Azure rejects longer inputs (400) rather than truncating them.
    fn max_input_tokens(&self) -> usize {
        MAX_INPUT_TOKENS
    }

    /// Texts go to Azure over the network.
    fn locality(&self) -> Locality {
        Locality::Hosted
    }

    /// Gets a token (az, then azd, each killed after 10 s) or checks the key was given; never calls the endpoint.
    fn ready(&self) -> Result<(), EmbedError> {
        match &self.auth {
            Auth::Key { .. } => Ok(()),
            Auth::Entra(cache) => {
                cache
                    .bearer()
                    .map(drop)
                    .map_err(|TokenError(reason)| EmbedError::Unavailable {
                        reason: format!(
                            "Azure OpenAI: no Entra token for {COGNITIVE_SERVICES_SCOPE} ({reason})"
                        ),
                        next: NEXT_LOGIN.to_string(),
                    })
            }
        }
    }

    /// `api_key` and the env var NAME it was read from; or `az`/`azd` and the program that gave the cached token.
    fn credential(&self) -> Option<Credential> {
        match &self.auth {
            Auth::Key { env_var_name, .. } => Some(Credential {
                kind: "api_key",
                from: env_var_name.clone(),
            }),
            Auth::Entra(cache) => cache.from().map(|(kind, program)| Credential {
                kind,
                from: program,
            }),
        }
    }
}

/// `{endpoint}/openai/deployments/{deployment}/embeddings?api-version=…`, after checking each
/// part. The endpoint must be https (http only to loopback, for tests), so a token never
/// travels in clear.
fn embeddings_url(config: &AzureOpenAiConfig) -> Result<String, EmbedError> {
    let endpoint = config.endpoint.trim().trim_end_matches('/');
    let (scheme, rest) = endpoint.split_once("://").unwrap_or(("", endpoint));
    let (host, port) = rest.split_once(':').unwrap_or((rest, ""));
    let loopback = matches!(host, "127.0.0.1" | "localhost");
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'));
    let port_ok = port.is_empty() || port.parse::<u16>().is_ok();
    if !(scheme == "https" || scheme == "http" && loopback) || !host_ok || !port_ok {
        return Err(EmbedError::Config(format!(
            "Azure OpenAI endpoint `{endpoint}` must be the resource root, \
             https://<resource>.openai.azure.com, with no path or query"
        )));
    }
    let safe = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !safe(&config.deployment) {
        return Err(EmbedError::Config(format!(
            "Azure OpenAI deployment `{}` must be letters, digits, '-', '_' or '.'",
            config.deployment
        )));
    }
    if !safe(&config.api_version) {
        return Err(EmbedError::Config(format!(
            "Azure OpenAI api_version `{}` is not a version string",
            config.api_version
        )));
    }
    Ok(format!(
        "{endpoint}/openai/deployments/{}/embeddings?api-version={}",
        config.deployment, config.api_version
    ))
}

/// The index Azure names in `Invalid 'input[3]': maximum input length is 8192 tokens`.
fn input_too_long(message: &str) -> Option<usize> {
    if !message.contains("maximum input length") {
        return None;
    }
    let (_, tail) = message.split_once("input[")?;
    tail.split_once(']')?.0.parse().ok()
}

fn normalise(vector: &mut [f32]) -> Option<()> {
    let norm = vector
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    for x in vector.iter_mut() {
        *x = (f64::from(*x) / norm) as f32;
    }
    Some(())
}

/// Place each vector at the index the API claims; reject missing, duplicate or out-of-range ones.
fn order_embeddings(
    data: Vec<EmbeddingDatum>,
    expected: usize,
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let bad = |text: String| EmbedError::BadResponse(format!("Azure OpenAI embeddings: {text}"));
    if data.len() != expected {
        return Err(bad(format!(
            "asked for {expected} vectors, got {}",
            data.len()
        )));
    }
    let mut slots: Vec<Option<Vec<f32>>> = vec![None; expected];
    for datum in data {
        let slot = slots.get_mut(datum.index).ok_or_else(|| {
            bad(format!(
                "index {} out of range for a batch of {expected}",
                datum.index
            ))
        })?;
        if slot.is_some() {
            return Err(bad(format!("index {} returned twice", datum.index)));
        }
        *slot = Some(datum.embedding);
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(index, slot)| slot.ok_or_else(|| bad(format!("no vector for index {index}"))))
        .collect()
}

struct Rejection {
    status: u16,
    retry_after: Option<Duration>,
    error: AzureErrorBody,
}

/// `{"error": {"code": …, "message": …}}`, or nothing if the body is not that.
#[derive(Default)]
struct AzureErrorBody {
    code: Option<String>,
    message: Option<String>,
}

impl AzureErrorBody {
    fn parse(bytes: &[u8]) -> Self {
        #[derive(Deserialize)]
        struct Outer {
            error: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            code: Option<serde_json::Value>,
            message: Option<String>,
        }
        let Ok(Outer { error }) = serde_json::from_slice::<Outer>(bytes) else {
            return Self::default();
        };
        let code = error.code.map(|c| match c {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        });
        // A code is an identifier; anything else is not echoed.
        let code = code.filter(|c| {
            c.len() <= 64
                && c.chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))
        });
        Self {
            code,
            message: error.message,
        }
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    input: &'a [String],
    dimensions: usize,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
    model: Option<String>,
    /// What Azure counted, when it says: pacing corrects its estimate by it (ws004 s25).
    usage: Option<EmbeddingUsage>,
}

#[derive(Deserialize)]
struct EmbeddingUsage {
    prompt_tokens: u64,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    index: usize,
    embedding: Vec<f32>,
}
