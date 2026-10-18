import copy,json,unittest
from worker_analysis import FIELDS,GROUPS,analyze,from_log

def fixture():
    e=dict.fromkeys(FIELDS,0)
    e.update(ordinal=1,direction=1,sequence=4,slot=0,ticket=5,captured=48000,flags=255,start_frame=200,checked_frame=190,returned_frame=191,previous_sequence=0,previous_start_frame=168,previous_last_packet_ticks=100,previous_callback_ticks=120,previous_consumed_ticks=150,feedback_sequence=4,feedback_last_packet_ticks=10,feedback_callback_ticks=20,feedback_consumed_ticks=30,loop_enter_ticks=140,poll_end_ticks=160,pump_enter_ticks=110,pump_exit_ticks=130,check_begin_ticks=170,check_end_ticks=180,prepare_end_ticks=220,submit_begin_ticks=230,submit_end_ticks=250,query_end_ticks=260)
    return dict(version=1,valid=1,epoch=3,timebase_numer=125,timebase_denom=3,queue_depth=4,threshold_ms=16,observed=1,dropped=0,recent_overwritten=0,late_seen=1,late_retained=1,late_overflow=0,worst=[copy.deepcopy(e)],recent=[copy.deepcopy(e)],late=[copy.deepcopy(e)])

class WorkerAnalysisTests(unittest.TestCase):
    def test_units_stages_and_duplicate_bank_records(self):
        a=analyze(fixture(),3);self.assertTrue(a['accepted']);self.assertEqual(len(a['events']),1)
        e=a['events'][0];self.assertEqual(e['lead_ms'],9);self.assertEqual(e['lead_reference'],'returned')
        self.assertAlmostEqual(e['preparation_ms'],40*125/3/1e6);self.assertAlmostEqual(e['submission_ms'],20*125/3/1e6)
        self.assertAlmostEqual(e['previous_consumption_ms'],30*125/3/1e6)
    def test_unavailable_is_not_zero(self):
        s=fixture();s['worst']=[];s['late']=[];s.update(late_seen=0,late_retained=0)
        e=s['recent'][0];e.update(direction=0,sequence=0,ticket=1,flags=4|8|16,captured=0)
        for bit,keys in GROUPS.items():
            if not e['flags']&bit:
                for k in keys:e[k]=None
        a=analyze(s);self.assertTrue(a['accepted'],a);self.assertIsNone(a['events'][0]['previous_consumption_ms']);self.assertEqual(a['events'][0]['lead_reference'],'checked')
    def test_bad_counters_epochs_flags_order_and_records(self):
        for edit in (lambda s:s.update(valid=0),lambda s:s.update(dropped=1),lambda s:s.update(recent_overwritten=1),lambda s:s.update(late_overflow=1),lambda s:s.update(timebase_denom=0),lambda s:s['recent'][0].update(flags=256),lambda s:s['recent'][0].update(ordinal=2),lambda s:s['recent'][0].update(ticket=8),lambda s:s['recent'][0].update(previous_sequence=1),lambda s:s['recent'][0].update(submit_end_ticks=1),lambda s:s['recent'][0].update(returned_frame=None)):
            s=fixture();edit(s);self.assertFalse(analyze(s)['accepted'],s)
        self.assertFalse(analyze(fixture(),4)['accepted'])
    def test_failure_retained_and_unknown_query_not_success(self):
        s=fixture()
        for bank in ('worst','recent','late'):
            e=s[bank][0];e.update(flags=255-32,returned_frame=None,return_clock_status=-4)
        a=analyze(s);self.assertFalse(a['accepted']);self.assertIn('observed_return_clock_status',a['rejections'])
    def test_readback_log_and_empty_handling(self):
        s=fixture();log='status: '+json.dumps(dict(epoch=3))+'\nworker timing: '+json.dumps(s)+'\n'
        self.assertTrue(from_log(log)['accepted']);self.assertFalse(from_log(log+log)['accepted']);self.assertFalse(from_log('')['accepted'])
        self.assertFalse(from_log(log,4)['accepted'])
        self.assertFalse(from_log(log.split('\n',1)[1])['accepted'])
        self.assertFalse(from_log('status: {\"epoch\":3}\n'+log)['accepted'])
        for bank in ('worst','recent','late'):s[bank][0].update(flags=255-32,returned_frame=None)
        self.assertFalse(analyze(s)['accepted'])
        s.update(observed=0,late_seen=0,late_retained=0,recent=[],worst=[],late=[])
        self.assertFalse(analyze(s)['accepted']);self.assertTrue(analyze(s,require_observed=False)['accepted'])

if __name__=='__main__':unittest.main()
