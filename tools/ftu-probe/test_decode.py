"""Offline tests for decode.py.

Every fixture here is SYNTHETIC: hand-built bytes shaped after public
documentation and the Linux quirk hypothesis, not captured from hardware.
No test touches USB devices.
"""

import contextlib
import io
import json
import os
import tempfile
import unittest

import decode


def le16(v):
    return [v & 0xFF, v >> 8]


def le24(v):
    return [v & 0xFF, (v >> 8) & 0xFF, v >> 16]


def desc(dtype, *payload):
    return bytes([2 + len(payload), dtype, *payload])


def configuration(*descs, num_interfaces=1, total_adjust=0):
    body = b"".join(descs)
    total = 9 + len(body) + total_adjust
    return bytes([9, 2, *le16(total), num_interfaces, 1, 0, 0x80, 50]) + body


def interface(num, alt, neps, cls, sub, proto=0):
    return desc(0x04, num, alt, neps, cls, sub, proto, 0)


def endpoint(addr, attrs, mps, interval, audio=False):
    payload = [addr, attrs, *le16(mps), interval]
    if audio:
        payload += [0, 0]
    return desc(0x05, *payload)


def as_general(link=1, tag=0x0001):
    return desc(0x24, 1, link, 1, *le16(tag))


def type_i_discrete(channels, subframe, bits, rates, count=None):
    count = len(rates) if count is None else count
    return desc(0x24, 2, 1, channels, subframe, bits, count, *[b for r in rates for b in le24(r)])


def type_i_continuous(channels, subframe, bits, low, high):
    return desc(0x24, 2, 1, channels, subframe, bits, 0, *le24(low), *le24(high))


# Synthetic fixture modelled on the Linux quirk hypothesis (vendor interface 0,
# interface 1 alt 1 OUT 0x01, interface 2 alt 1 IN 0x81, 8ch S24_3LE). Not real bytes.
SYNTHETIC_HYPOTHESIS = configuration(
    interface(0, 0, 0, 0xFF, 0x00),
    interface(1, 0, 0, 0xFF, 0x02),
    interface(1, 1, 1, 0xFF, 0x02),
    as_general(),
    type_i_discrete(8, 3, 24, [44100, 48000, 88200, 96000]),
    endpoint(0x01, 0x09, 312, 1, audio=True),
    desc(0x25, 1, 0x01, 0, *le16(0)),
    interface(2, 0, 0, 0xFF, 0x02),
    interface(2, 1, 1, 0xFF, 0x02),
    as_general(),
    type_i_discrete(8, 3, 24, [44100, 48000, 88200, 96000]),
    endpoint(0x81, 0x05, 312, 1, audio=True),
    num_interfaces=3,
)


def stream_prefix():
    """Synthetic audio streaming interface; the next descriptor starts at offset 18."""
    return interface(1, 1, 0, 0x01, 0x02)


