import unittest
from transition_analysis import analyze

def status(ticks,epoch=1,phase=2,captured=48000,missing=0,fault=0):
    return dict(kind='status',before_ticks=ticks,after_ticks=ticks+1,data=dict(epoch=epoch,phase=phase,captured=captured,playback_missing=missing,capture_missing=0,fault=fault,reason=0,cleanup=0,pending=[4,4] if phase==2 else [0,0]))
def wrap(*events):return [dict(kind='identity',build='fixture',timebase_numer=1,timebase_denom=1),*events,dict(kind='end')]
class TransitionTests(unittest.TestCase):
    def test_counter_increase_retained_and_marker_correlated(self):
        rows=wrap(status(10),status(100,missing=128))
        result=analyze(rows,[dict(ticks=50,event='record')])
        self.assertFalse(result['accepted']);self.assertEqual(result['counter_changes'][0]['delta'],128)
        self.assertEqual(result['counter_changes'][0]['nearby_markers'][0]['event'],'record')
    def test_epoch_reset_is_separate_from_loss(self):
        result=analyze(wrap(status(10,missing=100),status(100,epoch=2,captured=0)))
        self.assertTrue(result['accepted']);self.assertEqual(result['counter_changes'],[])
        self.assertEqual(result['epochs'],[1,2])
    def test_early_epoch_is_retained_not_erased(self):
        result=analyze(wrap(status(10,captured=0),status(100,captured=8000,missing=128)))
        self.assertTrue(result['accepted']);self.assertEqual(len(result['counter_changes']),1)
        self.assertTrue(result['counter_changes'][0]['early_epoch'])
    def test_fault_regression_order_and_truncation_fail(self):
        cases=[wrap(status(10,fault=1)),wrap(status(10,missing=100),status(100,missing=1)),wrap(status(100),status(10)),wrap(status(10))[:-1]]
        for rows in cases:self.assertFalse(analyze(rows)['accepted'])
