"""Replay the actual 2026-10-15 desktop Ultra descriptor capture, not synthetic data."""

import hashlib
from pathlib import Path
import random
import unittest

import decode


FIXTURE = (Path(__file__).resolve().parents[2] / "tests" / "fixtures" /
           "descriptors" / "ultra-config-0.bin")


class HardwareFixture(unittest.TestCase):
    def test_actual_geometry_preserves_difference_from_linux_override(self):
        raw = FIXTURE.read_bytes()
        self.assertEqual(hashlib.sha256(raw).hexdigest(),
                         "b61f44d1b742de07c0587155acca8e33f45d67a1ce0360230d6350ae004e112d")
        report = decode.decode(raw)
        self.assertEqual(b"".join(bytes.fromhex(d["raw_hex"])
                                 for d in report["descriptors"]), raw)
        self.assertEqual(report["warnings"], [])
        endpoints = {}
        for d in report["descriptors"]:
            if d.get("kind") == "endpoint":
                context = d["interface_context"]
                f = d["fields"]
                endpoints[(context["bInterfaceNumber"], context["bAlternateSetting"],
                           f["bEndpointAddress"])] = f
        playback = endpoints[(1, 1, "0x01")]
        self.assertEqual(playback["bmAttributes"], "0x05")
        self.assertEqual(playback["wMaxPacketSize"], "0x0138")
        self.assertEqual(playback["high_speed_isochronous"]["service_interval_us"], 125)
        self.assertEqual(endpoints[(1, 1, "0x82")]["wMaxPacketSize"], "0x0003")
        self.assertEqual(endpoints[(2, 2, "0x81")]["wMaxPacketSize"], "0x0138")
        formats = [d["fields"] for d in report["descriptors"]
                   if d.get("kind") == "audio_streaming.FORMAT_TYPE_I"]
        self.assertEqual(len(formats), 3)
        for f in formats:
            self.assertEqual((f["bNrChannels"], f["bSubframeSize"], f["bBitResolution"]),
                             (8, 3, 24))
            self.assertEqual(f["sample_rates"]["discrete_hz"], [44100, 48000, 88200, 96000])

    def test_seeded_mutations_remain_bounded_and_lossless_when_accepted(self):
        original = FIXTURE.read_bytes()
        rng = random.Random(2080)
        for trial in range(1000):
            data = bytearray(original)
            for _ in range(rng.randint(1, 4)):
                data[rng.randrange(len(data))] = rng.randrange(256)
            with self.subTest(trial=trial):
                try:
                    report = decode.decode(data)
                except decode.DecodeError as error:
                    self.assertIsInstance(error.offset, int)
                else:
                    self.assertEqual(b"".join(bytes.fromhex(d["raw_hex"])
                                             for d in report["descriptors"]), data)


if __name__ == "__main__":
    unittest.main()
