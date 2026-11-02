# Development roadmap

The 48 kHz desktop-Ultra baseline is preserved in
[qualification](LOOPBACK-QUALIFICATION.md). This roadmap is work to do, not a
list of supported features. Keep one driver for Ultra and Ultra 8R, Rust for
protocol/timing/ownership policy, and thin native macOS boundaries.

## Immediate reliability work — failed recovery startup (2 November)

Qualify the [bounded failed-restart candidate](FAILED-RESTART-RECOVERY.md):
a clean deadline failure while reopening must use remaining recovery attempts
without requiring every HAL client to stop. Keep startup/USB completion failures
without that proof refused. Confirm actual Spotify audio after recovery, not
just counters. Investigate the observed 33 ms scheduling gap and retain the
latest raw USB completion error for the separate capture-startup incident.
Review persistent-failure availability and logging against E-MU's explicit
not-alive notification. Do not claim uninterrupted playback for a recovered run.

## Immediate reliability work — disconnect quarantine (30 October)

The installed recovery build records 13 successful clean-deadline restarts,
then an output completion failure, failed stop/drain and terminal quarantine.
Reconnected hardware is refused while the logical device remains selected.
Use [the live evidence and offline reproduction](DISCONNECT-QUARANTINE-20261030.md)
and [the disconnect retirement candidate](DISCONNECT-RECOVERY.md). Native
shutdown diagnostics and safe fully-drained removal handling are implemented
offline; outstanding callbacks still quarantine. Next qualify normal audio,
active disconnect/reconnect and availability notifications through actual
CoreAudio with unchanged acceptance gates.
Keep the earlier deadline-fault frequency as separate reliability work.

## Immediate reliability investigation — runtime stop (28 October)

The installed reconnect candidate stopped at the 16 ms output lead guard during
ordinary use, with clean teardown and no detected USB removal. The
[current evidence](RUNTIME-STOP-20261028.md) is preserved. The
[offline recovery follow-up](RUNTIME-RECOVERY.md) adds the exact fault record,
retained-client recovery on the same binding, a fresh clock epoch, bounded
backoff/budget, and cancellation on last Stop or detach. Normal qualification
now rejects an epoch change even if recovery completes between polls.

Offline validation is complete. Next test the prepared candidate's normal silence,
loopback and client transitions with the same gates. Real client resumption after
a runtime fault remains a hardware question; there is no production fault-injection
control. Diagnose any recurrence from its retained record. Do not tune latency
margins until this reliability check passes. Reboot testing remains deferred.

## Immediate reliability check — USB reconnect (26 October)

Qualify the [offline reconnect candidate](HOTPLUG-RECONNECT.md) before further
latency tuning: idle unplug/replug at the same port, direct-to-hub moves, repeated
reconnects, and a fresh stream afterward without restarting CoreAudio.
Check the device list, stable UID, USB generation, clean teardown and unchanged
audio/timing gates. Active removal is conditional on safe callback retirement;
quarantine must remain terminal. Sleep/wake, model changes and automatic recovery
from uncertain teardown are separate work. The [E-MU audit](EMU-HOTPLUG-AUDIT.md)
records related issues without changing that driver.

## 1. Timing and monitoring latency

