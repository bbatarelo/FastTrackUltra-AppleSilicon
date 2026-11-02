"""Both historical v1 and geometry-aware v2 records must remain interpretable."""
import copy
import unittest
import worker_analysis as worker
import delivery_analysis as delivery
import failure_analysis as failure
from test_worker_analysis import fixture as worker_fixture
from test_delivery_analysis import fixture as delivery_fixture
from test_failure_analysis import fixture as failure_fixture


class TransportAnalysisTests(unittest.TestCase):
    def test_worker_geometry_and_predecessor_use_active_depth(self):
        for ms, depth, history in [(8,4,32),(4,8,64),(4,10,64),(2,16,128),(1,32,256)]:
            s=worker_fixture()
            s.update(version=2,block_ms=ms,entries=8*ms,feedback_slots=history,replay_ms=64,
                     queue_depth=depth,threshold_ms=(depth-2)*ms)
            for bank in ('worst','recent','late'):
                s[bank][0].update(sequence=depth,ticket=depth+1,feedback_sequence=depth)
            self.assertTrue(worker.analyze(s)['accepted'],worker.analyze(s))
            for key,value in [('entries',33),('feedback_slots',16),('replay_ms',32),('queue_depth',33),('block_ms',3)]:
                bad=copy.deepcopy(s);bad[key]=value
                self.assertFalse(worker.analyze(bad)['accepted'],key)
            bad=copy.deepcopy(s);bad['recent'][0]['previous_sequence']=1
            self.assertFalse(worker.analyze(bad)['accepted'])

    def test_delivery_slot_identity_tracks_geometry(self):
        for ms,depth in [(8,4),(4,8),(4,10),(2,16),(1,32)]:
            s=delivery_fixture();s.update(version=2,queue_depth=depth,block_ms=ms,entries=8*ms)
            for bank in ('recent','worst_delivery'):
                for r in s[bank]:r.update(sequence=depth-1,slot=depth-1,ticket=depth)
            self.assertTrue(delivery.analyze(s)['accepted'],delivery.analyze(s))
            for key,value in [('entries',33),('queue_depth',33),('block_ms',3)]:
                bad=copy.deepcopy(s);bad[key]=value
                self.assertFalse(delivery.analyze(bad)['accepted'],key)
            s['recent'][0]['slot']=depth
            self.assertFalse(delivery.analyze(s)['accepted'])

    def test_failure_only_active_packets_are_available(self):
        for ms,depth,history in [(8,4,32),(4,8,64),(4,10,64),(2,16,128),(1,32,256)]:
            s=failure_fixture();n=8*ms
            s.update(version=2,entries=n,queue_depth=depth,block_ms=ms,feedback_slots=history)
            s['requested'][n:]=[0]*(64-n);s['packets'][n:]=[[0]*4 for _ in range(64-n)]
            s['packets'][n-1]=[failure.NOT_DONE,312,0,0]
            r=failure.analyze(s);self.assertTrue(r['evidence_valid'],r)
            self.assertEqual(r['untouched_status_indices'],[n-1])
            if n<64:
                s['packets'][n][1]=312
                self.assertFalse(failure.analyze(s)['evidence_valid'])
