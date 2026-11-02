#!/usr/bin/env python3
"""Validate ftuP v1, then summarize measurements without attributing OS causality."""
import argparse
import json
from pathlib import Path
from delivery_analysis import LIMITS, from_log as delivery_from_log

PHASES = ("startup", "steady", "drain")
METRICS = ("duration", "empty_timeout_duration", "empty_timeout_overshoot",
           "entry_to_first_callback", "last_callback_to_return", "between_pumps")
HEADER = ("version valid epoch timebase_numer timebase_denom steady_after calls "
          "unavailable invalid dropped overflow id_discontinuities link_misses").split()


def partition_ticks(a, b, current, previous):
    """Partition one exact packet-to-callback interval; missing context stays unknown."""
    def overlap(x, y):
        return max(0, min(b, y)-max(a, x))
    parts = [overlap(previous[4], previous[5]) if previous[0] else 0,
             overlap(previous[5], current[4]) if previous[0] else 0,
             overlap(current[4], b)]
    if b < a or sum(parts) > b-a:
        raise ValueError("window_partition")
    return parts+[b-a-sum(parts)]


def analyze(s, expected_epoch=None, require_observed=True, delivery=None):
    def need(ok, why):
        if not ok:
            raise ValueError(why)
    def uint(n):
        return type(n) is int and 0 <= n < 2**64
    def words(v, n):
        return isinstance(v, list) and len(v) == n and all(uint(x) for x in v)
    try:
        need(isinstance(s, dict) and all(uint(s[k]) for k in HEADER), "header")
        need(s["version"] == 1 and s["valid"] == 1, "unsupported_or_incoherent")
        need(s["steady_after"] == 48000 and s["bucket_upper_ns"] == LIMITS, "geometry")
        need(expected_epoch is None or expected_epoch == s["epoch"], "epoch_mismatch")
        need(not any(s[k] for k in HEADER[7:]), "telemetry_loss_or_invalid")
        if s["calls"]:
            need(s["timebase_numer"] > 0 and s["timebase_denom"] > 0, "timebase")
        def ns(a, b):
            need(b >= a and s["timebase_denom"] > 0, "tick_order")
            return (b-a)*s["timebase_numer"]//s["timebase_denom"]
        def record(r):
            need(words(r, 13), "record_shape")
            if not r[0]:
                need(not any(r), "nonempty_absent_record")
                return False
            need(r[0] <= s["calls"] and r[1] in (0, 1, 2) and r[3] == r[0], "record_identity")
            need(r[4] > 0 and r[5] >= r[4] and r[6] in (1, 2, 3, 4), "pump_order")
            need(r[7] <= r[8] <= r[4] and r[12] == 0, "sample_or_invalid_callback")
            need((r[2] < 48000 if r[1] == 0 else r[2] >= 48000 or r[1] == 2), "record_phase")
            if r[9]:
                need(r[4] <= r[10] <= r[11] <= r[5], "callback_order")
                need(r[9] != 1 or r[10] == r[11], "single_callback")
            else:
                need(r[10] == r[11] == 0, "empty_callback")
            return True
        def pair(v):
            need(words(v, 26), "pair_shape")
            r, prev = v[:13], v[13:]
            if record(r):
                if r[0] > 1:
                    need(record(prev), "missing_previous")
                    need(prev[0]+1 == r[0] and prev[3]+1 == r[3]
                         and prev[5] <= r[4], "previous_identity")
                else:
                    need(not record(prev), "first_has_previous")
            else:
                need(not record(prev), "absent_pair")
            return r, prev
        need(len(s["counts"]) == len(s["metrics"]) == len(s["worst"]) == 3, "phase_shape")
        need(sum(c[0] for c in s["counts"]) == s["calls"], "call_total")
        summary = {}
        for p, name in enumerate(PHASES):
            c = s["counts"][p]
            need(words(c, 12) and c[0] == c[1] and c[2] == c[3] == 0, "counts")
            need(sum(c[4:8]) == c[1] and c[9] <= c[1] and c[10] <= c[1]-c[9], "result_total")
            need(c[8] >= c[1]-c[9]+c[10] and c[11] <= c[1]-c[9], "callback_total")
            need(len(s["metrics"][p]) == len(s["worst"][p]) == 6, "metric_shape")
            result = {}
            for m, label in enumerate(METRICS):
                h = s["metrics"][p][m]
                need(words(h, 21) and sum(h[3:]) == h[0], "histogram_total")
                r, prev = pair(s["worst"][p][m])
                if not h[0]:
                    need(not any(h) and not any(r), "empty_metric")
                    result[label] = {"count": 0, "min_ns": None, "max_ns": None}
                    continue
                need(h[1] <= h[2] and r[0] and r[1] == p, "extrema")
                lo = next((i for i, t in enumerate(LIMITS) if h[1] < t), 17)
                hi = next((i for i, t in enumerate(LIMITS) if h[2] < t), 17)
                need(h[3+lo] and h[3+hi] and not any(h[3:3+lo]+h[4+hi:]), "extrema_buckets")
                if m in (0, 1, 2):
                    value = ns(r[4], r[5])
                    if m in (1, 2):
                        need(r[6] == 3 and not r[9], "timeout_context")
                    if m == 2:
                        value = max(0, value-2_000_000)
                elif m == 3:
                    need(r[9] > 0, "entry_context")
                    value = ns(r[4], r[10])
                elif m == 4:
                    need(r[9] > 0, "tail_context")
                    value = ns(r[11], r[5])
                else:
                    need(prev[0] and prev[1] == p, "gap_context")
                    value = ns(prev[5], r[4])
                need(value == h[2], "worst_metric_disagrees")
                result[label] = dict(count=h[0], min_ns=h[1], max_ns=h[2],
                                     at_least_4ms=sum(h[3+LIMITS.index(4_000_000)+1:]),
                                     current=r, previous=prev)
            hist = s["metrics"][p]
            need(hist[0][0] == c[1] and hist[1][0] == hist[2][0] <= min(c[6], c[9]), "metric_opportunities")
            need(hist[3][0] == hist[4][0] == c[1]-c[9], "callback_opportunities")
            need(hist[5][0] == max(0, c[1]-1), "gap_opportunities")
            summary[name] = dict(calls=c[0], results=dict(zip(("finished","stopped","timedout","handled"),c[4:8])),
                                 callbacks=c[8], empty=c[9], multiple=c[10],
                                 late_first_callback=c[11], metrics=result)
        need(len(s["recent"]) == 32 and len(s["links"]) == 4, "retention_shape")
        retained = [r[0] for r in s["recent"] if record(r)]
        need(sorted(retained) == list(range(max(1, s["calls"]-31), s["calls"]+1)), "recent_coverage")
        for link in s["links"]:
            need(words(link, 30) and link[0] in (0, 1), "link_shape")
            if link[0]:
                need(pair(link[4:])[0][0] > 0, "missing_link_context")
            else:
                need(not any(link), "absent_link")
        if require_observed:
            need(s["counts"][1][0] > 0 and all(v[0] for v in s["links"]), "no_steady_or_links")
        windows = []
        if delivery is not None:
            need(delivery["epoch"] == s["epoch"], "delivery_epoch_mismatch")
            for d in delivery["worst_delivery"]:
                l = s["links"][d["direction"]]
                r = l[4:17]
                need(l[:4] == [1,d["sequence"],d["ticket"],d["callback_ticks"]-d["last_packet_ticks"]], "delivery_link_identity")
                need(r[3:7] == [d["pump_id"],d["pump_enter_ticks"],d["pump_exit_ticks"],d["pump_result"]]
                     and r[10] <= d["callback_ticks"] <= r[11], "delivery_link_pump")
                prev = l[17:30]
                a, b = d["last_packet_ticks"], d["callback_ticks"]
                parts = partition_ticks(a, b, r, prev)
                windows.append(dict(direction=d["direction"],sequence=d["sequence"],pump_id=r[3],
                    delay_ns=ns(a,b), previous_pump_ns=ns(0,parts[0]), between_pumps_ns=ns(0,parts[1]),
                    current_pump_ns=ns(0,parts[2]), earlier_uncovered_ns=ns(0,parts[3]),
                    previous_was_empty_timeout=prev[6]==3 and prev[9]==0))
            for w in delivery["worst_lead"]:
                e, p = w["event"], w["callback_pump"]
                l = s["links"][2+e["direction"]]
                lead = e["start_frame"]-(e["returned_frame"] if e["flags"] & 32 else e["checked_frame"])
                need(l[:4] == [1,e["sequence"],e["ticket"],lead % 2**64], "lead_link_identity")
                need(p and l[7:11] == [p["id"],p["enter_ticks"],p["exit_ticks"],p["result"]], "lead_link_pump")
        return dict(accepted=True, rejections=[], epoch=s["epoch"], calls=s["calls"],
                    phases=summary, links=s["links"], delivery_windows=windows,
                    scope="Outer pump spans and callback timestamps do not identify kernel message readiness or prove scheduler causality. Audio qualification is separate.")
    except (KeyError, TypeError, ValueError, IndexError, OverflowError) as error:
        return dict(accepted=False, rejections=[str(error)])


