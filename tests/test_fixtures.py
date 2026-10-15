"""Verify every curated fixture against its sealed archive provenance."""
import hashlib
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class FixtureIntegrity(unittest.TestCase):
    def test_manifest_bytes_and_inventory(self):
        manifest = json.loads((ROOT / 'tests/fixtures/manifest.json').read_text())
        self.assertEqual(len(manifest['archive_commit']), 40)
        self.assertEqual(len(manifest['files']), 38)
        for name, expected in manifest['files'].items():
            with self.subTest(fixture=name):
                path = ROOT / name
                self.assertTrue(path.resolve().is_relative_to(ROOT.resolve()))
                data = path.read_bytes()
                self.assertEqual(len(data), expected['bytes'])
                self.assertEqual(hashlib.sha256(data).hexdigest(), expected['sha256'])
        actual = {str(p.relative_to(ROOT)) for p in (ROOT / 'tests/fixtures').rglob('*')
                  if p.is_file() and p.name not in ('manifest.json', 'README.md')}
        listed = {name for name in manifest['files'] if name.startswith('tests/fixtures/')}
        self.assertEqual(actual, listed)


if __name__ == '__main__':
    unittest.main()
