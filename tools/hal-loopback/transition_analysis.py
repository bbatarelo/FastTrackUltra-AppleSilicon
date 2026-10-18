#!/usr/bin/env python3
"""Offline analysis of read-only hal-observe JSONL and UI action brackets."""
import argparse,json
from pathlib import Path

def analyze(rows,markers=()):
    problems=[];changes=[];phases=[];status=[];last_ticks=None;identity=[];timings=[]
    for row in rows:
        kind=row.get('kind')
        if kind=='identity':identity.append(row);continue
        if kind in ('timing','final_timing'):timings.append(row)
        if 'before_ticks' in row:
            if row['after_ticks']<row['before_ticks'] or (last_ticks is not None and row['before_ticks']<last_ticks):problems.append('invalid_observation_order')
            last_ticks=row['after_ticks']
        if kind not in ('status','final_status'):continue
        current=row['data']
        if any(current[k] for k in ('fault','reason','cleanup')):problems.append('driver_fault')
        if status:
            before=status[-1];prior=before['data']
            if current['epoch']==prior['epoch']:
                for key in ('playback_missing','capture_missing'):
                    delta=current[key]-prior[key]
                    if delta<0:problems.append('counter_regressed_within_epoch')
                    if delta>0:
                        overlap=[m for m in markers if before['before_ticks']<=m['ticks']<=row['after_ticks']]
                        changes.append(dict(counter=key,delta=delta,epoch=current['epoch'],before_ticks=before['before_ticks'],after_ticks=row['after_ticks'],before_captured=prior['captured'],after_captured=current['captured'],before_phase=prior['phase'],after_phase=current['phase'],early_epoch=prior['captured']<48000,nearby_markers=overlap))
            elif current['epoch']<prior['epoch']:problems.append('epoch_regressed')
            if current['epoch']!=prior['epoch'] or current['phase']!=prior['phase']:
                phases.append(dict(ticks=row['after_ticks'],from_epoch=prior['epoch'],to_epoch=current['epoch'],from_phase=prior['phase'],to_phase=current['phase']))
        status.append(row)
    if len(identity)!=1 or not status or not rows or rows[-1].get('kind')!='end':problems.append('incomplete_trace')
    steady=[x for x in changes if not x['early_epoch']]
    if steady:problems.append('loss_in_established_epoch')
    intervals=[(b['before_ticks']-a['before_ticks'])*identity[0]['timebase_numer']/identity[0]['timebase_denom']/1e6 for a,b in zip(status,status[1:])] if len(identity)==1 else []
    return dict(instrument_trace_valid=not [p for p in problems if p!='loss_in_established_epoch'],accepted=not problems,rejections=sorted(set(problems)),identity=identity,status_samples=len(status),epochs=sorted({s['data']['epoch'] for s in status}),phase_changes=phases,counter_changes=changes,established_epoch_changes=steady,sampling_gap_ms_max=max(intervals) if intervals else None,final_status=status[-1]['data'] if status else None,scope='Sampled cumulative counters bracket changes but do not identify every HAL call, prove audibility, or assign a change to one client.')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('trace',type=Path);p.add_argument('--markers',type=Path);args=p.parse_args()
    rows=[json.loads(l) for l in args.trace.read_text().splitlines()]
    markers=[json.loads(l) for l in args.markers.read_text().splitlines()] if args.markers else []
    result=analyze(rows,markers);print(json.dumps(result,indent=2));raise SystemExit(not result['accepted'])
