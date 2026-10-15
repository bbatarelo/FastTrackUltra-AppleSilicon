"""Offline evidence audit; supports original files and lossless .gz archives.

Cross-checks packet records against raw slots, compacted PCM and final counters.
Timing statistics describe USB observations, never analogue latency.
"""
import collections
import gzip
import json
import math
from pathlib import Path
import statistics
import sys


def read_file(root, name):
    path = Path(root) / name
    if path.exists():
        return path.read_bytes()
    return gzip.decompress(path.with_name(path.name + ".gz").read_bytes())


def audit(root):
    plan = json.loads(read_file(root, "plan.json"))
    summary = json.loads(read_file(root, "summary.json"))
    rows = [json.loads(x) for x in read_file(root, "observations.jsonl").splitlines()]
    raw = read_file(root, "packets.bin")
    audio = read_file(root, "audio.s24le")
    def check(ok, message):
        if not ok:
            raise ValueError(message)
    check(len(rows) == plan["duration_ms"] * 8, "planned packet count")
    raw_pos = audio_pos = accepted = rejected = unobserved = empty = underruns = short = 0
    counts = collections.Counter()
    status = collections.Counter()
    nonzero = [0] * 8
    peak = [0.0] * 8
    sums = [0.0] * 8
    timestamps, callback_lag = [], []
    scale_us = plan["timebase_numer"] / plan["timebase_denom"] / 1000
    for i, row in enumerate(rows):
        check((row["packet"], row["block"], row["slot"], row["microframe_offset"]) ==
              (i, i // 64, i % 64, i % 64), f"packet {i}: schedule order")
        if summary["scheduling_clock"]:
            check(row["frame_start"] == summary["scheduling_clock"]["first_frame"] + (i // 64) * 8,
                  f"packet {i}: bus frame start")
        actual = row["actual_length"]
        counts[str(actual)] += 1
        status[str(row["packet_status"])] += 1
        trusted = (row["completion_status"] in ("0x00000000", "0xe00002e7") and
                   row["packet_status"] in ("0x00000000", "0xe00002e7") and
                   actual is not None and 0 <= actual <= 312 and actual % 24 == 0 and
                   row["raw_file_offset"] is not None)
        check(row["accepted"] == trusted, f"packet {i}: acceptance")
        check(row["audio_frames"] == (actual // 24 if trusted else 0), f"packet {i}: frame count")
        check(row["requested_length"] == 312, f"packet {i}: capacity")
        if row["raw_file_offset"] is not None:
            check(row["raw_file_offset"] == raw_pos, f"packet {i}: raw offset")
            check(raw_pos + 312 <= len(raw), f"packet {i}: raw bounds")
            payload = raw[raw_pos:raw_pos + 312]
            raw_pos += 312
        if trusted:
            check(row["audio_file_offset"] == audio_pos, f"packet {i}: PCM offset")
            check(payload[:actual] == audio[audio_pos:audio_pos + actual], f"packet {i}: PCM bytes")
            accepted += 1
            empty += actual == 0
            short += actual < 312
            underruns += row["packet_status"] == "0xe00002e7"
            audio_pos += actual
            for offset in range(0, actual, 3):
                ch = (offset // 3) % 8
                value = int.from_bytes(payload[offset:offset + 3], "little", signed=True) / 8388608
                nonzero[ch] += value != 0
                peak[ch] = max(peak[ch], abs(value))
                sums[ch] += value * value
            stamp = row["usb_timestamp_ticks"]
            if stamp:
                timestamps.append(stamp)
                callback_lag.append((row["callback_ticks"] - stamp) * scale_us)
        elif actual is None:
            unobserved += 1
        else:
            rejected += 1
    expected = {"accepted_packets": accepted, "empty_packets": empty,
                "rejected_packets": rejected, "unobserved_packets": unobserved,
                "short_packets": short, "underrun_packets": underruns,
                "accepted_audio_frames": audio_pos // 24,
                "audio_bytes": audio_pos, "raw_bytes": raw_pos}
    for key, value in expected.items():
        check(summary[key] == value, f"summary mismatch: {key}")
    check(len(raw) == raw_pos and len(audio) == audio_pos, "unaccounted file bytes")
    states = [rows[i] for i in range(0, len(rows), 64)]
    check(summary["submitted_blocks"] == sum(r["submitted"] for r in states), "submitted blocks")
    check(summary["completed_blocks"] == sum(r["completion_status"] is not None for r in states), "completed blocks")
    check(summary["pending_blocks"] == sum(r["submitted"] and r["completion_status"] is None for r in states), "pending blocks")
    frames = audio_pos // 24
    for ch, report in enumerate(summary["channels"]):
        check(report["nonzero_samples"] == nonzero[ch], f"channel {ch + 1}: nonzero")
        check(math.isclose(report["peak"], peak[ch], rel_tol=1e-8, abs_tol=1e-12), f"channel {ch + 1}: peak")
        rms = math.sqrt(sums[ch] / frames) if frames else 0
        check(math.isclose(report["rms"], rms, rel_tol=1e-8, abs_tol=1e-12), f"channel {ch + 1}: RMS")
    delta = [(b - a) * scale_us for a, b in zip(timestamps, timestamps[1:])]
    positive = [x for x in delta if x > 0]
    groups = [rows[i:i + 8] for i in range(0, len(rows), 8)]
    observed_groups = [g for g in groups if all(r["accepted"] and r["usb_timestamp_ticks"] for r in g)]
    return {"audit": "passed", "directory": Path(root).name, **expected,
            "actual_length_histogram": dict(counts), "packet_status_histogram": dict(status),
            "audio_frames_per_scheduled_second": frames * 1000 / plan["duration_ms"]
                if accepted == len(rows) and not empty else None,
            "timestamps": {"accepted_nonzero_count": len(timestamps),
                           "equal_adjacent_count": delta.count(0),
                           "backward_count": sum(x < 0 for x in delta),
                           "positive_delta_us_min": min(positive) if positive else None,
                           "positive_delta_us_median": statistics.median(positive) if positive else None,
                           "positive_delta_us_max": max(positive) if positive else None,
                           "complete_usb_ms_groups": len(observed_groups),
                           "groups_with_one_shared_timestamp": sum(len({r['usb_timestamp_ticks'] for r in g}) == 1 for g in observed_groups)},
            "callback_delivery_lag_us": {"min": min(callback_lag) if callback_lag else None,
                                         "max": max(callback_lag) if callback_lag else None},
            "note": "Rate is accepted frame count per scheduled USB second; it is not an independent oscillator or analogue latency measurement."}


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: analyze_capture.py CAPTURE_DIRECTORY")
    try:
        print(json.dumps(audit(sys.argv[1]), indent=2))
    except (ValueError, KeyError, OSError) as error:
        raise SystemExit(f"capture audit failed: {error}")
