# Current status

As of 19 October 2026, observation candidate **`dev-b683adc88b54ab87` is installed
and verified loaded** on the desktop Fast Track Ultra, Apple Silicon, macOS
26.7.1. Its 13 isolated short runs and two ten-minute duplex sessions passed
with unchanged latency and zero steady audio losses. The five-pair tone test
also passed its instrument checks, and **the owner confirmed all five pairs
clean and correctly routed**. This qualifies the observation checkpoint for the
bounded tests described here.

This candidate adds read-only completion-delivery diagnostics. Native 48 kHz,
four 8 ms USB slots per direction, input/output safety 1024/2560, the 3072-frame
PCM offset and scheduling policy are unchanged. The short sweep reached the
unchanged **16 ms submission-lead gate exactly**; these results do not justify
reducing it. Monitoring latency remains high.

## Latest measured behavior

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
| Playback | Current candidate: two ten-minute output-3 duplex tests and owner-confirmed clean output-1/2 tones | Longer music sessions and other outputs |
| Recording | Current candidate: two ten-minute input-4 captures; prior qualified build: 216-second GarageBand take with all 36 chirps | Other inputs; input-3 same-channel loop saturation unresolved |
| Buffers | Current candidate: two loopback starts each at actual 64–4096 frames | External minimum 15 differs from declared 64; below 64 unqualified |
| Latency | Current candidate: 78.539 ms at 64 frames | Still high for monitoring |
| Two clients | Prior qualified build: silent A → A+B → B at 64/4096 and shared GarageBand recording | Not repeated on this instrumentation candidate; more client combinations |
| Applications | Prior qualified build: measured GarageBand cycling, recording and transport/project transitions; earlier VLC/Spotify owner reports | Earlier unclassified 128-frame transition loss remains unexplained; more DAWs |
| MIDI | Planned | System handling and bidirectional CoreMIDI behavior unqualified |
| Mixer/DSP | Audio uses existing hardware configuration | Effective control read-back, routing app and effects unqualified |
| Install/package | Current install verified by loaded identity and installed hash; earlier checkpoint rollback verified | Fresh-Mac setup and live `.pkg` installation remain untested |

After this installation, two read-only enumeration queries timed out while
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

The current observation bundle and all 37 source inputs are frozen under
`build/candidates/delivery-observation-20261018/`; evidence is under
`build/trials/2026-10-18-delivery-observation/`. The later
`qualification-checkpoint.json` records the hardware outcome separately from
the immutable pre-install plan. All source hashes and the installed binary
still match the frozen candidate. The offline investigation is committed as
`67286e5`. The subsequent repository checkpoint includes the qualified
instrumentation, hardware results and standalone run-loop control study.

Older checkpoints `dev-517459af36927152` and `dev-2820c37b9ed14d39` remain
preserved. The three-slot `dev-a70335afdd0f3a22` remains rejected: one run reached
6 ms against its predeclared 8 ms gate. See [the queue experiment](QUEUE-MODEL.md).

The [standalone run-loop control](RUNLOOP-STUDY.md) subsequently passed 12 runs
and 12,000 synthetic messages without touching USB or audio. It confirms that
whole-pump duration must be separated from delivery delay; it does not identify
the USB cause. Two subsequent 20-second no-producer controls also showed timeout
overshoot under both scheduling policies. Next, record all real worker pumps,
including no-callback timeouts, with wait observers optional. That follow-up
candidate is not yet implemented; no margin is reduced.
The [offline completion study](COMPLETION-STUDY.md) shows that shorter batches
alone leave modeled RTL unchanged at current offsets. Resolve native
visibility/ownership before early frame-list access, and extend production-pool,
drift and lifecycle coverage before another safety reduction. The
[ASFW comparison](ASFW-REUSE.md) records concrete methods for a later E-MU audit;
no E-MU source has changed.

Only native 48 kHz is supported. Earlier 44.1 kHz application sessions exercised
conversion. Other rates, clock modes, MIDI, hotplug/sleep/wake, automatic recovery
and multiple Ultras are roadmap work. These bounded tests cover one unit and
Mac; they do not establish a reliability guarantee or compatibility matrix.

See [ROADMAP.md](ROADMAP.md) for priorities and [PROVENANCE.md](PROVENANCE.md)
for archive identity and extraction scope.
