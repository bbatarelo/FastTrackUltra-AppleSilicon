"""Validate read-only ftuR v1 diagnostics; no inference that a stream is healthy."""
import json
import sys

COUNT_KEYS = ("checks", "attempts", "ready", "deferrals", "fallbacks", "retries",
              "over_budget", "invalid_time", "max_retry_ns", "last_retry_ns")
HEADER_KEYS = ("version", "valid", "epoch", "policy", "wait_cutoff_bus_frames",
               "lead_gate_bus_frames", "retry_budget_ns", "work_budget_ns",
               "steady_after", "timebase_numer", "timebase_denom", "waiting")

def analyze(s, *, expected_epoch=None):
    def need(condition, name):
        if not condition:
            raise ValueError(name)
    def word(v):
        return type(v) is int and 0 <= v < 2**64
    need(isinstance(s, dict), "shape")
    need(all(word(s.get(k)) for k in HEADER_KEYS), "header")
    need(s["version"] == 1 and s["valid"] == 1, "version_or_torn_read")
    need(s["policy"] in (0, 1) and s["waiting"] in (0, 1), "policy_or_waiting")
    need((s["wait_cutoff_bus_frames"], s["lead_gate_bus_frames"], s["retry_budget_ns"], s["work_budget_ns"], s["steady_after"]) ==
         (26, 16, 8_000_000, 2_000_000, 48000), "changed_policy")
    if expected_epoch is not None:
        need(s["epoch"] == expected_epoch, "epoch_mismatch")
    for bank in ("all", "steady"):
        c = s.get(bank)
        need(isinstance(c, dict) and all(word(c.get(k)) for k in COUNT_KEYS), "counts")
        need(c["attempts"] == c["ready"] + c["deferrals"] + c["fallbacks"], "decision_total")
        need(c["attempts"] <= c["checks"] and c["retries"] <= c["checks"], "check_total")
        need(c["over_budget"] + c["invalid_time"] <= c["retries"], "retry_total")
        measured = c["retries"] - c["invalid_time"]
        need(0 <= c["last_retry_ns"] <= c["max_retry_ns"], "duration_order")
        need(measured > 0 or c["max_retry_ns"] == 0, "empty_duration")
        need(bool(c["over_budget"]) == (c["max_retry_ns"] > s["retry_budget_ns"]), "budget_extrema")
        if measured:
            need(s["timebase_numer"] > 0 and s["timebase_denom"] > 0, "invalid_timebase")
    a, t = s["all"], s["steady"]
    need(a["deferrals"] == a["retries"] + s["waiting"], "waiting_balance")
    # Retry belongs to the observation bank at resume, even if its deferral was
    # during startup; steady retries can therefore exceed steady deferrals.
    need(all(t[k] <= a[k] for k in COUNT_KEYS if k != "last_retry_ns"), "steady_subset")
    if s["policy"] == 0:
        need(not any(a.values()) and not s["waiting"], "eager_has_retry_activity")
    return dict(epoch=s["epoch"], policy=s["policy"], observed=a["checks"] > 0,
                all=a.copy(), steady=t.copy(), waiting=bool(s["waiting"]),
                retry_budget_exceeded=a["over_budget"] > 0,
                timing_incomplete=a["invalid_time"] > 0,
                audio_health="not_determined_by_retry_counters")

if __name__ == "__main__":
    with open(sys.argv[1]) as stream:
        print(json.dumps(analyze(json.load(stream)), indent=2))
