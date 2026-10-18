#!/usr/bin/env python3
"""Validate and decode sample-free ftuW snapshots; no hardware or audio access."""
import argparse
import json
from pathlib import Path
FIELDS = ['ordinal', 'direction', 'sequence', 'slot', 'ticket', 'captured', 'flags', 'start_frame', 'checked_frame', 'returned_frame', 'previous_sequence', 'previous_start_frame', 'previous_last_packet_ticks', 'previous_callback_ticks', 'previous_consumed_ticks', 'feedback_sequence', 'feedback_last_packet_ticks', 'feedback_callback_ticks', 'feedback_consumed_ticks', 'loop_enter_ticks', 'poll_end_ticks', 'pump_enter_ticks', 'pump_exit_ticks', 'check_begin_ticks', 'check_end_ticks', 'prepare_end_ticks', 'submit_begin_ticks', 'submit_end_ticks', 'query_end_ticks', 'check_status', 'submit_status', 'return_clock_status']
GROUPS = {1:FIELDS[10:15],2:FIELDS[15:19],4:['checked_frame'],8:['prepare_end_ticks'],16:['submit_begin_ticks','submit_end_ticks','submit_status'],32:['returned_frame'],64:['pump_enter_ticks','pump_exit_ticks'],128:['query_end_ticks','return_clock_status']}
STAGES = {
    'run_loop_ms': ('pump_enter_ticks','pump_exit_ticks'),
    'poll_pass_ms': ('loop_enter_ticks','poll_end_ticks'),
    'clock_check_ms': ('check_begin_ticks','check_end_ticks'),
    'preparation_ms': ('check_end_ticks','prepare_end_ticks'),
    'submission_ms': ('submit_begin_ticks','submit_end_ticks'),
    'post_submit_observation_ms': ('submit_end_ticks','query_end_ticks'),
    'previous_callback_after_last_packet_ms': ('previous_last_packet_ticks','previous_callback_ticks'),
    'previous_consumption_ms': ('previous_callback_ticks','previous_consumed_ticks'),
    'previous_consumed_to_check_ms': ('previous_consumed_ticks','check_begin_ticks'),
    'feedback_callback_after_last_packet_ms': ('feedback_last_packet_ticks','feedback_callback_ticks'),
    'feedback_consumption_ms': ('feedback_callback_ticks','feedback_consumed_ticks'),
    'feedback_consumed_to_check_ms': ('feedback_consumed_ticks','check_begin_ticks'),
}

