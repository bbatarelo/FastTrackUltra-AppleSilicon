#!/usr/bin/env python3
"""Compatibility entry point; make and the packages use the same deployment code."""
from pathlib import Path
import argparse
import subprocess

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['check', 'install', 'remove'])
    parser.add_argument('--bundle', type=Path, default=ROOT / 'build/FastTrackUltra.driver')
    parser.add_argument('--restart', action='store_true')
    args = parser.parse_args()
    command = ['/bin/sh', str(ROOT / 'scripts/deploy_hal.sh'), args.action]
    if args.action != 'remove':
        command.append(str(args.bundle.absolute()))
    if args.restart:
        command.append('--restart')
    raise SystemExit(subprocess.call(command))


if __name__ == '__main__':
    main()
