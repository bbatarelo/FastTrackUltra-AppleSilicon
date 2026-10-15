"""Offline tests of `ftu-probe control-plan`, the exact request list `controls` issues.

Runs the built binary (FTU_PROBE, default build/ftu-probe); control-plan never
enumerates or opens hardware.
"""

import collections
import json
import os
import subprocess
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
BINARY = os.environ.get("FTU_PROBE", os.path.join(HERE, "build", "ftu-probe"))

GET_CUR, GET_MIN, GET_MAX, GET_RES = 0x81, 0x82, 0x83, 0x84


def run(*args):
    return subprocess.run([BINARY, *args], capture_output=True, text=True, timeout=10)


def num(v):
    return int(v, 16)


class ControlPlan(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not os.path.exists(BINARY):
            raise RuntimeError(f"{BINARY} not found; run `make` first")
        proc = run("control-plan")
        if proc.returncode != 0:
            raise RuntimeError(f"control-plan failed ({proc.returncode}): {proc.stderr}")
        cls.plan = [json.loads(line) for line in proc.stdout.splitlines()]

    def select(self, rtype=0xA1, req=GET_CUR, entity=None, selector=None):
        return [p for p in self.plan
                if num(p["bmRequestType"]) == rtype and num(p["bRequest"]) == req
                and (entity is None or p.get("entity_id") == entity)
                and (selector is None or p["control_selector"] == selector)]

    def test_count_and_indices(self):
        self.assertEqual(len(self.plan), 157)
        self.assertEqual([p["index"] for p in self.plan], list(range(157)))
        self.assertEqual(len(self.select(req=GET_CUR)) + len(self.select(0xA2)), 154)

    def test_requests_are_unique(self):
        keys = [(p["bmRequestType"], p["bRequest"], p["wValue"], p["wIndex"]) for p in self.plan]
        self.assertEqual(len(set(keys)), len(keys))
        labels = [p["label"] for p in self.plan]
        self.assertEqual(len(set(labels)), len(labels))
        self.assertTrue(all(labels))

    def test_direction_and_allowlist(self):
        for p in self.plan:
            rtype, req = num(p["bmRequestType"]), num(p["bRequest"])
            self.assertTrue(rtype & 0x80, p)          # device-to-host
            self.assertEqual(rtype & 0x60, 0x20, p)   # class request
            self.assertIn(rtype, (0xA1, 0xA2), p)
            self.assertIn(req, (GET_CUR, GET_MIN, GET_MAX, GET_RES), p)
            self.assertEqual(p["request_name"],
                             {GET_CUR: "GET_CUR", GET_MIN: "GET_MIN", GET_MAX: "GET_MAX",
                              GET_RES: "GET_RES"}[req])

    def test_no_result_fields(self):
        for p in self.plan:
            for key in ("ioreturn", "actual_length", "raw_hex", "decoded", "accepted"):
                self.assertNotIn(key, p)

    def test_interface_targeting(self):
        for p in self.plan:
            value, index = num(p["wValue"]), num(p["wIndex"])
            self.assertEqual(p["control_selector"], value >> 8)
            if p["recipient"] == "interface":
                self.assertEqual(num(p["bmRequestType"]), 0xA1)
                self.assertEqual(index & 0xFF, 0, p)
                self.assertEqual(p["interface"], 0)
                self.assertEqual(p["entity_id"], index >> 8)
                self.assertIn(p["entity_id"], (5, 6, 7))
                self.assertEqual(p["channel"], value & 0xFF)
            else:
                self.assertEqual(p["recipient"], "endpoint")

    def test_endpoint_sample_rate(self):
        eps = self.select(0xA2)
        self.assertEqual(len(eps), 2)
        self.assertEqual(sorted(p["wIndex"] for p in eps), ["0x0001", "0x0081"])
        self.assertEqual(sorted(p["endpoint"] for p in eps), ["0x01", "0x81"])
        for p in eps:
            self.assertEqual(p["wValue"], "0x0100")
            self.assertEqual(p["wLength"], 3)
            self.assertEqual(p["value_kind"], "u24le_hz")
        # Last, so their possible failure at alt 0 can't stop the mixer reads early.
        self.assertEqual([p["index"] for p in eps], [155, 156])

    def test_matrix_destinations_and_one_based_channels(self):
        matrix = [p for p in self.select(entity=5) if 1 <= p["control_selector"] <= 8]
        self.assertEqual(len(matrix), 128)
        by_dest = collections.defaultdict(list)
        for p in matrix:
            self.assertEqual(p["wLength"], 2)
            self.assertEqual(p["value_kind"], "s16le_raw")
            by_dest[p["control_selector"]].append(p["channel"])
        self.assertEqual(sorted(by_dest), list(range(1, 9)))
        for channels in by_dest.values():
            self.assertEqual(sorted(channels), list(range(1, 17)))
        self.assertIn("output destination 3 <- hardware input 8",
                      next(p["label"] for p in matrix if p["wValue"] == "0x0308"))
        self.assertIn("output destination 3 <- host return 1",
                      next(p["label"] for p in matrix if p["wValue"] == "0x0309"))

    def test_effect_sends_and_returns(self):
        sends = self.select(entity=5, selector=9)
        self.assertEqual(sorted(p["channel"] for p in sends), list(range(1, 17)))
        returns = self.select(entity=7)
        self.assertEqual(sorted(p["channel"] for p in returns), [1, 2, 3, 4])
        for p in sends + returns:
            self.assertEqual(p["wLength"], 2)
        self.assertEqual({p["control_selector"] for p in returns}, {7})

    def test_no_master_channel_on_matrix_or_returns(self):
        for p in self.plan:
            if p.get("entity_id") in (5, 7):
                self.assertNotEqual(p["channel"], 0, p)

    def test_effect_unit(self):
        effect = self.select(entity=6)
        got = {p["control_selector"]: (p["channel"], p["wLength"], p["value_kind"])
               for p in effect}
        self.assertEqual(got, {
            1: (0, 2, "low_byte_of_2_bytes"),
            2: (0, 1, "u8"),
            3: (0, 2, "s16le_raw"),
            4: (0, 1, "u8"),
        })
        self.assertEqual(len(effect), 4)

    def test_range_only_for_one_crosspoint(self):
        for req in (GET_MIN, GET_MAX, GET_RES):
            ranges = self.select(req=req)
            self.assertEqual(len(ranges), 1)
            p = ranges[0]
            self.assertEqual((p["wValue"], p["wIndex"], p["wLength"]), ("0x0101", "0x0500", 2))
            self.assertIn("this crosspoint only", p["label"])
        self.assertEqual(self.select(0xA2, req=GET_MIN), [])

    def test_lengths(self):
        lengths = collections.Counter(p["wLength"] for p in self.plan)
        self.assertEqual(lengths, {2: 153, 1: 2, 3: 2})

    def test_rejects_arguments(self):
        proc = run("control-plan", "--output", "x")
        self.assertEqual(proc.returncode, 2)
        self.assertEqual(proc.stdout, "")


if __name__ == "__main__":
    unittest.main()