class StructureErrors(unittest.TestCase):
    def assertDecodeError(self, data, offset, fragment):
        with self.assertRaises(decode.DecodeError) as cm:
            decode.decode(data)
        self.assertEqual(cm.exception.offset, offset, str(cm.exception))
        self.assertIn(fragment, cm.exception.message)

    def test_zero_length_descriptor(self):
        self.assertDecodeError(configuration(bytes([0, 0x41])), 9, "bLength 0")

    def test_one_length_descriptor(self):
        self.assertDecodeError(configuration(bytes([1, 0x41])), 9, "bLength 1")

    def test_truncated_descriptor_header(self):
        self.assertDecodeError(configuration(bytes([7])), 9, "truncated descriptor header")

    def test_truncated_descriptor_body(self):
        data = configuration(bytes([9, 0x04, 0, 0, 0]))
        self.assertDecodeError(data, 9, "extends past end")

    def test_truncated_configuration_header(self):
        self.assertDecodeError(bytes([9, 2, 5, 0, 1]), 0, "truncated configuration header")

    def test_empty_input(self):
        self.assertDecodeError(b"", 0, "truncated configuration header")

    def test_not_a_configuration(self):
        data = bytearray(configuration())
        data[1] = 0x01
        self.assertDecodeError(bytes(data), 1, "expected 0x02")

    def test_short_configuration_blength(self):
        data = bytearray(configuration(desc(0x41, 0, 0)))
        data[0] = 8
        self.assertDecodeError(bytes(data), 0, "less than 9")

    def test_total_length_larger_than_input(self):
        data = configuration(stream_prefix(), total_adjust=4)
        self.assertDecodeError(data, 2, "does not match input length 18")

    def test_total_length_smaller_than_input(self):
        data = configuration(stream_prefix(), total_adjust=-1)
        self.assertDecodeError(data, 2, "wTotalLength 17")

    def test_second_configuration_descriptor(self):
        inner = bytes([9, 2, 9, 0, 1, 1, 0, 0x80, 50])
        self.assertDecodeError(configuration(inner), 9, "unexpected configuration")

    def test_short_interface(self):
        self.assertDecodeError(configuration(bytes([5, 0x04, 0, 0, 0])), 9,
                               "interface descriptor needs bLength >= 9")

    def test_short_endpoint(self):
        data = configuration(stream_prefix(), bytes([6, 0x05, 0x81, 1, 0, 1]))
        self.assertDecodeError(data, 18, "endpoint descriptor needs bLength >= 7")

    def test_short_as_general(self):
        data = configuration(stream_prefix(), desc(0x24, 1, 1, 1))
        self.assertDecodeError(data, 18, "AS_GENERAL descriptor needs bLength >= 7")

    def test_class_specific_without_subtype(self):
        data = configuration(stream_prefix(), bytes([2, 0x24]))
        self.assertDecodeError(data, 18, "bDescriptorSubtype")


class FormatDescriptors(unittest.TestCase):
    def test_discrete_rates_under_vendor_class(self):
        data = configuration(interface(1, 1, 0, 0xFF, 0x02),
                             type_i_discrete(8, 3, 24, [44100, 48000, 88200, 96000]))
        out = decode.decode(data)
        d = out["descriptors"][2]
        self.assertTrue(d["decoded"])
        self.assertEqual(d["kind"], "audio_streaming.FORMAT_TYPE_I")
        self.assertEqual(d["fields"]["bNrChannels"], 8)
        self.assertEqual(d["fields"]["bSubframeSize"], 3)
        self.assertEqual(d["fields"]["bBitResolution"], 24)
        self.assertEqual(d["fields"]["sample_rates"],
                         {"continuous": False, "discrete_hz": [44100, 48000, 88200, 96000]})
        self.assertIn("hypothesis", out["descriptors"][1]["fields"]["kind_basis"])

    def test_discrete_list_truncated(self):
        data = configuration(stream_prefix(),
                             type_i_discrete(8, 3, 24, [44100, 48000, 88200], count=4))
        with self.assertRaises(decode.DecodeError) as cm:
            decode.decode(data)
        self.assertEqual(cm.exception.offset, 18)
        self.assertIn("bSamFreqType 4 needs bLength >= 20, got 17", cm.exception.message)

    def test_discrete_list_exact_bound(self):
        data = configuration(stream_prefix(), type_i_discrete(2, 2, 16, [48000]))
        fields = decode.decode(data)["descriptors"][2]["fields"]
        self.assertEqual(fields["sample_rates"]["discrete_hz"], [48000])
        self.assertNotIn("extra_bytes_not_decoded", fields)

    def test_type_i_header_too_short(self):
        data = configuration(stream_prefix(), desc(0x24, 2, 1, 8, 3, 24))
        with self.assertRaises(decode.DecodeError) as cm:
            decode.decode(data)
        self.assertEqual(cm.exception.offset, 18)

    def test_continuous_rates(self):
        data = configuration(stream_prefix(), type_i_continuous(2, 3, 24, 44100, 96000))
        fields = decode.decode(data)["descriptors"][2]["fields"]
        self.assertEqual(fields["sample_rates"],
                         {"continuous": True, "min_hz": 44100, "max_hz": 96000})

    def test_continuous_truncated(self):
        data = configuration(stream_prefix(), desc(0x24, 2, 1, 2, 3, 24, 0, *le24(44100)))
        with self.assertRaises(decode.DecodeError) as cm:
            decode.decode(data)
        self.assertEqual(cm.exception.offset, 18)
        self.assertIn("needs bLength >= 14", cm.exception.message)

    def test_continuous_inverted_bounds(self):
        data = configuration(stream_prefix(), type_i_continuous(2, 3, 24, 96000, 44100))
        with self.assertRaises(decode.DecodeError) as cm:
            decode.decode(data)
        self.assertEqual(cm.exception.offset, 26)
        self.assertIn("exceeds upper bound", cm.exception.message)

    def test_non_type_i_format_left_raw(self):
        data = configuration(stream_prefix(), desc(0x24, 2, 2, 0, 0, 0, 0))
        d = decode.decode(data)["descriptors"][2]
        self.assertFalse(d["decoded"])
        self.assertEqual(d["bFormatType"], 2)

    def test_audio_control_entity_not_mislabelled_as_format(self):
        # Synthetic AC INPUT_TERMINAL: subtype 2, same number as AS FORMAT_TYPE.
        terminal = desc(0x24, 2, 1, 0x01, 0x01, 0, 2, 0x03, 0x00, 0, 0)
        data = configuration(interface(0, 0, 0, 0x01, 0x01), terminal)
        d = decode.decode(data)["descriptors"][2]
        self.assertFalse(d["decoded"])
        self.assertEqual(d["subtype_name"], "INPUT_TERMINAL")
        self.assertNotIn("fields", d)


