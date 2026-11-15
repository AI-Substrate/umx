"""Failure handling of run-beir.sh and run-ort.sh, with fake commands (no models, no cargo, no network).

Each case copies the four shell scripts into a temporary tree, puts a fake venv python (BAKEOFF_PY)
and a fake tools/cargo-slot (CARGO_SLOT) in front of them, and fails the Nth call with exit 7.
Run: python3 tools/embed-bakeoff/test_runners.py   (RUNNER_SH=dash to try another POSIX shell)
"""

import json
import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPTS = ["run-beir.sh", "run-ort.sh", "beir.sh", "locked.sh"]
MODEL = "embeddinggemma-2"

# Counts its calls in $FAKE_LOG; the call numbered $FAIL_AT exits 7 without output.
FAKE_HEAD = """#!/bin/sh
n=$(( $(wc -l < "$FAKE_LOG") + 1 ))
echo "$*" >> "$FAKE_LOG"
[ "$n" = "${FAIL_AT:-0}" ] && exit 7
"""
# beir.sh runs: $BAKEOFF_PY beir_eval.py <cmd> <model> [task]; score writes the results file.
FAKE_PY = FAKE_HEAD + """[ "$2" = score ] && echo '{"fresh": true}' > "$RESULTS/beir-$3.json"
echo '{"fake": "line 1"}'
echo '{"fake": "last line"}'
"""
# locked.sh runs: $CARGO_SLOT run --release -q -- <cmd> <model>; stdout is the result JSON.
FAKE_SLOT = FAKE_HEAD + """echo '{"fresh": true}'
"""
FAKE_SYSCTL = """#!/bin/sh
echo "{ 0.50 0.40 0.30 }"
"""


def write_exe(path, text):
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class Runners(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        tool = self.tmp / "tools" / "embed-bakeoff"
        tool.mkdir(parents=True)
        for s in SCRIPTS:
            shutil.copy2(HERE / s, tool / s)
        self.results = self.tmp / "docs" / "project" / "benchmarks" / "embed-bakeoff" / "results"
        self.results.mkdir(parents=True)
        bin_ = self.tmp / "bin"
        bin_.mkdir()
        write_exe(bin_ / "python", FAKE_PY)
        write_exe(bin_ / "cargo-slot", FAKE_SLOT)
        write_exe(bin_ / "sysctl", FAKE_SYSCTL)
        self.log = self.tmp / "calls.log"
        self.log.write_text("")
        self.tool = tool
        self.env = dict(os.environ, PATH=f"{bin_}:{os.environ['PATH']}", BAKEOFF_PY=str(bin_ / "python"),
                        CARGO_SLOT=str(bin_ / "cargo-slot"), FAKE_LOG=str(self.log), RESULTS=str(self.results),
                        MAX_LOAD="16", WAIT_MIN="0")

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def stale(self, *names):
        for n in names:
            (self.results / f"{n}.json").write_text('{"stale": true}\n')

    def run_script(self, script, *args, fail_at=0):
        env = dict(self.env, FAIL_AT=str(fail_at))
        return subprocess.run([os.environ.get("RUNNER_SH", "sh"), str(self.tool / script), *args], env=env, capture_output=True, text=True)

    def calls(self):
        return self.log.read_text().splitlines()

    def fresh(self, name):
        return json.loads((self.results / f"{name}.json").read_text()) == {"fresh": True}

    # run-beir.sh: 4 encodes then 1 score per model.

    def test_beir_success_runs_every_stage(self):
        self.stale(f"beir-{MODEL}")
        r = self.run_script("run-beir.sh", MODEL)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(self.calls()), 5)
        self.assertTrue(self.fresh(f"beir-{MODEL}"))
        self.assertEqual(r.stdout.splitlines(), ['{"fake": "last line"}'] * 5)

    def test_beir_failed_encode_stops_and_removes_stale_result(self):
        self.stale(f"beir-{MODEL}")
        r = self.run_script("run-beir.sh", MODEL, fail_at=2)
        self.assertEqual(r.returncode, 7)
        self.assertEqual(len(self.calls()), 2)
        self.assertFalse((self.results / f"beir-{MODEL}.json").exists())
        self.assertIn("FAILED", r.stderr)

    def test_beir_failed_score_returns_its_status(self):
        self.stale(f"beir-{MODEL}")
        r = self.run_script("run-beir.sh", MODEL, fail_at=5)
        self.assertEqual(r.returncode, 7)
        self.assertEqual(len(self.calls()), 5)
        self.assertFalse((self.results / f"beir-{MODEL}.json").exists())

    # run-ort.sh: check, parity, quality, then speed.

    def test_ort_success_replaces_every_result(self):
        names = [f"{s}-{MODEL}" for s in ("ort-check", "parity", "ort-quality", "ort-speed")]
        self.stale(*names)
        r = self.run_script("run-ort.sh", MODEL)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(self.calls()), 4)
        self.assertEqual([l for l in r.stdout.splitlines() if l.startswith("done")], [f"done {n}" for n in names])
        self.assertTrue(all(self.fresh(n) for n in names))

    def test_ort_failed_stage_stops_without_done_or_stale_json(self):
        self.stale(f"parity-{MODEL}")
        r = self.run_script("run-ort.sh", MODEL, fail_at=2)
        self.assertEqual(r.returncode, 7)
        self.assertEqual(len(self.calls()), 2)
        self.assertEqual([l for l in r.stdout.splitlines() if l.startswith("done")], [f"done ort-check-{MODEL}"])
        self.assertFalse((self.results / f"parity-{MODEL}.json").exists())
        self.assertEqual(list(self.results.glob("*.part")), [])

    def test_ort_failed_speed_returns_its_status(self):
        self.stale(f"ort-speed-{MODEL}")
        r = self.run_script("run-ort.sh", MODEL, fail_at=4)
        self.assertEqual(r.returncode, 7)
        self.assertNotIn(f"done ort-speed-{MODEL}", r.stdout)
        self.assertFalse((self.results / f"ort-speed-{MODEL}.json").exists())


if __name__ == "__main__":
    unittest.main()
