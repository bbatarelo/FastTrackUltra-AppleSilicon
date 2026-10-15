#!/usr/bin/env python3
"""Audit saved VLC/CoreAudio observations without opening any hardware."""
import argparse
import array
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import sys
import wave


def interface_state(text):
    result = {}
    current = None
    for line in text.splitlines() + ['+-o END']:
        if '+-o ' in line:
            if current is not None and 'bInterfaceNumber' in current:
                result[str(current['bInterfaceNumber'])] = current
            current = {} if '<class IOUSBHostInterface,' in line else None
        if current is not None:
            match = re.search(r'"(bInterfaceNumber|bAlternateSetting|bNumEndpoints|bInterfaceClass|bInterfaceSubClass|bInterfaceProtocol)" = (\d+)', line)
            if match:
                current[match[1]] = int(match[2])
    return result


def audit(run):
    read = lambda name: json.loads((run / name).read_text())
    invocation, info = read('invocation.json'), read('fixture.json')
    before, after = read('before.json'), read('after.json')
    observations = read('observations.json')
    candidate = json.loads((run.parent / 'candidate.json').read_text())
    assert invocation['exit_code'] == 0 and invocation['error'] is None
    assert hashlib.sha256((run.parent / 'hal-check.bin').read_bytes()).hexdigest() == candidate['client_sha256']
    for sample in [before] + observations + [after]:
        assert 'build: ' + candidate['driver_build'] + '\n' in sample['stdout']
        assert 'alive: 1, rate: 48000, buffer: 192' in sample['stdout']
    running = [s['status'] for s in observations if s['status']['phase'] == 2]
    assert len(running) >= info['seconds']
    epoch = before['status']['epoch'] + 1
    assert all(s['epoch'] == epoch for s in running)
    assert all(s['fault'] == s['reason'] == s['cleanup'] == s['capture_missing'] == 0 for s in running + [after['status']])
    assert all(b['captured'] > a['captured'] and b['prepared'] > a['prepared'] for a, b in zip(running, running[1:]))
    # The first Running poll can land while startup zero-fill is still being
    # counted. Use the same explicit one-second warm-up boundary for every run;
    # retain all earlier observations rather than calling them steady-state.
    warmup = [s for s in running if s['captured'] < 48000]
    steady = [s for s in running if s['captured'] >= 48000]
    assert len(steady) >= info['seconds'] - 2
    misses = {s['playback_missing'] for s in steady + [after['status']]}
    assert len(misses) == 1, misses
    assert before['status']['phase'] == after['status']['phase'] == 0
    assert before['status']['pending'] == after['status']['pending'] == [0, 0]
    assert after['status']['epoch'] == epoch
    assert after['status']['captured'] >= (info['seconds'] - 1) * 48000
    interfaces = [interface_state((run / ('ioreg-' + side + '.txt')).read_text()) for side in ['before', 'after']]
    assert len(interfaces[0]) == len(interfaces[1]) == 5 and interfaces[0] == interfaces[1]
    assert all(s['bAlternateSetting'] == 0 for s in interfaces[1].values())
    raw = gzip.decompress((run / 'fixture.wav.gz').read_bytes())
    assert hashlib.sha256(raw).hexdigest() == info['sha256']
    with wave.open(io.BytesIO(raw), 'rb') as wav:
        assert (wav.getframerate(), wav.getnchannels(), wav.getsampwidth(), wav.getnframes()) == (info['rate'], 2, 2, info['rate'] * info['seconds'])
        pcm = array.array('h', wav.readframes(wav.getnframes()))
    if sys.byteorder != 'little':
        pcm.byteswap()
    peak = max(map(abs, pcm))
    assert peak == info['peak_pcm16'] and peak <= 327
    if info['signal'] == 'silence':
        assert peak == 0
    else:
        rate = info['rate']
        assert max(map(abs, pcm[1:4 * rate * 2:2])) == 0
        assert max(map(abs, pcm[4 * rate * 2:8 * rate * 2:2])) == 0
        assert max(map(abs, pcm[:4 * rate * 2:2])) > 200
        assert max(map(abs, pcm[4 * rate * 2 + 1:8 * rate * 2:2])) > 200
    logs = (run / 'stderr.txt').read_text()
    assert f'Trying to use stored audio device {before["device"]} (analog)' in logs
    assert 'analog output successfully opened' in logs
    assert f'Current AU format: [{info["rate"]}.000000]' in logs
    assert 'fallback to default' not in logs and 'using default audio device' not in logs
    errors = [line for line in logs.splitlines() if ' error:' in line]
    warnings = [line for line in logs.splitlines() if ' warning:' in line]
    rate_corrections = [line for line in logs.splitlines() if 'resampling stopped' in line]
    assert not errors, errors
    events = invocation.get('overlap_events', [])
    if events:
        assert [event['event'] for event in events] == ['secondary_launch', 'secondary_exit']
        assert all(event['status']['epoch'] == epoch and event['status']['phase'] == 2 for event in events)
        assert events[1]['exit_code'] == 0
        overlap_log = (run / 'overlap-stderr.txt').read_text()
        assert f'Trying to use stored audio device {before["device"]} (analog)' in overlap_log
        assert 'analog output successfully opened' in overlap_log
        assert 'fallback to default' not in overlap_log and ' error:' not in overlap_log
    result = dict(verdict='transport_and_app_checks_pass', epoch=epoch,
                  nominal_device_rate=48000, client_audio_unit_rate=info['rate'],
                  coreaudio_buffer_frames=192, running_status_samples=len(running),
                  startup_boundary_captured_frames=48000,
                  startup_observations=[dict(captured=s['captured'], output_missing=s['playback_missing']) for s in warmup],
                  startup_output_missing=steady[0]['playback_missing'], additional_output_missing=0,
                  capture_missing=0, stopped=after['status'], interfaces_unchanged=True,
                  fixture_peak_pcm16=peak, vlc_errors=errors, vlc_warnings=warnings,
                  rate_correction_completions=rate_corrections,
                  overlap_events=events,
                  limit='Polling driver counters does not measure application callback continuity or physical latency; listening evidence is separate.')
    (run / 'audit.json').write_text(json.dumps(result, indent=2) + '\n')
    print(f'{run.name}: PASS; epoch={epoch}, samples={len(running)}, steady misses=0, idle/debt=0, VLC warnings={len(warnings)}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    audit(parser.parse_args().run)
