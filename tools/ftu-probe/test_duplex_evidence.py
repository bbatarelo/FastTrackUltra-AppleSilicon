import json
from pathlib import Path
import tempfile
import unittest
import analyze_duplex as duplex


class DuplexEvidence(unittest.TestCase):
    def fixture(self, root):
        rates=dict(status=[0,0],actual=[3,3],hz=[48000,48000])
        plan=dict(engine='rust',mode='silence',duration_ms=8,channels=8,packet_capacity=312,
                  entries_per_block=64,feedback_delay_ms=64,feedback_source='IN81_actual_packet_counts')
        lengths=[(5+i%3)*24 for i in range(64)]
        summary=dict(engine='rust',tone=False,duration_ms=8,planned_blocks_per_direction=1,
                     submitted=[1,1],completed=[1,1],pending=[0,0],output_planned_frames=sum(lengths)//24,
                     status='ok',file_error=False,drain_failed=False,restored_mask=3,open_mask=0,
                     cleanup_status=0,callback_fault=0,rate_before=rates,rate_after=rates)
        rows=[]
        for d in range(2):
            offset=0
            for i,actual in enumerate(lengths):
                rows.append(dict(direction=d,block=0,slot=i,frame_start=100+d*64,state='Complete',
                                 submit_status=0,completion_status=0,callback_matches=1,callback_ticks=200,
                                 planned_length=312 if d==0 else actual,requested=312 if d==0 else actual,
                                 actual=actual,packet_status=0,usb_ticks=100,accepted=True,raw_offset=offset))
                offset+=312 if d==0 else actual
        (root/'plan.json').write_text(json.dumps(plan));(root/'summary.json').write_text(json.dumps(summary))
        (root/'observations.jsonl').write_text(''.join(json.dumps(row)+'\n' for row in rows))
        for n in ('input','output'):(root/(n+'-packets.bin')).write_bytes(bytes(duplex.BLOCK_BYTES))
        return rows

    def test_variable_packet_counts_and_zero_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);self.fixture(root);result=duplex.analyze(root)
            self.assertEqual(result['audit'],'passed')
            self.assertEqual(result['input_audio_frames'],383)
            self.assertEqual(result['output_completed_frames'],383)
            self.assertEqual(result['output_nonzero_samples'],[0]*8)

    def test_detect_nonzero_silent_payload(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);self.fixture(root)
            p=root/'output-packets.bin';p.write_bytes(b'\x01'+p.read_bytes()[1:])
            with self.assertRaisesRegex(ValueError,'waveform'):
                duplex.analyze(root)

    def test_detect_wrong_feedback_mapping(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);rows=self.fixture(root);rows[64]['planned_length']=144
            (root/'observations.jsonl').write_text(''.join(json.dumps(row)+'\n' for row in rows))
            with self.assertRaises(ValueError):duplex.analyze(root)

    def test_success_cannot_hide_failed_restore(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);self.fixture(root);p=root/'summary.json';s=json.loads(p.read_text());s['restored_mask']=1;p.write_text(json.dumps(s))
            with self.assertRaisesRegex(ValueError,'cleanup'):duplex.analyze(root)


if __name__=='__main__':unittest.main()
