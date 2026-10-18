import copy
import json
import unittest
from completion_evidence import describe, horizon_screen, summarize
from test_worker_analysis import fixture


def log(snapshot=None, **status):
    state = dict(epoch=3, phase=0, pending=[0, 0], fault=0, reason=0, cleanup=0)
    state.update(status)
    return ('build: fixture\nstatus: ' + json.dumps(state) + '\nworker timing: '
            + json.dumps(snapshot or fixture()) + '\n')


class EvidenceTests(unittest.TestCase):
    def test_linked_interval_includes_unlabelled_between_stage_time(self):
        result = describe(log(), 'fixture')
        self.assertEqual(result['retained_unique'], 1)
        self.assertAlmostEqual(result['worst_out']['callback_to_observation_or_submit_end_ms'],
                               (260 - 120) * 125 / 3 / 1e6)

    def test_unavailable_predecessor_does_not_turn_into_zero_elapsed(self):
        sample = fixture()
        for bank in ('worst', 'recent', 'late'):
            e = sample[bank][0]
            e['flags'] &= ~1
            for key in ('previous_sequence', 'previous_start_frame', 'previous_last_packet_ticks',
                        'previous_callback_ticks', 'previous_consumed_ticks'):
                e[key] = None
        self.assertIsNone(describe(log(sample), 'fixture')['worst_out']
                          ['callback_to_observation_or_submit_end_ms'])

    def test_wrong_identity_shared_stop_missing_worst_and_drops_rejected(self):
        with self.assertRaises(ValueError): describe(log(), 'other')
        with self.assertRaises(ValueError): describe(log(phase=2, pending=[4, 4]), 'fixture')
        for change in (dict(worst=[]), dict(dropped=1)):
            sample = fixture(); sample.update(change)
            with self.assertRaises(ValueError): describe(log(sample), 'fixture')

    def test_duplicate_epoch_is_not_more_evidence(self):
        item = describe(log(), 'fixture')
        with self.assertRaises(ValueError): summarize([item, copy.deepcopy(item)], 'fixture')

    def test_horizon_counterfactual_is_separate_from_actual_measurement(self):
        self.assertEqual(horizon_screen(17, 8, 4), 17)
        self.assertEqual(horizon_screen(17, 4, 6), 13)
        self.assertEqual(horizon_screen(17, 4, 7), 17)
        self.assertEqual(horizon_screen(17, 4, 8), 21)
        # Negative lead is evidence, not clamped to a misleading zero.
        self.assertEqual(horizon_screen(1, 4, 4), -11)


if __name__ == '__main__': unittest.main()
