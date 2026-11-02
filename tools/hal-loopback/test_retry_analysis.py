import unittest
from retry_analysis import analyze, COUNT_KEYS


def fixture():
    return dict(version=1, valid=1, epoch=3, policy=1, wait_cutoff_bus_frames=26,
                lead_gate_bus_frames=16, retry_budget_ns=8_000_000, work_budget_ns=2_000_000,
                steady_after=48000, timebase_numer=125, timebase_denom=3, waiting=0,
                all=dict.fromkeys(COUNT_KEYS,0), steady=dict.fromkeys(COUNT_KEYS,0))

class RetryTests(unittest.TestCase):
    def test_empty_is_not_a_health_or_time_measurement(self):
        r=analyze(fixture(),expected_epoch=3)
        self.assertFalse(r["observed"])
        self.assertEqual(r["audio_health"],"not_determined_by_retry_counters")

    def test_startup_deferral_can_resume_in_steady_bank(self):
        s=fixture();s["all"].update(checks=2,attempts=2,deferrals=1,ready=1,retries=1,max_retry_ns=9_000_000,last_retry_ns=9_000_000,over_budget=1)
        s["steady"].update(checks=1,attempts=1,ready=1,retries=1,max_retry_ns=9_000_000,last_retry_ns=9_000_000,over_budget=1)
        self.assertTrue(analyze(s)["retry_budget_exceeded"])

    def test_fault_before_stage_is_still_a_retry(self):
        s=fixture();s["all"].update(checks=2,attempts=1,deferrals=1,retries=1,invalid_time=1)
        self.assertTrue(analyze(s)["timing_incomplete"])

    def test_epoch_torn_shape_and_policy_reject(self):
        for key,value in [("valid",0),("version",2),("policy",3),("waiting",2),("epoch",True),("lead_gate_bus_frames",15)]:
            s=fixture();s[key]=value
            with self.assertRaises(ValueError):analyze(s)
        with self.assertRaises(ValueError):analyze(fixture(),expected_epoch=4)

    def test_inconsistent_counts_and_bounds_reject(self):
        for key,value in [("attempts",1),("retries",1),("over_budget",1),("last_retry_ns",1),("checks",-1),("checks",2**64)]:
            s=fixture();s["all"][key]=value
            with self.assertRaises(ValueError):analyze(s)
        s=fixture();s["waiting"]=1
        with self.assertRaises(ValueError):analyze(s)

    def test_fallback_does_not_mean_loss_and_waiting_is_retained(self):
        s=fixture();s["all"].update(checks=2,attempts=2,deferrals=1,fallbacks=1);s["waiting"]=1
        self.assertTrue(analyze(s)["waiting"])
        for key in ("playback_missing","capture_missing"):
            self.assertNotIn(key,analyze(s))
        s["policy"]=0
        with self.assertRaises(ValueError):analyze(s)