class ProtocolGate(unittest.TestCase):
    def test_uac2_protocol_streaming_left_raw(self):
        # Synthetic: valid UAC1-shaped bytes under protocol 0x20 must not be decoded.
        data = configuration(interface(1, 1, 1, 0x01, 0x02, proto=0x20),
                             as_general(),
                             type_i_discrete(2, 3, 24, [48000]),
                             endpoint(0x01, 0x09, 100, 1, audio=True),
                             desc(0x25, 1, 0x01, 0, *le16(0)))
        out = decode.decode(data)
        iface, general, fmt, cs_ep = (out["descriptors"][i] for i in (1, 2, 3, 5))
        self.assertIn("0x20", iface["fields"]["protocol_note"])
        for d in (general, fmt, cs_ep):
            self.assertFalse(d["decoded"])
            self.assertNotIn("fields", d)
            self.assertNotIn("subtype_name", d)
            self.assertIn("only for protocol 0x00", d["note"])
        self.assertNotIn("format", out["interfaces"][0]["alternates"][0])
        self.assertEqual("".join(d["raw_hex"] for d in out["descriptors"]), data.hex())

    def test_vendor_class_other_protocol_left_raw(self):
        data = configuration(interface(0, 0, 0, 0xFF, 0x01, proto=0x01), bytes([3, 0x24, 1]))
        d = decode.decode(data)["descriptors"][2]
        self.assertFalse(d["decoded"])
        self.assertNotIn("subtype_name", d)
        self.assertIn("left raw", d["note"])

    def test_protocol_zero_still_decoded(self):
        data = configuration(interface(1, 1, 0, 0x01, 0x02, proto=0),
                             type_i_discrete(2, 3, 24, [48000]))
        out = decode.decode(data)
        self.assertNotIn("protocol_note", out["descriptors"][1]["fields"])
        self.assertTrue(out["descriptors"][2]["decoded"])


