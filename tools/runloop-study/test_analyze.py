import copy
import unittest
from analyze import analyze, distribution


def fixture():
    sample = {'begin': 1, 'end': 2, 'cpu_ns': 0, 'cpu_status': 0, 'policy_status': 0,
              'basic': 1, 'default': 1, 'constraint': [0, 120000, 240000, 1]}
    return {'schema': 1, 'valid': True, 'policy': 'normal', 'observer': True, 'pairs': 1,
            'timebase': [1, 1], 'requested_pump_ns': 2000000, 'producer_interval_ns': 8000000,
            'setup': 0, 'configure': 0, 'restore': 0, 'producer_configure': 0, 'errors': 0, 'observer_errors': 0,
            'start': 3, 'stop': 100, 'first': sample, 'last': dict(sample, begin=101, end=102, cpu_ns=10),
            'producer_policy': copy.deepcopy(sample),
            'sent': [[0, 0, 10, 11, 22, 0], [0, 1, 10, 23, 26, 0]],
            'received': [[0, 0, 20, 0], [0, 1, 28, 1]],
            'pumps': [[0, 3, 21, 4, 4, 19, 1, 15, 1], [1, 22, 30, 4, 24, 27, 1, 3, 1]]}


class AnalysisTests(unittest.TestCase):
    def test_callback_can_precede_send_return(self):
        r = analyze(fixture())
        self.assertEqual(r['send_to_callback_lower']['min_ms'], 0)
        self.assertAlmostEqual(r['send_to_callback_upper']['max_ms'], .000009)

    def test_reject_missing_callback(self):
        s = fixture(); s['received'].pop()
        with self.assertRaisesRegex(ValueError, 'message_count'): analyze(s)

    def test_reject_duplicate_generation(self):
        s = fixture(); s['received'][1] = s['received'][0]
        with self.assertRaisesRegex(ValueError, 'received_identity'): analyze(s)

    def test_reject_wrong_pump(self):
        s = fixture(); s['received'][0][3] = 1
        with self.assertRaisesRegex(ValueError, 'callback_order'): analyze(s)

    def test_reject_unbracketed_wait(self):
        s = fixture(); s['pumps'][0][5] = 25
        with self.assertRaisesRegex(ValueError, 'wait_order'): analyze(s)

    def test_reject_send_error(self):
        s = fixture(); s['sent'][0][-1] = -1
        with self.assertRaisesRegex(ValueError, 'send_order_or_status'): analyze(s)

    def test_reject_default_policy_as_realtime(self):
        s = fixture(); s['policy'] = 'realtime'
        with self.assertRaisesRegex(ValueError, 'policy_readback'): analyze(s)

    def test_plain_run_has_no_wait_observations(self):
        s = fixture(); s['observer'] = False
        for row in s['pumps']: row[4:8] = [0, 0, 0, 0]
        self.assertIsNone(analyze(s)['wait_intervals'])

    def test_nearest_rank_and_empty(self):
        self.assertIsNone(distribution([]))
        self.assertEqual(distribution([4, 1, 2, 3])['p99_ms'], 4)

    def test_realtime_uses_integer_truncation_on_other_timebase(self):
        s = fixture(); s['policy'] = 'realtime'; s['timebase'] = [7, 3]
        for key in ('first', 'last'):
            s[key].update(basic=2, default=0, constraint=[n * 3 // 7 for n in (8000000, 1000000, 2000000)] + [1])
        self.assertTrue(analyze(s)['accepted'])

    def test_timed_out_pump_cannot_claim_callback(self):
        s = fixture(); s['pumps'][0][3] = 3
        with self.assertRaisesRegex(ValueError, 'pump_result'): analyze(s)

    def test_wait_requires_observer(self):
        s = fixture(); s['observer'] = False
        with self.assertRaisesRegex(ValueError, 'wait_order'): analyze(s)

    def test_callback_accounting(self):
        s = fixture(); s['pumps'][0][3] = 3; s['pumps'][0][8] = 0
        with self.assertRaisesRegex(ValueError, 'callback_accounting'): analyze(s)

    def test_multiple_waits_keep_sum_and_last_pair_distinct(self):
        s = fixture(); s['pumps'][0][4:8] = [14, 19, 2, 15]
        self.assertEqual(analyze(s)['pump_phases']['crosses_producer_start']['worst']['wait_count'], 2)

    def test_normal_policy_default_is_checked(self):
        s = fixture(); s['last']['default'] = 0
        with self.assertRaisesRegex(ValueError, 'policy_readback'): analyze(s)

    def test_idle_run_has_no_message_statistics(self):
        s = fixture(); s.update(pairs=0, idle_seconds=1, sent=[], received=[], stop=1000000003)
        s['pumps'] = [[0, 3, s['stop'], 3, 4, s['stop'], 1, s['stop']-4, 0]]
        s['last'].update(begin=s['stop']+1, end=s['stop']+2)
        r = analyze(s)
        self.assertIsNone(r['worst_delivery'])
        self.assertIsNone(r['send_to_callback_upper'])
        self.assertEqual(r['worst_pump']['phase'], 'idle_only')


if __name__ == '__main__':
    unittest.main()
