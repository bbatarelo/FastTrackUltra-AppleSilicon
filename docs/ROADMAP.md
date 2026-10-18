# Development roadmap

The 48 kHz desktop-Ultra baseline is preserved in
[qualification](LOOPBACK-QUALIFICATION.md). This roadmap is work to do, not a
list of supported features. Keep one driver for Ultra and Ultra 8R, Rust for
protocol/timing/ownership policy, and thin native macOS boundaries.

## 1. Timing and monitoring latency

The qualified four-slot checkpoint `dev-8387395854108015` now measures
**78.539 ms at actual 64-frame buffers**, down from 89.206 ms and the earlier
110.539 ms. Input safety remains 1024; output safety is now 2560, with four
8 ms USB slots unchanged. Its sweep, two-client checks and owner listening passed.
The earlier three-slot experiment reached the same latency but remains rejected:
one 256-frame run reached 6 ms submission lead against its required 8 ms gate.

The [geometry build](TIMING-GEOMETRY.md) fixed timestamp-period/capacity
consistency and demonstrated actual 64–4096-frame callbacks. The subsequent
[observation and safety experiment](TIMING-OBSERVATION.md) confirmed a 21.333 ms
raw improvement with zero steady misses, two-client checks and owner listening.
The [three-slot outcome](QUEUE-MODEL.md) preserves all seven attempts, the
10.667 ms further reduction and its separate failed headroom check. The roughly
1.206 ms timestamp remainder excludes scheduling and is not monitoring latency.

- Keep the current `dev-8387395854108015`, its exact rollback
  `dev-517459af36927152`, and preceding `dev-2820c37b9ed14d39` preserved.
  Short qualification does not establish long-session application stability.
- Use the qualified [bounded worker observations](WORKER-TIMING.md) to investigate
  completion delivery and worker scheduling. The four-slot build passed 13
  physical runs, two-client checks and owner listening, with latency within about
  one sample of the previous checkpoint. Its worst retained OUT event spent
  about 5.568 ms before callback delivery, versus 0.109 ms preparing PCM and
  0.106 ms submitting. This identifies the next boundary to investigate, not the
  cause of the earlier three-slot failure. Keep that experiment's original gate
  and failed attempt; no passing rerun erases it.
- Extend the [four-slot/output-2560 qualification](PLACEMENT-MODEL.md) to longer
  sessions and application transitions. The approximately 10.667 ms prediction
  was confirmed; minimum submission lead was 22 ms and steady losses stayed zero.
  The independent model rejects a larger output cut under the selected
  HAL-publication disturbance. Keep this measured checkpoint before another change.
- Investigate restart/startup reliability. The first read-only enumeration after
  the latest install timed out during high CoreAudio CPU use; a later query and
  all audio tests succeeded. Preserve the diagnostic evidence and establish cause
  before attributing it to this driver or macOS.
- Extend the independent nominal model into a production-pool simulator with
  drift, measured HAL entry times and lifecycle changes. HAL writes, PCM ownership,
  USB service and capture publication now have explicit reference coordinates. [ASFW's actual output-fill architecture](ASFW-REUSE.md)
  separates packet preparation from PCM deadlines; the earlier FTU model screens
  transport scheduling and the new model adds nominal sample placement. Neither
  establishes architectural parity or measured OS scheduling bounds.
- Evaluate shorter transfers using recorded scheduling distributions. A 4 ms
  experiment must jointly revisit native packet-count ABI, queue capacity,
  feedback history and worker budget. The current model rejects naive 1 ms
  transfers with unchanged feedback capacity and finds phase-dependent deadline
  failures for four 4 ms slots. Keep actual missing samples separate from margins;
  characterize capture constraints before lowering input safety, and prove native
  ownership/visibility before considering any writes to submitted USB payloads.
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
