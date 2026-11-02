# Current status

**3 November: bounded retries after failed recovery startup are ready for hardware testing.**
On 2 November, the installed `dev-fdb892cf3bcabfec` had a clean output-deadline failure during
recovery startup, then permanently refused more attempts with one client still
registered. Restarting Spotify and changing default outputs did not release it.
The user confirmed audio returned after restarting CoreAudio. The
[failed-restart follow-up](FAILED-RESTART-RECOVERY.md) lets a newly verified clean
deadline failure use the remaining bounded attempts, with the same clients and
binding. Candidate `dev-215622171af39021` passed offline validation. It retains every timing/ownership gate; hardware qualification remains
pending. The E-MU comparison and the separate 31 October capture-startup failure
are documented there. Older dated checkpoints below describe their original state.

**30 October: disconnect teardown left the installed driver quarantined.**
`dev-369d1059e326aa6d` is installed and loaded. Its counters record 13 successful
automatic deadline recoveries, followed by an epoch-14 USB output completion
error and unsuccessful stop/drain. Reconnected USB hardware is visible, but
quarantine prevents rebind; macOS still selects the silent logical device.
[The disconnect investigation](DISCONNECT-QUARANTINE-20261030.md) preserves the
live fault, identifies stale availability/recovery reporting, and reproduces
one conservative AbortPipe/NoDevice shutdown path offline. Native stop details
remain insufficient to prove which shutdown branch occurred on hardware.
No audio restart or new installation was performed. The [disconnect retirement candidate](DISCONNECT-RECOVERY.md) now adds native
shutdown evidence, safe fully-drained removal handling and truthful terminal
reporting. Build `dev-fdb892cf3bcabfec` passed offline validation and is prepared for
installation. Hardware reconnect validation is next, before latency tuning.

**28 October offline follow-up: fault diagnostics and bounded recovery.**
The [runtime recovery implementation](RUNTIME-RECOVERY.md) records the exact
failed guard and clock readings, retains them across restarts, and automatically
reopens a clean output-deadline stop while clients remain. It preserves the USB
binding and uses a fresh clock epoch; last-client Stop and detach cancel recovery.
Unsafe cleanup and quarantine remain terminal. The candidate `dev-369d1059e326aa6d` passed offline validation and was subsequently
installed, as verified in the 30 October investigation above.
Recovery is enabled only in the experimental 4 ms/twelve-slot profile.

**28 October: installed reconnect candidate stopped on an output deadline.**
Read-only inspection verified `dev-0d800cbf6bdead51` installed/loaded, with USB
and CoreAudio still responsive. The stream is in Fault at epoch 48, reason 55
(`output_lead_deadline`), with clean teardown and zero pending transfers. No
USB removal/rebind was detected. The underlying delay is not recorded; the
older epoch-1 first-completion failure is a separate event. Evidence and the
next reliability work are in [the runtime-stop report](RUNTIME-STOP-20261028.md).
No restart, stream start, installation or audio-selection change was made.
Prioritize fault diagnostics and safe recovery before more latency tuning.

**26 October offline checkpoint: a USB reconnect fix was prepared, not yet installed.**
The previous `dev-d180f6efb5393f27` binds to its first USB location; moving the
Ultra to a hub left it visible on USB but absent from CoreAudio. The
[reconnect candidate](HOTPLUG-RECONNECT.md) rebinds the resident Rust session,
pins opening to the discovered USB enumeration, and protects new sessions from
delayed stops by retired clients. Idle/clean-drain reconnect is the intended
scope; active removal can still quarantine and physical testing is pending.
After the new runtime-stop investigation, resume this bounded reconnect check, then the fresh 64-frame
latency comparison below. Transport geometry and all latency gates are unchanged.

