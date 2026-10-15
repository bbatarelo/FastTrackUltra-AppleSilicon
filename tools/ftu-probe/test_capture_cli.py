"""Capture CLI guardrails. Every invocation rejects before hardware access."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


BINARY = Path(os.environ.get("FTU_PROBE", Path(__file__).parent / "build" / "ftu-probe")).resolve()


class CaptureCLI(unittest.TestCase):
    command = "capture"
    def reject(self, command, options):
        with tempfile.TemporaryDirectory(prefix="ftu-cli-test-") as root:
            output = Path(root) / "must-not-be-created"
            result = subprocess.run(
                [str(BINARY), command, "--output", str(output), *options],
                capture_output=True, text=True, timeout=5,
            )
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertFalse(output.exists(), result.stderr)

    def test_invalid_duration(self):
        for value in ("0", "-8", "1", "7", "9", "2049", "2056", "1.0", "8x",
                      "0x100", "", "999999999999999999999999999999"):
            with self.subTest(value=value):
                self.reject(self.command, ["--duration-ms", value])

    def test_missing_and_duplicate_duration(self):
        self.reject(self.command, ["--duration-ms"])
        self.reject(self.command, ["--duration-ms", "256", "--duration-ms", "256"])

    def test_unsupported_capture_controls(self):
        for option in ("--rate", "--alt", "--endpoint", "--seize", "--playback"):
            with self.subTest(option=option):
                self.reject(self.command, [option, "1"])

    def test_duration_does_not_leak_into_old_commands(self):
        for command in ("descriptors", "controls"):
            with self.subTest(command=command):
                self.reject(command, ["--duration-ms", "256"])

    def test_capture_rejects_bad_or_duplicate_selection(self):
        for options in (["--product", "ffff"], ["--location", "xyz"],
                        ["--product", "2080", "--product", "2081"],
                        ["--location", "1", "--location", "2"]):
            with self.subTest(options=options):
                self.reject(self.command, options)

    def test_existing_evidence_is_unchanged(self):
        with tempfile.TemporaryDirectory(prefix="ftu-cli-test-") as root:
            marker = Path(root) / "metadata.json"
            marker.write_bytes(b"previous evidence\x00\xff")
            result = subprocess.run(
                [str(BINARY), self.command, "--output", root, "--duration-ms", "8"],
                capture_output=True, text=True, timeout=5,
            )
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertEqual(marker.read_bytes(), b"previous evidence\x00\xff")
            self.assertEqual(list(Path(root).iterdir()), [marker])


class FeedbackCLI(CaptureCLI):
    command = "feedback"


class DuplexCLI(CaptureCLI):
    command = "duplex-silence"


class ToneCLI(CaptureCLI):
    command = "playback-tone"

    def test_existing_evidence_is_unchanged(self):
        self.reject(self.command, ["--duration-ms", "8"])

    def test_only_fixed_duration(self):
        for value in ("8", "256", "1000"):
            self.reject(self.command, ["--duration-ms", value])


class StreamCLI(CaptureCLI):
    command = "stream-silence"
    def test_invalid_duration(self):
        for value in ("0", "-8", "1", "7", "9", "60001", "60008", "1.0", "8x", "", "99999999999999999999999"):
            self.reject(self.command, ["--duration-ms", value])
    def test_invalid_cycles(self):
        for value in ("0", "4", "-1", "", "1x", "999999999999999"):
            self.reject(self.command, ["--cycles", value])
        self.reject(self.command, ["--cycles"])
        self.reject(self.command, ["--cycles", "1", "--cycles", "1"])

class StreamToneCLI(ToneCLI):
    command = "stream-tone"
    def test_only_fixed_duration(self):
        for value in ("8", "256", "1000", "2048", "60000"):
            self.reject(self.command, ["--duration-ms", value])
        self.reject(self.command, ["--cycles", "2"])

if __name__ == "__main__":
    unittest.main()
