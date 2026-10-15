#!/usr/bin/env python3
"""Independent replay of recycled-slot ownership, output bytes and capture-clock fit."""
import argparse
from collections import Counter
import gzip
import json
from pathlib import Path
import zlib
from analyze_duplex import expected_tone

OK = (0, -536870169)
BLOCK_BYTES = 64*312

def read(path):
    return path.read_bytes() if path.exists() else gzip.decompress(Path(str(path)+'.gz').read_bytes())

def check(ok, message):
    if not ok: raise ValueError(message)

def accepted(r):
    c=r['completion']
    return bool(c and c['ticket']==r['ticket'] and c['matched']==1 and c['status'] in OK and
        all(status in OK and requested==planned and (0<actual<=312 and actual%24==0 if r['direction']==0 else actual==requested)
            for status,requested,actual,planned in zip(c['packet_status'],c['requested'],c['actual'],r['lengths'])))

def waveform(first, frames, tone, table):
    data=bytearray(BLOCK_BYTES);peaks=[0]*8;nonzero=[0]*8
    if tone:
        for i in range(frames):
            values=table[(first+i)%96000]
            for ch,value in enumerate(values):
                data[i*24+ch*3:i*24+ch*3+3]=value.to_bytes(3,'little',signed=True)
                peaks[ch]=max(peaks[ch],abs(value));nonzero[ch]+=value!=0
    return zlib.crc32(data),peaks,nonzero

