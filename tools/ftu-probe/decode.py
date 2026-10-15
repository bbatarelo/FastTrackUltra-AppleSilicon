#!/usr/bin/env python3
"""Offline, lossless walker for a saved USB configuration descriptor blob.

Usage: python3 decode.py config-0.bin

The input file is only read. Output is JSON on stdout. Every descriptor keeps
its offset, type, length and raw bytes; only layouts known here are decoded,
everything else stays raw. Structural problems (bad lengths, truncated
descriptors or lists) exit with status 1 and an offset-precise message on
stderr; unusual-but-parseable values are reported as warnings.
"""

import argparse
import json
import sys

MAX_TOTAL_LENGTH = 0xFFFF

DT_CONFIGURATION = 0x02
DT_INTERFACE = 0x04
DT_ENDPOINT = 0x05
DT_CS_INTERFACE = 0x24
DT_CS_ENDPOINT = 0x25

TYPE_NAMES = {
    0x01: "DEVICE",
    0x02: "CONFIGURATION",
    0x03: "STRING",
    0x04: "INTERFACE",
    0x05: "ENDPOINT",
    0x06: "DEVICE_QUALIFIER",
    0x07: "OTHER_SPEED_CONFIGURATION",
    0x08: "INTERFACE_POWER",
    0x09: "OTG",
    0x0A: "DEBUG",
    0x0B: "INTERFACE_ASSOCIATION",
    0x0F: "BOS",
    0x24: "CS_INTERFACE",
    0x25: "CS_ENDPOINT",
}

CLASS_AUDIO = 0x01
CLASS_VENDOR = 0xFF
UAC1_PROTOCOL = 0x00
SUBCLASS_KINDS = {1: "audio_control", 2: "audio_streaming", 3: "midi_streaming"}

AC_SUBTYPES = {
    1: "HEADER",
    2: "INPUT_TERMINAL",
    3: "OUTPUT_TERMINAL",
    4: "MIXER_UNIT",
    5: "SELECTOR_UNIT",
    6: "FEATURE_UNIT",
    7: "PROCESSING_UNIT",
    8: "EXTENSION_UNIT",
}
AS_SUBTYPES = {1: "AS_GENERAL", 2: "FORMAT_TYPE", 3: "FORMAT_SPECIFIC"}
AS_EP_SUBTYPES = {1: "EP_GENERAL"}
MS_SUBTYPES = {1: "MS_HEADER", 2: "MIDI_IN_JACK", 3: "MIDI_OUT_JACK", 4: "ELEMENT"}
MS_EP_SUBTYPES = {1: "MS_GENERAL"}

FORMAT_TAGS = {0x0001: "PCM", 0x0002: "PCM8", 0x0003: "IEEE_FLOAT", 0x0004: "ALAW", 0x0005: "MULAW"}
LOCK_DELAY_UNITS = {0: "undefined", 1: "milliseconds", 2: "decoded PCM samples"}

TRANSFER_TYPES = ("control", "isochronous", "bulk", "interrupt")
SYNC_TYPES = ("none", "asynchronous", "adaptive", "synchronous")
USAGE_TYPES = ("data", "feedback", "implicit_feedback_data", "reserved")

HIGH_SPEED_ASSUMPTION = (
    "isochronous capacity figures assume a high-speed (USB 2.0, 480 Mb/s) link: "
    "125 us microframes, service interval 2^(bInterval-1) microframes; confirm the "
    "negotiated speed in metadata.json"
)
VENDOR_CLASS_ASSUMPTION = (
    "for vendor-specific interfaces (class 0xff) subclasses 1/2/3 are interpreted with "
    "USB Audio Class 1.0 numbering and layouts; this is a hypothesis, not a class guarantee"
)
PROTOCOL_NOTE = (
    "bInterfaceProtocol {} is not 0x00; UAC1 class-specific layouts are decoded only for "
    "protocol 0x00, so this interface's class-specific descriptors are left raw"
)


class DecodeError(Exception):
    def __init__(self, offset, message):
        super().__init__(message)
        self.offset = offset
        self.message = message

    def __str__(self):
        return f"offset {self.offset} (0x{self.offset:04x}): {self.message}"


