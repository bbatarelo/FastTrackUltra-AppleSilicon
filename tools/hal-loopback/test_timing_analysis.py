import json
import unittest
from timing_analysis import analyze_timing, LIMITS, NAMES


class TimingAnalysisTests(unittest.TestCase):
    def snapshot(self):
        s = dict(version=1, epoch=3, valid=1, steady_after=48000, limits=LIMITS)
        for bank in ('all','steady'):
            s[bank] = {}
            for name in NAMES:
                value = 0 if name in ('output_missing','read_missing','submit_clock_error') else 32
                b = next((i for i,limit in enumerate(LIMITS) if value < limit),16)
                buckets = [0]*17; buckets[b] = 20
                s[bank][name] = dict(valid=1,count=20,min=value,max=value,last=value,dropped=0,buckets=buckets)
        return s

    def analyze(self, change=lambda s:None):
        s=self.snapshot();change(s)
        return analyze_timing('stopped status: {"epoch":3}\nstopped timing: '+json.dumps(s))

    def test_legacy_and_complete_observations(self):
        self.assertIsNone(analyze_timing('stopped status: {"epoch":3}'))
        self.assertTrue(self.analyze()['accepted'])

    def test_missing_final_cannot_look_like_no_instrumentation(self):
        self.assertFalse(analyze_timing('running timing: {}')['accepted'])

    def test_reset_schema_and_missing_metric_rejected(self):
        for mutate in (lambda s:s.update(epoch=4), lambda s:s.update(valid=0),
                       lambda s:s.update(version=2),lambda s:s['steady'].pop('read_frames')):
            self.assertFalse(self.analyze(mutate)['accepted'])

    def test_drops_empty_incoherent_and_bad_histogram_rejected(self):
        for change in ({'dropped':1},{'valid':0},{'count':0},{'count':21},{'min':None},{'last':999}):
            self.assertFalse(self.analyze(lambda s:s['steady']['write_frames'].update(change))['accepted'])

    def test_negative_margin_is_preserved_not_called_payload_loss(self):
        def mutate(s):
            m=s['steady']['output_high_water_margin'];m.update(min=-1,max=-1,last=-1,buckets=[20]+[0]*16)
        a=self.analyze(mutate)
        self.assertTrue(a['accepted']);self.assertEqual(a['final']['steady']['output_high_water_margin']['min'],-1)

    def test_steady_loss_and_submission_deadline_rejected(self):
        for name in ('output_missing','read_missing','submit_clock_error'):
            self.assertFalse(self.analyze(lambda s:s['steady'][name].update(max=1))['accepted'])
        self.assertFalse(self.analyze(lambda s:s['steady']['submit_return_lead_ms'].update(min=1))['accepted'])
        self.assertTrue(self.analyze(lambda s:s['all']['output_missing'].update(max=100))['accepted'])


if __name__=='__main__':unittest.main()