**Current decision (20 October):** aim for measured analog RTL **below 10 ms at
48 kHz**, with actual buffer size stated. The [ten-minute 128-frame session and
shorter-transfer design](SHORTER-TRANSFERS.md) completed 120 chirps with zero
steady loss, but failed its ±2-frame unchanged-latency gate by approximately
six frames. The timing evidence supports moving to an offline configurable
4/2/1 ms transport, starting with 4 ms / eight slots at unchanged margins.
The [eight-slot implementation](SHORTER-TRANSFERS.md#offline-implementation-and-decision-20-october)
retains a 15 ms lead under the extra-stall screen. The
[ten-slot follow-up](TEN-SLOT-PROTOTYPE.md) improves that lead to 23 ms, but fails
the unchanged late-HAL sample-availability test. Both remain offline. The [PCM-readiness follow-up](PCM-READY-PROTOTYPE.md) now
implements a bounded preparation/submission window separately from queue
capacity, retaining immutable pending USB data and both acceptance gates.
Its 2,304-case PCM sweep and retained lead screens pass offline, including
coarse pump retries. The expanded model still rejects three cases combining
8 ms owner cadence with a 12.5 ms stall, also rejected by eager packing. The
opt-in prototype deliberately stops at a below-gate return; that availability
tradeoff must be retained in qualification results. The
[retry/cost follow-up](RETRY-COST-OBSERVATION.md) now exposes retry counters and
tests recurring 1/2 ms submission costs together with stalls, coarse pumps and
late HAL writes. The [bounded deadline-order prototype](DEADLINE-SUBMISSIONS.md)
now closes the 8 ms stall plus 1 ms/call failure (192/192 cases pass), but retains
larger combined-stall and capture-delivery rejections. The
[separate dispatch/idle model](CALLBACK-DISPATCH-MODEL.md) now compares ready-first
and wait-first service with paid callback work and independent entry delay. It
retains capture loss under 12.5 ms stalls, so it does not qualify a native
candidate. The [capture-delivery follow-up](CAPTURE-DELIVERY.md) now traces the
first missing samples and implements bounded dispatch with immediate ordered
publication in the test fixture. It fixes batching loss but retains a concrete
idle-plus-stall counterexample: the owner returns after the HAL read deadline.
The [fixed-time replay](FIXED-STALL-REPLAY.md) now reproduces the causal failure
and makes the decision: capture needs independent progress under the retained
combined absence, while output submission still fails in some fixed placements.
The [independent-completion prototype](INDEPENDENT-COMPLETION.md) now implements
generation-fenced slot return, bounded count handoff, independent PCM/clock
publication and joint scheduling in a closed-loop fixture. Ten slots still fail
OUT coverage; twelve slots pass the main and uniform 5 ms delay screens while
compounded delay remains rejected. Keep that capacity comparison explicit.
The [native boundary](NATIVE-COMPLETION.md) now tests callback routing on real
Mach-port run loops, callback-before-submit-return, explicit ACK and retained
storage across stop/abort/join. The [linked integration](INTEGRATED-COMPLETION.md)
now connects that boundary to real Rust capture/clock publication and joint
scheduling with unchanged timing gates, plus cross-language lifetime tests.
The [live candidate](LIVE-COMPLETION-CANDIDATE.md) now connects that path to the
real prepared USB interfaces. Its first hardware start exposed a legacy
ten-slot assertion in diagnostic setup before any audio callback. The corrected
build passes the expanded linked suite, including real worker startup failures.
The corrected build passes silence, two short 128-frame loopbacks and a
ten-minute 128-frame session, with zero steady losses, unchanged latency within
±2 frames of the fresh reference and minimum OUT return lead 25 ms against
16 ms. The 64-frame short recording is clean but fails the older latency
reference by about six frames, so its second short run and soak were skipped.
Next obtain a matched fresh 64-frame baseline from the preserved prior driver
before resuming that progression; retain the failed historical comparison. Ordinary installation stays on the default factory.
Actual IOUSBLib concurrency, OS scheduling and measured latency remain hardware
qualification items; retain the same loss, lead, safety and ownership gates.
The 4 ms profile alone is not a latency reduction. Budget both 64- and 128-frame HAL callbacks;
current declarations and 16 ms OUT lead cannot achieve the target.

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
- **Keep reboot/startup reliability as an unresolved release gate.** The
  [all-pump candidate](ALL-PUMP-OBSERVATION.md), diagnostic and GET-only baseline
  reproduced first-IN overrun with endpoint-0x81 babble after reboot. The
  [48 kHz initializer](STARTUP-48K.md) reproduced it on dock and direct USB,
  then streamed after USB/DC removal without a Mac reboot. Today's long-run
  signal checks also completed, but neither result proves reset causality or
  first-boot reliability. The owner deferred the next reboot comparison. Keep
  those failed attempts and the [offline sequence audit](STARTUP-AUDIT.md);
  do not bundle initialization changes or automatic retries into the latency
  prototype. The installed driver is idle and defaults remain built-in.
- Keep the separate [CoreAudio restart investigation](STARTUP-RESTART.md).
  Logged AirPlay registrations doubled across daemon restarts, reset after reboot,
  then grew from one to two on rollback and to four on diagnostic installation. Preserve this finding for
  both FTU and E-MU installation workflows; no privileged helper workaround or
  general macOS-root-cause claim is justified yet.
- Extend the independent nominal model into a production-pool simulator with
  drift, measured HAL entry times and lifecycle changes. HAL writes, PCM ownership,
  USB service and capture publication now have explicit reference coordinates. [ASFW's actual output-fill architecture](ASFW-REUSE.md)
  separates packet preparation from PCM deadlines; the earlier FTU model screens
  transport scheduling and the new model adds nominal sample placement. Neither
  establishes architectural parity or measured OS scheduling bounds.
- The [completion-delivery study](COMPLETION-STUDY.md), standalone control and
  [all-pump session](SHORTER-TRANSFERS.md) now justify closing the observation
  phase and designing shorter transfers offline. The representative 4.364 ms
  OUT interval was fully covered, with 90.52% inside pumps and 0.414 ms between
  them. This does not identify the kernel/scheduler cause. The whole session
  remains rejected by its separate unchanged-latency gate; preserve that fact.
  The versioned live-pool geometry/ABI and actual-pool fault simulation are now
  implemented; keep the eight-slot profile as the offline reference.
  Eight 4 ms slots passed the earlier nominal envelope, but an additional
  4 ms owner stall still made 16/64 phases miss the 16 ms lead screen. Replay
  measured tails and explicit extra disturbance before selecting a candidate.
  Retain capture-derived feedback, immutable pending OUT data and callback
  ownership. Early IN publication is separately gated on a valid native
  visibility/lifetime protocol. Do not copy FireWire DMA late-fill permission
  onto pending USB data. Lower safety only after actual availability and
  submission limits support the budget.
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
