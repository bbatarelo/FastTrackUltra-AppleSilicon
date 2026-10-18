import unittest
import numpy as np
from analyze import chirp
from recording_analysis import analyze_samples
class RecordingTests(unittest.TestCase):
    def signal(self,starts):
        x=np.random.default_rng(42).normal(0,.00002,48000*14)
        for start in starts:x[start:start+2048]+=chirp()*.8
        return x
    def test_spaced_returns_and_quiet_noise(self):
        r=analyze_samples(self.signal([48000*2,48000*7,48000*12]),3,5)
        self.assertTrue(r['accepted'],r);self.assertEqual(r['detected_chirps'],3)
    def test_missing_shifted_and_extra_return_rejected(self):
        for starts in ([96000,576000],[96000,336100,576000],[96000,192000,336000,576000]):
            self.assertFalse(analyze_samples(self.signal(starts),3,5)['accepted'])
    def test_silence_and_high_input_rejected(self):
        for x in (np.zeros(48000*14),np.ones(48000*14),np.full(48000*14,np.nan)):
            self.assertFalse(analyze_samples(x,3,5)['accepted'])
