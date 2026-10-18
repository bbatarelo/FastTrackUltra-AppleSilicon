#!/usr/bin/env python3
"""Offline linked-stage evidence from retained ftuW logs; never accesses audio.

The retained windows are biased selections, not a latency distribution. A
counterfactual queue-horizon screen is arithmetic, not a transport simulation.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'hal-loopback'))
from worker_analysis import from_log

PROFILES = [(8, 4), (8, 3), (4, 4), (4, 6), (4, 7), (4, 8)]


def describe(text, expected_build):
    builds = [line.removeprefix('build: ') for line in text.splitlines()
              if line.startswith('build: ')]
    if builds != [expected_build]:
        raise ValueError('missing_or_wrong_build')
    result = from_log(text)
    if not result['accepted']:
        raise ValueError(','.join(result['rejections']))
    state = json.loads(next(line.split(': ', 1)[1] for line in text.splitlines()
                            if line.startswith('status: ')))
    if state['phase'] != 0 or state['pending'] != [0, 0] or any(
            state[key] for key in ('fault', 'reason', 'cleanup')):
        raise ValueError('not_cleanly_stopped')
    snapshot = json.loads(next(line.split(': ', 1)[1] for line in text.splitlines()
                               if line.startswith('worker timing: ')))
    if snapshot['queue_depth'] != 4:
        raise ValueError('screen_requires_four_slot_eight_ms_source')
    raw = {e['ordinal']: e for bank in ('recent', 'worst', 'late')
           for e in snapshot[bank]}
    scale = snapshot['timebase_numer'] / snapshot['timebase_denom'] / 1e6
    linked = []
    for event in result['events']:
        row = dict(event)
        e = raw[event['ordinal']]
        endpoint = e['query_end_ticks'] if e['query_end_ticks'] is not None else e['submit_end_ticks']
        begin = e['previous_callback_ticks']
        if begin is not None and endpoint is not None and endpoint < begin:
            raise ValueError('negative_linked_local_interval')
        row['callback_to_observation_or_submit_end_ms'] = (
            (endpoint - begin) * scale if begin is not None and endpoint is not None else None)
        linked.append(row)
    # Do not infer a worst event from an arbitrary recent window.
    worst = [e for e in snapshot['worst'] if e['direction'] == 1]
    if len(worst) != 1:
        raise ValueError('missing_worst_out_record')
    worst_row = next(row for row in linked if row['ordinal'] == worst[0]['ordinal'])
    if worst_row['lead_reference'] != 'returned':
        raise ValueError('worst_out_has_no_post_submit_clock')
    return dict(epoch=result['epoch'], observed=result['observed'],
                retained_unique=len(linked), events=linked, worst_out=worst_row)


def horizon_screen(observed_out_lead, block_ms, depth):
    # Preserve the source event's integer-ms quantization. Changing geometry
    # also changes cadence/cost/feedback/ownership, which this proxy omits.
    return observed_out_lead + (depth - 1) * block_ms - 24


def summarize(inputs, expected_build):
    seen_epochs = set()
    for item in inputs:
        if item['epoch'] in seen_epochs:
            raise ValueError('duplicate_epoch_would_double_count_observations')
        seen_epochs.add(item['epoch'])
    if not inputs:
        raise ValueError('no_inputs')
    observed_min = min(item['worst_out']['lead_ms'] for item in inputs)
    return dict(schema=1, build=expected_build, hardware_access=False,
                observed=sum(item['observed'] for item in inputs),
                retained_unique=sum(item['retained_unique'] for item in inputs),
                inputs=inputs, measured_min_out_lead_ms=observed_min,
                geometry_screens=[dict(block_ms=b, depth=d,
                    nominal_recycle_lead_ms=(d - 1) * b,
                    counterfactual_out_lead_ms=horizon_screen(observed_min, b, d))
                    for b, d in PROFILES],
                scope='Biased retained windows; no percentiles, CPU WCET, hardware qualification, or latency prediction. Geometry screens preserve one elapsed-delay assumption, not new-geometry behavior.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--expected-build', required=True)
    parser.add_argument('logs', type=Path, nargs='+')
    args = parser.parse_args()
    inputs = []
    for path in args.logs:
        data = path.read_bytes()
        item = describe(data.decode(), args.expected_build)
        item.update(path=str(path), sha256=hashlib.sha256(data).hexdigest())
        inputs.append(item)
    print(json.dumps(summarize(inputs, args.expected_build), indent=2))


if __name__ == '__main__':
    main()
