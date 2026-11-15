"""Writes the tiny stub model the always-on tests load: models/umx--stub/fixture/stub.gguf, a one-layer llama-architecture
GGUF, 16 wide, with random weights from a fixed seed and a character-level SentencePiece vocabulary (byte fallback),
which adds BOS and EOS like the embeddinggemma-2 GGUF. Synthetic: no real model or text in it. A few tens of KB.

Also writes stub-bf16.gguf, the same model with its matrices in BF16 (like the embeddinggemma-2 file), and
stub-bf16-widened.gguf, its F32 widening by tools/embed-bakeoff/widen_f32.py (gguf-py's writer): the CPU-path tests
check the adapter's own widening against it byte for byte (ws004 s16).

Run with Python that has gguf (llama.cpp's gguf-py, MIT) and numpy:
    python make_stub.py     # then update the stubs' sha256s in tests/stub.rs
"""

import string
import subprocess
import sys
from pathlib import Path

import gguf
import numpy as np

HERE = Path(__file__).resolve().parent
OUT = HERE / "models" / "umx--stub" / "fixture" / "stub.gguf"
BF16 = HERE / "stub-bf16.gguf"
WIDENED = HERE / "stub-bf16-widened.gguf"
WIDEN = HERE.parents[3] / "tools" / "embed-bakeoff" / "widen_f32.py"
WIDTH, FF, HEADS, CTX = 16, 32, 2, 64

tokens = ["<unk>", "<s>", "</s>"] + [f"<0x{b:02X}>" for b in range(256)]
types = [gguf.TokenType.UNKNOWN, gguf.TokenType.CONTROL, gguf.TokenType.CONTROL] + [gguf.TokenType.BYTE] * 256
for c in ["\u2581"] + sorted(set(string.ascii_letters + string.digits + string.punctuation)):
    tokens.append(c)
    types.append(gguf.TokenType.NORMAL)
scores = [0.0] * len(tokens)

def write(out, bf16):
    w = gguf.GGUFWriter(str(out), "llama")
    w.add_name("umx-stub")
    w.add_context_length(CTX)
    w.add_embedding_length(WIDTH)
    w.add_block_count(1)
    w.add_feed_forward_length(FF)
    w.add_head_count(HEADS)
    w.add_head_count_kv(HEADS)
    w.add_layer_norm_rms_eps(1e-6)
    w.add_rope_dimension_count(WIDTH // HEADS)
    w.add_file_type(gguf.LlamaFileType.MOSTLY_BF16 if bf16 else gguf.LlamaFileType.ALL_F32)
    w.add_tokenizer_model("llama")
    w.add_token_list(tokens)
    w.add_token_scores(scores)
    w.add_token_types(types)
    w.add_unk_token_id(0)
    w.add_bos_token_id(1)
    w.add_eos_token_id(2)
    w.add_add_bos_token(True)
    w.add_add_eos_token(True)
    w.add_pooling_type(gguf.PoolingType.MEAN)

    rng = np.random.default_rng(19)
    def t(*shape):
        m = (rng.standard_normal(shape) * 0.2).astype(np.float32)
        return m if not bf16 else gguf.quants.quantize(m, gguf.GGMLQuantizationType.BF16)
    def add(name, m):
        w.add_tensor(name, m, raw_dtype=gguf.GGMLQuantizationType.BF16 if bf16 and m.dtype != np.float32 else None)
    add("token_embd.weight", t(len(tokens), WIDTH))
    w.add_tensor("output_norm.weight", np.ones(WIDTH, dtype=np.float32))
    w.add_tensor("blk.0.attn_norm.weight", np.ones(WIDTH, dtype=np.float32))
    for name in ("attn_q", "attn_k", "attn_v", "attn_output"):
        add(f"blk.0.{name}.weight", t(WIDTH, WIDTH))
    w.add_tensor("blk.0.ffn_norm.weight", np.ones(WIDTH, dtype=np.float32))
    add("blk.0.ffn_gate.weight", t(FF, WIDTH))
    add("blk.0.ffn_up.weight", t(FF, WIDTH))
    add("blk.0.ffn_down.weight", t(WIDTH, FF))
    add("output.weight", t(len(tokens), WIDTH))

    out.parent.mkdir(parents=True, exist_ok=True)
    w.write_header_to_file()
    w.write_kv_data_to_file()
    w.write_tensors_to_file()
    w.close()


for out, bf16 in ((OUT, False), (BF16, True)):
    write(out, bf16)
    print(f"wrote {out} ({out.stat().st_size} bytes)")
WIDENED.unlink(missing_ok=True)
subprocess.run([sys.executable, str(WIDEN), str(BF16), str(WIDENED)], check=True)
print(f"wrote {WIDENED} ({WIDENED.stat().st_size} bytes)")
