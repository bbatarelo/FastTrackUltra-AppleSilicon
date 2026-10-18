#!/usr/bin/env python3
"""Offline sample-coordinate and availability checks; never opens audio devices."""
import argparse
import csv
import hashlib
import json
from pathlib import Path


def analyze(root, reduction=512):
    summary_path = root / 'build/trials/2026-10-18-worker-timing/summary.json'
    trials = summary_path.parent
    summary = json.loads(summary_path.read_text())
    evidence = json.loads((trials / 'hardware-evidence.json').read_text())['sha256']
    rows = []
    for run in summary['runs']:
        path = trials / run['run'] / 'callbacks.csv'
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if evidence[str(path.relative_to(root))] != digest:
            raise ValueError('changed callback evidence: ' + str(path))
        b = run['buffer']
        distance = 2 * b + summary['input_safety'] + summary['output_safety']
        with path.open() as stream:
            cycles = list(csv.DictReader(stream))
        if not cycles or any(float(c['output_sample']) - float(c['input_sample']) != distance
                             or int(c['frames']) != b for c in cycles):
            raise ValueError('sample-coordinate or buffer mismatch')
        s = run['steady']
        margin = min(s[k]['min'] for k in ('write_preparation_lead', 'output_high_water_margin'))
        rows.append(dict(run=run['run'],buffer=b,callbacks=len(cycles),sample_distance=distance,
                         baseline_raw_ms=run['raw_rtl_ms'],predicted_raw_ms=[v-reduction/48 for v in run['raw_rtl_ms']],
                         baseline_margin_frames=margin,extra_reserve_frames=b+832,
                         residual_after_cut_frames=margin-reduction-b-832,input_margin_frames=s['read_capture_margin']['min']))
    validation = root / 'build/validation/worker-timing-20261018'
    additional = []
    for buffer in (64,4096):
        path=validation/f'lifecycle-{buffer}.json'
        s=json.loads(path.read_text())['timing']['final']['steady']
        margin=min(s[k]['min'] for k in ('write_preparation_lead','output_high_water_margin'))
        additional.append(dict(kind='two_clients',buffer=buffer,residual_after_cut_frames=margin-reduction-buffer-832))
    listening=Path(json.loads((validation/'latest-listening.json').read_text())['metadata'])
    s=json.loads(listening.read_text())['timing']['final']['steady']
    margin=min(s[k]['min'] for k in ('write_preparation_lead','output_high_water_margin'))
    additional.append(dict(kind='listening',buffer=512,residual_after_cut_frames=margin-reduction-512-832))
    return dict(schema=1,baseline_build=summary['measured_build'],reduction_frames=reduction,
                predicted_reduction_ms=reduction/48,rows=rows,additional_runs=additional,
                minimum_residual_frames=min(r['residual_after_cut_frames'] for r in rows+additional),
                scope='Measured index identity plus counterfactual margin screen. Not an event replay, contiguous-coverage proof or OS latency bound. now_host is not callback entry.')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--root',type=Path,default=Path(__file__).resolve().parents[2])
    p.add_argument('--reduction',type=int,default=512)
    args=p.parse_args()
    if args.reduction<0:p.error('reduction must be nonnegative')
    print(json.dumps(analyze(args.root.resolve(),args.reduction),indent=2))
