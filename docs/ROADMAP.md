# Development roadmap

The 48 kHz desktop-Ultra baseline is preserved in
[qualification](LOOPBACK-QUALIFICATION.md). This roadmap is work to do, not a
list of supported features. Keep one driver for Ultra and Ultra 8R, Rust for
protocol/timing/ownership policy, and thin native macOS boundaries.

## 1. Timing and monitoring latency

Current raw round trip is 110.54/113.21/115.87 ms at actual 64/128/192-frame
buffers. Input/output safety offsets total 5120 frames. The roughly 1.206 ms
timestamp remainder excludes scheduling distance and is not monitoring latency.

- Model queue lead, payload availability, ownership and capture publication.
  Measure the ranges HAL writes versus transport deadlines before reducing
  margins or experimenting with direct payload binding.
- Resolve the 512-frame zero-timestamp period and host-visible 15–192 range.
  Check SDK requirements and buffer capacity together; earlier 1024-frame
  requests succeeded but were coerced to 192.
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

Verify the shared install/update/remove and package paths on a live system and
a fresh Mac, preserving rollback and loaded-build checks. Repeat the basic
hardware baseline from this repository before calling a new build qualified.
Then prepare signed/notarized distribution and clear nontechnical instructions.
Intel support remains separate work requiring builds and test hardware.

Retain the exact artifact identity, tests and known limits with each release.
Use the [ASFireWire methods](ASFW-REUSE.md) for measurement/ownership work and
keep possible E-MU backports separately scoped.