def analyze(directory):
    root=Path(directory);p=json.loads(read(root/'plan.json'));s=json.loads(read(root/'summary.json'))
    check(p['engine']==s['engine']=='rust_pool' and p['schema_version'] in (2,3),'engine/schema')
    grouped_last=p['schema_version']==3
    if grouped_last:check(p['clock_group_anchor']=='last_processing_tick','clock convention')
    check((p['entries'],p['block_ms'],p['channels'],p['packet_capacity'])==(64,8,8,312),'geometry')
    check(p['pool_slots_per_direction']==4 and p['feedback_slots']==32 and p['feedback_delay_ms']==64 and p['lead_ms']==16,'pool/schedule')
    check(p['native_audio_bytes']==8*BLOCK_BYTES,'fixed native memory')
    check(not p['mixer_writes'] and not p['rate_writes'],'control writes')
    check(p['epoch']==s['epoch'] and p['duration_ms']==s['duration_ms'],'session identity')
    check(p['mode'] in ('tone','silence') and s['tone']==(p['mode']=='tone'),'mode')
    if s['tone']:check(p['duration_ms']==10000 and p['tone_period_frames']==96000 and p['tone_peak']==.01,'tone bounds')
    count=p['duration_ms']//8;check(p['journal_records']==count*2,'journal size')
    rows=[json.loads(v) for v in read(root/'trace.jsonl').splitlines()];directions=[[r for r in rows if r['direction']==d] for d in range(2)]
    check(len(rows)==sum(map(len,directions)),'unknown direction')
    counts=[Counter(),Counter()];frames=[0,0];hist=[Counter(),Counter()];observations=[];input_cursor=0;output_cursor=0;max_peaks=[0]*8
    table=[(expected_tone(i,0),expected_tone(i,1)) for i in range(96000)] if s['tone'] else None
    for d,records in enumerate(directions):
        previous=[None]*4
        for seq,r in enumerate(records):
            check(seq<count and r['sequence']==seq and r['epoch']==s['epoch'] and r['ticket']==seq+1 and r['slot']==seq%4,'sequence/slot/generation')
            check(r['start']==s['bus_frame']+16+seq*8+d*64 and r['submitted_at']+1<r['start'],'late/rebased schedule')
            check(len(r['lengths'])==64 and all(0<n<=312 and n%24==0 for n in r['lengths']),'packet plan')
            old=previous[r['slot']]
            if old:
                check(old['completion'] is not None and old['completion']['ticket']==old['ticket'] and old['completion']['matched']==1,'reuse without matching callback')
                check(r["admission_ticks"]>=old["completion"]["callback_ticks"], "reuse precedes previous callback")
            if r['submit_status']==0:counts[d]['submitted']+=1;previous[r['slot']]=r
            else:counts[d]['failed']+=1
            c=r['completion']
            if c:
                check(r['submit_status']==0,'completion after failed submit')
                check(all(len(c[k])==64 for k in ('requested','actual','packet_status','ticks')),'completion shape')
                counts[d]['completed']+=1
            if accepted(r):
                n=sum(c['actual'])//24;frames[d]+=n;hist[d].update(c['actual']);counts[d]['accepted']+=1
            else:counts[d]['rejected']+=int(c is not None)
            if d==0 and accepted(r):
                # Only contiguous capture is admitted to the diagnostic clock.
                if seq==len(observations)//8:
                    for group in range(8):
                        ticks=c['ticks'][group*8:(group+1)*8]
                        if grouped_last:
                            if not all(ticks) or ticks!=sorted(ticks):break
                            if observations and ticks[0]<observations[-1][1]:break
                            if (ticks[-1]-ticks[0])*s['timebase_numer']//s['timebase_denom']>5000000:break
                        elif len(set(ticks))!=1:break
                        observations.append((input_cursor,ticks[-1] if grouped_last else ticks[0]))
                        input_cursor+=sum(c['actual'][group*8:(group+1)*8])//24
            if d==1:
                source=directions[0][seq]
                check(accepted(source) and r['lengths']==source['completion']['actual'],'output feedback source')
                check(r['first_sample']==output_cursor,'sample cursor discontinuity')
                n=sum(r['lengths'])//24
                crc,peaks,nonzero=waveform(output_cursor,n,s['tone'],table)
                check(r['crc_before']==crc and r['peaks']==peaks and r['nonzero']==nonzero,'output waveform CRC/channel/level')
                if accepted(r):check(r['crc_after']==crc,'submitted output changed before completion')
                max_peaks=[max(a,b) for a,b in zip(max_peaks,peaks)];output_cursor+=n
        check(counts[d]['submitted']==s['submitted'][d] and counts[d]['completed']==s['completed'][d],'callback accounting')
        check(s['pending'][d]==counts[d]['submitted']-counts[d]['completed'],'callback debt')
        check(frames[d]==s['accepted_frames'][d] and s['max_pending'][d]<=4,'accepted frames/queue bound')
        check(s['reused'][d]==max(0,s['submitted'][d]-4),'reuse count')
    check(output_cursor==s['output_planned_frames'],'output planned total')
    fit=s['capture_fit'];check(fit['epoch']==s['epoch'],'clock epoch')
    # The runtime can stop before folding a completed block after a signal/error.
    observations=observations[:fit['observations']]
    if fit['observations']:
        check(len(observations)==fit['observations'],'clock coverage')
        check((fit['first_frame'],fit['first_ticks'])==observations[0] and (fit['last_frame'],fit['last_ticks'])==observations[-1],'clock anchors')
        numer=s['timebase_numer'];denom=s['timebase_denom'];check(numer>0 and denom>0,'timebase')
        span=(observations[-1][1]-observations[0][1])*numer//denom;check(span==fit['span_ns'],'clock span')
        for a,b in zip(observations,observations[1:]):
            interval=(b[1]-a[1])*numer//denom
            check(b[0]>a[0] and 500000<=interval<=5000000,'clock discontinuity accepted')
        if fit['valid']:
            check(len(observations)>=64 and span>=100000000,'clock warmup')
            xs=[(t-observations[0][1])*numer//denom for _,t in observations];ys=[f-observations[0][0] for f,_ in observations]
            mx=sum(xs)/len(xs);my=sum(ys)/len(ys)
            measured=sum((x-mx)*(y-my) for x,y in zip(xs,ys))/sum((x-mx)**2 for x in xs)*1e9
            check(abs(measured-fit['rate_hz'])<.0001 and 47520<=measured<=48480,'independent clock fit')
    if s['status']=='ok':
        check(fit['observations']==count*8,'successful clock coverage')
        check(all(counts[d]['accepted']==count for d in range(2)) and frames[0]==frames[1],'successful frame conservation')
        check(s['pending']==[0,0] and not s['drain_failed'] and not s['file_error'] and s['cleanup_status']==0 and s['restored_mask']==3 and s['open_mask']==0 and s['callback_fault']==0,'successful cleanup')
        check(s['restore_status']==[0,0] and s['native_cleanup_status']==[0,0],'native cleanup')
        for key in ('rate_before','rate_after'):check(s[key]['hz']==[48000,48000] and s[key]['status']==[0,0] and s[key]['actual']==[3,3],'rate postcondition')
        if p['duration_ms']>=112:check(fit['valid'],'missing mature capture fit')
    return dict(audit='passed',experiment_status=s['status'],mode=p['mode'],epoch=s['epoch'],counts=counts,frames=frames,reused=s['reused'],pending=s['pending'],output_peak_s24=max_peaks,packet_histograms=hist,capture_fit=fit,native_audio_bytes=p['native_audio_bytes'],note='Submitted digital payload and diagnostic capture-clock fit; no analogue or HAL timing claim.')

if __name__=='__main__':
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument('directory');args=ap.parse_args();print(json.dumps(analyze(args.directory),indent=2,sort_keys=True))
