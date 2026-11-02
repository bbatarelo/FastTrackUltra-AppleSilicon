"""Profile identities and output isolation, without building or installing audio."""
import importlib.util
import json
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('identity', ROOT / 'scripts/build_identity.py')
identity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(identity)


def builder():
    spec = importlib.util.spec_from_file_location('prototype', ROOT / 'scripts/build_prototype.py')
    prototype = importlib.util.module_from_spec(spec)
    with mock.patch.dict('sys.modules', {'build_identity': identity}):
        spec.loader.exec_module(prototype)
    return prototype


class PrototypeBuildTests(unittest.TestCase):
    def test_separate_outputs_and_profile_specific_identity(self):
        normal = ROOT / 'build/build-identity.json'
        before = normal.read_bytes() if normal.exists() else None
        with tempfile.TemporaryDirectory() as folder:
            identities = []
            sources = []
            for profile in identity.PROFILES:
                path = Path(folder) / profile
                build = identity.prepare(ROOT, output_dir=path, profile=profile)
                self.assertEqual(build, identity.prepare(ROOT, output_dir=path, profile=profile))
                identities.append(build)
                manifest = json.loads((path / 'build-identity.json').read_text())
                plist = plistlib.loads((path / 'Info.plist').read_bytes())
                self.assertEqual(manifest['transport_profile'], profile)
                self.assertEqual(plist['FTUTransportProfile'], profile)
                sources.append(manifest['source_sha256'])
            self.assertEqual(len(set(identities)), 6)
            self.assertTrue(all(source == sources[0] for source in sources))
            with self.assertRaises(ValueError):
                identity.prepare(ROOT, output_dir=Path(folder), profile='typo')
        self.assertEqual(before, normal.read_bytes() if normal.exists() else None)

    def test_failed_rebuild_cannot_retain_success_or_touch_other_profile(self):
        prototype = builder()
        for slots, extra in [(8, []), (10, ['--slots', '10']), (10, ['--slots', '10', '--pcm-ready']), (10, ['--slots', '10', '--pcm-ready', '--deadline'])]:
            with self.subTest(slots=slots), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                for profile, _ in [*prototype.PROFILE_BY_SLOTS.values(), prototype.READY_PROFILE, prototype.DEADLINE_PROFILE]:
                    (root / profile).mkdir()
                    (root / profile / 'offline-result.json').write_text('previous success')
                selected, feature = prototype.DEADLINE_PROFILE if '--deadline' in extra else prototype.READY_PROFILE if '--pcm-ready' in extra else prototype.PROFILE_BY_SLOTS[slots]
                with mock.patch.object(prototype, 'OUTPUT_ROOT', root), \
                     mock.patch.object(prototype, 'run', side_effect=SystemExit(1)) as run, \
                     mock.patch('sys.argv', ['build_prototype.py', *extra]):
                    with self.assertRaises(SystemExit):
                        prototype.main()
                output, label, cmd = run.call_args.args
                self.assertEqual(output, root / selected)
                self.assertEqual(label, 'rust-build')
                self.assertEqual(cmd[cmd.index('--features') + 1], feature)
                self.assertEqual(cmd[cmd.index('--target-dir') + 1], str(root / selected / 'target'))
                for profile, _ in [*prototype.PROFILE_BY_SLOTS.values(), prototype.READY_PROFILE, prototype.DEADLINE_PROFILE]:
                    marker = root / profile / 'offline-result.json'
                    self.assertEqual(marker.exists(), profile != selected)
                    if marker.exists():
                        self.assertEqual(marker.read_text(), 'previous success')

    def test_unknown_profile_does_not_start_a_build(self):
        prototype = builder()
        for args in [['--slots', '9'], ['--deadline'],
                     ['--slots', '10', '--deadline'], ['--pcm-ready']]:
            with self.subTest(args=args), mock.patch.object(prototype, 'run') as run, \
                 mock.patch('sys.argv', ['build_prototype.py', *args]), \
                 mock.patch('sys.stderr'):
                with self.assertRaises(SystemExit) as result:
                    prototype.main()
                self.assertEqual(result.exception.code, 2)
                run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
