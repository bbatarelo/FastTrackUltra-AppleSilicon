#!/usr/bin/env python3
"""Validate sample-free ftuD observations. This does not qualify audio or attribute OS delays."""
import argparse
import json
from worker_analysis import FIELDS, GROUPS
from pathlib import Path

LIMITS = [1000, 2000, 4000, 8000, 16000, 32000, 64000, 128000, 256000,
          512000, 1000000, 2000000, 4000000, 8000000, 16000000, 32000000, 64000000]
THRESHOLDS = [1000000, 4000000, 8000000, 16000000]
METRICS = ['packet_to_callback', 'callback_to_consumed', 'callback_pump_duration',
           'preparation', 'submission', 'post_submit_observation',
           'callback_to_resubmission', 'consumed_to_admission']
COUNTERS = ['version', 'valid', 'epoch', 'timebase_numer', 'timebase_denom', 'steady_after',
            'completed_in', 'completed_out', 'steady_completed_in', 'steady_completed_out',
            'admissions_in', 'admissions_out', 'steady_admissions_in', 'steady_admissions_out',
            'dropped', 'unmatched_pump_in', 'unmatched_pump_out', 'sample_count', 'cpu_errors',
            'policy_errors', 'policy_defaults', 'sampler_invalid', 'sampler_max_ns', 'overflow', 'drained_in', 'drained_out', 'basic_policy_changes', 'constraint_policy_changes']
RECORD_FIELDS = ['ordinal', 'direction', 'sequence', 'slot', 'ticket', 'captured', 'last_packet_ticks',
                 'callback_ticks', 'consumed_ticks', 'pump_id', 'pump_enter_ticks',
                 'pump_exit_ticks', 'pump_result', 'status', 'matched', 'draining', 'sample_begin_ticks', 'sample_end_ticks']
SAMPLE_FIELDS = ['begin_ticks', 'end_ticks', 'cpu_user_ns', 'cpu_system_ns', 'cpu_status',
                 'policy_status', 'policy_default', 'period_ticks', 'computation_ticks',
                 'constraint_ticks', 'preemptible', 'basic_policy']