class ContextAndRaw(unittest.TestCase):
    def test_unknown_descriptor_stays_raw(self):
        data = configuration(desc(0x41, 1, 2, 3))
        d = decode.decode(data)["descriptors"][1]
        self.assertFalse(d["decoded"])
        self.assertEqual(d["raw_hex"], "0541010203")
        self.assertEqual(d["offset"], 9)
        self.assertIsNone(d["type_name"])

    def test_class_specific_outside_audio_stays_raw(self):
        data = configuration(interface(0, 0, 0, 0x03, 0x00), desc(0x24, 2, 1, 8, 3, 24, 0))
        d = decode.decode(data)["descriptors"][2]
        self.assertFalse(d["decoded"])
        self.assertIn("left raw", d["note"])

    def test_midi_context(self):
        # Synthetic MIDI IN jack: subtype 2 must not be read as a format descriptor.
        data = configuration(interface(3, 0, 1, 0x01, 0x03),
                             desc(0x24, 2, 1, 1, 0),
                             endpoint(0x02, 0x02, 64, 0, audio=True),
                             desc(0x25, 1, 1, 1))
        out = decode.decode(data)
        jack, ep, cs_ep = out["descriptors"][2], out["descriptors"][3], out["descriptors"][4]
        self.assertEqual(out["interfaces"][0]["alternates"][0]["kind"], "midi_streaming")
        self.assertEqual(jack["subtype_name"], "MIDI_IN_JACK")
        self.assertFalse(jack["decoded"])
        self.assertEqual(ep["fields"]["transfer_type"], "bulk")
        self.assertEqual(cs_ep["subtype_name"], "MS_GENERAL")
        self.assertEqual(cs_ep["endpoint"], "0x02")
        self.assertFalse(cs_ep["decoded"])

    def test_multiple_alternates(self):
        out = decode.decode(SYNTHETIC_HYPOTHESIS)
        self.assertEqual(out["warnings"], [])
        by_num = {i["bInterfaceNumber"]: i["alternates"] for i in out["interfaces"]}
        self.assertEqual(sorted(by_num), [0, 1, 2])
        self.assertEqual([a["bAlternateSetting"] for a in by_num[1]], [0, 1])
        self.assertEqual(by_num[1][0]["endpoints"], [])
        self.assertEqual(by_num[1][1]["endpoints"][0]["bEndpointAddress"], "0x01")
        self.assertEqual(by_num[1][1]["endpoints"][0]["direction"], "OUT")
        self.assertEqual(by_num[2][1]["endpoints"][0]["bEndpointAddress"], "0x81")
        self.assertEqual(by_num[2][1]["format"]["bNrChannels"], 8)
        cs_ep = next(d for d in out["descriptors"] if d.get("kind") == "audio_streaming.EP_GENERAL")
        self.assertEqual(cs_ep["endpoint"], "0x01")
        self.assertEqual(cs_ep["interface_context"],
                         {"bInterfaceNumber": 1, "bAlternateSetting": 1})

    def test_lossless(self):
        out = decode.decode(SYNTHETIC_HYPOTHESIS)
        joined = "".join(d["raw_hex"] for d in out["descriptors"])
        self.assertEqual(joined, SYNTHETIC_HYPOTHESIS.hex())
        offsets = [d["offset"] for d in out["descriptors"]]
        lengths = [d["bLength"] for d in out["descriptors"]]
        self.assertEqual(offsets[1:], [o + l for o, l in zip(offsets, lengths)][:-1])

    def test_endpoint_count_mismatch_warns(self):
        data = configuration(interface(1, 1, 2, 0x01, 0x02), endpoint(0x01, 0x09, 100, 1))
        warnings = decode.decode(data)["warnings"]
        self.assertTrue(any("declares 2 endpoint(s) but 1" in w["message"] for w in warnings))

    def test_duplicate_alternate_warns(self):
        data = configuration(interface(1, 1, 0, 0x01, 0x02), interface(1, 1, 0, 0x01, 0x02))
        warnings = decode.decode(data)["warnings"]
        self.assertTrue(any(w["offset"] == 18 and "duplicate" in w["message"] for w in warnings))


