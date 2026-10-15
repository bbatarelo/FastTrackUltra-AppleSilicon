#!/usr/bin/env python3
"""Audit Rust duplex packet accounting and the bytes submitted to OUT01."""
import argparse
from collections import Counter
import gzip
import json
import math
from pathlib import Path
import struct

OK = (0, -536870169)  # kIOReturnSuccess / kIOReturnUnderrun
BLOCK_BYTES = 64 * 312


def read(path):
    return path.read_bytes() if path.exists() else gzip.decompress(Path(str(path)+'.gz').read_bytes())


def check(condition, message):
    if not condition:
        raise ValueError(message)


def expected_tone(frame, channel):
    if channel > 1:
        return 0
    begin, end, hz = (4800, 33600, 440) if channel == 0 else (43200, 72000, 660)
    if not begin <= frame < end:
        return 0
    distance = min(frame-begin, end-1-frame, 960)
    envelope = .5-.5*math.cos(math.pi*distance/960)
    value = .01*envelope*math.sin(2*math.pi*hz*(frame-begin)/48000)
    value = struct.unpack('<f', struct.pack('<f', value))[0]
    return int(math.copysign(math.floor(abs(value)*8388608+.5), value))


def analyze(directory):
    root = Path(directory)
    plan = json.loads(read(root/'plan.json')); summary = json.loads(read(root/'summary.json'))
    check(plan['engine'] == summary['engine'] == 'rust', 'engine mismatch')
    check(plan['channels'] == 8 and plan['packet_capacity'] == 312 and plan['entries_per_block'] == 64, 'geometry')
    check(plan['feedback_delay_ms'] == 64 and plan['feedback_source'] == 'IN81_actual_packet_counts', 'feedback plan')
    check(plan['mode'] in ('tone', 'silence') and summary['tone'] == (plan['mode']=='tone'), 'mode mismatch')
    check(summary['duration_ms'] == plan['duration_ms'], 'duration mismatch')
    count = summary['planned_blocks_per_direction']
    check(count * 8 == plan['duration_ms'], 'planned blocks')
    rows = [json.loads(line) for line in read(root/'observations.jsonl').splitlines()]
    check(len(rows) == count*128, 'planned slots')
    data = [read(root/'input-packets.bin'), read(root/'output-packets.bin')]
    blocks = [[], []]; raw_cursors = [0, 0]; frames = [0, 0]; counts = [Counter(), Counter()]
    capture_lengths = Counter(); output_lengths = Counter(); peaks = [0]*8; nonzero = [0]*8
    output_cursor = 0
    for d in range(2):
        for b in range(count):
            group = rows[(d*count+b)*64:(d*count+b+1)*64]
            first = group[0]; state = first['state']; completed = state=='Complete'
            owned = completed and first['callback_matches']==1
            for row in group:
                check(all(row[k]==first[k] for k in ('state','frame_start','submit_status','completion_status','callback_matches','callback_ticks')), 'inconsistent block')
            counts[d][state] += 1
            requested = sum(r['planned_length'] for r in group)
            check(requested <= BLOCK_BYTES, 'oversize request')
            for slot, row in enumerate(group):
                check((row['direction'],row['block'],row['slot']) == (d,b,slot), 'slot order')
                actual = row['actual']; req = row['requested']; status_ok = row['completion_status'] in OK and row['packet_status'] in OK
                accepted = owned and status_ok and (req == 312 and actual > 0 and actual <= 312 and actual % 24 == 0 if d == 0 else req == row['planned_length'] and actual == req)
                check(row['accepted'] == accepted, 'acceptance mismatch')
                if not owned:
                    check(row['raw_offset'] is None and actual is None, 'unowned observation')
                else:
                    offset = raw_cursors[d] + (slot*312 if d==0 else sum(r['planned_length'] for r in group[:slot]))
                    check(row['raw_offset']==offset and raw_cursors[d]+BLOCK_BYTES<=len(data[d]), 'raw slot bounds/stride')
                if accepted:
                    frames[d] += actual//24
                    (capture_lengths if d==0 else output_lengths)[actual] += 1
                if d==1 and state in ('Pending','Complete','Failed'):
                    source = blocks[0][b][slot]
                    check(source['accepted'] and row['planned_length']==source['actual'], 'output does not mirror valid capture')
                    check(row['frame_start']-source['frame_start']==64, 'feedback delay')
                if d==1 and owned:
                    # These are submitted bytes; OUT completion cannot verify analogue sound.
                    for f in range(row['planned_length']//24):
                        for ch in range(8):
                            at=row['raw_offset']+f*24+ch*3
                            value=int.from_bytes(data[d][at:at+3],'little',signed=True)
                            check(abs(value)<=83886, 'tone exceeds -40 dBFS ceiling')
                            expected=expected_tone(output_cursor+f,ch) if plan['mode']=='tone' else 0
                            check(abs(value-expected)<= (1 if plan['mode']=='tone' else 0), 'output waveform/channel mismatch')
                            peaks[ch]=max(peaks[ch],abs(value));nonzero[ch]+=value!=0
                if d==1 and row['frame_start'] is not None:
                    output_cursor += row['planned_length']//24
            if owned:raw_cursors[d]+=BLOCK_BYTES
            blocks[d].append(group)
        check(raw_cursors[d]==len(data[d]), 'raw file size')
        submitted=counts[d]['Pending']+counts[d]['Complete']
        check(submitted==summary['submitted'][d] and counts[d]['Complete']==summary['completed'][d] and counts[d]['Pending']==summary['pending'][d], 'callback accounting')
    planned_frames=sum(sum(r['planned_length'] for r in group)//24 for group in blocks[1] if group[0]['frame_start'] is not None)
    check(planned_frames==summary['output_planned_frames'], 'planned output frames')
    if summary['status']=='ok':
        check(frames[0]==frames[1] and counts[0]['Complete']==count and counts[1]['Complete']==count,'successful frame conservation')
        check(not summary['file_error'] and not summary['drain_failed'] and summary['restored_mask']==3 and summary['open_mask']==0 and summary['cleanup_status']==0 and summary['callback_fault']==0,'successful cleanup')
        for key in ('rate_before','rate_after'):
            check(summary[key]['status']==[0,0] and summary[key]['actual']==[3,3] and summary[key]['hz']==[48000,48000], 'rate postcondition')
    return dict(audit='passed', experiment_status=summary['status'], mode=plan['mode'],
                input_audio_frames=frames[0],output_completed_frames=frames[1],output_planned_frames=planned_frames,
                capture_length_histogram=dict(capture_lengths),output_length_histogram=dict(output_lengths),
                output_peak_s24=peaks,output_nonzero_samples=nonzero,
                submitted=summary['submitted'],completed=summary['completed'],pending=summary['pending'],
                restored_mask=summary['restored_mask'],rate_after=summary['rate_after'],
                note='Transport accounting and submitted digital bytes only; no analogue sound or latency claim.')


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('directory');args=parser.parse_args()
    print(json.dumps(analyze(args.directory),indent=2,sort_keys=True))
