# Playback and capture timing observations

The instrumented build `dev-8a0b011505081ceb` was installed and measured on
17 October 2026. One silence preflight and all twelve chirp runs passed on the
desktop Ultra. Its raw round trip matches the earlier
[geometry build](TIMING-GEOMETRY.md), `dev-4130a04a1146d999`. These observation
measurements use safety offsets of 1024/4096 frames, four eight-millisecond USB
blocks per direction, and 64 ms capture-feedback replay.

The subsequent 1024/3072-frame safety experiment, `dev-2820c37b9ed14d39`,
is now installed and its physical sweep passed. It measures 89.206 ms raw round
trip at 64 frames, a 21.333 ms improvement. See the separate results below.

## Purpose

Measure the distance between HAL publication, immutable USB payload preparation,
USB submission, and HAL capture delivery before reducing margins. All timing
policy and observations remain in Rust; the native layer exposes a read-only
CoreAudio property and formats it outside realtime IO. No audio samples are
stored in the observations.

`build/hal-check --timing` reads the custom CFString property `ftuT` without
starting audio. The loopback tool retains it at startup, roughly once per second,
and after stop when available. Older drivers without the property remain
supported, but cannot qualify timing observations. The runner hashes the timing
analyzer alongside the other diagnostic sources.

## Version 1 fields

The root contains `version`, `epoch`, `valid`, `steady_after`, histogram `limits`,
and two metric maps: `all` and `steady`. The latter admits observations whose
latest completed capture cursor is at least 48000 frames. This is an explicit
one-second device-frame startup boundary, not a claim that startup losses do
not matter. `all` retains them, including preparation before HAL has started IO.

Each metric has `valid`, `count`, `min`, `max`, `last`, `dropped`, and 17 histogram
`buckets`. Empty/invalid extrema are JSON null. Bucket upper bounds are exclusive:
0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, then
an unbounded final bucket. Negative margins remain negative. The same numeric
buckets apply in each metric's own units.

| ABI index / JSON name | Unit | Observation |
|---|---|---|
| 0 `write_frames` | audio frames | Size of each successful HAL playback publication |
| 1 `write_capture_lead` | audio frames | HAL write first frame minus completed capture end, sampled after publication |
| 2 `write_preparation_lead` | audio frames | HAL write first frame minus the latest USB-requested playback end, sampled after publication |
| 3 `output_capture_lead` | audio frames | USB-requested playback first frame minus completed capture end, before copying PCM |
| 4 `output_high_water_margin` | audio frames | Newest published playback end minus USB-requested end, before copying PCM |
| 5 `output_missing` | audio frames | Actual missing frames while copying each USB output payload |
| 6 `read_frames` | audio frames | Size of each admitted HAL capture read |
| 7 `read_capture_margin` | audio frames | Completed capture end minus HAL-requested end, before copying samples |
| 8 `read_missing` | audio frames | Actual missing frames during each HAL capture read |
| 9 `submit_check_lead_ms` | USB milliseconds | OUT scheduled start minus the bus-frame check before preparation/submission |
| 10 `submit_return_lead_ms` | USB milliseconds | OUT scheduled start minus a fresh bus-frame query after successful submission |
| 11 `submit_clock_error` | boolean observation | 1 if that extra post-submit clock query failed; otherwise 0 |

The requested playback end is published **before** any sample of that USB block
is read. Consequently write lead includes preparation currently in progress,
not just blocks whose packing has finished. This cursor observes preparation;
it never blocks a write or changes the playback schedule. Output cursor values
include the existing 3072-frame playback offset. Capture and HAL frame values
are relative to the same stream epoch.

A high-water margin is not contiguous data coverage: a newer write can coexist
with a hole. The actual missing-frame metric is authoritative for that copy.
A negative write/preparation margin can signal overlap with already requested
audio, but does not by itself prove which samples were read before publication.
Likewise a capture read can overlap new publication. The metrics are local
observations, not a globally atomic ordering trace or a calibrated physical
presentation clock.

