import copy
import json
import unittest
from delivery_analysis import analyze, from_log, COUNTERS, LIMITS, THRESHOLDS, METRICS


def fixture():
    s = dict.fromkeys(COUNTERS, 0)
    s.update(version=1, valid=1, epoch=3, timebase_numer=1000, timebase_denom=1,
             steady_after=48000, completed_in=1, completed_out=1, steady_completed_in=1,
             steady_completed_out=1, sample_count=2, sampler_max_ns=10000,
             units='ns', bucket_upper_ns=LIMITS[:], exceed_at_least_ns=THRESHOLDS[:])
    s['metrics'] = [{name: dict(count=0, unavailable=0, invalid=0, overflow=0, sampler_overlaps=0, min_ns=None,
                               max_ns=None, buckets=[0]*18, exceed=[0]*4) for name in METRICS} for _ in range(2)]
    def metric(d, name, ns):
        h = s['metrics'][d][name]
        h.update(count=1, min_ns=ns, max_ns=ns)
        h['buckets'][next((i for i, limit in enumerate(LIMITS) if ns < limit), 17)] = 1
        h['exceed'] = [int(ns >= limit) for limit in THRESHOLDS]
    s['recent'] = []
    for d in range(2):
        r = dict(ordinal=d+1, direction=d, sequence=0, slot=0, ticket=1, captured=48000,
                 last_packet_ticks=100, callback_ticks=1100, consumed_ticks=1110,
                 pump_id=1, pump_enter_ticks=90, pump_exit_ticks=1105, pump_result=4, status=0, matched=1, draining=0, sample_begin_ticks=0, sample_end_ticks=0)
        s['recent'].append(r)
        metric(d, METRICS[0], 1000000)
        metric(d, METRICS[1], 10000)
        metric(d, METRICS[2], 1015000)
    s['worst_delivery'] = copy.deepcopy(s['recent'])
    s['worst_lead'] = []
    def sample(start, cpu):
        return dict(begin_ticks=start, end_ticks=start+10, cpu_user_ns=cpu, cpu_system_ns=0,
                    cpu_status=0, policy_status=0, policy_default=0, period_ticks=8000,
                    computation_ticks=1000, constraint_ticks=2000, preemptible=1, basic_policy=2)
    s['samples'] = [sample(10, 100), sample(1000010, 10000100)]
    return s


class DeliveryTests(unittest.TestCase):
    def test_complete_histograms_exact_threshold_and_cpu_average(self):
        result = analyze(fixture(), 3)
        self.assertTrue(result['accepted'], result)
        self.assertEqual(result['completions'], 2)
        self.assertEqual(result['metrics'][0][METRICS[0]]['exceed'], [1, 0, 0, 0])
        self.assertAlmostEqual(result['average_worker_cpu_percent_estimate'], 1.0)

    def test_rejects_corruption_missing_samples_and_cross_epoch(self):
        changes = [
            lambda s: s.update(valid=0),
            lambda s: s.update(dropped=1),
            lambda s: s.update(overflow=1),
            lambda s: s.update(epoch=4),
            lambda s: s['metrics'][0][METRICS[0]]['buckets'].__setitem__(0, 1),
            lambda s: s['metrics'][0][METRICS[0]]['exceed'].__setitem__(0, 0),
            lambda s: s['metrics'][0][METRICS[0]].update(count=2),
            lambda s: s['recent'][0].update(ticket=2),
            lambda s: s['recent'][0].update(pump_exit_ticks=1099),
            lambda s: s['recent'].clear(),
            lambda s: s['worst_delivery'].clear(),
            lambda s: s['samples'].clear(),
            lambda s: s['samples'][1].update(cpu_user_ns=0),
        ]
        for mutate in changes:
            s = fixture(); mutate(s)
            self.assertFalse(analyze(s, 3)['accepted'], s)

    def test_unavailable_metadata_and_failed_readbacks_are_not_clean(self):
        s = fixture()
        s['unmatched_pump_in'] = 1
        for bank in ('recent', 'worst_delivery'):
            s[bank][0].update(pump_id=None, pump_enter_ticks=None, pump_exit_ticks=None, pump_result=None)
        h = s['metrics'][0]['callback_pump_duration']
        h.update(count=0, unavailable=1, min_ns=None, max_ns=None, buckets=[0]*18, exceed=[0]*4)
        result = analyze(s, 3)
        self.assertIn('unmatched_pump_in', result['rejections'])
        s = fixture(); s['cpu_errors'] = 1
        s['samples'][1].update(cpu_status=5, cpu_user_ns=None, cpu_system_ns=None, basic_policy=None)
        result = analyze(s, 3)
        self.assertFalse(result['accepted'])
        self.assertIsNone(result['average_worker_cpu_percent_estimate'])

    def test_default_policy_is_preserved_as_a_measurement_not_applied_policy(self):
        s = fixture(); s['policy_defaults'] = 2
        for sample in s['samples']:
            sample['policy_default'] = 1
        result = analyze(s)
        self.assertTrue(result['accepted'], result)
        self.assertEqual(result['policy_defaults'], 2)

    def test_drain_records_are_retained_without_replacing_steady_worst(self):
        s = fixture()
        r = copy.deepcopy(s['recent'][0])
        r.update(ordinal=3, sequence=1, slot=1, ticket=2, draining=1, status=-536870212,
                 last_packet_ticks=99999)
        s['recent'].append(r); s['completed_in'] += 1; s['drained_in'] = 1
        self.assertTrue(analyze(s)['accepted'])
        s['metrics'][0][METRICS[0]]['sampler_overlaps'] = 2
        self.assertFalse(analyze(s)['accepted'])

    def test_identity_and_clean_final_stop_are_required(self):
        state = dict(epoch=3, phase=0, pending=[0, 0], fault=0, reason=0, cleanup=0)
        def log():
            return 'build: fixture\nstatus: '+json.dumps(state)+'\ndelivery: '+json.dumps(fixture())+'\n'
        self.assertTrue(from_log(log(), 'fixture')['accepted'])
        self.assertFalse(from_log(log(), 'different')['accepted'])
        state['phase'] = 2
        self.assertFalse(from_log(log(), 'fixture')['accepted'])
        self.assertFalse(from_log(log()+log(), 'fixture')['accepted'])


if __name__ == '__main__':
    unittest.main()
