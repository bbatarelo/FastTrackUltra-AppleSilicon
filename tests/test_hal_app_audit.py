"""Ensure explicit startup admission never hides a later stream failure."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
ARCHIVE = ROOT / 'tests/fixtures/hal-app'
spec = importlib.util.spec_from_file_location('app_audit', ROOT / 'scripts/audit_hal_app_trial.py')
app_audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(app_audit)


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ftu-audit-test-')
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name)
        for name in ['candidate.json', 'hal-check.bin']:
            shutil.copy2(ARCHIVE / name, base / name)
        self.run_path = base / 'trial'
        shutil.copytree(ARCHIVE / 'run-004-vlc-melody-repeat', self.run_path)

    def rewrite(self, name, change):
        path = self.run_path / name
        data = json.loads(path.read_text())
        change(data)
        path.write_text(json.dumps(data))

    def audit(self):
        with contextlib.redirect_stdout(io.StringIO()):
            app_audit.audit(self.run_path)

    def test_actual_startup_then_flat_counter_is_admitted(self):
        self.audit()
        result = json.loads((self.run_path / 'audit.json').read_text())
        self.assertEqual(result['startup_boundary_captured_frames'], 48000)
        self.assertEqual(result['startup_observations'][0]['output_missing'], 1153)
        self.assertEqual(result['startup_output_missing'], 4800)

    def test_miss_after_startup_is_rejected(self):
        self.rewrite('observations.json', lambda data: data[20]['status'].update(playback_missing=4801))
        with self.assertRaises(AssertionError):
            self.audit()

    def test_input_miss_during_startup_is_rejected(self):
        self.rewrite('observations.json', lambda data: data[1]['status'].update(capture_missing=1))
        with self.assertRaises(AssertionError):
            self.audit()

    def test_fault_during_startup_is_rejected(self):
        self.rewrite('observations.json', lambda data: data[1]['status'].update(fault=6))
        with self.assertRaises(AssertionError):
            self.audit()

    def test_callback_debt_at_stop_is_rejected(self):
        self.rewrite('after.json', lambda data: data['status'].update(pending=[0, 1]))
        with self.assertRaises(AssertionError):
            self.audit()


if __name__ == '__main__':
    unittest.main()