There is one additional `GetBusFrameNumber` call per successful OUT submission,
on the existing USB owner thread. Its resolution is bus milliseconds and its
measurement cost is part of the candidate to qualify. Failure is reported as
unavailable evidence, not a new transport stop condition. Native submission
errors continue to follow the existing fail/cleanup policy.

## Realtime and evidence boundaries

Fixed atomic storage is allocated with the session. Each metric makes one
fail-fast writer-gate attempt; contention increments `dropped` rather than
waiting. Readers make at most two attempts, returning invalid on a concurrent
update. Every metric is individually coherent; different metrics need not come
from one instant. A separate reset generation prevents a snapshot from being
presented as valid across an epoch reset. Reset occurs only after the worker
and admitted realtime calls are quiescent.

No allocation, formatting, lock, filesystem operation or OS call was added to
HAL audio callbacks. Snapshot formatting allocates only on the property path.
Submitted USB payload ownership and scheduling decisions are unchanged.

For instrumented loopback runs the independent analyzer requires a final valid
snapshot in the stopped epoch, every metric present, nonempty observations,
consistent histogram counts, and no dropped observations. It rejects steady
capture/output misses, unavailable submission-clock observations, and observed
post-submit lead of at most one millisecond. Existing waveform, timestamp,
health, isolated-stop and restoration gates still apply. A bounded live-read
race is retained in the log; a valid final stopped snapshot includes all totals.

## Offline validation

- 106 Rust tests passed, including coherent concurrent inspection, reset/epoch
  handling, signed margins, histogram boundaries, explicit drops, and holes
  despite positive high-water margins.
- Recycled-pool simulation exercises submission observation and verifies zero
  allocations over thousands of transfers.
- Driver and tools build; bundle validation and ad-hoc signing pass.
- HAL ABI/property and loopback callback tests pass under AddressSanitizer and
  UndefinedBehaviorSanitizer; the observation property is read-only and empty
  values remain unavailable rather than measured zeroes.
- 13 Python analysis tests pass, including malformed/reset/dropped observations,
  retained negative margins, and steady loss/deadline rejection.
- Rust formatting, Clippy with warnings denied, and whitespace checks pass.

Logs, including the initial lint finding and corrected rerun, remain under
`build/validation/timing-observation-offline/`. The known measured driver and
its exact hashed source inputs are preserved under
`build/baselines/dev-4130a04a1146d999/` before instrumentation changes.

## Hardware observations, 17 October 2026

Installed executable SHA-256:
`7f80c2a04d70d2a41874e3bd11513bcfaa56fd265c9653ad512aa2c9c117c85d`.
Tests used rear output 3 → rear input 4, channel 4 Rear, at native 48 kHz.
Only output 3 carried the -40 dBFS chirps; software input monitoring was off.
The owner authorized leaving microphone access enabled for future diagnostics.

One eight-second silence run at 64 frames passed with input peak 0.00005984.
Two independent eight-second starts at each size below accepted all 24 chirps.
Actual callback sizes matched every request; the original 512-frame buffer was
restored. Every run stopped alone, with no fault, cleanup error or pending USB
transfer. The final loaded driver was idle at epoch 13.

| Actual buffer | Raw RTL (ms) | Minimum write/preparation lead (frames) | Minimum output high-water margin (frames) | Minimum capture read margin (frames) |
|---|---:|---:|---:|---:|
| 64 | 110.539 | 2590 | 2590 | 679 |
| 128 | 113.206 | 2687 | 2687 | 705 |
| 256 | 118.539 | 2806 | 2806 | 736 |
| 512 | 129.206 | 3030 | 3030 | 681 |
| 1024 | 150.539–150.560 | 3582 | 3582 | 722 |
| 4096 | 278.539 | 6615 | 6615 | 737 |

