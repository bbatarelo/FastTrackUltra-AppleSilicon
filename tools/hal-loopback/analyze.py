#!/usr/bin/env python3
"""Offline test instrument, separate from the Rust driver. Requires NumPy.

Raw RTL counts delivered frames between output submission and input delivery.
Timestamp RTL maps the same detected events through saved IOProc timestamps.
Neither number alone splits ADC/DAC delay or proves the absolute host clock.
"""
import argparse
from collections import Counter
import csv
import hashlib
import json
from pathlib import Path
import re
import numpy as np
from timing_analysis import analyze_timing

RATE = 48000


def chirp(n=2048):
    t = np.arange(n, dtype=np.float64) / RATE
    return (0.01 * np.hanning(n) * np.sin(2 * np.pi *
            (200 * t + 0.5 * (8000 - 200) / (n / RATE) * t * t))).astype('<f4')


def prepare(path, silence=False, input_channel=3):
    path.mkdir(parents=True, exist_ok=False)
    ref = chirp()
    plan = np.zeros(RATE * 8, dtype='<f4')
    starts = [] if silence else [RATE * 2, RATE * 5]
    for start in starts:
        plan[start:start + len(ref)] = ref
    (path / 'stimulus.f32').write_bytes(plan.tobytes())
    (path / 'reference.f32').write_bytes(ref.tobytes())
    config = dict(rate=RATE, frames=len(plan), output_channel=3, input_channel=input_channel,
                  starts=starts, reference_frames=len(ref), max_lag=RATE,
                  peak_limit=0.01, other_outputs='zero', input_monitoring=False,
                  stimulus_sha256=hashlib.sha256(plan.tobytes()).hexdigest())
    (path / 'config.json').write_text(json.dumps(config, indent=2) + '\n')


def detect(reference, signal, max_lag):
    ref = np.asarray(reference, dtype=float)
    x = np.asarray(signal, dtype=float)
    n = len(ref)
    if n < 8 or len(x) < n + max_lag or not np.isfinite(ref).all() or not np.isfinite(x).all():
        return dict(accepted=False, reason='invalid_or_short_signal')
    x = x[:n + max_lag]
    if np.max(np.abs(x)) >= 0.98:
        return dict(accepted=False, reason='clipping')
    ref = ref - ref.mean()
    energy = np.dot(ref, ref)
    if energy < 1e-20:
        return dict(accepted=False, reason='empty_reference')
    fft_n = 1 << (len(x) + n - 2).bit_length()
    corr = np.fft.irfft(np.fft.rfft(x, fft_n) * np.fft.rfft(ref[::-1], fft_n), fft_n)[n-1:n+max_lag]
    sums = np.r_[0., np.cumsum(x)]
    squares = np.r_[0., np.cumsum(x*x)]
    local_energy = np.maximum(squares[n:] - squares[:-n] - (sums[n:] - sums[:-n])**2 / n, 0)
    denom = np.sqrt(local_energy * energy)
    quality = np.divide(corr, denom, out=np.zeros_like(corr), where=denom>1e-12)
    q = np.abs(quality)
    peak = int(np.argmax(q)); best = float(q[peak])
    rivals = q.copy(); rivals[max(0,peak-32):peak+33] = 0
    result = dict(accepted=False, peak_lag=peak, quality=best,
                  rival_quality=float(rivals.max()), polarity=1 if quality[peak]>=0 else -1)
    if best < 0.6:
        return dict(result, reason='weak_correlation')
    if peak < 2 or peak > max_lag-2:
        return dict(result, reason='search_edge')
    if rivals.max() > best * 0.9:
        return dict(result, reason='ambiguous_return')
    curvature = q[peak-1] - 2*q[peak] + q[peak+1]
    fraction = 0.5*(q[peak-1]-q[peak+1])/curvature if curvature else 0.
    return dict(result, accepted=True, lag=peak+float(fraction),
                signal_peak=float(np.max(np.abs(x[peak:peak+n]))))


def check_cycles(cycles, captured):
    if not cycles:
        return ['no_callbacks']
    problems = []
    position = 0
    previous = None
    for row in cycles:
        n = row['frames']
        if row['raw'] != position or not 1 <= n <= 4096:
            problems.append('delivered_frame_gap')
        for key in ('input', 'output'):
            if not row[key+'_flags'] & 1 or not np.isfinite(row[key+'_sample']):
                problems.append('invalid_timestamp')
            if previous and row[key+'_sample'] != previous[key+'_sample']+previous['frames']:
                problems.append(key+'_timestamp_gap')
        position += n
        previous = row
    if not position-cycles[-1]['frames'] < captured <= position:
        problems.append('capture_length_mismatch')
    return sorted(set(problems))


