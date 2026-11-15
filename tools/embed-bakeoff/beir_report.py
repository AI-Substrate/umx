#!/usr/bin/env python3
"""Print the s03 BEIR table and verdict (markdown) from results/beir-*.json and the ort runs;
--write splices it into RESULT.md between the beir markers. Stdlib only."""
import json
import random
import sys
from pathlib import Path

R = Path(__file__).resolve().parents[2] / "docs/project/benchmarks/embed-bakeoff/results"
MODELS = ["embeddinggemma-2", "qwen3-embedding-0.6b", "voyage-4-nano", "harrier-oss-v1-0.6b"]
TASKS = ["scifact", "nfcorpus", "arguana"]
LICENCE = {"embeddinggemma-2": "Apache 2.0", "qwen3-embedding-0.6b": "Apache 2.0",
           "voyage-4-nano": "Apache 2.0", "harrier-oss-v1-0.6b": "MIT"}
PROMPT = {
    "embeddinggemma-2": "q `task: search result | query: {q}`; d `title: {title or none} | text: {text}`",
    "qwen3-embedding-0.6b": "q `Instruct: Given a web search query, retrieve relevant passages that answer the query\\nQuery:{q}`; d plain",
    "voyage-4-nano": "q `Represent the query for retrieving supporting documents: {q}`; d `Represent the document for retrieval: {d}`",
    "harrier-oss-v1-0.6b": "q `Instruct: Given a web search query, retrieve relevant passages that answer the query\\nQuery: {q}`; d plain",
}


def get(name):
    p = R / f"{name}.json"
    return json.loads(p.read_text()) if p.exists() else None


def f(x, n=3):
    return "n/a" if x is None else f"{x:.{n}f}"


def native(res):
    return str(res["tasks"]["scifact"]["native_dims"])


def mean_at(res, dims):
    return sum(res["tasks"][t][dims]["ndcg10"] for t in TASKS) / len(TASKS)


def paired_boot(a, b, dims, n=5000, seed=19):
    """Bootstrap of mean-over-tasks nDCG@10 (a - b), resampling queries within each task."""
    rng = random.Random(seed)
    per = []
    for t in TASKS:
        pa, pb = a["tasks"][t][dims]["per_query_ndcg10"], b["tasks"][t][dims]["per_query_ndcg10"]
        per.append([pa[q] - pb[q] for q in pa])
    diffs = sorted(sum(sum(rng.choice(d) for _ in d) / len(d) for d in per) / len(per) for _ in range(n))
    lo, hi = diffs[int(0.025 * n)], diffs[int(0.975 * n) - 1]
    p = 2 * min(sum(x <= 0 for x in diffs), sum(x >= 0 for x in diffs)) / n
    return sum(diffs) / n, lo, hi, min(p, 1.0)


out = []
w = out.append
res = {m: get(f"beir-{m}") for m in MODELS}
res = {m: r for m, r in res.items() if r}
ctrl = get("beir-qwen3-embedding-0.6b-noprompt")

w("### BEIR nDCG@10 (sentence-transformers reference path, one generic retrieval prompt per model)\n")
w("Per-task 95% CIs: 2,000 bootstrap resamples of queries. Leaderboard (MTEB) numbers use per-task")
w("instructions and will differ.\n")
w("| Model | Licence | Dims | SciFact | NFCorpus | ArguAna | Mean | Cut cost (native - 256) | Fixture recall@10 at 256 |")
w("|---|---|---|---|---|---|---|---|---|")
for m, r in sorted(res.items(), key=lambda kv: -mean_at(kv[1], "256")):
    nat = native(r)
    for d in ["256", nat]:
        cells = [f"{f(r['tasks'][t][d]['ndcg10'])} [{f(r['tasks'][t][d]['ndcg10_ci95'][0], 2)}, {f(r['tasks'][t][d]['ndcg10_ci95'][1], 2)}]" for t in TASKS]
        cost = mean_at(r, nat) - mean_at(r, "256")
        flag = f"{cost:+.3f}{' **flag** (> 0.03)' if cost > 0.03 else ''}" if d == "256" else ""
        fx = f(r["tasks"]["fixture"]["256"]["recall10"]) if d == "256" and "fixture" in r["tasks"] else ""
        w(f"| {m} | {LICENCE[m]} | {d} | " + " | ".join(cells) + f" | {f(mean_at(r, d))} | {flag} | {fx} |")
if ctrl:
    w(f"| qwen3-embedding-0.6b, no Instruct line (control) | | 256 | " + " | ".join(f(ctrl['tasks'][t]['256']['ndcg10']) for t in TASKS) + f" | {f(mean_at(ctrl, '256'))} | | {f(ctrl['tasks']['fixture']['256']['recall10'])} |")

w("\nPrompts (from each model card): " + "; ".join(f"**{m}** {PROMPT[m]}" for m in res) + ".")

