#!/usr/bin/env python3
"""Print the s03 bake-off tables and verdict (markdown) from results/*.json; --write splices them into RESULT.md. Stdlib only."""
import json
import math
import random
import sys
from pathlib import Path

R = Path(__file__).resolve().parents[2] / "docs/project/benchmarks/embed-bakeoff/results"


def get(name):
    p = R / f"{name}.json"
    return json.loads(p.read_text()) if p.exists() else None


def f(x, n=3):
    return "n/a" if x is None else f"{x:.{n}f}"


def sign_test(a, b):
    """One-sided: P(wins >= observed) under p=0.5, wins = queries where b's RR beats a's."""
    ra = {q["id"]: q["rr10"] for q in a}
    wins = sum(1 for q in b if q["rr10"] > ra[q["id"]])
    losses = sum(1 for q in b if q["rr10"] < ra[q["id"]])
    n = wins + losses
    p = sum(math.comb(n, k) for k in range(wins, n + 1)) / 2**n if n else 1.0
    return wins, losses, p


out = []
w = out.append
models = {"nomic": "nomic-embed-text-v1.5", "gemma": "embeddinggemma-300m", "gemma-q": "embeddinggemma-300m-q (int8)"}

w("### Instrument checks\n")
w("| Check | Result |\n|---|---|")
for m in ["nomic", "gemma"]:
    c = get(f"check-{m}")
    if c:
        w(f"| {models[m]} @256: cos(cat, kitten) > cos(cat, carburetor) | {f(c['cos_cat_kitten'])} > {f(c['cos_cat_carburetor'])}: {'pass' if c['pass'] else 'FAIL'} |")
qn, qx = get("quality-nomic"), get("quality-nomic-noprefix")
if qn and qx:
    a, b = qn["dims"]["256"], qx["dims"]["256"]
    w(f"| Nomic @256 without prefixes (control) must score clearly worse | recall@10 {f(b['recall10'])} vs {f(a['recall10'])}, MRR@10 {f(b['mrr10'])} vs {f(a['mrr10'])} |")

q5 = get("quality-nomic-512")
if qn and q5:
    a, b = qn["dims"]["256"]["by_kind"]["tail"], q5["dims"]["256"]["by_kind"]["tail"]
    w(f"| Nomic @256 tail queries at max_length 512 must score worse than at 2048 (facts sit past token 512) | recall@10 {f(b['recall10'])} vs {f(a['recall10'])}, MRR@10 {f(b['mrr10'])} vs {f(a['mrr10'])} |")

w("\n### Quality (recall is per query: relevant found in top k over relevant count, averaged)\n")
w("| Model | Dims | recall@5 | recall@10 | MRR@10 | cut cost (r@10 768 - 256) | docs truncated at max_length | effective max_length |")
w("|---|---|---|---|---|---|---|---|")
for name, label in [("quality-nomic", "nomic-embed-text-v1.5"), ("quality-nomic-noprefix", "nomic, no prefixes (control)"), ("quality-nomic-512", "nomic, max_length 512 (control)"), ("quality-gemma", "embeddinggemma-300m"), ("quality-gemma-q", "embeddinggemma-300m-q (int8)")]:
    q = get(name)
    if not q:
        continue
    nat = str(q["native_dims"])
    for d in ["256", nat]:
        x = q["dims"][d]
        cost = f(q["dims"][nat]["recall10"] - q["dims"]["256"]["recall10"]) if d == "256" else ""
        w(f"| {label} | {d} | {f(x['recall5'])} | {f(x['recall10'])} | {f(x['mrr10'])} | {cost} | {q['doc_tokens']['truncated']} of {q['docs']} | {q['max_length_effective']} |")

qg = get("quality-gemma")
qq = get("quality-gemma-q")
if qn and qg:
    kinds = sorted(qn["dims"]["256"]["by_kind"])
    cols = [("Nomic", qn), ("Gemma", qg)] + ([("Gemma int8", qq)] if qq else [])
    w("\n### recall@10 / MRR@10 at 256 by query kind\n")
    w("| Kind | n | " + " | ".join(c for c, _ in cols) + " |\n|---|---|" + "---|" * len(cols))
    for k in kinds:
        cells = [q["dims"]["256"]["by_kind"][k] for _, q in cols]
        w(f"| {k} | {cells[0]['n']} | " + " | ".join(f"{f(c['recall10'])} / {f(c['mrr10'])}" for c in cells) + " |")

    w("\n### Cosine at 256: each relevant pair vs the best irrelevant hit per query (for the similar_to floor, D28)\n")
    w("| Model | relevant p5 / p50 / p95 | best irrelevant p5 / p50 / p95 | proposed floor (irrelevant p95 + relevant p5) / 2 |")
    w("|---|---|---|---|")
    for label, q in [("nomic-embed-text-v1.5", qn), ("embeddinggemma-300m", qg)]:
        x = q["dims"]["256"]
        r, i = x["cos_relevant"], x["cos_best_irrelevant"]
        w(f"| {label} | {f(r['p5'])} / {f(r['p50'])} / {f(r['p95'])} | {f(i['p5'])} / {f(i['p50'])} / {f(i['p95'])} | {f(x['proposed_floor'])} |")

w("\n### Speed (warm, ONNX intra-op threads capped; texts of 300 tokens cut from the fixture)\n")
w("| Model | Setup | batch | docs/s | query p50 ms | query p95 ms | first load s | warm load s/session | RSS MB after load / after run / peak | load avg start/end |")
w("|---|---|---|---|---|---|---|---|---|---|")
SETUPS = [("pool", "pool of 4 x 1-thread sessions"), ("single", "1 session x 4 threads (default)"),
          ("single-b16", "1 session x 4 threads (default)"), ("single-b64", "1 session x 4 threads (default)")]
