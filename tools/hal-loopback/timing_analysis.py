"""Validate the optional v1 timing property; these observations contain no audio."""
import json

LIMITS = [0,1,2,4,8,16,32,64,128,256,512,1024,2048,4096,8192,16384]
NAMES = ("write_frames", "write_capture_lead", "write_preparation_lead",
         "output_capture_lead", "output_high_water_margin", "output_missing",
         "read_frames", "read_capture_margin", "read_missing",
         "submit_check_lead_ms", "submit_return_lead_ms", "submit_clock_error")


def analyze_timing(log):
    lines = log.splitlines()
    if not any(line.startswith(('initial timing: ', 'running timing: ', 'stopped timing: ')) for line in lines):
        return None  # Older bundles have no observation property.
    problems = []
    final = None
    try:
        snapshots = [json.loads(line.split(': ',1)[1]) for line in lines if line.startswith('stopped timing: ')]
        states = [json.loads(line.split(': ',1)[1]) for line in lines if line.startswith('stopped status: ')]
        if len(snapshots) != 1 or len(states) != 1:
            raise ValueError('missing final snapshot')
        final = snapshots[0]
        if final['version'] != 1 or final['limits'] != LIMITS or final['steady_after'] != 48000:
            problems.append('unsupported_timing_schema')
        if final['valid'] != 1 or final['epoch'] != states[0]['epoch']:
            problems.append('timing_epoch_or_reset_race')
        for bank in ('all', 'steady'):
            if set(final[bank]) != set(NAMES):
                raise ValueError('metric names')
            for name in NAMES:
                m = final[bank][name]
                if m['valid'] != 1 or m['dropped'] != 0:
                    problems.append('invalid_or_dropped_observation')
                count, buckets = m['count'], m['buckets']
                if type(count) is not int or count <= 0:
                    problems.append('missing_observations')
                    continue
                if len(buckets) != 17 or any(type(v) is not int or v < 0 for v in buckets) or sum(buckets) != count:
                    problems.append('inconsistent_histogram')
                if not all(type(m[k]) is int for k in ('min', 'max', 'last')) or not m['min'] <= m['last'] <= m['max']:
                    problems.append('invalid_observation_range')
        for name in ('output_missing', 'read_missing', 'submit_clock_error'):
            if final['steady'][name]['max'] != 0:
                problems.append('steady_' + name)
        if final['steady']['submit_return_lead_ms']['min'] <= 1:
            problems.append('insufficient_observed_submit_lead')
    except (KeyError, TypeError, ValueError, json.JSONDecodeError):
        problems.append('malformed_timing_snapshot')
    return dict(accepted=not problems, rejections=sorted(set(problems)), final=final)
