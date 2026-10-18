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

- Keep the qualified observation checkpoint `dev-b683adc88b54ab87` and its
  exact rollback `dev-8387395854108015` preserved, along with earlier
  `dev-517459af36927152` and `dev-2820c37b9ed14d39`. Bounded qualification
  does not establish general long-session application stability.
- Use the qualified [bounded worker observations](WORKER-TIMING.md) to investigate
  completion delivery and worker scheduling. The four-slot build passed 13
  physical runs, two-client checks and owner listening, with latency within about
  one sample of the previous checkpoint. Its worst retained OUT event spent
  about 5.568 ms before callback delivery, versus 0.109 ms preparing PCM and
  0.106 ms submitting. This identifies the next boundary to investigate, not the
  cause of the earlier three-slot failure. Keep that experiment's original gate
  and failed attempt; no passing rerun erases it.
- The [four-slot/output-2560 qualification](PLACEMENT-MODEL.md) now includes
  [two ten-minute duplex runs and GarageBand transitions](LONG-SESSION.md), with
  zero steady losses and unchanged latency. Minimum submission lead fell from
  22 ms in the short sweep to 17 ms in the longer runs, against the same 16 ms
  gate. Keep this measured checkpoint and its margins while investigating
  scheduling; extend to longer continuous music, other DAWs and more hosts.
  The independent model rejects a larger output cut under the selected
  HAL-publication disturbance.
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
- Follow the [completion-delivery study](COMPLETION-STUDY.md) before selecting
  shorter transfers. The [observation candidate](DELIVERY-OBSERVATION.md) adds
  callback-to-pump correlation, bounded full-epoch histograms and worker
  CPU/policy sampling at unchanged geometry. Its 13 isolated short trials and
  two ten-minute sessions passed with unchanged latency; tone instrument checks
  and owner listening passed. Minimum lead reached the unchanged
  16 ms gate exactly. A 13.277 ms input pump preceded a short output pump whose
  packet-to-callback interval was still 8.650 ms. Investigate native delivery
  and owner scheduling before changing policy or margins. The new histograms
  describe event-conditioned distributions, not OS bounds; whole-worker CPU
  is not isolated observer overhead. Preserve the prior selected-window evidence.
  The [standalone Mach-message control](RUNLOOP-STUDY.md) passed 12 runs and
  showed why whole-pump duration is insufficient. Timeout-only controls also
  exceeded the requested 2 ms under both policies. Next record all pump returns
  in the real owner, including no-callback timeouts; add wait observers only as
  a separate condition if needed. Preserve geometry and the unchanged 16 ms gate.
  In the corrected synthetic envelope, eight 4 ms slots retain 19 ms OUT lead,
  but an additional 4 ms owner stall makes 16/64 phases miss the 16 ms screen.
  Smaller batches alone do not reduce modeled RTL at unchanged offsets.
  Any prototype must jointly revisit native packet-count ABI, queue capacity,
  feedback history, publication cadence and measured worker cost. Resolve native
  publication/lifetime before early frame-list observation or submitted-buffer
  access. Keep actual missing samples separate from scheduling margin and
  characterize capture constraints before lowering input safety.
- Retain geometry/read-back regressions: the 16384-frame period now permits
  actual 4096-frame callbacks on this Mac. Extend qualification to other hosts
  and investigate the external minimum of 15 versus the declared 64.
- Keep continuous transition tracing for future candidates. The current
  GarageBand play/stop/record/seek/save/close/reopen/quit sequence had no established-
  epoch losses; the earlier 128-frame event was not reproduced and remains
  unexplained. Do not discard it because the newer sequence passed.
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
