# Development roadmap

The 48 kHz desktop-Ultra baseline is preserved in
[qualification](LOOPBACK-QUALIFICATION.md). This roadmap is work to do, not a
list of supported features. Keep one driver for Ultra and Ultra 8R, Rust for
protocol/timing/ownership policy, and thin native macOS boundaries.

## 1. Timing and monitoring latency

Current raw round trip is 89.206 ms at actual 64-frame buffers, down from
110.539 ms after reducing output safety from 4096 to 3072 frames. Input safety
remains 1024. The roughly 1.206 ms timestamp remainder excludes scheduling
distance and is not monitoring latency.

The [geometry build](TIMING-GEOMETRY.md) fixed timestamp-period/capacity
consistency and demonstrated actual 64–4096-frame callbacks. The subsequent
[observation and safety experiment](TIMING-OBSERVATION.md) measured spare
publication/preparation margin, then confirmed a 21.333 ms raw improvement at
64 frames with zero steady misses. The current 4096-frame raw round trip is
257.206 ms. Short hardware sweeps, silent two-client scenarios and the owner's 18 October
listening check passed; monitoring latency is still high.

- Use the preserved `dev-2820c37b9ed14d39` checkpoint for future comparisons.
  Repeat application playback/recording and longer sessions before making
  broader stability claims.
- Extend the explicit availability screen into phase/stall and application-
  transition coverage before another safety reduction. Characterize smaller USB
  batches/queue depth and capture-publication cadence as separate candidates;
  retain captured packet-count replay and immutable payload ownership.
  The current disturbance budget does not support removing another 1024 output
  frames without changing its assumptions. Keep actual missing samples separate
  from high-water margins, and measure capture constraints before changing input
  safety or experimenting with direct payload binding.
- Retain geometry/read-back regressions: the 16384-frame period now permits
  actual 4096-frame callbacks on this Mac. Extend qualification to other hosts
  and investigate the external minimum of 15 versus the declared 64.
- Trace play/stop/record/seek transitions continuously to explain the observed
  128-frame GarageBand output-miss increase between sampled phases.
- Repeat raw/timestamp loopback across actual buffers and independent starts.
  Calibrate latency declarations only after reference planes are understood.

Acceptance: a documented monitoring-latency improvement, correct host geometry,
continuous timestamps, flat steady-state loss counters, safe ownership and
clean teardown across repeated starts. Do not trade correctness for a small
latency number or infer separate converter delays from round trip alone.

## 2. Diagnostics, routing and mixer companion app

Start with a read-only app showing the loaded build, device/model, rate, actual
buffers, clock validity, faults and counters. Make support-report export explicit
and distinguish unavailable/stale values from zero. Audio must work with the app
closed; it must not become part of the realtime audio path.

Next establish reliable control selectors/ranges and effective read-back, then
expose a routing matrix, levels, mute/monitor mixes and recovery to a known
configuration. Investigate why the original output-3/input-3 loop saturated
before a test signal; hardware monitor feedback remains a hypothesis.

Acceptance: one verified route/level change at a time with predictable read-back,
rollback/recovery and no audio-thread control transfers. Serialize requests
through the existing device owner; do not create a competing USB owner.

## 3. MIDI input and output

First inspect MIDI interfaces/endpoints on each model and verify whether macOS
already exposes usable CoreMIDI endpoints. Qualify existing system support if
it works; implement the missing CoreMIDI integration only where necessary.
MIDI support is not presently claimed.

Keep USB/MIDI parsing, bounded queues and protocol policy in Rust; use a thin
native layer for CoreMIDI and OS transport APIs. Resolve USB ownership alongside
the audio session so opening MIDI does not steal or reset audio interfaces.

Acceptance covers bidirectional Note On/Off, controllers, program changes,
pitch bend, SysEx, realtime messages and timestamp ordering; sustained traffic
and overflow reporting; concurrent audio and MIDI; stop/restart, unplug/replug
and recovery. Use a physical MIDI loop or known external device, record exact
routing and test both Ultra and Ultra 8R. Do not require the companion app to
stay open for MIDI service.

## 4. On-device DSP and effects

Identify supported effect programs, send/return routing, parameter encodings,
levels and persistence per model/firmware. Begin with bypass and a known quiet
signal; validate each control with effective read-back and audible/captured
results. Keep DSP bypassed for timing baselines.

Acceptance: predictable effect selection and adjustment, safe gain staging,
known startup/reconnect state and a recovery path through the companion app.
Host-based DSP is a separate future design choice with its own latency/CPU
budget; controlling the onboard DSP does not imply adding host processing.

## 5. Audio coverage and reliability

Qualify all analog channels, longer audible/recording sessions and more DAWs.
Add native 44.1/88.2/96 kHz and internal/external/S/PDIF clock modes through
coherent configuration changes. File conversion in an app is not native-rate
support. Add device removal/reconnect, sleep/wake and fault recovery without
releasing callback-owned storage prematurely.

Acceptance: a per-model/rate/clock/OS matrix, measured clock/timing behavior,
clean multi-client lifecycle and explicit unsupported modes. Actual Ultra 8R
hardware is required; matching its product ID is not a hardware pass.

## 6. Installation and releases

The shared install/update helper has now deployed both the unchanged contributor
baseline and timing candidate with backup and loaded-build verification. Still
verify removal, `.pkg` installation and fresh-Mac setup on live systems. Retain
rollback and repeat the hardware baseline for each new build.
Then prepare signed/notarized distribution and clear nontechnical instructions.
Intel support remains separate work requiring builds and test hardware.

Retain the exact artifact identity, tests and known limits with each release.
Use the [ASFireWire methods](ASFW-REUSE.md) for measurement/ownership work and
keep possible E-MU backports separately scoped.
