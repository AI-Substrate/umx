//! Adapter tests with no network: a stub HTTP server on 127.0.0.1 and a stub token source.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: serde_json::Value,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn inputs(&self) -> Vec<String> {
        serde_json::from_value(self.body["input"].clone()).unwrap()
    }
}

pub(crate) struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: u16, body: impl Into<String>) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.into(),
    }
}

fn azure_error(status: u16, code: &str, message: &str) -> Reply {
    reply(
        status,
        serde_json::json!({"error": {"code": code, "message": message}}).to_string(),
    )
}

/// A deterministic vector per text: same text, same vector; different texts, different vectors.
pub(crate) fn vector_for(text: &str, dims: usize) -> Vec<f32> {
    let mut seed: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        seed = (seed ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (0..dims)
        .map(|j| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(j as u64 | 1);
            ((seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5
        })
        .collect()
}

/// A well-formed embeddings answer for the request, `model` included.
pub(crate) fn embeddings(req: &Recorded, model: &str) -> Reply {
    let dims = req.body["dimensions"].as_u64().unwrap() as usize;
    let data: Vec<_> = req
        .inputs()
        .iter()
        .enumerate()
        .map(|(index, text)| serde_json::json!({"index": index, "embedding": vector_for(text, dims)}))
        .collect();
    let tokens = prompt_tokens(&req.inputs());
    reply(
        200,
        serde_json::json!({
            "object": "list",
            "data": data,
            "model": model,
            "usage": {"prompt_tokens": tokens, "total_tokens": tokens}
        })
        .to_string(),
    )
}

/// What the stub says it counted: one token a word, as the service reports in `usage`.
fn prompt_tokens(inputs: &[String]) -> u64 {
    inputs
        .iter()
        .map(|t| t.split_whitespace().count() as u64)
        .sum()
}

/// The service's own count (`usage.prompt_tokens`) is what the adapter reports, so the pace is charged what was
/// counted, not the estimate (ws004 s25).
#[test]
fn the_services_token_count_is_reported() {
    let stub = serve(|req, _| embeddings(req, MODEL));
    let (port, _) = entra(&stub, 16);
    let out = port
        .embed(&texts(&["one two three", "four five"]), TextKind::Document)
        .unwrap();
    assert_eq!(out.tokens, Some(5));
}

pub(crate) struct Stub {
    pub endpoint: String,
    pub seen: Arc<Mutex<Vec<Recorded>>>,
}

impl Stub {
    pub fn requests(&self) -> Vec<Recorded> {
        self.seen.lock().unwrap().clone()
    }
}

/// Serve `answer` on 127.0.0.1, one request per connection, until the test process ends.
pub(crate) fn serve(answer: impl Fn(&Recorded, usize) -> Reply + Send + 'static) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let target = line.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut headers = Vec::new();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            }
            let len = headers
                .iter()
                .find(|(k, _)| k == "content-length")
                .map_or(0, |(_, v)| v.parse().unwrap());
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let recorded = Recorded {
                target,
                headers,
                body: serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
            };
            let n = {
                let mut seen = log.lock().unwrap();
                seen.push(recorded.clone());
                seen.len() - 1
            };
            let r = answer(&recorded, n);
            let mut out = format!(
                "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                r.status,
                r.body.len()
            );
            for (k, v) in &r.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(&r.body);
            let _ = stream.write_all(out.as_bytes());
        }
    });
    Stub { endpoint, seen }
}

/// A token source that counts its calls and hands out tokens living `life` from now.
pub(crate) struct CountingTokens {
    pub calls: Arc<AtomicUsize>,
    pub life: Duration,
    pub fail: bool,
}

impl TokenSource for CountingTokens {
    fn token(&self) -> Result<Token, TokenError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(TokenError("az exited with exit status: 1".to_string()));
        }
        Ok(
            Token::new(format!("tok-{n}"), SystemTime::now() + self.life)
                .from_program("az", "/stub/az"),
        )
    }
}

pub(crate) const MODEL: &str = "text-embedding-3-small";
const DEPLOYMENT: &str = "my-deployment";

pub(crate) fn config(endpoint: &str, dims: usize) -> AzureOpenAiConfig {
    AzureOpenAiConfig::new(endpoint, DEPLOYMENT, MODEL, dims)
}

