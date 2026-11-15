//! Manual live probe, never run in CI: embeds two synthetic texts on the current `az login` and
//! prints only dims, norms, latency and the served-model check. Never prints a token.
//!
//! cargo run -p umx-adapter-azure-openai --example live_probe -- <endpoint> <deployment> <model> <dims>

use std::time::Instant;

use umx_adapter_azure_openai::{AzureOpenAiConfig, AzureOpenAiEmbedder};
use umx_core::ports::{EmbedError, Embedder, TextKind};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [endpoint, deployment, model, dims] = args.as_slice() else {
        eprintln!("usage: live_probe <endpoint> <deployment> <model> <dims>");
        std::process::exit(2);
    };
    let dims: usize = dims.parse().expect("dims is a number");
    let texts = vec![
        "A red kite circles over a wheat field.".to_string(),
        "Quarterly invoices are reconciled on Fridays.".to_string(),
    ];

    // A model name Azure cannot serve: a BadResponse naming the served model shows Azure returns
    // the field; success would mean it does not.
    let probe = AzureOpenAiConfig::new(endpoint, deployment, "umx-model-field-probe", dims);
    let check = AzureOpenAiEmbedder::new(probe, |_| None).expect("config");
    match check.embed(&texts[..1], TextKind::Document) {
        Err(EmbedError::BadResponse(m)) if m.contains("served model") => {
            let served = m.split('`').nth(3).unwrap_or("?");
            println!("model field: present, value `{served}`");
        }
        Ok(_) => println!("model field: absent"),
        Err(e) => println!("model field check failed: {e:?}"),
    }

    let config = AzureOpenAiConfig::new(endpoint, deployment, model, dims);
    let embedder = AzureOpenAiEmbedder::new(config, |_| None).expect("config");
    for round in ["cold (includes az token)", "warm (cached token)"] {
        let started = Instant::now();
        match embedder.embed(&texts, TextKind::Document) {
            Ok(embedded) => {
                let vectors = embedded.vectors;
                let norms: Vec<String> = vectors
                    .iter()
                    .map(|v| format!("{:.6}", v.iter().map(|x| x * x).sum::<f32>().sqrt()))
                    .collect();
                println!(
                    "{round}: key {}@{} vectors {} dims {:?} norms {norms:?} latency {} ms",
                    embedder.key().model(),
                    embedder.key().dims(),
                    vectors.len(),
                    vectors.iter().map(Vec::len).collect::<Vec<_>>(),
                    started.elapsed().as_millis()
                );
            }
            Err(e) => println!("{round}: {e:?}"),
        }
    }
}
