"""Independent replay of real capture evidence; never accesses hardware."""
import json
from pathlib import Path
import tempfile
import unittest

from analyze_capture import audit, read_file

ROOT = Path(__file__).resolve().parents[2] / "tests/fixtures/capture"


class CaptureEvidence(unittest.TestCase):
    def test_all_hardware_trials_conserve_bytes_frames_and_requests(self):
        trials = [p for p in sorted(ROOT.glob("run-*")) if p.is_dir()]
        self.assertEqual(len(trials), 5)
        for path in trials:
            with self.subTest(trial=path.name):
                self.assertEqual(audit(path)["audit"], "passed")

    def test_interrupted_blocks_do_not_enter_accepted_audio(self):
        path = ROOT / "run-004-interrupted"
        summary = json.loads(read_file(path, "summary.json"))
        self.assertEqual(summary["reason"], "signal")
        self.assertEqual(summary["pending_blocks"], 0)
        self.assertTrue(summary["alternate_restored"])
        rows = [json.loads(x) for x in read_file(path, "observations.jsonl").splitlines()]
        aborted = [r for r in rows if r["completion_status"] == "0xe00002eb"]
        self.assertEqual(len(aborted), 256)
        self.assertTrue(any(r["actual_length"] for r in aborted))
        self.assertTrue(all(not r["accepted"] and r["audio_file_offset"] is None for r in aborted))

    def test_auditor_detects_pcm_corruption(self):
        with tempfile.TemporaryDirectory(prefix="ftu-evidence-test-") as directory:
            root = Path(directory)
            for name in ("plan.json", "summary.json", "observations.jsonl", "packets.bin", "audio.s24le"):
                (root / name).write_bytes(read_file(ROOT / "run-001", name))
            audio = bytearray((root / "audio.s24le").read_bytes())
            audio[0] ^= 1
            (root / "audio.s24le").write_bytes(audio)
            with self.assertRaisesRegex(ValueError, "PCM bytes"):
                audit(root)


if __name__ == "__main__":
    unittest.main()
