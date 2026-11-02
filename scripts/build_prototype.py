#!/usr/bin/env python3
"""Build/test the opt-in 4 ms pool offline in an isolated Cargo/output directory.

Never invokes a hardware diagnostic, installer, sudo or CoreAudio restart.
This profile preserves the baseline safety/replay settings; it is not a
low-latency release or a candidate approved for hardware installation.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess

from build_identity import ROOT, prepare

PROFILE_BY_SLOTS = {8: ('prototype-4ms-eight-slot', 'prototype-4ms'),
                    10: ('prototype-4ms-ten-slot', 'prototype-4ms-ten-slot')}
READY_PROFILE = ('prototype-4ms-ready', 'prototype-4ms-ready')
DEADLINE_PROFILE = ('prototype-4ms-deadline', 'prototype-4ms-deadline')
OUTPUT_ROOT = ROOT / 'build'
FRAMEWORKS = ['-framework', 'CoreAudio', '-framework', 'CoreFoundation',
              '-framework', 'IOKit', '-framework', 'Security', '-liconv']
FLAGS = ['-std=c11', '-O2', '-g', '-Wall', '-Wextra', '-Werror', '-Wno-deprecated-declarations']


def run(output, label, args):
    print(label, flush=True)
    log = output / (label + '.log')
    with log.open('w') as out:
        result = subprocess.run(args, cwd=ROOT, stdout=out, stderr=subprocess.STDOUT)
    if result.returncode:
        print(log.read_text()[-8000:], flush=True)
        raise SystemExit(f'{label} failed ({result.returncode}); full log: {log}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test', action='store_true', help='also run Rust and native/HAL sanitizer tests')
    parser.add_argument('--slots', type=int, choices=sorted(PROFILE_BY_SLOTS), default=8, help='slots per direction; eight-slot comparison remains the default')
    parser.add_argument('--pcm-ready', action='store_true', help='defer missing PCM within its deadline; requires --slots 10')
    parser.add_argument('--deadline', action='store_true', help='bounded deadline-ordered submissions; requires --slots 10 --pcm-ready')
    args = parser.parse_args()
    if args.deadline and not (args.slots == 10 and args.pcm_ready):
        parser.error('--deadline requires --slots 10 --pcm-ready')
    if args.pcm_ready and args.slots != 10:
        parser.error('--pcm-ready requires --slots 10')
    profile, feature = DEADLINE_PROFILE if args.deadline else READY_PROFILE if args.pcm_ready else PROFILE_BY_SLOTS[args.slots]
    output = OUTPUT_ROOT / profile
    output.mkdir(parents=True, exist_ok=True)
    # A failed rebuild must not retain a previous run's success declaration.
    (output / 'offline-result.json').unlink(missing_ok=True)
    cargo = ['--release', '--locked', '--offline', '--no-default-features',
             '--features', feature, '--target-dir', str(output / 'target')]
    run(output, 'rust-build', ['cargo', 'build', '-p', 'ftu-stream', *cargo])
    lib = str(output / 'target/release/libftu_stream.a')
    probe = str(output / 'transport-profile')
    run(output, 'transport-build', ['clang', *FLAGS, '-o', probe, 'tests/transport-profile.c', lib, *FRAMEWORKS])
    run(output, 'transport-readback', [probe, '4', str(args.slots), '26' if args.pcm_ready else '0', '1' if args.deadline else '0'])
    actual = json.loads((output / 'transport-readback.log').read_text())
    identity = prepare(ROOT, output_dir=output, profile=profile)
    bundle = output / 'FastTrackUltra.driver'
    if bundle.exists():
        shutil.rmtree(bundle)
    binary = bundle / 'Contents/MacOS/FastTrackUltra'
    binary.parent.mkdir(parents=True)
    (bundle / 'Contents/Resources').mkdir()
    shutil.copyfile(output / 'Info.plist', bundle / 'Contents/Info.plist')
    shutil.copyfile(ROOT / 'LICENSE', bundle / 'Contents/Resources/LICENSE')
    run(output, 'driver-build', ['clang', *FLAGS, '-arch', 'arm64', '-include', str(output / 'build_identity.h'),
                        '-bundle', '-mmacosx-version-min=14.0', '-o', str(binary),
                        'driver/plugin.c', 'tools/ftu-probe/duplex.c', lib, *FRAMEWORKS])
    run(output, 'driver-sign', ['/bin/sh', 'scripts/sign.sh', str(bundle)])
    if args.test:
        run(output, 'rust-tests', ['cargo', 'test', '--workspace', *cargo, '--', '--nocapture'])
        for name, source in [('native', 'tools/ftu-probe/test_pool_native.c'), ('hal', 'tests/hal-boundary.c')]:
            binary = str(output / ('test-' + name))
            run(output, name + '-build', ['clang', *FLAGS, '-O1', '-fsanitize=address,undefined',
                                 '-fno-omit-frame-pointer', '-o', binary, source, lib, *FRAMEWORKS])
            run(output, name + '-tests', [binary])
    (output / 'offline-result.json').write_text(json.dumps(dict(
        profile=profile, build=identity, rust_feature=feature, block_ms=actual['entries']//8,
        queue_depth=actual['slots'], entries=actual['entries'], feedback_slots=actual['history'],
        replay_ms=actual['replay_ms'], pcm_offset=actual['pcm_offset'],
        output_wait_ms=actual['output_wait_ms'], submission_order=actual['submission_order'],
        catch_up_attempts=10 if args.deadline else None,
        recheck_attempts=2 if args.deadline else None, recheck_ns=2000000 if args.deadline else None,
        safety=[1024,2560], policy_ms=[8,1,2], hardware_approved=False,
        sanitizer_tests_run=args.test, installed=False), indent=2)+'\n')
    print(f'Offline prototype {identity}: {bundle}\nHardware approval: no. Default build/install profile: 8 ms / four slots.')


if __name__ == '__main__':
    main()
