#!/usr/bin/env python3
"""s03 BEIR run: nDCG@10 at 256 and native dims for the four candidates, through sentence-transformers.

Runs in a throwaway venv (see README). Datasets are fetched at run time into ~/.cache/umx/beir and never
committed. Hugging Face: pinned revisions, token=False, HF_HOME pointed away from ~/.cache/huggingface.

  beir_eval.py encode <model> <task>     embed one task, cache vectors in ~/.cache/umx/beir/emb
  beir_eval.py score <model>             score every cached task, write results/beir-<model>.json
  beir_eval.py check <model>             cat/kitten check, EOS and prompt samples
  beir_eval.py vectors <model>           20 fixture texts per kind at 256, for the ort parity gate
"""
import hashlib
import json
import os
import random
import sys
import urllib.request
import zipfile
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parents[1] / "docs/project/benchmarks/embed-bakeoff"
RESULTS = FIXTURE / "results"
CACHE = Path.home() / ".cache/umx/beir"
WEB = "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:"

# One generic retrieval prompt per model (what s04 would ship) and the model's own document form.
MODELS = {
    "embeddinggemma-2": dict(
        repo="google/embeddinggemma-2", revision="f9972ac22e696a105c482202646979e33e4b5a1e", remote=False,
        query=lambda q: f"task: search result | query: {q}",
        doc=lambda t, x: f"title: {t.strip() or 'none'} | text: {x}"),
    "harrier-oss-v1-0.6b": dict(
        repo="microsoft/harrier-oss-v1-0.6b", revision="f9b9dc8d367d443f2479d27aa5d8d2850c0774ee", remote=False,
        query=lambda q: f"{WEB} {q}",
        doc=lambda t, x: f"{t} {x}".strip()),
    # trust_remote_code: modeling_qwen3_bidirectional.py read at this revision (91 lines: a
    # bidirectional mask over Qwen3Model plus a linear 1024->2048 head; no I/O, no network).
    "voyage-4-nano": dict(
        repo="MongoDB/voyage-4-nano", revision="5fb34e870f49f19e79c94d5e118f5ee730133279", remote=True,
        query=lambda q: f"Represent the query for retrieving supporting documents: {q}",
        doc=lambda t, x: f"Represent the document for retrieval: {f'{t} {x}'.strip()}"),
    "qwen3-embedding-0.6b": dict(
        repo="Qwen/Qwen3-Embedding-0.6B", revision="97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3", remote=False,
        query=lambda q: f"{WEB}{q}",
        doc=lambda t, x: f"{t} {x}".strip()),
}
# Controls: Qwen3 queries without the Instruct line.
CONTROLS = {"qwen3-embedding-0.6b-noprompt": ("qwen3-embedding-0.6b", lambda q: q)}

BEIR = "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/{}.zip"
TASKS = ["scifact", "nfcorpus", "arguana"]
# sha256 of each BEIR zip as first fetched on 2026-11-10 (RESULT.md); checked on every use, new or cached.
BEIR_SHA256 = {
    "scifact": "536e14446a0ba56ed1398ab1055f39fe852686ecad24a6306c80c490fa8e0165",
    "nfcorpus": "efe5be03f8c5b86a5870102d0599d227c8c6e2484328e68c6522560385671b0b",
    "arguana": "cfdf79adce27a401b3cd3ea267903134dbfab2c6afeb95d7fe5724a00bf7557b",
}
MAX_LENGTH = 2048
CUT = 256