for m in models:
    for run, setup in SETUPS:
        s = get(f"speed-{m}-{run}")
        if s:
            w(f"| {models[m]} | {setup} | {s.get('batch', 1)} | {f(s['docs_per_s'], 1)} | {f(s['query_ms']['p50'], 1)} | {f(s['query_ms']['p95'], 1)} | {f(s['first_load_s'], 2)} | {f(s['warm_load_s_per_session'], 2)} | {f(s.get('rss_after_load_mb'), 0)} / {f(s.get('rss_after_run_mb'), 0)} / {f(s['peak_rss_mb'], 0)} | {f(s['load_avg_1m_at_start'], 0)} / {f(s['load_avg_1m_at_end'], 0)} |")

# Floors and win rule (embeddings-expert, 2026-11-10; RSS floor replaced by the 13:30 ruling).
passing = {}
floors = {}
for m, q in [("nomic", qn), ("gemma", qg)]:
    if not q:
        continue
    x = q["dims"]["256"]
    fl = {
        "recall@10 >= 0.80": x["recall10"] >= 0.80,
        "cut cost <= 0.05": q["dims"][str(q["native_dims"])]["recall10"] - x["recall10"] <= 0.05,
    }
    single = get(f"speed-{m}-single")
    if single:
        fl["query p95 <= 150 ms"] = single["query_ms"]["p95"] <= 150
        fl["warm load <= 2 s"] = single["warm_load_s_per_session"] <= 2
    runs = [r for r in (get(f"speed-{m}-{n}") for n in ("single", "single-b16", "single-b64")) if r and "rss_after_load_mb" in r]
    if runs:
        good = [str(r["batch"]) for r in runs if r["peak_rss_mb"] <= 3072 and r["docs_per_s"] >= 10]
        fl["default config: peak RSS <= 3 GB and >= 10 docs/s"] = bool(good)
        passing[m] = good
    floors[m] = fl

if floors:
    names = list(next(iter(floors.values())).keys())
    w("\n### Floors at 256\n")
    w("| Floor | " + " | ".join(models[m] for m in floors) + " |")
    w("|---|" + "---|" * len(floors))
    for n in names:
        w(f"| {n} | " + " | ".join(("pass" if floors[m].get(n) else "FAIL") if n in floors[m] else "not run" for m in floors) + " |")
    if passing:
        w("\nBatch sizes that meet the default-config floor: " + "; ".join(f"{models[m]}: {', '.join(v) or 'none'}" for m, v in passing.items()) + ".")
    w("\nThe pool rows are for reference: the pool is a setting, off by default, and is not a floor (ruling 13:30).")

if qn and qg:
    a, b = qn["dims"]["256"], qg["dims"]["256"]
    dr, dm = b["recall10"] - a["recall10"], b["mrr10"] - a["mrr10"]
    wins, losses, p = sign_test(a["per_query"], b["per_query"])
    ra = {q["id"]: q["rr10"] for q in a["per_query"]}
    d = [q["rr10"] - ra[q["id"]] for q in b["per_query"]]
    rng = random.Random(19)
    boot = sorted(sum(rng.choice(d) for _ in d) / len(d) for _ in range(2000))
    lo, hi = boot[49], boot[1949]
    SUPERSEDED = ", **superseded by the 13:47 candidate set**"
    margin = dr >= 0.05 or (dm >= 0.05 and dr >= -0.02)
    w("\n### Verdict\n")
    w(f"- Gemma minus Nomic at 256: recall@10 {dr:+.3f}, MRR@10 {dm:+.3f}.")
    w(f"- Sign test on per-query reciprocal rank (Gemma better / worse / tied): {wins} / {losses} / {len(a['per_query']) - wins - losses}, one-sided p = {p:.3f}.")
    w(f"- Bootstrap 95% CI on the MRR@10 difference (2,000 resamples of the {len(d)} queries, seed 19): [{lo:+.3f}, {hi:+.3f}].")
    ok = {m: all(v.values()) for m, v in floors.items()}
    if qq:
        dq = qq["dims"]["256"]["mrr10"] - qg["dims"]["256"]["mrr10"]
        w(f"- Gemma int8 (tried, dropped): MRR@10 {dq:+.3f} against fp32, but slower on Apple CPU and barely lighter.")
    both_fail = sorted(n for n in floors["nomic"] if not floors["nomic"][n] and not floors["gemma"].get(n, True))
    if both_fail:
        w(f"- Both models fail: {', '.join(both_fail)}. The win rule leaves that to the embeddings-expert.")
    if ok.get("nomic") != ok.get("gemma"):
        win = "nomic" if ok.get("nomic") else "gemma"
        w(f"- Only {models[win]} passes every floor, so it led this pair{SUPERSEDED}.")
    elif margin and wins > losses and p < 0.10:
        w(f"- Gemma clears the margin and the sign test: embeddinggemma-300m led this pair{' on quality' if both_fail else ''}{SUPERSEDED}.")
    else:
        w(f"- Gemma does not clear the margin and the sign test together, so this is a tie and nomic-embed-text-v1.5 led this pair (D27d){SUPERSEDED}.")

text = "\n".join(out)
if "--write" in sys.argv:
    # Replace the generated block in RESULT.md, leaving the hand-written text around it.
    res = R.parent / "RESULT.md"
    doc = res.read_text()
    a, b = doc.index("<!-- report:start -->"), doc.index("<!-- report:end -->")
    res.write_text(doc[: a + len("<!-- report:start -->")] + "\n\n" + text + "\n\n" + doc[b:])
else:
    print(text)