def u16(b, i):
    return b[i] | (b[i + 1] << 8)


def u24(b, i):
    return b[i] | (b[i + 1] << 8) | (b[i + 2] << 16)


def hex8(v):
    return f"0x{v:02x}"


def hex16(v):
    return f"0x{v:04x}"


def require(raw, offset, minimum, what):
    if len(raw) < minimum:
        raise DecodeError(offset, f"{what} needs bLength >= {minimum}, got {len(raw)}")


def note_extra(fields, raw, known):
    if len(raw) > known:
        fields["extra_bytes_not_decoded"] = len(raw) - known


def decode_configuration(raw, off):
    require(raw, off, 9, "configuration descriptor")
    attrs = raw[7]
    fields = {
        "wTotalLength": u16(raw, 2),
        "bNumInterfaces": raw[4],
        "bConfigurationValue": raw[5],
        "iConfiguration": raw[6],
        "bmAttributes": hex8(attrs),
        "self_powered": bool(attrs & 0x40),
        "remote_wakeup": bool(attrs & 0x20),
        "bMaxPower": raw[8],
        "max_power_mA_usb2": raw[8] * 2,
    }
    note_extra(fields, raw, 9)
    return fields


def decode_interface(raw, off):
    require(raw, off, 9, "interface descriptor")
    cls, sub = raw[5], raw[6]
    kind = SUBCLASS_KINDS.get(sub) if cls in (CLASS_AUDIO, CLASS_VENDOR) else None
    fields = {
        "bInterfaceNumber": raw[2],
        "bAlternateSetting": raw[3],
        "bNumEndpoints": raw[4],
        "bInterfaceClass": hex8(cls),
        "bInterfaceSubClass": hex8(sub),
        "bInterfaceProtocol": hex8(raw[7]),
        "iInterface": raw[8],
        "kind": kind,
    }
    if kind is not None:
        fields["kind_basis"] = (
            "USB Audio Class 1.0 subclass" if cls == CLASS_AUDIO else VENDOR_CLASS_ASSUMPTION
        )
        if raw[7] != UAC1_PROTOCOL:
            fields["protocol_note"] = PROTOCOL_NOTE.format(hex8(raw[7]))
    note_extra(fields, raw, 9)
    return fields


def high_speed_isochronous(mps, interval, off, warn):
    base = mps & 0x7FF
    mult_bits = (mps >> 11) & 0x3
    info = {
        "assumption": "high-speed link",
        "base_bytes_per_transaction": base,
        "additional_transaction_bits": mult_bits,
        "transactions_per_microframe": None,
        "bytes_per_microframe": None,
        "service_interval_microframes": None,
        "service_interval_us": None,
        "max_bytes_per_second": None,
    }
    if mps >> 13:
        warn(off + 4, f"wMaxPacketSize {hex16(mps)} has reserved bits 13-15 set")
    if mult_bits == 3:
        warn(off + 4, f"wMaxPacketSize {hex16(mps)} uses reserved transaction multiplier 3")
    else:
        info["transactions_per_microframe"] = mult_bits + 1
        info["bytes_per_microframe"] = base * (mult_bits + 1)
    if base > 1024:
        warn(off + 4, f"isochronous base packet size {base} exceeds the high-speed limit of 1024")
    if 1 <= interval <= 16:
        period = 1 << (interval - 1)
        info["service_interval_microframes"] = period
        info["service_interval_us"] = period * 125
        if info["bytes_per_microframe"] is not None:
            info["max_bytes_per_second"] = info["bytes_per_microframe"] * 8000 // period
    else:
        warn(off + 6, f"isochronous bInterval {interval} is outside 1..16")
    return info