Table minima combine the two chirp starts at each size. All thirteen runs
(including silence) collected 26,369 steady HAL writes, 26,369 steady HAL reads,
and 11,666 steady USB output preparations/submissions. There were zero observed
steady input/output misses, clock-query failures or dropped observations.
Post-submit lead was 23–24 USB milliseconds. Startup output misses remain in
`all`; they are not claimed to be fixed. The one-sample difference between
1024-frame starts reproduces the earlier measurement spread.

The timestamp-relative remainder is 1.206–1.227 ms; it still excludes the
scheduling distance and is not usable monitoring latency. These short isolated
runs do not qualify long sessions, application transitions, other Macs or Ultra
8R. They do not rule out all instrumentation cost or scheduling disturbance.

Raw evidence, invocation times, source/tool hashes, analysis and aggregate
`summary.json` are in `build/trials/2026-10-16-timing-observation/`. The directory
name records preparation on the 16th; every invocation records execution on the
17th. The exact observed binary, manifest and verified source-input archive
are in `build/candidates/timing-observation-20261016/`.

## First output-safety experiment

Reduce only output safety from 4096 to 3072 frames. Keep input safety at 1024,
USB queue depth, feedback replay, ring size, clock publication and immutable
payload ownership unchanged. The capture margin is smaller than the playback
margin, so leave capture policy for a separate experiment.

Use a deliberately limited availability calculation to choose this step. For
each accepted run, let `M` be the smaller of the observed minimum
write/preparation lead and output high-water margin, and `B` its actual callback
size. A hypothetical reduction `D` leaves `M - D`. Reserve another `B + 832`
frames: one additional callback period plus one protocol-maximum USB block
(`64 * 13` frames, larger than the ordinary 48 kHz block). The remaining
screening margin is:

```
remaining = M - D - B - 832
```

For `D = 1024`, the minimum remaining margin across all thirteen runs is
662 frames (13.79 ms at 48 kHz). For `D = 2048`, it is -362 frames. This chosen
disturbance budget supports taking the smaller first step; it is not a
measured worst-case scheduling bound or a simulation of all phases. It assumes
the same publication cadence after shifting safety and does not establish
contiguous coverage or a USB deadline guarantee. Keep both actual-miss and
submission-lead checks in the physical experiment.

Before installation, the predicted reduction was 21.333 ms, from 110.539 to
about 89.206 ms raw RTL at 64 frames. The physical results below test that
prediction; the resulting latency remains high for software monitoring.

Prepared candidate: `dev-2820c37b9ed14d39`; executable SHA-256:
`1862cad4603440f376ffcdc89c9ba97c7828b478743231f262ffcdcc09cf1241`.
Only `geometry.rs` differs among the hashed driver inputs from the measured
observation build. The HAL boundary expectation was updated to 3072.
All 106 Rust tests, HAL/loopback sanitizer checks, 13 Python tests, formatting,
Clippy and bundle validation passed. Validation logs and hash-checked install/
rollback helpers are in `build/validation/output-safety-3072-20261017/`; the
frozen bundle and exact source-input archive are in
`build/candidates/output-safety-3072-20261017/`. The owner installed this exact
bundle on 17 October. The previous observation build was backed up under
`/Library/Audio/Plug-Ins/FastTrackUltra-Backups/20261017T153206Z.0CSG4U/`.

The qualification plan repeats silence and two independent chirp starts at
each measured buffer size. Plans and results are retained under
`build/trials/2026-10-17-output-safety-3072/`.
Require actual geometry read-back, unchanged timestamp continuity, zero steady
misses, valid observations, adequate submission lead and clean stop/restoration.
Follow successful loopback with a fresh owner listening check and the silent
two-client lifecycle checks. Keep every failed attempt and roll back to the
preserved observation build if the candidate regresses.


## Reduced-output-safety hardware results, 17 October 2026

The loaded build and installed hash matched the frozen candidate; CoreAudio
reported input/output safety 1024/3072, native 48 kHz, zero-timestamp period
16384, external range 15–4096 and initial buffer 512. Wiring, chirp level,
channel isolation, tools and acceptance rules matched the observation sweep.
The same microphone authorization remained enabled.

