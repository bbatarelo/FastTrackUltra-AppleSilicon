import copy
import json
import os
from pathlib import Path
import subprocess
import unittest
from failure_analysis import analyze, from_log, HEAD, NOT_DONE

def fixture():
    s=dict.fromkeys(HEAD,0)
    s.update(version=1,valid=1,present=1,epoch=1,reason=23,ticket=1,start_frame=100,
             checked_frame=84,returned_frame=84,completion_status=-536870168,matched=1,
             record_valid=1,activated=1)
    s.update(rates=[[0]*12 for _ in range(2)],info=[[0]*17 for _ in range(2)],
             requested=[312]*64,packets=[[0,312,144,1000+i] for i in range(64)],pump=[0]*6)
    s["packets"][0][0]=(-536870168) % 2**64
    s["packets"][63]=[NOT_DONE,312,0,0]
    return s

class FailureTests(unittest.TestCase):
    def test_primary_error_is_evidence_not_audio_pass(self):
        r=analyze(fixture())
        self.assertTrue(r["evidence_valid"],r)
        self.assertFalse(r["audio_accepted"])
        self.assertEqual(r["untouched_status_indices"],[63])
        self.assertEqual(r["packet_errors"][0]["index"],0)
        self.assertIsNone(r["cleanup_status"])
    def test_terminal_cleanup_error_is_preserved_not_hidden(self):
        s=fixture();s.update(finished=1,cleanup_status=-1000001,pending_in=1,drain_failed=1)
        r=analyze(s);self.assertTrue(r["evidence_valid"],r)
        self.assertEqual(r["pending"],[1,0])
        self.assertEqual(r["cleanup_status"],-1000001)
    def test_unmatched_has_no_packet_evidence(self):
        s=fixture();s.update(matched=0,completion_status=0,packets=[[0]*4 for _ in range(64)])
        r=analyze(s);self.assertTrue(r["evidence_valid"],r)
        self.assertFalse(r["packets_available"])
    def test_corrupt_shapes_and_identities_rejected(self):
        for k,v in [("valid",0),("ticket",10),("record_valid",0),("reason",24),("direction",2),
                    ("packets",[]),("matched",True),("epoch",0),("start_frame",80),("completion_status",0)]:
            s=fixture();s[k]=v;self.assertFalse(analyze(s)["evidence_valid"],k)
    def test_bad_packet_and_untouched_sentinel_are_distinct(self):
        s=fixture();s.update(reason=22,completion_status=0);s["packets"][0]=[0,312,313,1000]
        r=analyze(s);self.assertTrue(r["evidence_valid"],r)
        self.assertEqual(r["packet_errors"][0]["actual"],313)
    def test_later_live_epoch_does_not_erase_first_failure(self):
        def log(epoch):
            return "build: dev-x\nstatus: "+json.dumps(dict(epoch=epoch))+"\nfirst failure: "+json.dumps(fixture())
        r=from_log(log(2),"dev-x")
        self.assertTrue(r["evidence_valid"]);self.assertFalse(r["same_epoch"])
        self.assertFalse(from_log(log(0),"dev-x")["evidence_valid"])
        self.assertFalse(from_log(log(2),"wrong")["evidence_valid"])
        self.assertFalse(from_log(log(2)+"\nfirst failure: {}","dev-x")["evidence_valid"])
    def test_native_formatter_roundtrip(self):
        binary=Path(__file__).resolve().parents[2]/"build/test-hal-boundary"
        if not binary.exists():self.skipTest("build test-hal first")
        r=subprocess.run([str(binary)],env=dict(os.environ,FTU_TEST_FAILURE_JSON="1"),capture_output=True,text=True,check=True)
        rows=[json.loads(l) for l in r.stdout.splitlines() if l.startswith('{"version"')]
        self.assertEqual(len(rows),3)
        for s in rows:self.assertTrue(analyze(s)["evidence_valid"],analyze(s))
        self.assertFalse(analyze(rows[0])["present"])
        self.assertEqual(analyze(rows[1])["untouched_status_indices"],[63])
        self.assertEqual(analyze(rows[2])["untouched_status_indices"],[31])

if __name__=="__main__":unittest.main()
