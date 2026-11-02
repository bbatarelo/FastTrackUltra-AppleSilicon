import copy
import json
import os
from pathlib import Path
import subprocess
import unittest
from pumps_analysis import analyze, from_log, partition_ticks, LIMITS


def fixture():
    s = dict(version=1,valid=1,epoch=17,timebase_numer=1,timebase_denom=1,steady_after=48000,
             calls=2,unavailable=0,invalid=0,dropped=0,overflow=0,id_discontinuities=0,link_misses=0,
             bucket_upper_ns=LIMITS,counts=[[0]*12 for _ in range(3)],
             metrics=[[[0]*21 for _ in range(6)] for _ in range(3)],
             worst=[[[0]*26 for _ in range(6)] for _ in range(3)],
             recent=[[0]*13 for _ in range(32)],links=[[0]*30 for _ in range(4)])
    first=[1,1,48000,1,1000,9001000,3,0,0,0,0,0,0]
    second=[2,1,48384,2,9002000,12102000,4,0,0,1,12002000,12002000,0]
    s["recent"][:2]=[first,second]
    s["counts"][1]=[2,2,0,0,0,0,1,1,1,1,0,1]
    def metric(m,values,r,prev):
        h=s["metrics"][1][m]
        h[:3]=[len(values),min(values),max(values)]
        for v in values:
            i=next((i for i,t in enumerate(LIMITS) if v<t),17)
            h[3+i]+=1
        s["worst"][1][m]=r+prev
    for m,vals,r,prev in [(0,[9000000,3100000],first,[0]*13),
                          (1,[9000000],first,[0]*13),(2,[7000000],first,[0]*13),
                          (3,[3000000],second,first),(4,[100000],second,first),
                          (5,[1000],second,first)]:
        metric(m,vals,r,prev)
    for i in range(4):
        s["links"][i]=[1,12+i,20+i,7 if i<2 else 16]+second+first
    return s


class PumpTests(unittest.TestCase):
    def test_missing_previous_context_stays_uncovered(self):
        current=[1,1,48000,1,100,120,4,0,0,1,110,110,0]
        self.assertEqual(partition_ticks(90,110,current,[0]*13),[0,0,10,10])

    def test_accepts_literal_late_callback_without_claiming_cause(self):
        result=analyze(fixture(),17)
        self.assertTrue(result["accepted"],result)
        self.assertEqual(result["phases"]["steady"]["metrics"]["between_pumps"]["max_ns"],1000)
    def test_rejects_missing_timeout_in_denominator(self):
        s=fixture();s["counts"][1][6]=0
        self.assertFalse(analyze(s)["accepted"])
    def test_rejects_histogram_extrema_or_total(self):
        for idx in (0,1,2,3):
            s=fixture();s["metrics"][1][0][idx] = s["metrics"][1][0][2]+1 if idx==1 else s["metrics"][1][0][idx]+1
            self.assertFalse(analyze(s)["accepted"])
    def test_rejects_joining_unrelated_previous_pump(self):
        s=fixture();s["worst"][1][3][16]=99
        self.assertFalse(analyze(s)["accepted"])
    def test_rejects_latest_pump_replacing_worst_context(self):
        s=fixture();s["worst"][1][1]=s["worst"][1][3][:]
        self.assertFalse(analyze(s)["accepted"])
    def test_rejects_mixed_epoch_and_lost_observations(self):
        self.assertFalse(analyze(fixture(),18)["accepted"])
        for key in ("unavailable","invalid","dropped","overflow","id_discontinuities","link_misses"):
            s=fixture();s[key]=1
            self.assertFalse(analyze(s)["accepted"])
    def test_rejects_bad_shapes_and_boolean_words(self):
        for key in ("recent","counts","metrics","worst","links"):
            s=fixture();s[key]=[]
            self.assertFalse(analyze(s)["accepted"])
        s=fixture();s["counts"][1][0]=True
        self.assertFalse(analyze(s)["accepted"])
    def test_requires_links_for_hardware_decision(self):
        s=fixture();s["links"][0]=[0]*30
        self.assertFalse(analyze(s)["accepted"])
    def test_delivery_correlation_uses_exact_ids_and_ticks(self):
        s=fixture()
        d=dict(epoch=17,worst_delivery=[dict(direction=0,sequence=12,ticket=20,callback_ticks=12002000,
               last_packet_ticks=12001993,pump_id=2,pump_enter_ticks=9002000,pump_exit_ticks=12102000,pump_result=4)],
               worst_lead=[])
        self.assertTrue(analyze(s,delivery=d)["accepted"])
        d["worst_delivery"][0]["sequence"]+=1
        self.assertFalse(analyze(s,delivery=d)["accepted"])
    def test_stopped_log_requires_exact_build_and_clean_epoch(self):
        state=dict(phase=0,pending=[0,0],fault=0,reason=0,cleanup=0,epoch=17)
        log="build: dev-test\nstatus: "+json.dumps(state)+"\npumps: "+json.dumps(fixture())
        self.assertTrue(from_log(log,"dev-test")["accepted"])
        self.assertFalse(from_log(log,"wrong")["accepted"])
        self.assertFalse(from_log(log+"\npumps: {}","dev-test")["accepted"])
    def test_native_formatter_round_trip(self):
        binary=Path(__file__).resolve().parents[2]/"build/test-hal-boundary"
        if not binary.exists():
            self.skipTest("build test-hal first")
        result=subprocess.run([str(binary)],env=dict(os.environ,FTU_TEST_PUMPS_JSON="1"),capture_output=True,text=True,check=True)
        snapshots=[json.loads(line) for line in result.stdout.splitlines() if line.startswith('{"version"')]
        self.assertEqual(len(snapshots),2)
        for s in snapshots:
            self.assertTrue(analyze(s,require_observed=False)["accepted"],analyze(s,require_observed=False))
        self.assertEqual(snapshots[1]["calls"],1)


if __name__=="__main__":
    unittest.main()