def from_log(text, expected_build, require_observed=True, delivery_text=None):
    def one(src, prefix):
        values = [line[len(prefix):] for line in src.splitlines() if line.startswith(prefix)]
        if len(values) != 1:
            raise ValueError("missing_or_ambiguous_"+prefix.strip())
        return values[0]
    try:
        if one(text, "build: ") != expected_build:
            raise ValueError("build_mismatch")
        state = json.loads(one(text, "status: "))
        if state["phase"] != 0 or state["pending"] != [0, 0] or any(state[k] for k in ("fault","reason","cleanup")):
            raise ValueError("not_cleanly_stopped")
        delivery = None
        if delivery_text is not None:
            audit = delivery_from_log(delivery_text, expected_build, require_observed)
            if not audit["accepted"]:
                raise ValueError("delivery_rejected:"+",".join(audit["rejections"]))
            delivery = json.loads(one(delivery_text, "delivery: "))
        return analyze(json.loads(one(text, "pumps: ")), state["epoch"], require_observed, delivery)
    except (KeyError, TypeError, ValueError) as error:
        return dict(accepted=False, rejections=[str(error)])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("--expected-build", required=True)
    parser.add_argument("--delivery-log", type=Path)
    parser.add_argument("--allow-empty", action="store_true")
    args = parser.parse_args()
    result = from_log(args.log.read_text(), args.expected_build, not args.allow_empty,
                      args.delivery_log.read_text() if args.delivery_log else None)
    print(json.dumps(result, indent=2))
    raise SystemExit(not result["accepted"])