def timestamp(cycles, frame, direction):
    for row in cycles:
        if row['raw'] <= frame < row['raw']+row['frames']:
            return row[direction+'_sample'] + frame-row['raw']
    raise ValueError('event outside callback coverage')


def analyze(path):
    config = json.loads((path/'config.json').read_text())
    ref = np.fromfile(path/'reference.f32', dtype='<f4')
    plan = (path/'stimulus.f32').read_bytes()
    if hashlib.sha256(plan).hexdigest() != config['stimulus_sha256']:
        raise ValueError('stimulus hash mismatch')
    capture = np.fromfile(path/'capture.f32', dtype='<f4')
    with (path/'callbacks.csv').open() as f:
        cycles = [{k: float(v) if k.endswith('_sample') else int(v) for k,v in row.items()}
                  for row in csv.DictReader(f)]
    problems = check_cycles(cycles, len(capture))
    if not np.isfinite(capture).all():
        problems.append('nonfinite_capture')
    elif len(capture) and np.max(np.abs(capture))>0.25:
        problems.append('unexpected_input_level')
    log = (path/'run.log').read_text()
    if 'done: 1 error: 0' not in log or 'stop: 0 destroy: 0 interrupted: 0' not in log:
        problems.append('run_failed')
    if 'restore_ok=1' not in log:
        problems.append('buffer_not_restored')
    final = [json.loads(line.split(': ',1)[1]) for line in log.splitlines()
             if line.startswith('stopped status: ')]
    if len(final)!=1 or final[0]['fault'] or final[0]['reason'] or final[0]['cleanup'] or final[0]['pending']!=[0,0] or final[0]['phase']!=0:
        problems.append('unclean_or_shared_stop')
    states = [json.loads(line.split(': ',1)[1]) for line in log.splitlines()
              if line.startswith('running status: ')]
    if len(states)<3:
        problems.append('insufficient_health_samples')
    elif any(s['fault'] or s['reason'] or s['phase']!=2 for s in states):
        problems.append('driver_fault')
    # The second sample is about one second after Start, before the first chirp.
    steady = states[1:]
    if steady and any(s[k]!=steady[0][k] for s in steady for k in ('epoch','playback_missing','capture_missing')):
        problems.append('steady_state_discontinuity')
    timing = analyze_timing(log)
    if timing is not None and not timing['accepted']:
        problems.append('timing_observation_invalid')
    declarations = {}
    for d,k,st,v in re.findall(r'declaration (input|output) (\w+): status=(-?\d+) value=(\d+)',log):
        declarations[d+'_'+k] = int(v) if int(st)==0 else None
    latency_keys = [d+'_'+p for d in ('input','output') for p in ('device_latency','stream_latency')]
    latency_sum = (sum(declarations[k] for k in latency_keys)
                   if all(declarations.get(k) is not None for k in latency_keys) else None)
    trials = []
    for start in config['starts']:
        trial = detect(ref, capture[start:], config['max_lag'])
        trial['emission_raw_frame'] = start
        if trial['accepted'] and not problems:
            returned = start+trial['lag']
            ts_rtl = timestamp(cycles,returned,'input') - timestamp(cycles,start,'output')
            trial.update(raw_rtl_frames=trial['lag'], timestamp_rtl_frames=ts_rtl,
                         raw_rtl_ms=trial['lag']/RATE*1000, timestamp_rtl_ms=ts_rtl/RATE*1000,
                         scheduling_distance_frames=trial['lag']-ts_rtl,
                         declaration_residual_frames=None if latency_sum is None else ts_rtl-latency_sum)
        if problems:
            trial.update(accepted=False, run_rejections=problems)
        trials.append(trial)
    result = dict(config=config, callback_count=len(cycles), captured_frames=len(capture),
                  callback_frame_distribution=dict(Counter(c['frames'] for c in cycles)),
                  declarations=declarations, run_rejections=problems, trials=trials,
                  health_samples=states, timing=timing, capture_peak=float(np.max(np.abs(capture))) if len(capture) and np.isfinite(capture).all() else None,
                  accepted=not problems and all(t['accepted'] for t in trials))
    (path/'analysis.json').write_text(json.dumps(result,indent=2,allow_nan=False)+'\n')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('prepare','prepare-silence','analyze'))
    parser.add_argument('directory', type=Path)
    parser.add_argument('--input', type=int, choices=(3,4), default=3)
    args = parser.parse_args()
    if args.command.startswith('prepare'):
        prepare(args.directory,args.command=='prepare-silence',args.input)
    else:
        result = analyze(args.directory)
        print(json.dumps({k:v for k,v in result.items() if k not in ('health_samples','config')},indent=2))
        raise SystemExit(0 if result['accepted'] else 1)