def analyze(s, expected_epoch=None, require_observed=True):
    issues = []
    def need(ok, reason):
        if not ok:
            raise ValueError(reason)
    def uint(value):
        return type(value) is int and 0 <= value <= 2**64-1
    def status(value):
        return type(value) is int and -(2**31) <= value < 2**31
    try:
        need(isinstance(s, dict), 'missing_snapshot')
        need(all(uint(s[k]) for k in COUNTERS), 'invalid_header')
        need(s['version'] in (1,2) and s['valid'] == 1, 'unsupported_or_incoherent')
        need(s['steady_after'] == 48000, 'steady_threshold')
        if expected_epoch is not None:
            need(s['epoch'] == expected_epoch, 'epoch_mismatch')
        need(s['units'] == 'ns' and s['bucket_upper_ns'] == LIMITS
             and s['exceed_at_least_ns'] == THRESHOLDS, 'histogram_geometry')
        total = s['completed_in'] + s['completed_out']
        if require_observed:
            need(s['steady_completed_in'] > 0 and s['steady_completed_out'] > 0, 'no_steady_completions')
            need(s['sample_count'] >= 2, 'missing_worker_samples')
        if total or s['sample_count']:
            need(s['timebase_numer'] > 0 and s['timebase_denom'] > 0, 'missing_timebase')
        depth=4
        if s['version']==2:
            geometry=('queue_depth','block_ms','entries')
            need(all(uint(s[k]) for k in geometry),'invalid_geometry_word')
            if total or s['sample_count'] or any(s[k] for k in geometry):
                depth=s['queue_depth']
                need(2<=depth<=32 and s['block_ms'] in (1,2,4,8) and s['entries']==8*s['block_ms'],'invalid_geometry')
        scale = s['timebase_numer'] / s['timebase_denom'] if s['timebase_denom'] else None
        for k in ('dropped', 'overflow', 'sampler_invalid', 'cpu_errors', 'policy_errors'):
            if s[k]:
                issues.append(k)
        for k in ('cpu_errors', 'policy_errors', 'policy_defaults', 'sampler_invalid'):
            need(s[k] <= s['sample_count'], 'sample_count_'+k)
        need(len(s['metrics']) == 2, 'metric_directions')
        for d, direction in enumerate(('in', 'out')):
            need(s['steady_completed_'+direction]+s['drained_'+direction] <= s['completed_'+direction], 'completion_count')
            need(s['steady_admissions_'+direction] <= s['admissions_'+direction], 'admission_count')
            need(s['unmatched_pump_'+direction] <= s['steady_completed_'+direction], 'pump_count')
            if s['unmatched_pump_'+direction]:
                issues.append('unmatched_pump_'+direction)
            metrics = s['metrics'][d]
            need(set(metrics) == set(METRICS), 'metric_names')
            for i, name in enumerate(METRICS):
                h = metrics[name]
                need(set(h) == {'count', 'unavailable', 'invalid', 'overflow', 'min_ns', 'max_ns', 'buckets', 'exceed', 'sampler_overlaps'}, 'histogram_fields')
                need(all(uint(h[k]) for k in ('count', 'unavailable', 'invalid', 'overflow')), 'histogram_counter')
                need(len(h['buckets']) == 18 and all(uint(n) for n in h['buckets']), 'bucket_shape')
                need(len(h['exceed']) == 4 and all(uint(n) for n in h['exceed']), 'exceed_shape')
                need(sum(h['buckets']) == h['count'], 'bucket_total')
                need(uint(h['sampler_overlaps']) and h['sampler_overlaps'] <= h['count'], 'sampler_overlap_count')
                opportunities = s[('steady_completed_' if i < 3 else 'steady_admissions_')+direction]
                need(h['count']+h['unavailable']+h['invalid']+h['overflow'] == opportunities, 'opportunity_total')
                need(h['exceed'] == [sum(h['buckets'][LIMITS.index(t)+1:]) for t in THRESHOLDS], 'exceed_total')
                if h['count']:
                    need(uint(h['min_ns']) and uint(h['max_ns']) and h['min_ns'] <= h['max_ns'], 'extrema')
                    lo = next((i for i, t in enumerate(LIMITS) if h['min_ns'] < t), 17)
                    hi = next((i for i, t in enumerate(LIMITS) if h['max_ns'] < t), 17)
                    need(h['buckets'][lo] > 0 and h['buckets'][hi] > 0
                         and not any(h['buckets'][:lo]+h['buckets'][hi+1:]), 'extrema_buckets')
                else:
                    need(h['min_ns'] is None and h['max_ns'] is None, 'empty_extrema')
                if h['invalid'] or h['overflow']:
                    issues.append(direction+'_'+name+'_invalid')
        seen = {}
        for bank, limit in [('recent', 8), ('worst_delivery', 2)]:
            records = s[bank]
            need(isinstance(records, list) and len(records) <= limit, 'record_bank')
            ordinals = []
            directions = []
            for r in records:
                need(set(r) == set(RECORD_FIELDS), 'record_fields')
                for k in RECORD_FIELDS:
                    if k == 'status':
                        need(status(r[k]), 'record_status')
                    elif k not in ('last_packet_ticks', 'callback_ticks', 'pump_id', 'pump_enter_ticks', 'pump_exit_ticks', 'pump_result'):
                        need(uint(r[k]), 'record_word')
                    else:
                        need(r[k] is None or uint(r[k]), 'optional_record_word')
                need(1 <= r['ordinal'] <= total and r['direction'] in (0, 1), 'record_identity')
                need(r['ticket'] == r['sequence']+1 and r['slot'] == r['sequence'] % depth, 'record_generation')
                need(r['ordinal'] not in seen or seen[r['ordinal']] == r, 'cross_bank_mismatch')
                seen[r['ordinal']] = r
                ordinals.append(r['ordinal']); directions.append(r['direction'])
                need(r['draining'] in (0, 1), 'completion_phase')
                if not r['draining'] and (r['matched'] != 1 or r['status'] not in (0, -536870169)):
                    issues.append('completion_error')
                if r['callback_ticks'] is not None:
                    need(r['callback_ticks'] <= r['consumed_ticks'], 'consumption_order')
                    if r['last_packet_ticks'] is not None and not r['draining']:
                        need(r['last_packet_ticks'] <= r['callback_ticks'], 'packet_order')
                if r['pump_id'] is None:
                    need(all(r[k] is None for k in ('pump_enter_ticks', 'pump_exit_ticks', 'pump_result')), 'unavailable_pump')
                else:
                    need(r['pump_id'] > 0 and r['pump_result'] in (1, 2, 3, 4), 'pump_identity')
                    need(r['pump_enter_ticks'] <= r['callback_ticks'] <= r['pump_exit_ticks'] <= r['consumed_ticks'], 'pump_callback_order')
                if bank == 'worst_delivery':
                    need(not r['draining'] and r['captured'] >= 48000 and r['last_packet_ticks'] is not None, 'worst_steady')
                    expected = s['metrics'][r['direction']]['packet_to_callback']['max_ns']
                    # Integer conversion is done before comparison, as in Rust.
                    actual = (r['callback_ticks']-r['last_packet_ticks'])*s['timebase_numer']//s['timebase_denom']
                    need(actual == expected, 'worst_maximum')
            need(len(set(ordinals)) == len(ordinals), 'duplicate_ordinal')
            if bank == 'recent':
                need(set(ordinals) == set(range(max(1, total-7), total+1)), 'recent_window')
            else:
                expected = {d for d in range(2) if s['metrics'][d]['packet_to_callback']['count']}
                need(set(directions) == expected and len(set(directions)) == len(directions), 'worst_directions')
        need(len(s['worst_lead']) <= 2, 'lead_bank')
        directions = []
        for item in s['worst_lead']:
            e = item['event']; p = item['callback_pump']
            need(set(e) == set(FIELDS), 'lead_fields')
            need(uint(e['flags']) and e['flags'] <= 255, 'lead_flags')
            for bit, keys in GROUPS.items():
                for key in keys:
                    need((e[key] is not None) == bool(e['flags'] & bit), 'lead_availability')
            for key, value in e.items():
                if value is not None:
                    need(status(value) if key.endswith('_status') else uint(value), 'lead_word')
            need(1 <= e['ordinal'] <= s['admissions_in']+s['admissions_out'], 'lead_ordinal')
            need(all(e[key] in (0, None) for key in ('check_status', 'submit_status', 'return_clock_status')), 'lead_status')
            if e['flags'] & 1:
                need(e['previous_sequence']+depth == e['sequence'], 'lead_previous')
            chain = [e[k] for k in FIELDS[23:29] if e[k] is not None]
            need(all(a <= b for a, b in zip(chain, chain[1:])), 'lead_stage_order')

            need(e['direction'] in (0, 1) and e['captured'] >= 48000, 'lead_identity')
            need(e['slot'] == e['sequence'] % depth and e['ticket'] == e['sequence']+1, 'lead_generation')
            directions.append(e['direction'])
            feedback = item['feedback_pump']
            if feedback is not None:
                need(e['direction'] == 1 and e['flags'] & 2 and e['feedback_sequence'] == e['sequence'], 'feedback_identity')
                need(feedback['id'] > 0 and feedback['result'] in (1, 2, 3, 4), 'feedback_pump')
                need(feedback['enter_ticks'] <= e['feedback_callback_ticks'] <= feedback['exit_ticks'] <= e['feedback_consumed_ticks'], 'feedback_pump_order')

            if p is not None:
                need(e['flags'] & 1 and e['previous_sequence']+depth == e['sequence'], 'lead_predecessor')
                need(p['id'] > 0 and p['result'] in (1, 2, 3, 4), 'lead_pump')
                need(p['enter_ticks'] <= e['previous_callback_ticks'] <= p['exit_ticks'] <= e['previous_consumed_ticks'], 'lead_pump_order')
        need(len(set(directions)) == len(directions), 'duplicate_lead_direction')
        need(set(directions) == {d for d, direction in enumerate(('in', 'out')) if s['steady_admissions_'+direction]}, 'missing_lead_direction')
        samples = s['samples']
        need(len(samples) == min(s['sample_count'], 2), 'retained_samples')
        for sample in samples:
            need(set(sample) == set(SAMPLE_FIELDS), 'sample_fields')
            need(status(sample['cpu_status']) and status(sample['policy_status']), 'sample_status')
            need(uint(sample['begin_ticks']) and uint(sample['end_ticks']) and sample['begin_ticks'] <= sample['end_ticks'], 'sample_order')
            for k in ('cpu_user_ns', 'cpu_system_ns', 'basic_policy'):
                need((uint(sample[k]) if sample['cpu_status'] == 0 else sample[k] is None), 'sample_cpu_availability')
            for k in SAMPLE_FIELDS[6:11]:
                need((uint(sample[k]) if sample['policy_status'] == 0 else sample[k] is None), 'sample_policy_availability')
            if sample['policy_status'] == 0:
                need(sample['policy_default'] in (0, 1) and sample['preemptible'] in (0, 1), 'policy_booleans')
        need(sum(sample['cpu_status'] != 0 for sample in samples) <= s['cpu_errors'], 'cpu_error_count')
        need(sum(sample['policy_status'] != 0 for sample in samples) <= s['policy_errors'], 'policy_error_count')
        need(sum(sample['policy_default'] == 1 for sample in samples) <= s['policy_defaults'], 'policy_default_count')
        if scale is not None:
            need(all((sample['end_ticks']-sample['begin_ticks'])*s['timebase_numer']//s['timebase_denom'] <= s['sampler_max_ns'] for sample in samples), 'sampler_maximum')
        cpu_percent = None
        if len(samples) == 2 and not s['cpu_errors'] and not s['sampler_invalid']:
            a, b = samples
            need(b['begin_ticks'] >= a['end_ticks'], 'sample_window')
            need(b['cpu_user_ns'] >= a['cpu_user_ns'] and b['cpu_system_ns'] >= a['cpu_system_ns'], 'cpu_monotonic')
            elapsed = ((b['begin_ticks']+b['end_ticks'])-(a['begin_ticks']+a['end_ticks']))*scale/2
            if elapsed > 0:
                cpu_percent = 100*(b['cpu_user_ns']+b['cpu_system_ns']-a['cpu_user_ns']-a['cpu_system_ns'])/elapsed
        return dict(accepted=not issues, rejections=sorted(set(issues)), epoch=s['epoch'],
                    completions=total, steady_completions=s['steady_completed_in']+s['steady_completed_out'],
                    sampler_max_ns=s['sampler_max_ns'], average_worker_cpu_percent_estimate=cpu_percent,
                    policy_defaults=s['policy_defaults'], basic_policy_changes=s['basic_policy_changes'], constraint_policy_changes=s['constraint_policy_changes'], drained=[s['drained_in'],s['drained_out']], metrics=s['metrics'], worst_delivery=s['worst_delivery'],
                    worst_lead=s['worst_lead'],
                    scope='Timing distributions are event-conditioned, not OS bounds. CPU is a sampled epoch average including instrumentation; pump membership cannot distinguish kernel dispatch from descheduling. Audio/lead/lifecycle qualification is separate.')
    except (KeyError, TypeError, ValueError, OverflowError, ZeroDivisionError) as error:
        return dict(accepted=False, rejections=sorted(set(issues+[str(error)])))


def from_log(text, expected_build, require_observed=True):
    try:
        def one(prefix):
            values = [line[len(prefix):] for line in text.splitlines() if line.startswith(prefix)]
            if len(values) != 1:
                raise ValueError('missing_or_ambiguous_'+prefix.strip())
            return values[0]
        if one('build: ') != expected_build:
            raise ValueError('build_mismatch')
        state = json.loads(one('status: '))
        if state['phase'] != 0 or state['pending'] != [0, 0] or any(state[k] for k in ('fault', 'reason', 'cleanup')):
            raise ValueError('not_cleanly_stopped')
        return analyze(json.loads(one('delivery: ')), state['epoch'], require_observed)
    except (KeyError, TypeError, ValueError) as error:
        return dict(accepted=False, rejections=[str(error)])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path)
    parser.add_argument('--expected-build', required=True)
    parser.add_argument('--allow-empty', action='store_true')
    args = parser.parse_args()
    result = from_log(args.log.read_text(), args.expected_build, not args.allow_empty)
    print(json.dumps(result, indent=2))
    raise SystemExit(not result['accepted'])
