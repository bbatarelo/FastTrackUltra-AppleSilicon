"""Independent synthetic evidence; these fixtures are never presented as hardware."""
import json
from pathlib import Path
import tempfile
import unittest
import zlib
from analyze_stream import analyze

class StreamEvidence(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.addCleanup(self.tmp.cleanup)
        self.plan=dict(schema_version=2,engine='rust_pool',epoch=1,mode='silence',duration_ms=32,entries=64,block_ms=8,channels=8,packet_capacity=312,pool_slots_per_direction=4,feedback_slots=32,feedback_delay_ms=64,lead_ms=16,native_audio_bytes=8*64*312,journal_records=8,mixer_writes=False,rate_writes=False)
        self.rows=[];crc=zlib.crc32(bytes(64*312))
        for d in range(2):
            for seq in range(4):
                start=116+seq*8+d*64
                c=dict(ticket=seq+1,status=0,matched=1,callback_ticks=(start+8)*24000,requested=[312 if d==0 else 144]*64,actual=[144]*64,packet_status=[0]*64,ticks=[(start+i//8)*24000 for i in range(64)])
                self.rows.append(dict(epoch=1,direction=d,sequence=seq,slot=seq,ticket=seq+1,start=start,submitted_at=start-8,admission_ticks=(start-8)*24000,submit_status=0,lengths=[312 if d==0 else 144]*64,first_sample=seq*384,crc_before=crc if d else 0,crc_after=crc if d else 0,peaks=[0]*8,nonzero=[0]*8,completion=c))
        rate=dict(hz=[48000,48000],status=[0,0],actual=[3,3])
        self.summary=dict(engine='rust_pool',status='ok',epoch=1,duration_ms=32,tone=False,submitted=[4,4],completed=[4,4],pending=[0,0],accepted_frames=[1536,1536],output_planned_frames=1536,max_pending=[4,4],reused=[0,0],drain_failed=False,file_error=False,cleanup_status=0,restored_mask=3,open_mask=0,callback_fault=0,restore_status=[0,0],native_cleanup_status=[0,0],rate_before=rate,rate_after=rate,timebase_numer=125,timebase_denom=3,bus_frame=100,capture_fit=dict(epoch=1,observations=32,first_frame=0,last_frame=1488,first_ticks=116*24000,last_ticks=147*24000,span_ns=31000000,valid=False,rate_hz=None))
    def run_audit(self):
        for name,obj in [('plan',self.plan),('summary',self.summary)]: (self.root/(name+'.json')).write_text(json.dumps(obj))
        (self.root/'trace.jsonl').write_text(''.join(json.dumps(v)+'\n' for v in self.rows))
        return analyze(self.root)
    def test_clean_silence(self):self.assertEqual(self.run_audit()['audit'],'passed')
    def test_wrong_slot_generation(self):
        self.rows[4]['completion']['ticket']=2
        with self.assertRaises(ValueError):self.run_audit()
    def test_unobserved_clock_anchor(self):
        self.summary['capture_fit']['last_ticks']+=24000
        with self.assertRaises(ValueError):self.run_audit()
    def test_changed_output_payload(self):
        self.rows[4]['crc_after']^=1
        with self.assertRaises(ValueError):self.run_audit()
    def test_wrong_feedback_packet(self):
        self.rows[4]['lengths'][0]=120
        with self.assertRaises(ValueError):self.run_audit()
    def test_failure_cannot_hide_in_success(self):
        self.summary['restore_status'][1]=-1
        with self.assertRaises(ValueError):self.run_audit()

    def use_v3(self):
        self.plan.update(schema_version=3,clock_group_anchor='last_processing_tick')

    def test_split_processing_group(self):
        self.use_v3()
        ticks=self.rows[2]['completion']['ticks']
        ticks[56]-=20809;ticks[57]-=17620
        self.assertEqual(self.run_audit()['audit'],'passed')

    def test_individual_microframe_processing(self):
        self.use_v3()
        for row in self.rows[:4]:
            row['completion']['ticks']=[t-(7-i%8)*3000 for i,t in enumerate(row['completion']['ticks'])]
        self.assertEqual(self.run_audit()['audit'],'passed')

    def test_bad_group_cannot_claim_complete_clock(self):
        self.use_v3()
        self.rows[2]['completion']['ticks'][1]+=1
        with self.assertRaises(ValueError):self.run_audit()

    def test_truncated_fit_cannot_hide_bad_group(self):
        self.use_v3()
        self.rows[3]['completion']['ticks'][56]=0
        fit=self.summary['capture_fit']
        fit.update(observations=31,last_frame=1440,last_ticks=146*24000,span_ns=30000000)
        with self.assertRaises(ValueError):self.run_audit()

if __name__=='__main__':unittest.main()
