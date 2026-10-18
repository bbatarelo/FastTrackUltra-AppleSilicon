#!/usr/bin/env python3
"""Analyze synthetic Mach-message delivery; never a USB qualification gate."""
import argparse
import collections
import json
import math
from pathlib import Path


def distribution(values):
    if not values:
        return None
    values = sorted(values)
    return {"count": len(values), "min_ms": values[0],
            "p50_ms": values[math.ceil(len(values) * .50) - 1],
            "p99_ms": values[math.ceil(len(values) * .99) - 1],
            "max_ms": values[-1]}


def analyze(s):
    def require(ok, reason):
        if not ok:
            raise ValueError(reason)
    require(s['schema'] == 1 and s['valid'] is True, 'invalid_run')
    require(s['policy'] in ('normal', 'realtime') and type(s['observer']) is bool, 'configuration')
    idle = s.get('idle_seconds', 0)
    require((1 <= s['pairs'] <= 2500 and idle == 0) or (s['pairs'] == 0 and 1 <= idle <= 30), 'pair_count')
    require(all(type(n) is int and n > 0 for n in s['timebase']) and len(s['timebase']) == 2, 'timebase')
    scale = s['timebase'][0] / s['timebase'][1] / 1e6
    require(all(s[k] == 0 for k in ('setup', 'configure', 'restore', 'producer_configure', 'errors', 'observer_errors')), 'reported_error')
    require(s['requested_pump_ns'] == 2000000 and s['producer_interval_ns'] == 8000000, 'timing_configuration')
    require(s['start'] < s['stop'], 'run_order')
    for sample in [s['first'], s['last']] + ([] if idle else [s['producer_policy']]):
        require(sample['cpu_status'] == sample['policy_status'] == 0, 'sample_error')
        require(sample['begin'] <= sample['end'] and sample['cpu_ns'] >= 0, 'sample_order')
    if not idle:
        require(s['producer_policy']['basic'] == 1, 'producer_not_timeshare')
    require(s['first']['end'] <= s['start'] < s['stop'] <= s['last']['begin'], 'sample_run_order')
    if s['policy'] == 'realtime':
        expected = [ns * s['timebase'][1] // s['timebase'][0] for ns in (8000000, 1000000, 2000000)] + [1]
        for sample in (s['first'], s['last']):
            require(sample['default'] == 0 and sample['basic'] == 2 and sample['constraint'] == expected, 'policy_readback')
    else:
        require(s['first']['basic'] == s['last']['basic'] == 1 and s['first']['default'] == s['last']['default'] == 1, 'policy_readback')
    if idle:
        require((s['stop'] - s['start']) * scale >= idle * 1000, 'idle_duration')
    sent = {}
    require(len(s['sent']) == len(s['received']) == 2 * s['pairs'], 'message_count')
    for seq, ep, planned, begin, end, status in s['sent']:
        require(0 <= seq < s['pairs'] and ep in (0, 1) and (seq, ep) not in sent, 'sent_identity')
        require(s['start'] < planned <= begin <= end and status == 0, 'send_order_or_status')
        sent[seq, ep] = (planned, begin, end)
    require(all(sent[n, 1][1] >= sent[n, 0][2] and sent[n, 0][0] == sent[n, 1][0] for n in range(s['pairs'])), 'pair_send_order')
    pumps = {}
    producer_base = min((v[0] for v in sent.values()), default=None)
    last_send = max((v[2] for v in sent.values()), default=None)
    if not idle and 'producer_base' in s:
        require(s['producer_base'] == producer_base, 'producer_base')
    def phase(p):
        if idle:
            return 'idle_only'
        if p[2] <= producer_base:
            return 'before_producer'
        if p[1] < producer_base:
            return 'crosses_producer_start'
        if p[1] > last_send:
            return 'after_last_send'
        return 'active'
    def pump_detail(p):
        previous = pumps.get(p[0] - 1)
        return {'id': p[0], 'phase': phase(p), 'result': p[3], 'callbacks': p[8],
                'entry_since_start_ms': (p[1] - s['start']) * scale,
                'entry_from_producer_ms': (p[1] - producer_base) * scale if producer_base is not None else None,
                'duration_ms': (p[2] - p[1]) * scale,
                'wait_count': p[6] if s['observer'] else None,
                'observed_wait_ms': p[7] * scale if s['observer'] else None,
                'gap_from_previous_pump_ms': (p[1] - previous[2]) * scale if previous else None}
    previous = s['start']
    for row in s['pumps']:
        i, begin, end, result, before, after, waits, wait_ticks, callbacks = row
        require(i == len(pumps) and previous <= begin <= end <= s['stop'], 'pump_order')
        require(result in (3, 4) and callbacks in (0, 1) and (result == 4) == bool(callbacks), 'pump_result')
        require(0 <= wait_ticks <= end - begin and waits >= 0, 'wait_duration')
        if waits:
            require(s['observer'] and begin <= before <= after <= end and wait_ticks >= after - before, 'wait_order')
        else:
            require(before == after == wait_ticks == 0, 'unexpected_wait_fields')
        pumps[i] = row
        previous = end
    received = set()
    per_endpoint = [-1, -1]
    cb_counts = collections.Counter()
    lower, upper, producer, callback_to_return, entry_to_callback = [], [], [], [], []
    details = []
    deadline_exceeded = []
    previous_callback = s['start']
    for seq, ep, now, pump_id in s['received']:
        require((seq, ep) in sent and (seq, ep) not in received and pump_id in pumps, 'received_identity')
        require(seq == per_endpoint[ep] + 1 and now >= previous_callback, 'receive_order')
        received.add((seq, ep)); per_endpoint[ep] = seq; previous_callback = now
        planned, begin, end = sent[seq, ep]
        p = pumps[pump_id]
        require(begin <= now and p[1] <= now <= p[2], 'callback_order')
        lo, hi = max(0, now - end) * scale, (now - begin) * scale
        lower.append(lo); upper.append(hi); producer.append((begin - planned) * scale)
        entry_to_callback.append((now - p[1]) * scale)
        callback_to_return.append((p[2] - now) * scale)
        cb_counts[pump_id] += 1
        details.append({'sequence': seq, 'endpoint': ep, 'pump': pump_id,
                        'send_to_callback_lower_ms': lo, 'send_to_callback_upper_ms': hi,
                        'producer_late_ms': (begin - planned) * scale,
                        'pump_ms': (p[2] - p[1]) * scale,
                        'observed_wait_ms': p[7] * scale if s['observer'] else None,
                        'callback_after_last_wait_ms': (now - p[5]) * scale if p[6] else None,
                        'callback_since_start_ms': (now - s['start']) * scale,
                        'delivery_before_own_pump_ms': max(0, p[1] - begin) * scale,
                        'previous_pump': pump_detail(pumps[pump_id - 1]) if pump_id else None,
                        'current_pump': pump_detail(p)})
        # A 0.1 ms descriptive allowance is not a scheduler gate. This records
        # late handling; it does not prove the timeout worker failed to wake.
        if (now - p[1]) * scale > 2.1:
            deadline_exceeded.append(details[-1])
    require(all(cb_counts[i] == p[8] for i, p in pumps.items()), 'callback_accounting')
    elapsed_ns = ((s['last']['begin'] + s['last']['end']) - (s['first']['begin'] + s['first']['end'])) / 2 * scale * 1e6
    cpu_ns = s['last']['cpu_ns'] - s['first']['cpu_ns']
    require(elapsed_ns > 0 and cpu_ns >= 0, 'cpu_interval')
    require(bool(pumps), 'no_pumps')
    phases = collections.defaultdict(list)
    for p in pumps.values():
        phases[phase(p)].append(p)
    return {'accepted': True, 'analysis_revision': 2, 'policy': s['policy'], 'observer': s['observer'],
            'idle_seconds': idle,
            'messages': len(received), 'pumps': len(pumps),
            'consumer_cpu_percent': 100 * cpu_ns / elapsed_ns,
            'send_to_callback_lower': distribution(lower), 'send_to_callback_upper': distribution(upper),
            'producer_lateness': distribution(producer),
            'callback_to_pump_return': distribution(callback_to_return),
            'pump_entry_to_callback': distribution(entry_to_callback),
            'pumps_all': distribution([(p[2] - p[1]) * scale for p in pumps.values()]),
            'pumps_timed_out': distribution([(p[2] - p[1]) * scale for p in pumps.values() if p[3] == 3]),
            'pumps_handled': distribution([(p[2] - p[1]) * scale for p in pumps.values() if p[3] == 4]),
            'wait_intervals': distribution([p[7] * scale for p in pumps.values() if p[6]]),
            'worst_delivery': max(details, key=lambda x: x['send_to_callback_upper_ms']) if details else None,
            'worst_pump': pump_detail(max(pumps.values(), key=lambda p: p[2] - p[1])),
            'pump_phases': {name: {'count': len(rows),
                'durations': distribution([(p[2] - p[1]) * scale for p in rows]),
                'timeout_count': sum(p[3] == 3 for p in rows),
                'worst': pump_detail(max(rows, key=lambda p: p[2] - p[1]))} for name, rows in phases.items()},
            'timeout_overshoot': distribution([max(0, (p[2] - p[1]) * scale - 2) for p in pumps.values() if p[3] == 3]),
            'handled_after_2_1_ms_count': len(deadline_exceeded),
            'handled_after_2_1_ms_worst': max(deadline_exceeded, key=lambda r: r['pump_ms']) if deadline_exceeded else None,
            'producer_lateness_by_endpoint': {str(ep): distribution([(begin - planned) * scale for (seq, e), (planned, begin, end) in sent.items() if e == ep]) for ep in (0, 1)},
            'endpoint_1_send_after_0_return': distribution([(sent[n, 1][1] - sent[n, 0][2]) * scale for n in range(s['pairs'])]),
            'limitations': 'Synthetic two-port messages, not USB. Send bounds bracket enqueue uncertainty. Wait observations include wakeup scheduling. Endpoint samples do not prove continuous policy. No scheduler bound or instrumentation-overhead claim.'}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('input', type=Path)
    args = p.parse_args()
    try:
        result = analyze(json.loads(args.input.read_text()))
    except (ValueError, KeyError, TypeError, IndexError) as exc:
        result = {'accepted': False, 'reason': str(exc)}
    print(json.dumps(result, indent=2))
    return 0 if result['accepted'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
