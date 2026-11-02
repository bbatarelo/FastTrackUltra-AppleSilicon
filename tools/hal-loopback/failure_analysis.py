#!/usr/bin/env python3
"""Decode ftuF evidence; valid evidence is not an accepted audio run."""
import argparse
import json
from pathlib import Path

HEAD = ("version valid present epoch reason direction sequence ticket slot start_frame checked_frame "
        "returned_frame admission_ticks submit_status completion_status matched callback_ticks activated "
        "drain_failed cleanup_status post_available close_available close_status record_valid submitted_in "
        "submitted_out completed_in completed_out pending_in pending_out finished").split()
SIGNED = {"submit_status", "completion_status", "cleanup_status", "close_status"}
NOT_DONE = 0x6c6c6974
UNDERRUN = -536870169

def signed(n):
    return n - 2**64 if n >= 2**63 else n

def analyze(s):
    def need(ok, why):
        if not ok:
            raise ValueError(why)
    def uint(x):
        return type(x) is int and 0 <= x < 2**64
    def words(v, n):
        return isinstance(v, list) and len(v) == n and all(uint(x) for x in v)
    try:
        need(isinstance(s, dict), "snapshot_type")
        for k in HEAD:
            need(type(s[k]) is int and (-(2**31) <= s[k] < 2**31 if k in SIGNED else uint(s[k])), "header:"+k)
        need(s["version"] in (1, 2) and s["valid"] == 1, "unsupported_or_publishing")
        for k in ("present", "matched", "activated", "drain_failed", "post_available",
                  "close_available", "record_valid", "finished"):
            need(s[k] in (0, 1), "flag:"+k)
        need(len(s["rates"]) == len(s["info"]) == 2, "context_shape")
        need(all(words(v, 12) for v in s["rates"]) and all(words(v, 17) for v in s["info"]), "context_words")
        need(words(s["requested"], 64) and words(s["pump"], 6), "request_shape")
        need(len(s["packets"]) == 64 and all(words(v, 4) for v in s["packets"]), "packet_shape")
        entries,depth=64,4
        if s['version']==2:
            for k in ('entries','queue_depth','block_ms','feedback_slots'):need(uint(s[k]),'geometry:'+k)
            if s['present']:
                entries,depth=s['entries'],s['queue_depth'];ms=s['block_ms']
                need(ms in (1,2,4,8) and entries==ms*8 and 2<=depth<=32,'geometry')
                need(s['feedback_slots']==1<<((64//ms+2*depth+1)-1).bit_length(),'history')
            else:need(not any(s[k] for k in ('entries','queue_depth','block_ms','feedback_slots')),'absent_geometry')
        need(not any(s['requested'][entries:]+sum(s['packets'][entries:],[])),'inactive_packet_data')
        if not s["present"]:
            need(all(s[k] == 0 for k in HEAD[2:]), "nonempty_absent_header")
            need(not any(s["requested"]+s["pump"]+sum(s["packets"], [])+sum(s["rates"], [])+sum(s["info"], [])), "nonempty_absent_arrays")
            return dict(evidence_valid=True, present=False, audio_accepted=False, scope="No retained request/packet validation failure; not proof of healthy audio.")
        need(s["record_valid"] == 1 and s["epoch"] > 0 and s["direction"] in (0, 1) and s["slot"] < depth, "identity")
        need(s["reason"] in (22, 23) and s["ticket"] == s["sequence"]+1 and s["submit_status"] == 0, "failure_identity")
        need(s["start_frame"] > s["checked_frame"], "admission_order")
        need(all(v <= 312 and (v == 312 if s["direction"] == 0 else v % 24 == 0) for v in s["requested"][:entries]), "requested_lengths")
        need(all(p[1] <= 65535 and p[2] <= 65535 and -(2**31) <= signed(p[0]) < 2**31 for p in s["packets"]), "native_packet_width")
        if not s["finished"]:
            need(not s["post_available"] and not s["close_available"] and not s["cleanup_status"] and not s["close_status"], "primary_has_terminal_fields")
        if not s["post_available"]:
            need(not any(s["rates"][1]+s["info"][1]), "absent_post_context")
        if not s["close_available"]:
            need(s["close_status"] == 0, "absent_close_status")
        packets_available = s["matched"] == 1
        if not packets_available:
            need(not any(sum(s["packets"], [])), "unmatched_packets")
        packet_errors = []
        pending = []
        if packets_available:
            for i, (status, req, actual, ticks) in enumerate(s["packets"][:entries]):
                status = signed(status)
                if status == NOT_DONE:
                    pending.append(i)
                elif status not in (0, UNDERRUN) or req != s["requested"][i] or (
                    not 0 < actual <= 312 or actual % 24 if s["direction"] == 0 else actual != req
                ):
                    packet_errors.append(dict(index=i, status=status, requested=req, actual=actual, ticks=ticks))
        if s["reason"] == 23:
            need(not packets_available or s["completion_status"] not in (0, UNDERRUN), "request_failure_not_explained")
        else:
            need(packets_available and s["completion_status"] in (0, UNDERRUN) and (packet_errors or pending), "packet_failure_not_explained")
        return dict(evidence_valid=True, present=True, audio_accepted=False,
                    epoch=s["epoch"], direction=s["direction"], sequence=s["sequence"],
                    completion_status=s["completion_status"], completion_status_hex=hex(s["completion_status"] & 0xffffffff),
                    reason=s["reason"], packets_available=packets_available,
                    packet_errors=packet_errors, untouched_status_indices=pending,
                    terminal_available=bool(s["finished"]),
                    cleanup_status=s["cleanup_status"] if s["finished"] else None,
                    pending=[s["pending_in"],s["pending_out"]],
                    scope="First validation failure that stopped transport, in poll order. Raw failed-packet lengths are not trusted PCM. Current HAL status may belong to a later epoch.")
    except (KeyError, TypeError, ValueError, IndexError) as e:
        return dict(evidence_valid=False, audio_accepted=False, error=str(e))

def from_log(text, expected_build):
    try:
        def one(prefix):
            rows=[l[len(prefix):] for l in text.splitlines() if l.startswith(prefix)]
            if len(rows)!=1:
                raise ValueError("missing_or_ambiguous:"+prefix)
            return rows[0]
        if one("build: ") != expected_build:
            raise ValueError("build_mismatch")
        s=json.loads(one("first failure: "))
        state=json.loads(one("status: "))
        result=analyze(s)
        if result.get("present") and s["epoch"]>state["epoch"]:
            raise ValueError("record_from_future_epoch")
        result["current_epoch"]=state["epoch"]
        result["same_epoch"]=not result.get("present") or s["epoch"]==state["epoch"]
        return result
    except (KeyError, TypeError, ValueError) as e:
        return dict(evidence_valid=False, audio_accepted=False,error=str(e))

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log",type=Path)
    parser.add_argument("--expected-build",required=True)
    args=parser.parse_args()
    result=from_log(args.log.read_text(),args.expected_build)
    print(json.dumps(result,indent=2))
    raise SystemExit(not result["evidence_valid"])
