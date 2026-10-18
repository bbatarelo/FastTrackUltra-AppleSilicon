import unittest
import numpy as np
from analyze import chirp, detect, check_cycles, timestamp


class AnalysisTests(unittest.TestCase):
    def setUp(self):
        self.ref = chirp(512)

    def signal(self, delays, sign=1):
        signal = np.zeros(4096 + len(self.ref))
        for delay in delays:
            signal[delay:delay+len(self.ref)] += self.ref*sign
        return signal

    def test_known_delays_and_inversion(self):
        for delay in (3, 101, 2048, 4000):
            for sign in (-1, 1):
                result = detect(self.ref, self.signal([delay],sign),4096)
                self.assertTrue(result['accepted'], result)
                self.assertAlmostEqual(result['lag'],delay,delta=0.01)
                self.assertEqual(result['polarity'],sign)

    def test_noise_and_gain(self):
        x = self.signal([700])*0.2+np.random.default_rng(42).normal(0,0.0001,4608)
        result = detect(self.ref,x,4096)
        self.assertTrue(result['accepted'],result)
        self.assertAlmostEqual(result['lag'],700,delta=0.1)

    def test_ambiguous_duplicate(self):
        self.assertEqual(detect(self.ref,self.signal([100,2200]),4096)['reason'],'ambiguous_return')

    def test_reject_invalid_silence_noise_clip_and_edge(self):
        for x in (np.zeros(4608), np.random.default_rng(2).normal(0,0.001,4608),
                  np.full(4608,np.nan),self.signal([0]),self.signal([4096]),self.signal([80])*200):
            self.assertFalse(detect(self.ref,x,4096)['accepted'])
        self.assertFalse(detect(self.ref,np.zeros(80),4096)['accepted'])

    def cycles(self):
        return [dict(raw=i*192, frames=192,input_sample=i*192-1024,output_sample=i*192+4480,
                     input_flags=1,output_flags=1) for i in range(4)]

    def test_reference_planes(self):
        rows=self.cycles()
        self.assertEqual(check_cycles(rows,768),[])
        raw=500.25-100
        ts=timestamp(rows,500.25,'input')-timestamp(rows,100,'output')
        self.assertEqual(raw-ts,5504)

    def test_maximum_buffer_and_partial_final_capture(self):
        rows=[dict(raw=i*4096,frames=4096,input_sample=i*4096-1024,
                   output_sample=i*4096+12288,input_flags=1,output_flags=1) for i in range(2)]
        self.assertEqual(check_cycles(rows,8000),[])
        self.assertEqual(timestamp(rows,5000,'input'),3976)
        rows[0]['frames']=4097
        self.assertIn('delivered_frame_gap',check_cycles(rows,8000))

    def test_reject_timestamp_and_raw_gaps(self):
        for key in ('raw','input_sample','output_sample'):
            rows=self.cycles();rows[2][key]+=1
            self.assertTrue(check_cycles(rows,768))
        rows=self.cycles();rows[1]['input_flags']=0
        self.assertIn('invalid_timestamp',check_cycles(rows,768))
        self.assertTrue(check_cycles(self.cycles(),400))


if __name__=='__main__': unittest.main()
