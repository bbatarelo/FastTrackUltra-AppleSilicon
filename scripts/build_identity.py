#!/usr/bin/env python3
"""Generate bundle metadata and a content identity, including uncommitted inputs."""
from pathlib import Path
import hashlib
import json
import plistlib
import re

ROOT = Path(__file__).resolve().parent.parent


PROFILES = ("baseline-8ms-four-slot", "prototype-4ms-eight-slot", "prototype-4ms-ten-slot", "prototype-4ms-ready", "prototype-4ms-deadline", "independent-4ms-twelve-slot")


def prepare(root=ROOT, *, output_dir=None, profile=PROFILES[0]):
    if profile not in PROFILES:
        raise ValueError("unknown transport profile")
    output_dir = root / "build" if output_dir is None else Path(output_dir)
    version = (root / 'VERSION').read_text().strip()
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', version):
        raise ValueError('VERSION must be three nonnegative integers, such as 0.1.0')
    paths = [p.relative_to(root) for folder in ('rust', 'shared', 'driver')
             for p in (root / folder).rglob('*')
             if p.is_file() and (p.suffix in ('.rs', '.h', '.c', '.plist') or p.name == 'Cargo.toml')]
    paths += [Path(p) for p in ('Cargo.toml', 'Cargo.lock', 'VERSION', 'LICENSE', 'Makefile',
                               'tools/ftu-probe/duplex.c', 'tools/ftu-probe/duplex.h',
                               'tools/ftu-probe/completion_route.c', 'tools/ftu-probe/completion_route.h',
                               'scripts/build_independent.py', 'scripts/build_identity.py', 'scripts/build_prototype.py', 'tests/transport-profile.c', 'scripts/sign.sh')]
    entries = {str(p): hashlib.sha256((root / p).read_bytes()).hexdigest()
               for p in sorted(set(paths))}
    identity = hashlib.sha256(json.dumps(dict(source_sha256=entries, transport_profile=profile), sort_keys=True).encode()).hexdigest()
    build_id = 'dev-' + identity[:16]
    info = plistlib.loads((root / 'driver/Info.plist').read_bytes())
    info['CFBundleShortVersionString'] = version
    info['CFBundleVersion'] = version
    info['FTUBuildIdentity'] = build_id
    info['FTUTransportProfile'] = profile
    info['LSMinimumSystemVersion'] = '14.0'
    outputs = {
        'build_identity.h': f'#define FTU_BUILD_ID "{build_id}"\n'.encode(),
        'Info.plist': plistlib.dumps(info),
        'build-identity.json': (json.dumps(dict(version=version, identity=identity,
                                                source_sha256=entries, transport_profile=profile), indent=2) + '\n').encode(),
    }
    output_dir.mkdir(parents=True, exist_ok=True)
    for name, content in outputs.items():
        output = output_dir / name
        if not output.exists() or output.read_bytes() != content:
            output.write_bytes(content)
    return build_id


if __name__ == '__main__':
    prepare()
