"""Offline tests for agent_eval.py (stdlib unittest, no API).

Run from the repo root after `cargo build --release -p orq-cli`:
    python3 -m unittest discover -s eval/agent
Set ORQ_BIN to use another orq binary.
"""
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

import agent_eval as ae  # noqa: E402


class ParseTests(unittest.TestCase):
    def test_parses_blocks(self):
        text = 'intro\n<file path="src/lib.rs">\nfn a() {}\n</file>\n<file path="Cargo.toml">x</file>'
        self.assertEqual(ae.parse_file_blocks(text),
                         [("src/lib.rs", "fn a() {}\n"), ("Cargo.toml", "x\n")])

    def test_no_blocks(self):
        self.assertEqual(ae.parse_file_blocks("nothing here"), [])

    def test_path_escape_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "proj"
            root.mkdir()
            bad = ["../evil.rs", "/etc/passwd.rs", "src/../../x.rs", "~/x.rs",
                   "a\\b.rs", "", "target/x.rs", "notes.txt"]
            for p in bad:
                self.assertIsNone(ae.safe_target(root, p), p)
            written, rejected = ae.apply_files(root, [("../evil.rs", "x"), ("src/lib.rs", "ok\n")])
            self.assertEqual(written, ["src/lib.rs"])
            self.assertEqual(rejected, ["../evil.rs"])
            self.assertFalse((Path(d) / "evil.rs").exists())
            self.assertEqual((root / "src/lib.rs").read_text(), "ok\n")

    def test_symlink_escape_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "proj"
            root.mkdir()
            (Path(d) / "outside").mkdir()
            (root / "link").symlink_to(Path(d) / "outside")
            self.assertIsNone(ae.safe_target(root, "link/x.rs"))


class PricingTests(unittest.TestCase):
    def test_opus(self):
        # 1M in * $4 + 1M out * $20 + 1M read * $0.4 + 1M write * $5
        c = ae.cost_usd("claude-opus-5-5", 1_000_000, 1_000_000, 1_000_000, 1_000_000)
        self.assertAlmostEqual(c, 4 + 20 + 0.4 + 5)

    def test_sonnet_haiku(self):
        self.assertAlmostEqual(ae.cost_usd("claude-sonnet-5-5", 1_000_000, 1_000_000), 12.0)
        self.assertAlmostEqual(ae.cost_usd("claude-haiku-5-5", 1_000_000, 1_000_000), 0.6)

    def test_unknown_model(self):
        self.assertIsNone(ae.cost_usd("other", 1, 1))

    def test_fallback_policy(self):
        self.assertTrue(ae.use_fallbacks("claude-opus-5-5", False))
        self.assertFalse(ae.use_fallbacks("claude-opus-5-5", True))
        self.assertFalse(ae.use_fallbacks("claude-haiku-5-5", False))


class MockRunTests(unittest.TestCase):
    def test_end_to_end(self):
        orq = os.environ.get("ORQ_BIN") or str(ROOT / "target" / "release" / "orq")
        if not Path(orq).exists():
            self.skipTest("build orq first: cargo build --release -p orq-cli")
        with tempfile.TemporaryDirectory() as out:
            r = subprocess.run(
                [sys.executable, str(HERE / "agent_eval.py"), "--mock", "--orq", orq,
                 "--cases", str(ROOT / "eval" / "cases"), "--out", out, "--repeat", "1"],
                capture_output=True, text=True, cwd=ROOT)
            self.assertEqual(r.returncode, 0, r.stderr)
            files = list(Path(out).glob("*.jsonl"))
            self.assertEqual(len(files), 1)
            self.assertTrue(list(Path(out).glob("*.md")))
            recs = [json.loads(line) for line in files[0].read_text().splitlines()]
            self.assertGreaterEqual(len(recs), 4)
            for rec in recs:
                for k in ("case", "mode", "repeat", "success", "turns", "input_tokens",
                          "output_tokens", "cache_read_tokens", "cache_creation_tokens",
                          "feedback_tokens", "wall_seconds", "stop_reasons", "error"):
                    self.assertIn(k, rec)
                self.assertIsNone(rec["error"])
                self.assertTrue(rec["success"], rec)
                self.assertEqual(len(rec["feedback_tokens"]), rec["turns"])
            for case in {r["case"] for r in recs}:
                by = {r["mode"]: r for r in recs if r["case"] == case}
                self.assertEqual(by["orq"]["turns"], 1)
                self.assertEqual(by["raw"]["turns"], 2)
                self.assertLess(by["orq"]["turns"], by["raw"]["turns"])


class GeminiTests(unittest.TestCase):
    def test_request_maps_roles(self):
        body = ae.gemini_request("m", "sys", [
            {"role": "user", "content": "a"},
            {"role": "assistant", "content": "b"}], 100)
        self.assertEqual(body["systemInstruction"]["parts"][0]["text"], "sys")
        self.assertEqual([c["role"] for c in body["contents"]], ["user", "model"])
        self.assertEqual(body["generationConfig"]["maxOutputTokens"], 100)

    def test_response_skips_thoughts_and_counts_them(self):
        resp = ae.gemini_response({
            "candidates": [{"finishReason": "STOP", "content": {"parts": [
                {"text": "thinking...", "thought": True}, {"text": "<file"},
                {"text": ' path="x.rs">y</file>'}]}}],
            "usageMetadata": {"promptTokenCount": 50, "candidatesTokenCount": 7,
                              "thoughtsTokenCount": 3, "cachedContentTokenCount": 10}})
        self.assertEqual(ae.response_text(resp), '<file path="x.rs">y</file>')
        self.assertEqual(resp.stop_reason, "end_turn")
        self.assertEqual((resp.usage.input_tokens, resp.usage.output_tokens,
                          resp.usage.cache_read_input_tokens), (40, 10, 10))

    def test_response_stop_reasons(self):
        cut = ae.gemini_response({"candidates": [{"finishReason": "MAX_TOKENS",
                                                  "content": {"parts": []}}]})
        self.assertEqual(cut.stop_reason, "max_tokens")
        blocked = ae.gemini_response({"promptFeedback": {"blockReason": "SAFETY"}})
        self.assertEqual(blocked.stop_reason, "refusal")
        self.assertEqual(blocked.stop_details.category, "SAFETY")

    def test_provider_auto(self):
        self.assertEqual(ae.provider_for("gemini-flash-lite-latest", "auto"), "gemini")
        self.assertEqual(ae.provider_for("claude-haiku-5-5", "auto"), "anthropic")
        self.assertEqual(ae.provider_for("x", "gemini"), "gemini")

    def test_missing_key_is_reported(self):
        env = {k: v for k, v in os.environ.items()
               if k not in ("GEMINI_API_KEY", "GOOGLE_API_KEY")}
        p = subprocess.run([sys.executable, str(HERE / "agent_eval.py"), "--provider", "gemini",
                            "--modes", "raw", "--cases", str(ROOT / "eval/cases")],
                           env=env, capture_output=True, text=True)
        self.assertEqual(p.returncode, 2)
        self.assertIn("GEMINI_API_KEY", p.stderr)


if __name__ == "__main__":
    unittest.main()
