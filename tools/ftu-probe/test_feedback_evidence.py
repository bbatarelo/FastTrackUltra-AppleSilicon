"""Independent packet replay guards, including success-looking aborted slots."""
import json
from pathlib import Path
import tempfile
import unittest
import analyze_feedback as feedback


class FeedbackEvidence(unittest.TestCase):
    def fixture(self, root, length=3, status="0x00000000", completion="0x00000000"):
        accepted = status in feedback.OK and completion in feedback.OK and length <= 3
        summary = dict(mode="feedback", interface=1, endpoint="0x82", planned_blocks=1,
                       submitted_blocks=1, completed_blocks=1, pending_blocks=0, raw_bytes=192,
                       audio_bytes=0, accepted_audio_frames=0, channels=[], status="ok" if accepted else "incomplete",
                       accepted_packets=64 if accepted else 0, rejected_packets=0 if accepted else 64,
                       unobserved_packets=0, empty_packets=64 if accepted and length == 0 else 0,
                       short_packets=64 if accepted and length < 3 else 0,
                       underrun_packets=64 if accepted and status == "0xe00002e7" else 0,
                       timebase_numer=1, timebase_denom=1, duration_ms=8, alternate=1,
                       packet_capacity=3, pipe_ref=2, reason=None, exit_code=0 if accepted else 3,
                       file_error=False, callback_error=False, drain_failed=False,
                       cleanup_error=False, cleanup_attempted=True, alternate_restored=True,
                       rate_before=[{"accepted": True, "hz": 48000}] * 2,
                       rate_after=[{"accepted": True, "hz": 48000}] * 2)
        rows = [dict(packet=i, block=0, slot=i, frame_start=100, requested_length=3,
                     frame_requested_length=3, submitted=True, submit_status="0x00000000",
                     completion_status=completion, callback_ticks=120000000,
                     callback_pointer_matches=True, packet_status=status, actual_length=length,
                     usb_timestamp_ticks=(100 + i // 8) * 1000000, raw_file_offset=i * 3,
                     accepted=accepted, classification="OVERSIZE" if length > 3 else "ERROR" if not accepted else "EMPTY" if length == 0 else "OK",
                     audio_frames=0, audio_file_offset=None) for i in range(64)]
        (root / "summary.json").write_text(json.dumps(summary))
        (root / "plan.json").write_text(json.dumps(dict(mode="feedback", interface=1, endpoint=130, alternate=1, packet_capacity=3, duration_ms=8)))
        (root / "observations.jsonl").write_text("".join(json.dumps(r)+"\n" for r in rows))
        (root / "packets.bin").write_bytes(bytes.fromhex("00000c") * 64)
        return rows

    def test_numeric_units_and_endianness(self):
        values = feedback.candidates(bytes.fromhex("00000c"))
        self.assertEqual(values["little_q14_samples_per_millisecond_hz"], 48000)
        self.assertEqual(values["little_q16_samples_per_microframe_hz"], 96000)
        self.assertEqual(feedback.candidates(bytes.fromhex("008001"))[
            "little_q14_samples_per_microframe_hz"], 48000)
        with self.assertRaises(ValueError):
            feedback.candidates(b"\x00\x0c")

    def test_short_empty_oversize_and_status(self):
        for length in range(5):
            for status in ("0x00000000", "0xe00002e7", "0xe00002eb"):
                with self.subTest(length=length, status=status), tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    self.fixture(root, length, status)
                    result = feedback.analyze(root)
                    self.assertEqual(result["audit"], "passed")
                    self.assertEqual(bool(result["numeric_hypotheses"]), length == 3 and status in feedback.OK)

    def test_abort_cannot_become_valid_feedback(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            rows = self.fixture(root, completion="0xe00002eb")
            self.assertEqual(feedback.analyze(root)["numeric_hypotheses"], [])
            rows[0]["accepted"] = True
            (root / "observations.jsonl").write_text("".join(json.dumps(r)+"\n" for r in rows))
            with self.assertRaisesRegex(ValueError, "acceptance mismatch"):
                feedback.analyze(root)

    def test_raw_stride_and_size_corruption(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root)
            raw = root / "packets.bin"
            raw.write_bytes(raw.read_bytes()[:-1])
            with self.assertRaisesRegex(ValueError, "raw slot bounds"):
                feedback.analyze(root)

    def test_transport_ok_can_have_no_full_payload(self):
        for length in (0, 1, 2):
            with self.subTest(length=length), tempfile.TemporaryDirectory() as tmp:
                self.fixture(Path(tmp), length=length)
                result = feedback.analyze(tmp)
                self.assertEqual(result["experiment_status"], "ok")
                self.assertEqual(result["full_payload_packets"], 0)
                self.assertEqual(result["distinct_full_payloads"], 0)
                self.assertIsNone(result["constant_full_payload"])
                self.assertEqual(result["numeric_hypotheses"], [])

    def test_constant_and_varying_full_payloads(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root)
            result = feedback.analyze(root)
            self.assertEqual(result["full_payload_packets"], 64)
            self.assertEqual(result["distinct_full_payloads"], 1)
            self.assertIs(result["constant_full_payload"], True)
            raw = root / "packets.bin"
            raw.write_bytes(bytes.fromhex("01000c") + raw.read_bytes()[3:])
            result = feedback.analyze(root)
            self.assertEqual(result["full_payload_packets"], 64)
            self.assertEqual(result["distinct_full_payloads"], 2)
            self.assertIs(result["constant_full_payload"], False)

    def test_failed_cleanup_is_visible_when_accounting_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root)
            path = root / "summary.json"
            summary = json.loads(path.read_text())
            summary.update(status="error", exit_code=1, cleanup_error=True, alternate_restored=False,
                           rate_after=[{"accepted": False, "hz": None}] * 2)
            path.write_text(json.dumps(summary))
            result = feedback.analyze(root)
            self.assertEqual(result["audit"], "passed")
            self.assertEqual(result["experiment_status"], "error")
            self.assertIs(result["experiment_outcome"]["alternate_restored"], False)
            self.assertIs(result["experiment_outcome"]["cleanup_error"], True)
            self.assertEqual(result["experiment_outcome"]["rate_after"], summary["rate_after"])

    def test_summary_geometry_and_pipe_corruption(self):
        for field, value in (("packet_capacity", 312), ("alternate", 0), ("duration_ms", 16),
                             ("pipe_ref", 0), ("pipe_ref", 3)):
            with self.subTest(field=field, value=value), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                self.fixture(root)
                path = root / "summary.json"
                summary = json.loads(path.read_text())
                summary[field] = value
                path.write_text(json.dumps(summary))
                with self.assertRaises(ValueError):
                    feedback.analyze(root)

    def test_timestamp_groups(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.fixture(Path(tmp))
            result = feedback.analyze(tmp)
            self.assertEqual(result["groups_with_eight_entries"], 8)
            self.assertEqual(result["groups_with_one_timestamp"], 8)
            self.assertEqual(result["adjacent_group_delta_us"]["median"], 1000)


if __name__ == "__main__":
    unittest.main()
