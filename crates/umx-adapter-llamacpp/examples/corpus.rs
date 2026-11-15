//! The speed corpus the measure example reads: `count` texts of `tokens` tokens (prompt, BOS and EOS included) cut
//! from the synthetic bake-off fixture with the model's own tokenizer, in the document form, as the s03 bake-off
//! built them (tools/embed-bakeoff speed_corpus). Loads the GGUF's vocabulary only; prints a JSON list of strings.
//!
//!     cargo run --release -p umx-adapter-llamacpp --example corpus -- <model.gguf> <nodes.jsonl> [count] [tokens]

use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;

const PROMPT: &str = "title: none | text: ";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (gguf, nodes) = (&args[0], &args[1]);
    let count: usize = args.get(2).map_or(160, |n| n.parse().unwrap());
    let tokens: usize = args.get(3).map_or(300, |n| n.parse().unwrap());
    llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default().with_logs_enabled(false));
    let backend = LlamaBackend::init().unwrap();
    let model = LlamaModel::load_from_file(
        &backend,
        gguf,
        &LlamaModelParams::default().with_vocab_only(true),
    )
    .unwrap();
    let vocab = model.vocab();
    let all = std::fs::read_to_string(nodes)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let n: serde_json::Value = serde_json::from_str(l).unwrap();
            format!(
                "title: {}\nnote: {}",
                n["title"].as_str().unwrap(),
                n["note"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let body = tokens - vocab.tokenize(PROMPT.as_bytes(), true, false).len();
    let ids = vocab.tokenize(all.as_bytes(), false, false);
    let corpus: Vec<String> = ids
        .chunks(body)
        .filter(|c| c.len() == body)
        .take(count)
        .map(|c| {
            let text = String::from_utf8_lossy(&vocab.detokenize(c, true, false)).into_owned();
            format!("{PROMPT}{text}")
        })
        .collect();
    let lens: Vec<usize> = corpus
        .iter()
        .map(|t| vocab.tokenize(t.as_bytes(), true, false).len())
        .collect();
    eprintln!(
        "{} texts, tokens min {} max {} mean {:.1}",
        corpus.len(),
        lens.iter().min().unwrap(),
        lens.iter().max().unwrap(),
        lens.iter().sum::<usize>() as f64 / lens.len() as f64
    );
    println!("{}", serde_json::to_string(&corpus).unwrap());
}