fn fast() -> RetryPolicy {
    RetryPolicy {
        attempts: 3,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(5),
    }
}

pub(crate) fn entra(stub: &Stub, dims: usize) -> (AzureOpenAiEmbedder, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let source = CountingTokens {
        calls: Arc::clone(&calls),
        life: Duration::from_secs(3600),
        fail: false,
    };
    let embedder =
        AzureOpenAiEmbedder::with_token_source(config(&stub.endpoint, dims), Box::new(source))
            .unwrap()
            .with_retry_policy(fast());
    (embedder, calls)
}

fn texts(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn embed(e: &AzureOpenAiEmbedder, items: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
    e.embed(&texts(items), TextKind::Document)
        .map(|r| r.vectors)
}

fn failing(status: u16, code: &str, message: &str) -> Result<Vec<Vec<f32>>, EmbedError> {
    let (code, message) = (code.to_string(), message.to_string());
    let stub = serve(move |_, _| azure_error(status, &code, &message));
    embed(&entra(&stub, 4).0, &["a"])
}

#[test]
fn the_request_names_deployment_api_version_dims_and_bearer() {
    let stub = serve(|req, _| embeddings(req, MODEL));
    let (embedder, _) = entra(&stub, 8);
    let vectors = embed(&embedder, &["alpha", "beta"]).unwrap();
    assert_eq!(vectors.len(), 2);
    let req = &stub.requests()[0];
    assert_eq!(
        req.target,
        "/openai/deployments/my-deployment/embeddings?api-version=2024-02-01"
    );
    assert_eq!(req.body["dimensions"], 8);
    assert_eq!(req.inputs(), texts(&["alpha", "beta"]));
    assert_eq!(req.header("authorization"), Some("Bearer tok-0"));
    assert_eq!(req.header("api-key"), None);
}

#[test]
fn one_token_serves_many_embeds_and_a_near_expiry_one_is_refreshed() {
    let stub = serve(|req, _| embeddings(req, MODEL));
    let (embedder, calls) = entra(&stub, 4);
    for _ in 0..5 {
        embed(&embedder, &["x"]).unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the token is cached");

    let calls = Arc::new(AtomicUsize::new(0));
    let short = CountingTokens {
        calls: Arc::clone(&calls),
        life: TOKEN_REFRESH_MARGIN - Duration::from_secs(1),
        fail: false,
    };
    let embedder =
        AzureOpenAiEmbedder::with_token_source(config(&stub.endpoint, 4), Box::new(short)).unwrap();
    for _ in 0..3 {
        embed(&embedder, &["x"]).unwrap();
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "inside the 5 min margin, ask again"
    );
}

#[test]
fn no_token_is_unavailable_with_the_az_login_hint_and_sends_nothing() {
    let stub = serve(|req, _| embeddings(req, MODEL));
    let source = CountingTokens {
        calls: Arc::new(AtomicUsize::new(0)),
        life: Duration::ZERO,
        fail: true,
    };
    let embedder =
        AzureOpenAiEmbedder::with_token_source(config(&stub.endpoint, 4), Box::new(source))
            .unwrap();
    match embed(&embedder, &["x"]) {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert_eq!(next, "run az login");
            assert!(reason.contains("no Entra token"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    assert!(stub.requests().is_empty());
}

#[test]
fn a_401_names_az_login_and_the_role_after_one_fresh_token() {
    let stub = serve(|_, _| azure_error(401, "PermissionDenied", "token expired"));
    let (embedder, calls) = entra(&stub, 4);
    match embed(&embedder, &["x"]) {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert_eq!(next, "run az login");
            assert!(
                reason.contains("Cognitive Services OpenAI User"),
                "{reason}"
            );
            assert!(
                !reason.contains("token expired"),
                "Azure's message is not echoed: {reason}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(stub.requests().len(), 2, "one retry, with a fresh token");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let auth: Vec<_> = stub
        .requests()
        .iter()
        .map(|r| r.header("authorization").unwrap().to_string())
        .collect();
    assert_eq!(auth, ["Bearer tok-0", "Bearer tok-1"]);
}

#[test]
fn a_revoked_token_is_replaced_and_the_embed_succeeds() {
    let stub = serve(|req, n| {
        if n == 0 {
            azure_error(401, "PermissionDenied", "revoked")
        } else {
            embeddings(req, MODEL)
        }
    });
    let (embedder, calls) = entra(&stub, 4);
    embed(&embedder, &["x"]).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_403_key_auth_disabled_says_use_cli() {
    match failing(
        403,
        "AuthenticationTypeDisabled",
        "Key based authentication is disabled",
    ) {
        Err(EmbedError::Config(m)) => assert!(m.contains("auth = cli"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_403_otherwise_names_the_role() {
    match failing(403, "PermissionDenied", "no") {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert!(
                reason.contains("Cognitive Services OpenAI User"),
                "{reason}"
            );
            assert!(next.contains("Cognitive Services OpenAI User"), "{next}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_404_says_it_is_the_deployment_name() {
    match failing(
        404,
        "DeploymentNotFound",
        "The API deployment for this resource does not exist.",
    ) {
        Err(EmbedError::Config(m)) => {
            assert!(m.contains("deployment `my-deployment`"), "{m}");
            assert!(m.contains("DEPLOYMENT name"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_400_api_version_and_input_too_long_are_told_apart() {
    match failing(400, "BadRequest", "Unsupported api-version '2099-01-01'") {
        Err(EmbedError::Config(m)) => assert!(m.contains("api-version `2024-02-01`"), "{m}"),
        other => panic!("{other:?}"),
    }
    match failing(
        400,
        "BadRequest",
        "Invalid 'input[3]': maximum input length is 8192 tokens.",
    ) {
        Err(EmbedError::InputTooLong { index }) => assert_eq!(index, 3),
        other => panic!("{other:?}"),
    }
    match failing(400, "BadRequest", "maximum input length is 8192 tokens.") {
        Err(EmbedError::BadResponse(m)) => assert!(m.contains("an input over the cap"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_429_is_retried_then_succeeds() {
    let stub = serve(|req, n| {
        if n < 2 {
            let mut r = azure_error(429, "429", "slow down");
            r.headers.push(("retry-after-ms", "1".to_string()));
            r
        } else {
            embeddings(req, MODEL)
        }
    });
    let (embedder, _) = entra(&stub, 4);
    embed(&embedder, &["x"]).unwrap();
    assert_eq!(stub.requests().len(), 3);
}

#[test]
fn a_429_past_the_attempts_is_rate_limited_with_the_services_wait() {
    let stub = serve(|_, _| {
        let mut r = azure_error(429, "429", "slow down");
        r.headers.push(("retry-after", "2".to_string()));
        r
    });
    let (embedder, _) = entra(&stub, 4);
    let embedder = embedder.with_retry_policy(RetryPolicy {
        attempts: 1,
        ..fast()
    });
    assert_eq!(
        embed(&embedder, &["x"]),
        Err(EmbedError::RateLimited {
            retry_after_ms: Some(2000)
        })
    );
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn a_401_under_api_key_is_never_retried() {
    let stub = serve(|_, _| azure_error(401, "", ""));
    let mut cfg = config(&stub.endpoint, 4);
    cfg.auth = AzureAuth::ApiKey {
        env_var_name: "UMX_TEST_AZURE_KEY".to_string(),
    };
    let embedder = AzureOpenAiEmbedder::new(cfg, |_| Some("k".to_string())).unwrap();
    let _ = embed(&embedder, &["x"]);
    assert_eq!(stub.requests().len(), 1);
}

#[test]
fn an_http_date_retry_after_flows_into_retries_and_rate_limited() {
    // Elapsed date: no wait, and the retry still happens.
    let stub = serve(|req, n| {
        if n == 0 {
            let mut r = azure_error(503, "", "");
            r.headers
                .push(("retry-after", "Sun, 06 Nov 1994 08:49:37 GMT".to_string()));
            r
        } else {
            embeddings(req, MODEL)
        }
    });
    let (embedder, _) = entra(&stub, 4);
    embed(&embedder, &["x"]).unwrap();
    assert_eq!(stub.requests().len(), 2);

    // Future date: RateLimited carries the wait to it. 2033-11-06T08:49:37Z is 2014879777.
    let stub = serve(|_, _| {
        let mut r = azure_error(429, "429", "slow down");
        r.headers
            .push(("retry-after", "Sun, 06 Nov 2033 08:49:37 GMT".to_string()));
        r
    });
    let (embedder, _) = entra(&stub, 4);
    let embedder = embedder.with_retry_policy(RetryPolicy {
        attempts: 1,
        ..fast()
    });
    let want = (std::time::UNIX_EPOCH + Duration::from_secs(2_014_879_777))
        .duration_since(SystemTime::now())
        .unwrap()
        .as_millis() as u64;
    match embed(&embedder, &["x"]) {
        Err(EmbedError::RateLimited {
            retry_after_ms: Some(ms),
        }) => assert!(ms.abs_diff(want) < 5_000, "{ms} vs {want}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_500_is_retried() {
    let stub = serve(|req, n| {
        if n == 0 {
            azure_error(500, "InternalServerError", "")
        } else {
            embeddings(req, MODEL)
        }
    });
    let (embedder, _) = entra(&stub, 4);
    embed(&embedder, &["x"]).unwrap();
    assert_eq!(stub.requests().len(), 2);
}

#[test]
fn a_redirect_is_an_error_and_is_not_followed() {
    let elsewhere = serve(|req, _| embeddings(req, MODEL));
    let target = format!("{}/openai/deployments/x/embeddings", elsewhere.endpoint);
    let stub = serve(move |_, _| Reply {
        status: 307,
        headers: vec![("location", target.clone())],
        body: String::new(),
    });
    let (embedder, _) = entra(&stub, 4);
    match embed(&embedder, &["x"]) {
        Err(EmbedError::BadResponse(m)) => assert!(m.contains("redirect"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(
        elsewhere.requests().is_empty(),
        "the token must not reach another host"
    );
}

#[test]
fn vectors_are_placed_by_index_and_bad_mappings_rejected() {
    let shuffled = serve(|req, _| {
        let data: Vec<_> = req
            .inputs()
            .iter()
            .enumerate()
            .rev()
            .map(|(i, t)| serde_json::json!({"index": i, "embedding": vector_for(t, 4)}))
            .collect();
        reply(200, serde_json::json!({"data": data}).to_string())
    });
    let (embedder, _) = entra(&shuffled, 4);
    let got = embed(&embedder, &["a", "b", "c"]).unwrap();
    let mut want = vector_for("a", 4);
    normalise(&mut want).unwrap();
    assert_eq!(got[0], want);

    for data in [
        serde_json::json!([{"index": 0, "embedding": [1.0, 0, 0, 0]}, {"index": 0, "embedding": [0, 1.0, 0, 0]}]),
        serde_json::json!([{"index": 0, "embedding": [1.0, 0, 0, 0]}, {"index": 9, "embedding": [0, 1.0, 0, 0]}]),
        serde_json::json!([{"index": 0, "embedding": [1.0, 0, 0, 0]}]),
    ] {
        let body = serde_json::json!({ "data": data }).to_string();
        let stub = serve(move |_, _| reply(200, body.clone()));
        let (embedder, _) = entra(&stub, 4);
        assert!(
            matches!(
                embed(&embedder, &["a", "b"]),
                Err(EmbedError::BadResponse(_))
            ),
            "{data}"
        );
    }
}

#[test]
fn a_wrong_width_is_rejected() {
    let stub = serve(|req, _| {
        let data: Vec<_> = req
            .inputs()
            .iter()
            .enumerate()
            .map(|(i, t)| serde_json::json!({"index": i, "embedding": vector_for(t, 1536)}))
            .collect();
        reply(200, serde_json::json!({ "data": data }).to_string())
    });
    match embed(&entra(&stub, 256).0, &["a"]) {
        Err(EmbedError::BadResponse(m)) => assert!(m.contains("asked for 256 dimensions"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_key_is_model_at_dims_and_a_different_served_model_is_rejected() {
    let stub = serve(|req, _| embeddings(req, "text-embedding-3-large"));
    let (embedder, _) = entra(&stub, 256);
    assert_eq!(embedder.key().to_string(), "text-embedding-3-small@256");
    assert_eq!(embedder.locality(), Locality::Hosted);
    match embed(&embedder, &["a"]) {
        Err(EmbedError::BadResponse(m)) => assert!(m.contains("text-embedding-3-large"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn api_key_auth_reads_the_named_variable_and_sends_only_the_api_key_header() {
    let stub = serve(|req, _| embeddings(req, MODEL));
    let mut cfg = config(&stub.endpoint, 4);
    cfg.auth = AzureAuth::ApiKey {
        env_var_name: "UMX_TEST_AZURE_KEY".to_string(),
    };
    let mut asked = None;
    let embedder = AzureOpenAiEmbedder::new(cfg.clone(), |name| {
        asked = Some(name.to_string());
        Some("key-DO-NOT-LEAK".to_string())
    })
    .unwrap();
    assert_eq!(asked.as_deref(), Some("UMX_TEST_AZURE_KEY"));
    embed(&embedder, &["x"]).unwrap();
    let req = &stub.requests()[0];
    assert_eq!(req.header("api-key"), Some("key-DO-NOT-LEAK"));
    assert_eq!(req.header("authorization"), None);
    assert!(!format!("{embedder:?}{embedder:#?}").contains("DO-NOT-LEAK"));

    for missing in [None, Some("  ".to_string())] {
        match AzureOpenAiEmbedder::new(cfg.clone(), |_| missing.clone()) {
            Err(EmbedError::Config(m)) => assert!(m.contains("UMX_TEST_AZURE_KEY"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_rejected_api_key_names_the_variable() {
    let stub =
        serve(|_, _| azure_error(401, "401", "Access denied due to invalid subscription key"));
    let mut cfg = config(&stub.endpoint, 4);
    cfg.auth = AzureAuth::ApiKey {
        env_var_name: "UMX_TEST_AZURE_KEY".to_string(),
    };
    let embedder = AzureOpenAiEmbedder::new(cfg, |_| Some("k".to_string())).unwrap();
    match embed(&embedder, &["x"]) {
        Err(EmbedError::Unavailable { reason, next }) => {
            assert!(reason.contains("UMX_TEST_AZURE_KEY"), "{reason}");
            assert!(next.contains("az login"), "{next}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_empty_batch_sends_nothing() {
    let stub = serve(|req, _| embeddings(req, MODEL));
    assert_eq!(embed(&entra(&stub, 4).0, &[]), Ok(Vec::new()));
    assert!(stub.requests().is_empty());
}

#[test]
fn a_blank_input_fails_the_batch_before_any_request() {
    let stub = serve(|req, _| embeddings(req, MODEL));
    assert_eq!(
        embed(&entra(&stub, 4).0, &["a", " \n", "c"]),
        Err(EmbedError::EmptyInput { index: 1 })
    );
    assert!(stub.requests().is_empty());
}

#[test]
fn a_model_that_cannot_name_a_key_is_a_config_error() {
    for (model, dims) in [
        ("Text-Embedding-3-Small", 256),
        ("org/model", 256),
        (MODEL, 0),
    ] {
        let cfg = AzureOpenAiConfig::new("https://example.openai.azure.com", "d", model, dims);
        assert!(
            matches!(
                AzureOpenAiEmbedder::new(cfg, |_| None),
                Err(EmbedError::Config(_))
            ),
            "{model}@{dims}"
        );
    }
}

#[test]
fn an_unreachable_endpoint_is_unavailable() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let stub = Stub {
        endpoint: format!("http://127.0.0.1:{port}"),
        seen: Arc::default(),
    };
    assert!(matches!(
        embed(&entra(&stub, 4).0, &["x"]),
        Err(EmbedError::Unavailable { .. })
    ));
}

#[test]
fn only_an_https_resource_root_is_accepted() {
    for bad in [
        "http://example.openai.azure.com",
        "https://example.openai.azure.com/openai",
        "https://example.openai.azure.com?x=1",
        "https://user@example.openai.azure.com",
        "example.openai.azure.com",
        "https://",
    ] {
        assert!(
            matches!(embeddings_url(&config(bad, 4)), Err(EmbedError::Config(_))),
            "{bad}"
        );
    }
    for good in [
        "https://example.openai.azure.com",
        "https://example.openai.azure.com/",
        "http://127.0.0.1:8080",
    ] {
        assert!(embeddings_url(&config(good, 4)).is_ok(), "{good}");
    }
    let mut odd = config("https://example.openai.azure.com", 4);
    odd.deployment = "../x".to_string();
    assert!(embeddings_url(&odd).is_err());
}

/// Online: the stub server, never the live endpoint. Offline: a closed loopback port.
fn contract_embedder(reach: umx_contracts::embedder::Reach) -> AzureOpenAiEmbedder {
    let endpoint = match reach {
        umx_contracts::embedder::Reach::Online => serve(|req, _| embeddings(req, MODEL)).endpoint,
        umx_contracts::embedder::Reach::Offline => {
            let port = TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            format!("http://127.0.0.1:{port}")
        }
    };
    let source = CountingTokens {
        calls: Arc::new(AtomicUsize::new(0)),
        life: Duration::from_secs(3600),
        fail: false,
    };
    AzureOpenAiEmbedder::with_token_source(config(&endpoint, 16), Box::new(source))
        .unwrap()
        .with_retry_policy(fast())
}

umx_contracts::embedder_contract!(
    contract_embedder;
    em_c1_one_vector_per_input_in_order(),
    em_c2_width_is_key_dims(),
    em_c3_vectors_are_unit_length(),
    em_c4_same_text_and_kind_give_the_same_vector(),
    em_c5_empty_batch_is_empty_even_offline(),
    em_c6_distinct_texts_give_distinct_vectors(),
    em_c7_identical_texts_in_one_batch_match(),
    em_c8_blank_input_is_refused_by_index(),
    em_c9_offline_is_unavailable(),
    em_c10_over_cap_input_is_refused_or_reported(),
    em_c11_ready_is_ok_online(),
    em_c12_a_hosted_adapter_names_its_credential_after_ready(),
    em_c13_reported_tokens_are_none_or_count_the_batch(),
    em_c14_a_batch_of_max_batch_inputs_embeds(),
);

/// ready() gets a token and calls nothing: the stub records no request at all, so no body carries input.
#[test]
fn ready_sends_no_text_and_calls_no_endpoint() {
    for fail in [false, true] {
        let stub = serve(|req, _| embeddings(req, MODEL));
        let calls = Arc::new(AtomicUsize::new(0));
        let source = CountingTokens {
            calls: Arc::clone(&calls),
            life: Duration::from_secs(3600),
            fail,
        };
        let port =
            AzureOpenAiEmbedder::with_token_source(config(&stub.endpoint, 16), Box::new(source))
                .unwrap();
        umx_contracts::embedder::ready_sends_no_text(&port, || stub.requests().len());
        assert!(stub.requests().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one token fetch");
        match port.ready() {
            Ok(()) => assert!(!fail),
            Err(EmbedError::Unavailable { next, .. }) => {
                assert!(fail);
                assert_eq!(next, NEXT_LOGIN);
            }
            Err(other) => panic!("expected Unavailable, got {other:?}"),
        }
    }
}

/// credential() names how the adapter signs in and never a key: the env var NAME for api_key, and for Entra the
/// tool that gave the cached token, only once one did.
#[test]
fn credential_names_the_sign_in_never_the_secret() {
    let mut cfg = config("https://example.openai.azure.com", 16);
    cfg.auth = AzureAuth::ApiKey {
        env_var_name: "UMX_TEST_AZURE_KEY".to_string(),
    };
    let keyed = AzureOpenAiEmbedder::new(cfg, |_| Some("key-DO-NOT-LEAK".to_string())).unwrap();
    let credential = keyed.credential().unwrap();
    assert_eq!(
        (credential.kind, credential.from.as_str()),
        ("api_key", "UMX_TEST_AZURE_KEY")
    );
    assert!(!format!("{credential:?}").contains("DO-NOT-LEAK"));
    assert_ne!(
        credential.from, "key-DO-NOT-LEAK",
        "from names the variable, never the key"
    );

    struct FromAz;
    impl TokenSource for FromAz {
        fn token(&self) -> Result<Token, TokenError> {
            Ok(Token::new(
                "tok-DO-NOT-LEAK",
                std::time::SystemTime::now() + Duration::from_secs(3600),
            )
            .from_program("az", "/opt/bin/az"))
        }
    }
    let entra = AzureOpenAiEmbedder::with_token_source(
        config("https://example.openai.azure.com", 16),
        Box::new(FromAz),
    )
    .unwrap();
    assert_eq!(entra.credential(), None, "no token yet");
    entra.ready().unwrap();
    let credential = entra.credential().unwrap();
    assert_eq!(
        (credential.kind, credential.from.as_str()),
        ("az", "/opt/bin/az")
    );
}
