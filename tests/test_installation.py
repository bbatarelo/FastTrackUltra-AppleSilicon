"""Exercise actual signed bundles and deployment, confined to temporary roots."""
from pathlib import Path
import importlib.util
import plistlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
DEPLOY = ROOT / 'scripts/deploy_hal.sh'


class InstallationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixture_dir = tempfile.TemporaryDirectory(prefix='ftu-install-fixtures-')
        cls.fixtures = Path(cls.fixture_dir.name).resolve()
        cls.bundles = []
        for version in ('1.0.0', '2.0.0'):
            bundle = cls.fixtures / version / 'FastTrackUltra.driver'
            (bundle / 'Contents/MacOS').mkdir(parents=True)
            (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps({
                'CFBundleIdentifier': 'net.batarelo.FastTrackUltra',
                'CFBundleExecutable': 'FastTrackUltra',
                'CFBundleVersion': version, 'CFBundlePackageType': 'BNDL',
            }))
            subprocess.run(['clang', '-arch', 'arm64', '-bundle', '-x', 'c', '-', '-o',
                            str(bundle / 'Contents/MacOS/FastTrackUltra')],
                           input=b'int unused_fixture(void) { return 0; }', check=True, capture_output=True)
            subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(bundle)], check=True, capture_output=True)
            cls.bundles.append(bundle)

    @classmethod
    def tearDownClass(cls):
        cls.fixture_dir.cleanup()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='ftu-deploy-test-')
        self.root = Path(self.temporary.name).resolve()
        self.parent = self.root / 'Library/Audio/Plug-Ins'
        self.dest = self.parent / 'HAL/FastTrackUltra.driver'

    def tearDown(self):
        self.temporary.cleanup()

    def deploy(self, action, bundle=None, *, success=True, script=DEPLOY, extra=()):
        command = ['/bin/sh', str(script), action]
        if bundle is not None:
            command.append(str(bundle))
        command += ['--root', str(self.root), *extra]
        result = subprocess.run(command, capture_output=True, text=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    @staticmethod
    def version(bundle):
        return plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())['CFBundleVersion']

    def test_install_upgrade_remove_preserve_other_drivers_and_backups(self):
        other = self.dest.parent / 'EMUTrackerPre.driver/untouched'
        other.parent.mkdir(parents=True)
        other.write_bytes(b'other driver')
        self.deploy('install', self.bundles[0])
        self.assertEqual(self.version(self.dest), '1.0.0')
        self.assertEqual((self.dest / 'Contents/MacOS/FastTrackUltra').stat().st_mode & 0o777, 0o755)
        self.deploy('install', self.bundles[1])
        self.assertEqual(self.version(self.dest), '2.0.0')
        backups = list((self.parent / 'FastTrackUltra-Backups').glob('*/FastTrackUltra.driver'))
        self.assertEqual([self.version(p) for p in backups], ['1.0.0'])
        self.deploy('remove')
        self.assertFalse(self.dest.exists())
        self.assertEqual(other.read_bytes(), b'other driver')
        backups = list((self.parent / 'FastTrackUltra-Backups').glob('*/FastTrackUltra.driver'))
        self.assertEqual(sorted(self.version(p) for p in backups), ['1.0.0', '2.0.0'])
        self.deploy('remove')

    def test_failed_replacement_restores_previous_bundle(self):
        self.deploy('install', self.bundles[0])
        # Inject failure at the single filesystem replacement operation, after
        # backup. This runs the real EXIT trap and rollback against real files.
        text = DEPLOY.read_text()
        operation = '    /bin/mv "$staged" "$destination"\n'
        self.assertEqual(text.count(operation), 1)
        failing = self.root / 'fail-replacement.sh'
        failing.write_text(text.replace(operation, '    /usr/bin/false # simulated rename failure\n'))
        self.deploy('install', self.bundles[1], script=failing, success=False)
        self.assertEqual(self.version(self.dest), '1.0.0')
        self.assertFalse((self.parent / '.FastTrackUltra-install.lock').exists())
        self.assertEqual(list(self.parent.glob('.ftu-stage.*')), [])

    def test_tampered_signature_leaves_existing_driver(self):
        self.deploy('install', self.bundles[0])
        bad = self.root / 'bad.driver'
        shutil.copytree(self.bundles[1], bad)
        (bad / 'Contents/Info.plist').write_bytes((bad / 'Contents/Info.plist').read_bytes() + b'\n')
        self.deploy('install', bad, success=False)
        self.assertEqual(self.version(self.dest), '1.0.0')

    def test_wrong_bundle_identity_is_rejected_even_if_signed(self):
        bad = self.root / 'other.driver'
        shutil.copytree(self.bundles[0], bad)
        path = bad / 'Contents/Info.plist'
        info = plistlib.loads(path.read_bytes())
        info['CFBundleIdentifier'] = 'net.example.Unrelated'
        path.write_bytes(plistlib.dumps(info))
        subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(bad)], check=True, capture_output=True)
        self.deploy('install', bad, success=False)
        self.assertFalse(self.dest.exists())

    def test_source_symlink_is_rejected(self):
        link = self.root / 'linked.driver'
        link.symlink_to(self.bundles[0], target_is_directory=True)
        self.deploy('install', link, success=False)
        self.assertFalse(self.dest.exists())

    def test_intel_bundle_is_rejected_even_if_signed(self):
        bad = self.root / 'intel.driver'
        shutil.copytree(self.bundles[0], bad)
        executable = bad / 'Contents/MacOS/FastTrackUltra'
        executable.unlink()
        subprocess.run(['clang', '-arch', 'x86_64', '-bundle', '-x', 'c', '-', '-o', str(executable)],
                       input=b'int unused_fixture(void) { return 0; }', check=True, capture_output=True)
        subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(bad)], check=True, capture_output=True)
        result = self.deploy('install', bad, success=False)
        self.assertIn('Expected an arm64 Mach-O bundle', result.stderr)
        self.assertFalse(self.dest.exists())

    def test_destination_ancestor_symlink_is_rejected(self):
        elsewhere = self.root / 'elsewhere'
        elsewhere.mkdir()
        (self.root / 'Library').symlink_to(elsewhere, target_is_directory=True)
        self.deploy('install', self.bundles[0], success=False)
        self.assertEqual(list(elsewhere.iterdir()), [])

    def test_deployment_lock_is_respected(self):
        lock = self.parent / '.FastTrackUltra-install.lock'
        lock.mkdir(parents=True)
        self.deploy('install', self.bundles[0], success=False)
        self.assertTrue(lock.is_dir())
        self.assertFalse(self.dest.exists())

    def test_test_root_never_restarts_coreaudio(self):
        result = self.deploy('install', self.bundles[0], extra=['--restart'], success=False)
        self.assertIn('cannot restart system audio', result.stderr)
        self.assertFalse(self.dest.exists())


class PackageDefinitionTests(unittest.TestCase):
    def test_installer_restricts_architecture_os_and_volume(self):
        spec = importlib.util.spec_from_file_location('ftu_package', ROOT / 'scripts/package.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        import xml.etree.ElementTree as ET
        for action in ('install', 'remove'):
            doc = ET.fromstring(module.distribution(action, '0.1.0', 'driver-component.pkg'))
            self.assertEqual(doc.find('options').get('hostArchitectures'), 'arm64')
            self.assertEqual(doc.find('volume-check/allowed-os-versions/os-version').get('min'), '14.0')
            self.assertEqual(doc.find('domains').get('enable_anywhere'), 'false')
            self.assertEqual(doc.find('pkg-ref').get('id'), 'net.batarelo.FastTrackUltra.' + action)


if __name__ == '__main__':
    unittest.main()