def analyze(snapshot, expected_epoch=None, require_observed=True):
    problems=[];decoded=[]
    def require(test,reason):
        if not test: raise ValueError(reason)
    def integer(n): return type(n) is int and 0<=n<=2**64-1
    try:
        require(isinstance(snapshot,dict),'missing_snapshot')
        require(snapshot['version']==1 and snapshot['valid']==1,'unsupported_or_incoherent_snapshot')
        for k in ('epoch','timebase_numer','timebase_denom','queue_depth','threshold_ms','observed','dropped','recent_overwritten','late_seen','late_retained','late_overflow'):
            require(integer(snapshot[k]),'invalid_counter_'+k)
        if expected_epoch is not None: require(snapshot['epoch']==expected_epoch,'epoch_mismatch')
        n=snapshot['observed'];require(n>0 or not require_observed,'empty_observations')
        require(snapshot['dropped']==0,'dropped_observations')
        require(snapshot['recent_overwritten']==max(n-16,0),'recent_count_mismatch')
        require(snapshot['late_retained']==min(snapshot['late_seen'],8),'late_count_mismatch')
        require(snapshot['late_overflow']==max(snapshot['late_seen']-8,0),'late_count_mismatch')
        require(snapshot['late_seen']<=n,'late_exceeds_observed')
        if n:
            require(snapshot['timebase_numer']>0 and snapshot['timebase_denom']>0,'missing_timebase')
            require(2<=snapshot['queue_depth']<=4,'unsupported_depth')
            require(snapshot['threshold_ms']==(snapshot['queue_depth']-2)*8,'threshold_mismatch')
        scale=snapshot['timebase_numer']/snapshot['timebase_denom']/1e6 if snapshot['timebase_denom'] else None
        seen={}
        for bank,capacity in (('worst',2),('recent',16),('late',8)):
            events=snapshot[bank];require(isinstance(events,list) and len(events)<=capacity,'invalid_bank')
            ordinals=[];directions=[]
            for e in events:
                require(isinstance(e,dict) and set(e)==set(FIELDS),'invalid_event_fields')
                flags=e['flags'];require(integer(flags) and flags<=255,'invalid_flags')
                require(not flags&32 or flags&128,'returned_without_query')
                require(not flags&128 or flags&16,'query_without_submission')
                require(not flags&128 or bool(flags&32)==(e['return_clock_status']==0),'return_status_availability')
                require(bool(flags&4)==(e['check_status']==0),'check_status_availability')
                require(not flags&16 or flags&8,'submitted_without_preparation')
                require(not flags&8 or flags&4,'prepared_without_clock')
                for bit,keys in GROUPS.items():
                    for k in keys:require((e[k] is not None)==bool(flags&bit),'availability_'+k)
                for k,v in e.items():
                    if v is None:require(any(k in keys for keys in GROUPS.values()),'unexpected_null_'+k)
                    elif k.endswith('_status'):require(type(v) is int and -(2**31)<=v<2**31,'invalid_status')
                    else:require(integer(v),'invalid_word_'+k)
                ordinal=e['ordinal'];require(1<=ordinal<=n,'invalid_ordinal');ordinals.append(ordinal)
                require(e['direction'] in (0,1),'invalid_direction');directions.append(e['direction'])
                require(e['slot']==e['sequence']%snapshot['queue_depth'] and e['ticket']==e['sequence']+1,'invalid_generation')
                if flags&1:require(e['previous_sequence']+snapshot['queue_depth']==e['sequence'],'invalid_predecessor')
                if flags&2:require(e['direction']==1 and e['feedback_sequence']==e['sequence'],'invalid_feedback')
                require(e['direction']==1 or not flags&2,'input_has_feedback')
                require(ordinal not in seen or seen[ordinal]==e,'cross_bank_mismatch');seen[ordinal]=e
                if bank=='worst':require(e['captured']>=48000,'startup_in_steady_worst')
                for k in ('check_status','submit_status','return_clock_status'):
                    if e[k] not in (None,0):problems.append('observed_'+k)
                # Linked stage order, separate from packet stamps whose precise
                # service-point semantics are deliberately not inferred here.
                chain=['loop_enter_ticks','poll_end_ticks','check_begin_ticks','check_end_ticks','prepare_end_ticks','submit_begin_ticks','submit_end_ticks','query_end_ticks']
                ticks=[e[k] for k in chain if e[k] is not None]
                require(all(a<=b for a,b in zip(ticks,ticks[1:])),'stage_order')
                if ordinal in {d['ordinal'] for d in decoded}:continue
                d={k:e[k] for k in ('ordinal','direction','sequence','slot','ticket','captured')}
                d['lead_reference']='returned' if flags&32 else 'checked' if flags&4 else None
                d['lead_ms']=e['start_frame']-e['returned_frame' if flags&32 else 'checked_frame'] if flags&4 else None
                for label,(a,b) in STAGES.items():
                    # Zero USB packet stamps mean no usable packet-time evidence.
                    available=e[a] is not None and e[b] is not None and not (a.endswith('last_packet_ticks') and e[a]==0)
                    if available:
                        require(e[b]>=e[a],'negative_'+label)
                        d[label]=(e[b]-e[a])*scale
                    else:d[label]=None
                decoded.append(d)
            require(len(set(ordinals))==len(ordinals),'duplicate_ordinal')
            if bank=='recent':require(set(ordinals)==set(range(max(1,n-15),n+1)),'recent_window_mismatch')
            if bank=='late':require(len(events)==snapshot['late_retained'] and ordinals==sorted(ordinals),'late_retention_mismatch')
            if bank=='worst':require(len(set(directions))==len(directions),'duplicate_worst_direction')
        return dict(accepted=not problems,rejections=sorted(set(problems)),epoch=snapshot['epoch'],observed=n,dropped=snapshot['dropped'],late_seen=snapshot['late_seen'],late_overflow=snapshot['late_overflow'],events=sorted(decoded,key=lambda e:e['ordinal']))
    except (KeyError,TypeError,ValueError,OverflowError,ZeroDivisionError) as error:
        return dict(accepted=False,rejections=sorted(set(problems+[str(error)])),events=[])

def from_log(text, expected_epoch=None, require_observed=True):
    try:
        values=[json.loads(line.split(': ',1)[1]) for line in text.splitlines() if line.startswith('worker timing: ')]
        if len(values)!=1:raise ValueError('missing_or_ambiguous_snapshot')
        states=[json.loads(line.split(': ',1)[1]) for line in text.splitlines() if line.startswith('status: ')]
        if len(states)!=1:raise ValueError('missing_or_ambiguous_status')
        epoch=states[0]['epoch']
        if expected_epoch is not None and epoch!=expected_epoch:raise ValueError('status_epoch_mismatch')
        return analyze(values[0],epoch,require_observed)
    except (ValueError,KeyError,TypeError) as error:return dict(accepted=False,rejections=[str(error)],events=[])

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('log',type=Path);p.add_argument('--allow-empty',action='store_true');args=p.parse_args()
    result=from_log(args.log.read_text(),require_observed=not args.allow_empty)
    print(json.dumps(result,indent=2));raise SystemExit(not result['accepted'])