One eight-second silence preflight passed with peak 0.00005949. All twelve
subsequent chirp runs passed: two independent starts and four accepted returns
at each of six sizes. No trial was rejected. Actual callback distributions
matched the requested sizes, including the capped final retained capture at
4096. Every run restored 512 frames and stopped idle without pending USB work,
fault or cleanup error.

| Actual buffer | Raw RTL (ms) | Minimum write/preparation lead (frames) | Minimum output high-water margin (frames) | Minimum capture read margin (frames) |
|---|---:|---:|---:|---:|
| 64 | 89.206 | 1567 | 1567 | 700 |
| 128 | 91.872 | 1643 | 1643 | 740 |
| 256 | 97.206 | 1755 | 1755 | 678 |
| 512 | 107.872 | 2029 | 2029 | 692 |
| 1024 | 129.206 | 2535 | 2535 | 736 |
| 4096 | 257.206 | 5615 | 5615 | 705 |

Across silence and chirp runs, the steady bank retained 26,385 HAL writes,
26,385 HAL reads and 11,655 USB output preparations/submissions. Actual steady
input/output misses, clock-query failures and dropped observations were zero.
Post-submit lead remained 23–24 USB milliseconds. Nonzero startup output-miss
counts remain recorded and are not a loss-free startup claim.

All 24 chirps had about 1.206 ms timestamp-relative remainder. Raw minus
remainder was exactly `2 * actual_buffer + 1024 + 3072` frames. The raw reduction
at 64 frames was 21.333 ms, or about 19.3 percent. At 1024, the baseline included
one start a sample longer; the new starts both matched its shorter result, so
the mean comparison there differs by half a sample. This is restart spread,
not evidence of a separate additional improvement.

Applying the same chosen extra-callback-plus-832-frame disturbance budget to
these observations leaves a minimum 667 frames across all thirteen runs,
compared with the baseline prediction of 662. This supports the limited
availability calculation for this step; it does not turn measured minima into
worst-case scheduling guarantees. The 89.206 ms result remains too high to call
this a low-latency monitoring driver.

Evidence is under `build/trials/2026-10-17-output-safety-3072/`, with raw input-4
captures, callback CSVs, logs, exact invocation/tool/binary hashes and aggregate
`summary.json`. The comparison script is retained with validation artifacts.
Two silent lifecycle scenarios then passed at requested/read-back 64 and 4096:
A alone for five seconds, A+B for five, then B for ten after A stopped. Both
clients reported zero invalid callbacks/gaps and clean stop/destruction. Sampled
counters and epoch stayed flat; final timing snapshots were valid, with zero
steady misses/drops and adequate submission lead. Each scenario restored 512
and stopped idle with no pending transfers, fault or cleanup error. Callback
totals were compatible with the requested sizes; per-callback distributions
come from the separate loopback sweep. The runner checks the post-stop timing
snapshot epoch against the scenario's stop, and retains that separate read.
Logs and invocation/hash/analysis records are beside the installer under
`build/validation/output-safety-3072-20261017/`.

On 18 October, the exact installed build passed a twelve-second output-1/2
listening check at the unchanged 512-frame host buffer. The tool emitted five
quiet lower-left/higher-right pairs (-40 dBFS peak, other outputs zero); the
owner confirmed: “Yes, all five pairs clean.” All 1132 callbacks had valid,
continuous output timestamps, sampled loss counters stayed flat, and final
timing observations passed. Stop/destruction returned success; the driver was
idle with no pending USB work, fault or cleanup error. The startup playback-miss
total of 3108 remained visible and unchanged throughout sampled playback.

The listening log, preflight, post-stop timing and owner confirmation are stored
as `listening-20261018T082439Z.*` in the validation directory. The frozen
candidate now has `qualification-checkpoint.json`, linking the exact source
archive and measured loopback/lifecycle/listening evidence by hash. This is the
preserved comparison for subsequent latency work. Longer sessions, GarageBand
transitions and other hardware remain separate work.
