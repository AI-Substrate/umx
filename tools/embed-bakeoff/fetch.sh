#!/bin/sh
# Fetch the bake-off models at a pinned Hugging Face commit, with no token, into
# ~/.cache/umx/models/<org>--<repo>/<commit>/, and print each file's sha256.
# Usage: tools/embed-bakeoff/fetch.sh [old|new|gguf|all]   (default new: the four 13:47 candidates; gguf: the
# embeddinggemma-2 GGUF that umx-adapter-llamacpp loads, until umx fetches models itself, ws004 s14)
set -eu

CACHE="${HOME}/.cache/umx/models"

fetch() {
	repo="$1"; commit="$2"; shift 2
	dir="${CACHE}/$(echo "$repo" | sed 's#/#--#')/${commit}"
	for f in "$@"; do
		mkdir -p "$dir/$(dirname "$f")"
		if [ ! -s "$dir/$f" ]; then
			# Plain curl: no Authorization header, no ~/.cache/huggingface/token, no HF_* env.
			curl -fsSL --proto '=https' --tlsv1.2 -o "$dir/$f.part" \
				"https://huggingface.co/${repo}/resolve/${commit}/${f}"
			mv "$dir/$f.part" "$dir/$f"
		fi
	done
	(cd "$dir" && shasum -a 256 "$@") > "$dir/SHA256SUMS"
	echo "## ${repo}@${commit}"
	cat "$dir/SHA256SUMS"
}

TOK="tokenizer.json config.json special_tokens_map.json tokenizer_config.json"
which="${1:-new}"

# Commits resolved from https://huggingface.co/api/models/<repo> on 2026-11-10.
if [ "$which" = old ] || [ "$which" = all ]; then
fetch nomic-ai/nomic-embed-text-v1.5 e9b6763023c676ca8431644204f50c2b100d9aab \
	onnx/model.onnx $TOK
fetch onnx-community/embeddinggemma-300m-ONNX 5090578d9565bb06545b4552f76e6bc2c93e4a66 \
	onnx/model.onnx onnx/model.onnx_data onnx/model_quantized.onnx onnx/model_quantized.onnx_data $TOK
fi
if [ "$which" = new ] || [ "$which" = all ]; then
fetch onnx-community/Qwen3-Embedding-0.6B-ONNX c25a394dd583836952667c12f008335071b3f43d \
	onnx/model.onnx onnx/model.onnx_data tokenizer.json config.json
fetch onnx-community/harrier-oss-v1-0.6b-ONNX e4daffa011e666dcd2ff2a3c6c05084090ac314d \
	onnx/model.onnx onnx/model.onnx_data onnx/model.onnx_data_1 tokenizer.json config.json
fetch onnx-community/voyage-4-nano-ONNX fb6f32cceb63b6eac92e1fdc08e00d03c41cdf5a \
	onnx/model.onnx onnx/model.onnx_data tokenizer.json config.json
fetch onnx-community/embeddinggemma-2-ONNX daa72c51243991dfcaf9f9137d2c573d8f7790c0 \
	onnx/model.onnx onnx/model.onnx_data tokenizer.json config.json
fi
if [ "$which" = gguf ] || [ "$which" = all ]; then
fetch ggml-org/embeddinggemma-2-GGUF 6ea3e3a309d12a4779cfda98b8c19800d2d5da71 embeddinggemma-2-BF16.gguf
fi
