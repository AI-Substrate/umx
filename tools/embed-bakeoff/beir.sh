#!/bin/sh
# Run beir_eval.py in the throwaway venv ($BAKEOFF_PY), niced with 4 CPU threads (MPS where it works),
# with the Hugging Face cache outside ~/.cache/huggingface so no stored token is ever read or sent.
# Usage: tools/embed-bakeoff/beir.sh encode <model> <task> | score <model> | check <model> | vectors <model>
set -eu
cd "$(dirname "$0")"
: "${BAKEOFF_PY:?set BAKEOFF_PY to the venv python (see README)}"
unset HF_TOKEN HF_ENDPOINT HUGGING_FACE_HUB_TOKEN
export HF_HOME="$HOME/.cache/umx/hf" HF_HUB_DISABLE_IMPLICIT_TOKEN=1 HF_HUB_DISABLE_TELEMETRY=1 TOKENIZERS_PARALLELISM=false
export OMP_NUM_THREADS=4
nice -n 19 "$BAKEOFF_PY" beir_eval.py "$@"