if res:
    ranked = sorted(res, key=lambda m: -mean_at(res[m], "256"))
    lead = ranked[0]
    w(f"\n### Top group at 256 (leader {lead}, mean {f(mean_at(res[lead], '256'))})\n")
    w("| Model | leader - model (mean nDCG@10) | paired bootstrap 95% CI (5,000) | p | Holm threshold (family 0.10) | In top group |")
    w("|---|---|---|---|---|---|")
    rows = [(m, *paired_boot(res[lead], res[m], "256")) for m in ranked[1:]]
    order = sorted(rows, key=lambda r: r[4])
    holm, stop = {}, False
    for i, r in enumerate(order):
        thr = 0.10 / (len(order) - i)
        sig = not stop and r[4] <= thr
        stop = stop or not sig
        holm[r[0]] = (thr, sig)
    top = [lead]
    for m, diff, lo, hi, p in rows:
        thr, sig = holm[m]
        gap = mean_at(res[lead], "256") - mean_at(res[m], "256")
        joins = gap <= 0.02 or not sig
        if joins:
            top.append(m)
        w(f"| {m} | {gap:+.3f} | [{lo:+.3f}, {hi:+.3f}] | {p:.4f} | {thr:.3f} | {'yes' if joins else 'no'} |")
    # Leave-one-task-out (expert ruling 14:40): the leader must still lead with each task dropped.
    w("\nLeave-one-task-out (mean nDCG@10 at 256 over the other two tasks):\n")
    w("| Task dropped | " + " | ".join(ranked) + " | Leader |")
    w("|---|" + "---|" * (len(ranked) + 1))
    for t in TASKS:
        rest = [x for x in TASKS if x != t]
        means = {m: sum(res[m]["tasks"][x]["256"]["ndcg10"] for x in rest) / len(rest) for m in ranked}
        best = max(means, key=means.get)
        w(f"| {t} | " + " | ".join(f(means[m]) for m in ranked) + f" | {best} |")
        if best not in top:
            top.append(best)
    w(f"\nTop group: {', '.join(top)}. Top two by mean (parity gate): {', '.join(ranked[:2])}.")

    w("\n### similar_to floor at 256 (D28), on the 300-node fixture\n")
    w("| Model | relevant p5 / p50 / p95 | best irrelevant p5 / p50 / p95 | proposed floor |\n|---|---|---|---|")
    for m in ranked:
        c = res[m]["tasks"].get("fixture", {}).get("cosine", {}).get("256")
        if c:
            r, i = c["relevant"], c["best_irrelevant"]
            w(f"| {m} | {f(r['p5'])} / {f(r['p50'])} / {f(r['p95'])} | {f(i['p5'])} / {f(i['p50'])} / {f(i['p95'])} | {f(c['proposed_floor'])} |")

    loads = [x for m in ranked if (sp_ := get(f"ort-speed-{m}")) for x in (sp_["load_avg_1m_at_start"], sp_["load_avg_1m_at_end"])]
    busy = f"; speed provisional: measured at 1-minute load up to {max(loads):.0f}, rule 16" if loads and max(loads) >= 16 else ""
    w(f"\n### Rust path (ort over the onnx-community export, shipped config: 1 session x 4 threads, batch 16{busy})\n")
    w("| Model | parity min cos docs / queries at 256 | batch invariance min cos | docs/s | query p50 / p95 ms | first / warm load s | RSS MB after load / after run / peak | fixture recall@10 / MRR@10 at 256 | load avg start / end |")
    w("|---|---|---|---|---|---|---|---|---|")
    for m in ranked:
        p, c, s, q = get(f"parity-{m}"), get(f"ort-check-{m}"), get(f"ort-speed-{m}"), get(f"ort-quality-{m}")
        if not (p or s):
            continue
        par = f"{f(p['docs']['min'], 4)} / {f(p['queries']['min'], 4)} ({'pass' if p['pass'] else 'FAIL'})" if p else "not run"
        inv = f(c["batch_invariance_min_cos_256"], 4) if c else "n/a"
        sp = (f"{f(s['docs_per_s'], 1)} | {f(s['query_ms']['p50'], 1)} / {f(s['query_ms']['p95'], 1)} | {f(s['first_load_s'], 2)} / {f(s['warm_load_s'], 2)} | "
              f"{f(s['rss_after_load_mb'], 0)} / {f(s['rss_after_run_mb'], 0)} / {f(s['peak_rss_mb'], 0)}") if s else "not run | | |"
        qq = f"{f(q['dims']['256']['recall10'])} / {f(q['dims']['256']['mrr10'])}" if q else "n/a"
        la = f"{f(s['load_avg_1m_at_start'], 0)} / {f(s['load_avg_1m_at_end'], 0)}" if s else ""
        w(f"| {m} | {par} | {inv} | {sp} | {qq} | {la} |")

    w("\n### Floors and pick\n")
    picks = []
    for m in top:
        p, s, q = get(f"parity-{m}"), get(f"ort-speed-{m}"), get(f"ort-quality-{m}")
        fx = res[m]["tasks"].get("fixture", {}).get("256", {}).get("recall10")
        checks = {
            "fixture recall@10 >= 0.80": fx is not None and fx >= 0.80,
            "parity cos >= 0.999": bool(p and p["pass"]),
            "docs/s >= 10": bool(s and s["docs_per_s"] >= 10),
            "query p95 <= 150 ms": bool(s and s["query_ms"]["p95"] <= 150),
            "warm load <= 3 s": bool(s and s["warm_load_s"] <= 3),
            "peak RSS <= 4 GB": bool(s and s["peak_rss_mb"] <= 4096),
        }
        failed = [k for k, v in checks.items() if not v]
        w(f"- {m}: " + ("passes every floor" if not failed else "fails or not measured: " + ", ".join(failed)) + ".")
        if not failed:
            picks.append((m, s["docs_per_s"], s["peak_rss_mb"]))
    if picks:
        picks.sort(key=lambda x: (-x[1], x[2]))  # licences all Apache 2.0 or MIT: equal rank
        w(f"- **Pick: {picks[0][0]}** (top group, passes parity and floors; licences rank equal, then docs/s, then RSS).")
    else:
        w("- No top-group model has passed parity and every floor yet.")

text = "\n".join(out)
if "--write" in sys.argv:
    res_md = R.parent / "RESULT.md"
    doc = res_md.read_text()
    a, b = doc.index("<!-- beir:start -->"), doc.index("<!-- beir:end -->")
    res_md.write_text(doc[: a + len("<!-- beir:start -->")] + "\n\n" + text + "\n\n" + doc[b:])
else:
    print(text)