**The 4 ms/twelve-slot driver passed a ten-minute 128-frame duplex session;
the 64-frame latency comparison failed.** The then-installed `dev-d180f6efb5393f27`
completed 225,000 callbacks and 120 chirps at actual 128 frames,
with zero steady losses, no faults and clean teardown. Raw RTL was
81.309810–81.330649 ms, within ±2 frames of the fresh reference.
Minimum OUT return lead was 25 ms (28 ms steady), above the 16 ms gate.
The [extended result](LIVE-COMPLETION-CANDIDATE.md#64-frame-comparison-rejected-ten-minute-128-frame-session-passed)
retains the failed 64-frame comparison: clean audio/counters, but approximately
six frames above the older 78.539 ms reference. Further 64-frame tests stopped;
that historical baseline was not adjusted. Next is a matched fresh 64-frame
baseline before resuming its qualification. No margin reduction or below-10-ms
claim follows from these tests. Ordinary `make install` remains 8 ms/four slots.

As of 20 October 2026, the requested **ten-minute duplex test at actual 128
frames completed with zero steady audio losses and clean teardown**. All
120 chirps passed signal checks across 225,000 callbacks. It nevertheless
**failed the unchanged-latency gate**: raw analog round trip was 81.331 ms,
about six frames / 0.125 ms above the earlier 81.206 ms baseline, outside the
predeclared ±2-frame tolerance. The recorded shift is independently confirmed;
its cause remains open. See [the result and design decision](SHORTER-TRANSFERS.md).

The all-pump records were valid: 90.52% of the worst representative OUT delivery
interval lay inside run-loop calls, with 0.414 ms between them. Minimum OUT
submission lead was 20 ms against the unchanged 16 ms gate. The decision is to
**develop configurable 4/2/1 ms transfers offline**. The eight-slot implementation fails the stronger stall screen; ten slots
pass that screen but fail delayed-HAL sample availability. The new ready-policy
follow-up separates those deadlines and retains the same safety margins. The independent twelve-slot build passed short and ten-minute 128-frame hardware
comparisons; its 64-frame historical latency gate remains rejected; the earlier eight/ten-slot builds remain offline. No further
observation-only installation is planned. The goal is measured analog RTL below 10 ms at 48 kHz, with actual
HAL buffer size stated; the report budgets both 64 and 128 frames. The six-frame
baseline shift remains a comparison/release gate; the fresh
same-session baseline is retained for the replacement candidate comparison. No latency
qualification or lower-margin approval follows from the offline results.

**At the 21 October checkpoint, `dev-d180f6efb5393f27` was installed**, idle at epoch 5 with pending
`[0,0]`, no fault/cleanup error/first-failure record, and buffer restored to
512. Startup playback misses were 1815, 1774 and 1864 in the initial short
suite, 1603 in the 64-frame run and 1855 in the ten-minute run;
zero steady loss is not zero lifetime loss. The earlier `dev-901b05fda865ee4f` failed a
legacy ten-slot assertion before configuration and was replaced directly at
the owner's request. The regression was reproduced and fixed, with expanded
startup tests and sanitizer checks retained in the candidate evidence.

The preserved `dev-673cc272caa79014` baseline passed the fresh 21 October
pre-install silence and two 128-frame loopbacks: 9,000 callbacks, four clean
chirps and raw RTL 81.330642–81.330645 ms. It remains the exact rollback.
Audio defaults were left on the built-in devices for the comparison.

That baseline binary previously failed its first start after normal Mac reboot on
both dock/hub and direct paths, with successful 48 kHz rate writes/readbacks
followed by first-IN overrun and kernel-confirmed endpoint-0x81 babble. It later
passed an [eight-second warm recovery](STARTUP-48K.md#warm-recovery-after-full-power-removal-passed)
after USB/DC power removal without rebooting the Mac, then today's silence and
long-run audio checks. This does not establish reset causality or reboot
reliability. The owner **deferred further reboot testing**; keep it as an open
release gate rather than the next required user action. The
[offline startup audit](STARTUP-AUDIT.md) and
[separate CoreAudio restart investigation](STARTUP-RESTART.md) retain their
findings and unresolved causes.

The original [all-pump candidate](ALL-PUMP-OBSERVATION.md) was rejected at
startup and rolled back. Today's timing session used the later initializer
build that includes those observers. Its valid timing data and failed latency
comparison must not be conflated with the earlier candidate's qualification.
Production geometry is still four 8 ms slots, 64 ms count replay, PCM offset
3072 and input/output safety 1024/2560. The installed experimental runtime uses twelve 4 ms slots, with those replay/PCM/safety margins unchanged; it passed its initial three short starts and a ten-minute 128-frame session, while the 64-frame historical latency comparison failed.

The earlier qualified checkpoint's 13 isolated short runs, two ten-minute
sessions and owner-confirmed listening retain their original bounded scope.
The [ASFW comparison](ASFW-REUSE.md) now records the concrete late-PCM-fill,
transport-coverage and capture-batching mechanisms, the owner's Fireface
measurement conditions, and the corresponding later E-MU audit items.

## Qualified checkpoint measurements

The [observation qualification](DELIVERY-OBSERVATION.md) used rear output 3 →
rear input 4, channel 4 Rear. It covered one silence preflight, two independent
starts at each buffer size and ten-minute sessions at 64 and 512 frames.
All **264 chirps and 530,438 callbacks** passed the signal, geometry and timing
checks. Each run stopped cleanly and restored the 512-frame buffer.

| Actual buffer | Raw round trip |
|---:|---:|
| 64 | 78.539 ms |
| 128 | 81.206 ms |
| 256 | 86.539 ms |
| 512 | 97.206 ms |
| 1024 | 118.539 ms |
| 4096 | 246.539 ms |

Steady input/output loss remained zero. Startup playback-miss counts remain
nonzero and are preserved; zero steady loss does not mean zero lifetime misses.
The roughly 1.206 ms timestamp-relative remainder is not calibrated converter
delay or application monitoring latency.

Across the isolated sweep and longer runs, the recorder retained 326,574 transfer
completions and the same number of admissions, plus 1,332 worker samples.
No dropped/invalid/overflowed observations, unmatched steady pumps, sampler
errors or sampled policy changes were found. Minimum OUT lead was 16 ms in the
short sweep, 19 ms in the long 64-frame run and 20 ms at 512 frames. The additional
availability screen retained at least 151 frames; this heuristic does not prove
contiguous sample coverage.

The new correlation caught an input callback pump lasting 13.277 ms, followed
by a 0.021 ms output pump whose packet-to-callback interval was still 8.650 ms.
A short callback pump does not establish prompt delivery. The underlying native
wait/descheduling cause remains unresolved. The [full report](DELIVERY-OBSERVATION.md)
includes distributions, sampler cost and interpretation limits.

Normal Mac playback and alerts were temporarily routed to built-in speakers
during the isolated tests; both saved selections were restored and verified.
The tone test used outputs 1/2 at 512 frames, with 1,133 callbacks, no reported
gaps, no steady losses and at least 23 ms OUT lead. The owner independently
confirmed all five pairs clean and correctly routed.

## Scope and remaining limits

| Area | Evidence | Remaining limits |
|---|---|---|
| Desktop Ultra | Native 48 kHz, eight Float32 inputs/outputs published | More units/Macs/OS versions and all-channel tests |
| Ultra 8R | Product ID recognized by shared driver | Hardware not tested |
| Playback | Qualified checkpoint: two ten-minute output-3 duplex tests and owner-confirmed clean output-1/2 tones | Longer music sessions and other outputs |
| Recording | Qualified checkpoint: two ten-minute input-4 captures; prior qualified build: 216-second GarageBand take with all 36 chirps | Other inputs; input-3 same-channel loop saturation unresolved |
| Buffers | Qualified checkpoint: two loopback starts each at actual 64–4096 frames | External minimum 15 differs from declared 64; below 64 unqualified |
| Latency | Qualified checkpoint: 78.539 ms at 64 frames | Still high for monitoring |
| Two clients | Prior qualified build: silent A → A+B → B at 64/4096 and shared GarageBand recording | Not repeated on this instrumentation candidate; more client combinations |
| Applications | Prior qualified build: measured GarageBand cycling, recording and transport/project transitions; earlier VLC/Spotify owner reports | Earlier unclassified 128-frame transition loss remains unexplained; more DAWs |
| MIDI | Planned | System handling and bidirectional CoreMIDI behavior unqualified |
| Mixer/DSP | Audio uses existing hardware configuration | Effective control read-back, routing app and effects unqualified |
| Install/package | Qualified checkpoint rollback reverified by loaded identity and installed hash after reboot | Fresh-Mac setup and live `.pkg` installation remain untested |

After the earlier observation installation, two read-only enumeration queries timed out while
CoreAudio showed high CPU. The same daemon later recovered without another
restart. Its later stack sample showed mostly waiting and does not identify
the earlier busy code. Exact recovery time and cause remain unknown. A similar
shorter timeout occurred with the prior build. Restart/startup reliability
remains unqualified; neither driver nor OS causation is established.

An early 64-frame attempt passed its audio checks but was rejected because
Chrome opened a new epoch between diagnostic readbacks. That attempt and the
startup failures remain alongside the fresh isolated suite, rather than being
overwritten or counted as passes.

## Preserved checkpoints and next work

The preceding qualified checkpoint and exact rollback remains
**`dev-8387395854108015`**, frozen under
`build/candidates/output-safety-2560-20261018/`. Its
[output-safety reduction](PLACEMENT-MODEL.md) passed thirteen loopback runs,
two silent two-client scenarios and owner listening, then
[two ten-minute duplex runs and GarageBand transitions](LONG-SESSION.md).
Its original summaries retain their historical scope and pending states.

The preserved observation bundle and all 37 source inputs are frozen under
`build/candidates/delivery-observation-20261018/`; evidence is under
`build/trials/2026-10-18-delivery-observation/`. The later
`qualification-checkpoint.json` records the hardware outcome separately from
the immutable pre-install plan. Its frozen source hashes and binary retain that checkpoint's identity;
its later first-start failure is recorded separately from the earlier successful
qualification. The installed 48 kHz startup candidate now has warm recovery and long-run audio
evidence, plus the failed unchanged-latency comparison; its first-boot failures
remain recorded. The offline investigation is committed as
`67286e5`. The subsequent repository checkpoint includes the qualified
instrumentation, hardware results and standalone run-loop control study.

Older checkpoints `dev-517459af36927152` and `dev-2820c37b9ed14d39` remain
preserved. The three-slot `dev-a70335afdd0f3a22` remains rejected: one run reached
6 ms against its predeclared 8 ms gate. See [the queue experiment](QUEUE-MODEL.md).

The [standalone run-loop control](RUNLOOP-STUDY.md) subsequently passed 12 runs
and 12,000 synthetic messages without touching USB or audio. It confirms that
whole-pump duration must be separated from delivery delay; it does not identify
the USB cause. Two subsequent 20-second no-producer controls also showed timeout
overshoot under both scheduling policies. The [all-pump session](SHORTER-TRANSFERS.md) now provides those real-worker
records and a concrete offline shorter-transfer decision. It does not establish
a scheduler bound or pass the physical-latency comparison. Keep current margins;
resolve the native publication/lifetime contract before early input access and
validate the production pool, drift and lifecycle before lowering safety.
The [ASFW comparison](ASFW-REUSE.md) records concrete methods for a later E-MU audit;
no E-MU source has changed.

Only native 48 kHz is supported. Earlier 44.1 kHz application sessions exercised
conversion. Other rates, clock modes, MIDI, hotplug/sleep/wake, automatic recovery
and multiple Ultras are roadmap work. These bounded tests cover one unit and
Mac; they do not establish a reliability guarantee or compatibility matrix.

See [ROADMAP.md](ROADMAP.md) for priorities and [PROVENANCE.md](PROVENANCE.md)
for archive identity and extraction scope.