def sha256(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def write_atomic(path, text):
    """Write to <path>.part and rename, so a failed run never leaves a half-written or fresh-looking file."""
    tmp = path.with_name(path.name + ".part")
    tmp.write_text(text)
    tmp.replace(path)


def save_atomic(path, arr):
    tmp = path.with_name(path.name + ".part")
    with open(tmp, "wb") as f:
        np.save(f, arr)
    tmp.replace(path)


def fetch(task):
    CACHE.mkdir(parents=True, exist_ok=True)
    z = CACHE / f"{task}.zip"
    if not z.exists():
        part = z.with_suffix(".part")
        urllib.request.urlretrieve(BEIR.format(task), part)
        part.rename(z)
    got = sha256(z)
    if got != BEIR_SHA256[task]:
        sys.exit(f"{z}: sha256 {got} does not match the pinned {BEIR_SHA256[task]}; refusing to use it")
    d = CACHE / task
    if not d.exists():
        with zipfile.ZipFile(z) as zf:
            zf.extractall(CACHE)
    return d, {"url": BEIR.format(task), "sha256": got}


def load_task(task):
    """(doc ids, (title, text) pairs, query ids, query texts, qrels {qid: {did: grade}}, source)."""
    if task == "fixture":
        nodes = [json.loads(l) for l in (FIXTURE / "nodes.jsonl").read_text().splitlines() if l.strip()]
        qs = [json.loads(l) for l in (FIXTURE / "queries.jsonl").read_text().splitlines() if l.strip()]
        # Whole-node Markdown key-value text, as in the fastembed runs; the title also goes to the
        # Gemma document form, so it sees "title: <title> | text: title: ... note: ...".
        docs = [(n["title"], f"title: {n['title']}\nnote: {n['note']}") for n in nodes]
        return ([n["id"] for n in nodes], docs, [q["id"] for q in qs], [q["text"] for q in qs],
                {q["id"]: {r: 1 for r in q["relevant"]} for q in qs}, {"path": "docs/project/benchmarks/embed-bakeoff"})
    d, src = fetch(task)
    corpus = [json.loads(l) for l in open(d / "corpus.jsonl")]
    queries = {q["_id"]: q["text"] for q in map(json.loads, open(d / "queries.jsonl"))}
    qrels = {}
    for line in list(open(d / "qrels/test.tsv"))[1:]:
        qid, did, g = line.rstrip("\n").split("\t")
        if int(g) > 0:
            qrels.setdefault(qid, {})[did] = int(g)
    qids = sorted(q for q in qrels if q in queries)
    return ([c["_id"] for c in corpus], [(c.get("title", ""), c["text"]) for c in corpus], qids,
            [queries[q] for q in qids], qrels, src)


def model_for(name):
    import torch
    from sentence_transformers import SentenceTransformer

    torch.set_num_threads(4)
    m = MODELS[name]
    dev = os.environ.get("BAKEOFF_DEVICE") or ("mps" if torch.backends.mps.is_available() else "cpu")
    st = SentenceTransformer(m["repo"], revision=m["revision"], trust_remote_code=m["remote"], device=dev,
                             token=False)
    st.max_seq_length = MAX_LENGTH
    return st


def embed(st, texts, batch=16):
    # prompt is already in the text; ST adds nothing. Raw (pooled, normalised by the model's own
    # Normalize module where it has one) native vectors; the cut and re-normalise happen in score().
    return st.encode(texts, batch_size=batch, convert_to_numpy=True, show_progress_bar=False).astype(np.float32)


def emb_path(name, task, kind):
    return CACHE / "emb" / f"{name}--{task}--{kind}.npy"


def cmd_encode(name, task):
    base = CONTROLS[name][0] if name in CONTROLS else name
    st = model_for(base)
    m = MODELS[base]
    qfn = CONTROLS[name][1] if name in CONTROLS else m["query"]
    dids, docs, qids, qtexts, qrels, src = load_task(task)
    (CACHE / "emb").mkdir(parents=True, exist_ok=True)
    dp = emb_path(base, task, "docs")
    dtexts = [m["doc"](t, x) for t, x in docs]
    tok = st.tokenizer
    lens = [len(tok(t, add_special_tokens=True)["input_ids"]) for t in dtexts]
    meta = {"docs": len(dtexts), "queries": len(qtexts), "doc_tokens_mean": float(np.mean(lens)),
            "doc_truncated": int(sum(l > MAX_LENGTH for l in lens)), "source": src, "device": str(st.device)}
    if not dp.exists():
        order = np.argsort(lens)  # length-sorted batches waste less padding; restore order after
        v = embed(st, [dtexts[i] for i in order])
        out = np.empty_like(v)
        out[order] = v
        save_atomic(dp, out)
    save_atomic(emb_path(name, task, "queries"), embed(st, [qfn(q) for q in qtexts]))
    write_atomic(CACHE / "emb" / f"{name}--{task}--meta.json", json.dumps(meta))
    print(json.dumps({"encoded": name, "task": task, **meta}))


def cut(v, dims):
    v = v[:, :dims]
    return v / np.maximum(np.linalg.norm(v, axis=1, keepdims=True), 1e-12)


def ndcg10(ranked, rel):
    dcg = sum(rel.get(d, 0) / np.log2(i + 2) for i, d in enumerate(ranked[:10]))
    ideal = sorted(rel.values(), reverse=True)[:10]
    idcg = sum(g / np.log2(i + 2) for i, g in enumerate(ideal))
    return dcg / idcg if idcg else 0.0


def boot_ci(xs, n=2000, seed=19):
    rng = random.Random(seed)
    means = sorted(sum(rng.choice(xs) for _ in xs) / len(xs) for _ in range(n))
    return [means[int(0.025 * n)], means[int(0.975 * n) - 1]]


def score_task(name, task):
    base = CONTROLS[name][0] if name in CONTROLS else name
    dids, docs, qids, qtexts, qrels, src = load_task(task)
    D = np.load(emb_path(base, task, "docs"))
    Q = np.load(emb_path(name, task, "queries"))
    meta = json.loads((CACHE / "emb" / f"{name}--{task}--meta.json").read_text())
    native = D.shape[1]
    out = {"native_dims": native, **meta}
    for dims in sorted({CUT, native}):
        S = cut(Q, dims) @ cut(D, dims).T
        per_q, rec10 = {}, []
        for i, qid in enumerate(qids):
            s = S[i]
            if task == "arguana":  # BEIR ignores a query's own id in the corpus
                s = s.copy()
                s[[j for j, d in enumerate(dids) if d == qid]] = -np.inf
            top = np.argpartition(-s, 10)[:10]
            ranked = [dids[j] for j in top[np.argsort(-s[top])]]
            per_q[qid] = ndcg10(ranked, qrels[qid])
            rec10.append(len(set(ranked) & set(qrels[qid])) / len(qrels[qid]))
        xs = list(per_q.values())
        if task == "fixture":
            # similar_to floor (D28): each relevant pair's cosine against the best irrelevant hit.
            rel_c, irr_c = [], []
            for i, qid in enumerate(qids):
                rel = qrels[qid]
                rel_c += [float(S[i][j]) for j, d in enumerate(dids) if d in rel]
                irr_c.append(max(float(S[i][j]) for j, d in enumerate(dids) if d not in rel))
            pc = lambda xs, p: float(np.percentile(xs, p))
            out.setdefault("cosine", {})[str(dims)] = {
                "relevant": {k: pc(rel_c, v) for k, v in (("p5", 5), ("p50", 50), ("p95", 95))},
                "best_irrelevant": {k: pc(irr_c, v) for k, v in (("p5", 5), ("p50", 50), ("p95", 95))},
                "proposed_floor": (pc(irr_c, 95) + pc(rel_c, 5)) / 2}
        out[str(dims)] = {"ndcg10": float(np.mean(xs)), "ndcg10_ci95": boot_ci(xs), "recall10": float(np.mean(rec10)),
                          "per_query_ndcg10": per_q}
    return out


def cmd_score(name):
    res = {"model": name, "tasks": {}}
    if name in MODELS:
        res.update(repo=MODELS[name]["repo"], revision=MODELS[name]["revision"])
    for task in TASKS + ["fixture"]:
        if emb_path(name, task, "queries").exists():
            res["tasks"][task] = score_task(name, task)
    RESULTS.mkdir(exist_ok=True)
    write_atomic(RESULTS / f"beir-{name}.json", json.dumps(res, indent=1) + "\n")
    print(json.dumps({t: {d: round(v[d]["ndcg10"], 4) for d in v if d.isdigit()} for t, v in res["tasks"].items()}))


def pooling_of(st):
    for mod in st:
        if type(mod).__name__ == "Pooling":
            cfg = mod.get_config_dict()
            return {k: v for k, v in cfg.items() if v not in (False, None)}
    return None


def cmd_check(name):
    st = model_for(name)
    m = MODELS[name]
    v = cut(embed(st, [m["query"](w) for w in ["cat", "kitten", "carburetor"]]), CUT)
    tok = st.tokenizer
    sample_q = m["query"]("how long may soup cool")
    ids = tok(sample_q)["input_ids"]
    out = {"model": name, "cos_cat_kitten": float(v[0] @ v[1]), "cos_cat_carburetor": float(v[0] @ v[2]),
           "pass": bool(v[0] @ v[1] > v[0] @ v[2]), "query_sample": sample_q, "doc_sample": m["doc"]("T", "body"),
           "last_tokens": tok.convert_ids_to_tokens(ids[-3:]), "eos_token": tok.eos_token,
           "modules": [type(x).__name__ for x in st], "pooling": pooling_of(st),
           "device": str(st.device), "max_seq_length": st.max_seq_length}
    RESULTS.mkdir(exist_ok=True)
    write_atomic(RESULTS / f"beir-check-{name}.json", json.dumps(out, indent=1) + "\n")
    print(json.dumps(out, indent=1))


def cmd_vectors(name):
    st = model_for(name)
    m = MODELS[name]
    dids, docs, qids, qtexts, _, _ = load_task("fixture")
    pick = list(range(0, 300, 15))[:20]
    d = [m["doc"](*docs[i]) for i in pick]
    q = [m["query"](qtexts[i]) for i in range(0, 80, 4)][:20]
    out = {"model": name, "docs": d, "queries": q,
           "doc_vectors_256": cut(embed(st, d), CUT).tolist(), "query_vectors_256": cut(embed(st, q), CUT).tolist()}
    write_atomic(RESULTS / f"parity-ref-{name}.json", json.dumps(out) + "\n")
    print(f"wrote parity-ref-{name}.json")


if __name__ == "__main__":
    a = sys.argv[1:]
    {"encode": lambda: cmd_encode(a[1], a[2]), "score": lambda: cmd_score(a[1]),
     "check": lambda: cmd_check(a[1]), "vectors": lambda: cmd_vectors(a[1])}[a[0]]()
