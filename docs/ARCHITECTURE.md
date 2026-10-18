# Architecture

One userspace CoreAudio HAL plug-in serves Fast Track Ultra (`0763:2080`) and
Ultra 8R (`0763:2081`). Model differences belong in verified device profiles.
The desktop Ultra is the measured baseline; shared code does not qualify 8R.

```text
CoreAudio applications
        │ Float32, sample-addressed IO and HAL properties
        ▼
driver/plugin.c — AudioServerPlugIn ABI
        │ shared/ftu_live.h
        ▼
rust/ftu-stream — session, timelines, ownership, clock and scheduler
        │ shared/ftu_native.h
        ▼
tools/ftu-probe/duplex.c — native USB handles, requests and callbacks
        ▼
USB isochronous audio — 8-channel packed S24_3LE
```

`rust/ftu-protocol` provides profiles, descriptors, typed controls, packed PCM
and packet accounting. `ftu-stream` owns feedback, queues, lifecycle policy,
waveforms for diagnostics and long-lived audio timelines. Native code performs
IOKit/CoreAudio operations and translates API structures. It must not introduce
a second scheduling or recovery policy.

## Timing and ownership

Playback uses capture-derived packet counts from IN81 to plan OUT01. The
additional IN82 endpoint has been observed, but its constant feedback payload
has not established clock tracking or a replacement playback strategy.

The reusable pool uses fixed native slots and generation identities. Rust
tracks admission, submission, completion, cancellation and resource ownership.
Live capture/playback storage and clock publication are preallocated and bounded.
No control transfers, allocation, file IO, locks or formatting belong in realtime
HAL callbacks. Panics/exceptions must not unwind across the ABI.

The live HAL quarantines uncertain native resources after terminal failure
rather than freeing possibly callback-owned memory or terminating the audio
host. Recovery beyond that terminal policy is roadmap work. Client stop must
not end a session still owned by another client.

Sample frames, host ticks, stream epochs and timestamp validity are distinct.
USB scheduling frames are not a direct reading of the device audio crystal.
A stable zero-timestamp boundary must return the same sample/host pair; advancing
time and resets need explicit semantics. The Rust geometry module defines
timestamp period, HAL call capacity, safety offsets and ring size together. Playback-write admission follows the capture
cursor rather than the held zero timestamp. The [measured geometry build](TIMING-GEOMETRY.md)
passed short physical loopback at actual 64–4096-frame callbacks on the desktop
Ultra. A subsequent [observation sweep](TIMING-OBSERVATION.md) measured publication,
preparation and capture margins. The output-safety reduction to 3072 then passed the same short loopback sweep,
measuring a 21.333 ms improvement at 64 frames. Wider hardware and application
qualification is still required. The [queue experiment](QUEUE-MODEL.md)
kept native packet layouts fixed and gave each Rust pool session an immutable
active depth. Its three-slot hardware sweep missed the submission-headroom gate;
live defaults and standalone diagnostics now both select four slots. The frozen
three-slot candidate remains available for investigation, without qualification.

## Configuration and controls

The current native mode is 48 kHz, internal clock, external power. CoreAudio
publishes eight interleaved Float32 channels, converted to/from packed 24-bit
USB audio. Rate/stream changes must go through the HAL configuration-change
mechanism and coherent engine configuration; unsupported modes must be rejected.

A future companion app reads diagnostics and requests controls through the
existing device owner, outside realtime callbacks. It must not take over USB
ownership or become required for audio to run. Routing and hardware DSP need
verified selectors, gain ranges and read-back before a UI implies support.

MIDI is a separate planned capability. First verify macOS's existing handling;
then qualify or add a thin CoreMIDI boundary with Rust protocol/queue policy.
Concurrent audio/MIDI ownership and lifecycle must be explicit. Both models
share the implementation, subject to per-model evidence.

## Tools and evidence

`ftu-probe` is a bounded raw-USB diagnostic; it must not compete with an installed
HAL session. Original portable C helpers remain offline acceptance oracles for
Rust exports. `hal-check`, `hal-scenarios` and `hal-loopback` are external
CoreAudio clients, distinct from driver production code. NumPy is used only by
the offline loopback analyzer/tests.

Preserve raw/timestamp latency reference planes, actual callbacks, failed
trials and clean teardown. Selected fixture bytes are in `tests/fixtures/` with
archive hashes. Full history stays in the frozen research repository. See
[findings](FINDINGS.md), [ASFW lessons](ASFW-REUSE.md) and [roadmap](ROADMAP.md).