def decode_endpoint(raw, off, warn):
    require(raw, off, 7, "endpoint descriptor")
    addr, attrs, mps, interval = raw[2], raw[3], u16(raw, 4), raw[6]
    ttype = attrs & 0x3
    fields = {
        "bEndpointAddress": hex8(addr),
        "endpoint_number": addr & 0x0F,
        "direction": "IN" if addr & 0x80 else "OUT",
        "bmAttributes": hex8(attrs),
        "transfer_type": TRANSFER_TYPES[ttype],
        "wMaxPacketSize": hex16(mps),
        "bInterval": interval,
    }
    if addr & 0x70:
        warn(off + 2, f"bEndpointAddress {hex8(addr)} has reserved bits 4-6 set")
    if ttype == 1:
        fields["sync_type"] = SYNC_TYPES[(attrs >> 2) & 0x3]
        fields["usage_type"] = USAGE_TYPES[(attrs >> 4) & 0x3]
    known = 7
    if len(raw) >= 9:
        # Audio-class endpoint descriptors are 9 bytes long.
        fields["bRefresh"] = raw[7]
        fields["bSynchAddress"] = hex8(raw[8])
        known = 9
    note_extra(fields, raw, known)
    if ttype == 1:
        fields["high_speed_isochronous"] = high_speed_isochronous(mps, interval, off, warn)
    return fields


def decode_as_general(raw, off):
    require(raw, off, 7, "AS_GENERAL descriptor")
    tag = u16(raw, 5)
    fields = {
        "bTerminalLink": raw[3],
        "bDelay": raw[4],
        "wFormatTag": hex16(tag),
        "format_tag_name": FORMAT_TAGS.get(tag),
    }
    note_extra(fields, raw, 7)
    return fields


def decode_type_i_format(raw, off, warn):
    """Decodes a FORMAT_TYPE descriptor whose bFormatType is 1 (Type I)."""
    require(raw, off, 8, "Type I FORMAT_TYPE descriptor")
    channels, subframe, bits, count = raw[4], raw[5], raw[6], raw[7]
    fields = {
        "bFormatType": 1,
        "bNrChannels": channels,
        "bSubframeSize": subframe,
        "bBitResolution": bits,
        "bSamFreqType": count,
    }
    if not 1 <= subframe <= 4:
        warn(off + 5, f"bSubframeSize {subframe} is outside 1..4")
    if bits > subframe * 8:
        warn(off + 6, f"bBitResolution {bits} exceeds bSubframeSize {subframe} * 8")
    if count == 0:
        need = 14
        if len(raw) < need:
            raise DecodeError(
                off, f"continuous Type I format needs bLength >= {need} for lower/upper rates, "
                f"got {len(raw)}")
        low, high = u24(raw, 8), u24(raw, 11)
        if low > high:
            raise DecodeError(
                off + 8, f"continuous rate lower bound {low} Hz exceeds upper bound {high} Hz")
        fields["sample_rates"] = {"continuous": True, "min_hz": low, "max_hz": high}
    else:
        need = 8 + 3 * count
        if len(raw) < need:
            raise DecodeError(
                off, f"Type I format with bSamFreqType {count} needs bLength >= {need}, "
                f"got {len(raw)}")
        rates = [u24(raw, 8 + 3 * i) for i in range(count)]
        fields["sample_rates"] = {"continuous": False, "discrete_hz": rates}
    note_extra(fields, raw, need)
    return fields


def decode_as_endpoint_general(raw, off):
    require(raw, off, 7, "audio streaming EP_GENERAL descriptor")
    attrs = raw[3]
    fields = {
        "bmAttributes": hex8(attrs),
        "sampling_frequency_control": bool(attrs & 0x01),
        "pitch_control": bool(attrs & 0x02),
        "max_packets_only": bool(attrs & 0x80),
        "bLockDelayUnits": raw[4],
        "lock_delay_units_name": LOCK_DELAY_UNITS.get(raw[4]),
        "wLockDelay": u16(raw, 5),
    }
    note_extra(fields, raw, 7)
    return fields


def read_subtype(d, raw, off, kind):
    if len(raw) < 3:
        raise DecodeError(
            off, f"class-specific {kind} descriptor needs bLength >= 3 for bDescriptorSubtype, "
            f"got {len(raw)}")
    d["bDescriptorSubtype"] = hex8(raw[2])
    return raw[2]


def non_uac1_protocol(d, current):
    """Leaves d raw with a note unless the interface uses protocol 0x00 (UAC1)."""
    if current["bInterfaceProtocol"] == hex8(UAC1_PROTOCOL):
        return False
    d["note"] = PROTOCOL_NOTE.format(current["bInterfaceProtocol"])
    return True


