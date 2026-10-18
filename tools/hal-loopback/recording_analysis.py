#!/usr/bin/env python3
"""Find bounded chirp returns in an offline mono GarageBand recording.

The input must first be converted offline to a 48 kHz mono WAV. File sample
positions do not establish physical round-trip latency or native device rate.
"""
import argparse,hashlib,json,wave
from pathlib import Path
import numpy as np
from analyze import chirp,detect

def read_pcm(path):
    with wave.open(str(path),'rb') as w:
        if w.getnchannels()!=1 or w.getframerate()!=48000 or w.getcomptype()!='NONE':raise ValueError('Require 48 kHz mono PCM WAV')
        width=w.getsampwidth();data=w.readframes(w.getnframes())
    if width==2:x=np.frombuffer(data,dtype='<i2').astype(float)/32768
    elif width==3:
        b=np.frombuffer(data,dtype=np.uint8).reshape(-1,3).astype(np.int32)
        x=((b[:,0]|(b[:,1]<<8)|(b[:,2]<<16))<<8>>8).astype(float)/8388608
    elif width==4:x=np.frombuffer(data,dtype='<i4').astype(float)/2147483648
    else:raise ValueError('Require 16, 24 or 32-bit integer PCM')
    return x

def analyze_samples(x,expected_count,spacing_seconds):
    problems=[];events=[]
    if not len(x) or not np.isfinite(x).all() or np.max(np.abs(x))>.25:
        return dict(accepted=False,rejections=['invalid_or_high_capture'],events=[])
    # Energy locates candidate windows only; independent normalized correlation
    # validates every candidate and rejects extra/ambiguous returns.
    padded=np.pad(np.abs(x),(0,(-len(x))%128))
    hot=np.flatnonzero(padded.reshape(-1,128).max(axis=1)>.0005)
    groups=np.split(hot,np.flatnonzero(np.diff(hot)>16)+1) if len(hot) else []
    for group in groups:
        begin=max(0,int(group[0])*128-4096)
        candidate=detect(chirp(),x[begin:begin+48000+2048],48000)
        if candidate.get('accepted') and candidate['quality']>=.90:
            candidate['recorded_frame']=begin+candidate['lag'];events.append(candidate)
        else:problems.append('unmatched_energy_event')
    if len(events)!=expected_count:problems.append('chirp_count_mismatch')
    spacing=[b['recorded_frame']-a['recorded_frame'] for a,b in zip(events,events[1:])]
    if any(abs(v-spacing_seconds*48000)>3 for v in spacing):problems.append('chirp_spacing_outside_three_frames')
    return dict(accepted=not problems,rejections=sorted(set(problems)),duration_seconds=len(x)/48000,peak=float(np.max(np.abs(x))),expected_chirps=expected_count,detected_chirps=len(events),spacing_frames_range=[min(spacing),max(spacing)] if spacing else [],events=events,scope='Recorded signal continuity and app conversion only; no independent physical RTL or listening claim.')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('wav',type=Path);p.add_argument('--expected',type=int,required=True);p.add_argument('--spacing',type=float,default=5);args=p.parse_args()
    result=analyze_samples(read_pcm(args.wav),args.expected,args.spacing);result['input_sha256']=hashlib.sha256(args.wav.read_bytes()).hexdigest()
    print(json.dumps(result,indent=2));raise SystemExit(not result['accepted'])
