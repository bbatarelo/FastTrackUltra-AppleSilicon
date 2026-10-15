#!/usr/bin/env python3
"""Replay bounded IN82 evidence. Numeric interpretations are hypotheses only."""
import argparse
from collections import Counter, defaultdict
import gzip
import json
from pathlib import Path
import statistics

OK = {"0x00000000", "0xe00002e7"}  # success / kIOReturnUnderrun
NOT_DONE = "0x6c6c6974"


def read(path):
    return path.read_bytes() if path.exists() else gzip.decompress(Path(str(path) + ".gz").read_bytes())


def require(condition, message):
    if not condition:
        raise ValueError(message)


def candidates(payload):
    """No sign extension; a clock-rate candidate is distinct from signed PCM."""
    require(len(payload) == 3, "numeric candidates require exactly three bytes")
    return {
        f"{order}_q{fraction}_samples_per_{unit}_hz":
        int.from_bytes(payload, order) * periods / (1 << fraction)
        for order in ("little", "big") for fraction in (14, 16)
        for unit, periods in (("millisecond", 1000), ("microframe", 8000))
    }


def analyze(directory):
    directory = Path(directory)
    summary = json.loads(read(directory / "summary.json"))
    plan = json.loads(read(directory / "plan.json"))
    rows = [json.loads(line) for line in read(directory / "observations.jsonl").splitlines()]
    raw = read(directory / "packets.bin")
    require(summary["mode"] == plan["mode"] == "feedback", "wrong mode")
    require(plan["interface"] == 1 and plan["endpoint"] == 0x82 and plan["packet_capacity"] == 3,
            "unexpected feedback geometry")
    require(summary["interface"] == 1 and summary["endpoint"] == "0x82"
            and summary["packet_capacity"] == 3 and summary["alternate"] == plan["alternate"] == 1,
            "summary geometry")
    require(summary["duration_ms"] == plan["duration_ms"], "duration mismatch")
    # Pipe references are resolved dynamically. Zero means no pipe was selected;
    # the active layout has two pipes, and submission requires a selected one.
    require(summary["pipe_ref"] in (0, 1, 2)
            and (summary["submitted_blocks"] == 0 or summary["pipe_ref"] != 0), "summary pipe")
    require(len(rows) == summary["planned_blocks"] * 64 == plan["duration_ms"] * 8, "slot count")
    counters = Counter()
    lengths, statuses, completions, payloads = Counter(), Counter(), Counter(), Counter()
    groups, blocks = defaultdict(list), defaultdict(list)
    offset = 0
    for index, row in enumerate(rows):
        require((row["packet"], row["block"], row["slot"]) == (index, index // 64, index % 64), "slot ordering")
        require(row["requested_length"] == 3, "wrong request length")
        blocks[row["block"]].append(row)
        actual = row["actual_length"]
        complete = row["completion_status"] is not None and row["callback_pointer_matches"] is True
        observed = complete and row["packet_status"] not in (None, NOT_DONE)
        require((actual is not None) == observed, "actual length observation mismatch")
        if complete:
            require(row["raw_file_offset"] == offset and offset + 3 <= len(raw), "raw slot bounds/stride")
            slot = raw[offset:offset + 3]
            offset += 3
        else:
            require(row["raw_file_offset"] is None, "uncompleted raw slot")
            slot = b""
        valid_status = (row["completion_status"] in OK and row["packet_status"] in OK
                        and row["frame_requested_length"] == 3)
        accepted = observed and valid_status and 0 <= actual <= 3
        kind = ("PENDING" if not observed else "OVERSIZE" if actual > 3 else
                "ERROR" if not valid_status else "EMPTY" if actual == 0 else "OK")
        require(row["accepted"] == accepted and row["classification"] == kind, "acceptance mismatch")
        require(row["audio_frames"] == 0 and row["audio_file_offset"] is None, "feedback treated as audio")
        counters["accepted_packets" if accepted else "rejected_packets" if observed else "unobserved_packets"] += 1
        if observed:
            lengths[actual] += 1
            statuses[row["packet_status"]] += 1
        if accepted:
            counters["empty_packets"] += actual == 0
            counters["short_packets"] += actual < 3
            counters["underrun_packets"] += row["packet_status"] == "0xe00002e7"
            payloads[slot[:actual].hex()] += 1
            require(row["frame_start"] is not None, "missing schedule")
            groups[row["frame_start"] + row["slot"] // 8].append(row["usb_timestamp_ticks"])
    require(offset == len(raw) == summary["raw_bytes"], "raw size mismatch")
    for key in ("accepted_packets", "rejected_packets", "unobserved_packets", "empty_packets", "short_packets", "underrun_packets"):
        require(counters[key] == summary[key], f"summary mismatch: {key}")
    require(summary["audio_bytes"] == summary["accepted_audio_frames"] == 0 and summary["channels"] == [], "feedback audio summary")
    submitted = completed = 0
    for block, entries in blocks.items():
        first = entries[0]
        for field in ("submitted", "submit_status", "completion_status", "callback_ticks", "callback_pointer_matches", "frame_start"):
            require(all(row[field] == first[field] for row in entries), f"block {block} {field}")
        submitted += first["submitted"]
        completed += first["completion_status"] is not None
        if first["completion_status"] is not None:
            completions[first["completion_status"]] += 1
    require(submitted == summary["submitted_blocks"] and completed == summary["completed_blocks"], "request counts")
    require(submitted - completed == summary["pending_blocks"], "callback debt")
    time_scale = summary["timebase_numer"] / summary["timebase_denom"] / 1000
    anchors = [(frame, values[0]) for frame, values in sorted(groups.items()) if len(set(values)) == 1]
    deltas = [(right[1] - left[1]) * time_scale for left, right in zip(anchors, anchors[1:]) if right[0] == left[0] + 1]
    interpretations = [dict(raw_hex=value, count=count, hypotheses=candidates(bytes.fromhex(value)))
                       for value, count in sorted(payloads.items()) if len(value) == 6]
    return {
        "audit": "passed",
        "audit_scope": "Stored packet and request accounting; not experiment success or feedback validity.",
        "experiment_status": summary["status"],
        "experiment_outcome": {key: summary[key] for key in (
            "reason", "exit_code", "pipe_ref", "file_error", "callback_error",
            "drain_failed", "cleanup_error", "cleanup_attempted", "alternate_restored",
            "rate_before", "rate_after")},
        "status_note": "Experiment ok means bounded transport and cleanup succeeded; empty or short payloads may still be accepted.",
        "duration_ms": plan["duration_ms"], "counts": dict(counters),
        "actual_length_histogram": dict(sorted(lengths.items())), "packet_status_histogram": dict(statuses),
        "completion_status_histogram": dict(completions), "accepted_payload_histogram": dict(sorted(payloads.items())),
        "full_payload_packets": sum(entry["count"] for entry in interpretations),
        "distinct_full_payloads": len(interpretations),
        "constant_full_payload": len(interpretations) == 1 if interpretations else None,
        "numeric_hypotheses": interpretations,
        "interpretation_note": "Candidates only; a constant nominal value cannot distinguish live clock tracking from a static/default report. One setting does not validate encoding, freshness or playback use.",
        "usb_millisecond_groups": len(groups),
        "groups_with_eight_entries": sum(len(v) == 8 for v in groups.values()),
        "groups_with_one_timestamp": sum(len(set(v)) == 1 for v in groups.values()),
        "adjacent_group_delta_us": {"count": len(deltas), "min": min(deltas), "median": statistics.median(deltas), "max": max(deltas)} if deltas else None,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    print(json.dumps(analyze(args.directory), indent=2, sort_keys=True))