def decode_cs_interface(d, raw, off, current, warn):
    kind = current["kind"] if current else None
    if kind is None:
        d["note"] = "class-specific descriptor outside a recognised audio/MIDI interface; left raw"
        return
    if non_uac1_protocol(d, current):
        return
    sub = read_subtype(d, raw, off, kind)
    if kind == "audio_control":
        d["subtype_name"] = AC_SUBTYPES.get(sub)
        d["note"] = "audio control entity; fields not decoded in this milestone"
    elif kind == "midi_streaming":
        d["subtype_name"] = MS_SUBTYPES.get(sub)
        d["note"] = "MIDI streaming descriptor; fields not decoded"
    else:
        d["subtype_name"] = AS_SUBTYPES.get(sub)
        if sub == 1:
            fields = decode_as_general(raw, off)
            d.update(decoded=True, kind="audio_streaming.AS_GENERAL", fields=fields)
            current["as_general"] = fields
        elif sub == 2:
            require(raw, off, 4, "FORMAT_TYPE descriptor")
            if raw[3] == 1:
                fields = decode_type_i_format(raw, off, warn)
                d.update(decoded=True, kind="audio_streaming.FORMAT_TYPE_I", fields=fields)
                current["format"] = fields
            else:
                d["bFormatType"] = raw[3]
                d["note"] = "non-Type-I format descriptor; fields not decoded"
        else:
            d["note"] = "audio streaming descriptor subtype not decoded"


def decode_cs_endpoint(d, raw, off, current, endpoint, warn):
    kind = current["kind"] if current else None
    if kind not in ("audio_streaming", "midi_streaming"):
        d["note"] = "class-specific endpoint descriptor outside a streaming interface; left raw"
        return
    if non_uac1_protocol(d, current):
        return
    if endpoint is None:
        warn(off, "class-specific endpoint descriptor without a preceding endpoint in this "
                  "alternate setting")
        d["note"] = "no associated endpoint; left raw"
        return
    sub = read_subtype(d, raw, off, kind)
    d["endpoint"] = endpoint["bEndpointAddress"]
    if kind == "midi_streaming":
        d["subtype_name"] = MS_EP_SUBTYPES.get(sub)
        d["note"] = "MIDI streaming endpoint descriptor; fields not decoded"
    elif sub == 1:
        d["subtype_name"] = AS_EP_SUBTYPES[1]
        d.update(decoded=True, kind="audio_streaming.EP_GENERAL",
                 fields=decode_as_endpoint_general(raw, off))
    else:
        d["subtype_name"] = None
        d["note"] = "audio streaming endpoint descriptor subtype not decoded"