class Endpoints(unittest.TestCase):
    def test_seven_and_nine_byte_endpoints(self):
        data = configuration(interface(1, 1, 2, 0x01, 0x02),
                             endpoint(0x01, 0x09, 100, 1),
                             endpoint(0x82, 0x11, 4, 4, audio=True))
        out = decode.decode(data)
        seven, nine = out["descriptors"][2], out["descriptors"][3]
        self.assertEqual(seven["bLength"], 7)
        self.assertNotIn("bRefresh", seven["fields"])
        self.assertEqual(seven["fields"]["sync_type"], "adaptive")
        self.assertEqual(seven["fields"]["usage_type"], "data")
        self.assertEqual(nine["bLength"], 9)
        self.assertEqual(nine["fields"]["bRefresh"], 0)
        self.assertEqual(nine["fields"]["bSynchAddress"], "0x00")
        self.assertEqual(nine["fields"]["direction"], "IN")
        self.assertEqual(nine["fields"]["usage_type"], "feedback")

    def test_high_speed_multiplier_and_interval(self):
        mps = (1 << 11) | 312  # two transactions of 312 bytes
        data = configuration(interface(1, 1, 2, 0x01, 0x02),
                             endpoint(0x01, 0x05, mps, 1),
                             endpoint(0x81, 0x05, mps, 4))
        out = decode.decode(data)
        every, eighth = (d["fields"]["high_speed_isochronous"] for d in out["descriptors"][2:4])
        self.assertEqual(every["base_bytes_per_transaction"], 312)
        self.assertEqual(every["transactions_per_microframe"], 2)
        self.assertEqual(every["bytes_per_microframe"], 624)
        self.assertEqual(every["service_interval_microframes"], 1)
        self.assertEqual(every["max_bytes_per_second"], 624 * 8000)
        self.assertEqual(eighth["service_interval_microframes"], 8)
        self.assertEqual(eighth["service_interval_us"], 1000)
        self.assertEqual(eighth["max_bytes_per_second"], 624 * 1000)
        self.assertTrue(any("high-speed" in a for a in out["assumptions"]))

    def test_reserved_multiplier_and_bad_interval_warn(self):
        data = configuration(interface(1, 1, 1, 0x01, 0x02),
                             endpoint(0x01, 0x05, (3 << 11) | 100, 0))
        out = decode.decode(data)
        hs = out["descriptors"][2]["fields"]["high_speed_isochronous"]
        self.assertIsNone(hs["transactions_per_microframe"])
        self.assertIsNone(hs["service_interval_microframes"])
        messages = [w["message"] for w in out["warnings"]]
        self.assertTrue(any("multiplier 3" in m for m in messages))
        self.assertTrue(any("outside 1..16" in m for m in messages))

    def test_bulk_endpoint_has_no_isochronous_figures(self):
        data = configuration(interface(1, 0, 1, 0x01, 0x03), endpoint(0x81, 0x02, 512, 0))
        fields = decode.decode(data)["descriptors"][2]["fields"]
        self.assertNotIn("high_speed_isochronous", fields)


class CommandLine(unittest.TestCase):
    def run_main(self, data):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "config-0.bin")
            with open(path, "wb") as f:
                f.write(data)
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                rc = decode.main([path])
            with open(path, "rb") as f:
                after = f.read()
        return rc, out.getvalue(), err.getvalue(), after

    def test_valid_input_prints_json(self):
        rc, out, err, after = self.run_main(SYNTHETIC_HYPOTHESIS)
        self.assertEqual(rc, 0, err)
        self.assertEqual(json.loads(out)["input_length"], len(SYNTHETIC_HYPOTHESIS))
        self.assertEqual(after, SYNTHETIC_HYPOTHESIS)

    def test_malformed_input_fails_and_file_untouched(self):
        bad = configuration(bytes([0, 0x41]))
        rc, out, err, after = self.run_main(bad)
        self.assertEqual(rc, 1)
        self.assertEqual(out, "")
        self.assertIn("offset 9 (0x0009)", err)
        self.assertEqual(after, bad)


if __name__ == "__main__":
    unittest.main()
