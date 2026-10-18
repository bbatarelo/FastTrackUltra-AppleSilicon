#!/usr/bin/env python3
"""Run one prepared, bounded output-3/input-3-or-4 measurement and retain evidence."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('directory', type=Path)
parser.add_argument('--buffer', type=int)
args = parser.parse_args()
path = args.directory.resolve()
if not (path/'config.json').is_file() or any((path/n).exists() for n in ('run.log','capture.f32','callbacks.csv','invocation.json')):
    parser.error('use a freshly prepared run directory')
if args.buffer is not None and not 64 <= args.buffer <= 4096:
    parser.error('buffer must be 64..4096')
command = [str(ROOT/'build/hal-loopback'),str(path)]
if args.buffer is not None:
    command.append(str(args.buffer))
config=json.loads((path/'config.json').read_text())
if config.get('input_channel') not in (3,4) or config.get('output_channel')!=3:
    parser.error('unsupported channel configuration')
command.extend(['--input',str(config['input_channel'])])
sources = ['tools/hal-loopback/main.c','tools/hal-loopback/permission.m',
           'tools/hal-loopback/analyze.py','tools/hal-loopback/timing_analysis.py','tools/hal-check/main.c',
           'scripts/run_hal_loopback.py','build/hal-loopback']
installed = Path('/Library/Audio/Plug-Ins/HAL/FastTrackUltra.driver/Contents/MacOS/FastTrackUltra')
meta = dict(started_utc=datetime.datetime.now(datetime.timezone.utc).isoformat(),argv=command,
            macos=platform.mac_ver()[0],machine=platform.machine(),
            source_revision=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
            hashes={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in sources},
            installed_sha256=hashlib.sha256(installed.read_bytes()).hexdigest())
with (path/'run.log').open('x') as log:
    proc = subprocess.Popen(command,stdout=log,stderr=subprocess.STDOUT,cwd=ROOT)
    try:
        result=proc.wait(timeout=40)
    except (subprocess.TimeoutExpired,KeyboardInterrupt):
        proc.terminate()
        try: proc.wait(timeout=3)
        except subprocess.TimeoutExpired: proc.kill();proc.wait()
        result=124
meta['returncode']=result
meta['finished_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
(path/'invocation.json').write_text(json.dumps(meta,indent=2)+'\n')
print((path/'run.log').read_text())
sys.exit(result)