def decode(data):
    data = bytes(data)
    n = len(data)
    if n > MAX_TOTAL_LENGTH:
        raise DecodeError(MAX_TOTAL_LENGTH,
                          f"input exceeds the {MAX_TOTAL_LENGTH}-byte wTotalLength limit")
    if n < 9:
        raise DecodeError(0, f"truncated configuration header: input is {n} byte(s), need 9")
    if data[1] != DT_CONFIGURATION:
        raise DecodeError(1, f"first descriptor type is {hex8(data[1])}, expected 0x02")
    if data[0] < 9:
        raise DecodeError(0, f"configuration bLength {data[0]} is less than 9")
    total = u16(data, 2)
    if total != n:
        raise DecodeError(2, f"wTotalLength {total} does not match input length {n}")

    descriptors = []
    warnings = []
    alternates = []
    seen = set()
    current = None
    endpoint = None

    def warn(offset, message):
        warnings.append({"offset": offset, "message": message})

    off = 0
    while off < n:
        remaining = n - off
        if remaining < 2:
            raise DecodeError(off, f"truncated descriptor header: {remaining} byte remains, need 2")
        length, dtype = data[off], data[off + 1]
        if length < 2:
            raise DecodeError(off, f"bLength {length} is less than 2")
        if length > remaining:
            raise DecodeError(
                off, f"bLength {length} extends past end of input ({remaining} bytes remain)")
        raw = data[off:off + length]
        d = {
            "offset": off,
            "bLength": length,
            "bDescriptorType": hex8(dtype),
            "type_name": TYPE_NAMES.get(dtype),
            "raw_hex": raw.hex(),
            "decoded": False,
        }

        if dtype == DT_CONFIGURATION:
            if off != 0:
                raise DecodeError(off, "unexpected configuration descriptor after offset 0")
            d.update(decoded=True, kind="configuration", fields=decode_configuration(raw, off))
        elif dtype == DT_INTERFACE:
            fields = decode_interface(raw, off)
            key = (fields["bInterfaceNumber"], fields["bAlternateSetting"])
            if key in seen:
                warn(off, f"duplicate interface {key[0]} alternate setting {key[1]}")
            seen.add(key)
            current = {
                "offset": off,
                "bInterfaceNumber": key[0],
                "bAlternateSetting": key[1],
                "bInterfaceClass": fields["bInterfaceClass"],
                "bInterfaceSubClass": fields["bInterfaceSubClass"],
                "bInterfaceProtocol": fields["bInterfaceProtocol"],
                "kind": fields["kind"],
                "bNumEndpoints": fields["bNumEndpoints"],
                "endpoints": [],
                "descriptor_offsets": [],
            }
            alternates.append(current)
            endpoint = None
            d.update(decoded=True, kind="interface", fields=fields)
        elif dtype == DT_ENDPOINT:
            fields = decode_endpoint(raw, off, warn)
            d.update(decoded=True, kind="endpoint", fields=fields)
            if current is None:
                warn(off, "endpoint descriptor before any interface descriptor")
            else:
                current["endpoints"].append({
                    "offset": off,
                    "bEndpointAddress": fields["bEndpointAddress"],
                    "direction": fields["direction"],
                    "transfer_type": fields["transfer_type"],
                    "wMaxPacketSize": fields["wMaxPacketSize"],
                    "bInterval": fields["bInterval"],
                })
            endpoint = fields
        elif dtype == DT_CS_INTERFACE:
            decode_cs_interface(d, raw, off, current, warn)
        elif dtype == DT_CS_ENDPOINT:
            decode_cs_endpoint(d, raw, off, current, endpoint, warn)
        else:
            d["note"] = "not decoded; raw bytes retained"

        if current is not None:
            d["interface_context"] = {
                "bInterfaceNumber": current["bInterfaceNumber"],
                "bAlternateSetting": current["bAlternateSetting"],
            }
            current["descriptor_offsets"].append(off)
        descriptors.append(d)
        off += length

    interfaces = {}
    for alt in alternates:
        interfaces.setdefault(alt["bInterfaceNumber"], []).append(alt)
        if len(alt["endpoints"]) != alt["bNumEndpoints"]:
            warn(alt["offset"], f"interface {alt['bInterfaceNumber']} alternate "
                                f"{alt['bAlternateSetting']} declares {alt['bNumEndpoints']} "
                                f"endpoint(s) but {len(alt['endpoints'])} follow")
    config = descriptors[0]["fields"]
    if len(interfaces) != config["bNumInterfaces"]:
        warn(0, f"bNumInterfaces {config['bNumInterfaces']} but {len(interfaces)} distinct "
                f"interface number(s) present")
    warnings.sort(key=lambda w: w["offset"])

    return {
        "decoder": "decode.py",
        "input_length": n,
        "assumptions": [HIGH_SPEED_ASSUMPTION, VENDOR_CLASS_ASSUMPTION],
        "configuration": config,
        "interfaces": [
            {"bInterfaceNumber": num, "alternates": alts}
            for num, alts in sorted(interfaces.items())
        ],
        "descriptors": descriptors,
        "warnings": warnings,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description="Decode a saved USB configuration descriptor.")
    parser.add_argument("path", help="configuration descriptor blob, e.g. config-0.bin")
    args = parser.parse_args(argv)
    try:
        with open(args.path, "rb") as f:
            # One byte past the limit is enough to detect an oversized input.
            data = f.read(MAX_TOTAL_LENGTH + 1)
    except OSError as e:
        print(f"decode.py: cannot read {args.path}: {e}", file=sys.stderr)
        return 2
    try:
        result = decode(data)
    except DecodeError as e:
        print(f"decode.py: {args.path}: {e}", file=sys.stderr)
        return 1
    result["input_file"] = args.path
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
