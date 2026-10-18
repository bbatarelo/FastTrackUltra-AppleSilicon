# USB queue model and three-slot candidate — 18 October 2026

The qualified comparison checkpoint remains `dev-2820c37b9ed14d39`: four 8 ms slots per
USB direction, 1024/3072 input/output safety, and 89.206 ms raw round trip at
64 frames. Its loopback, listening and short two-client checks passed.
The frozen **three-slot, 1024/2560-safety candidate** is
`dev-a70335afdd0f3a22`. It passed offline tests and was installed on 18 October.
The hardware sweep confirmed 78.539 ms at 64 frames, but stopped on the seventh
run when submission headroom fell below its predeclared gate. It is **not a
qualified replacement** for the four-slot checkpoint. The installed driver and
source defaults have both returned to four slots / 3072 output safety. See the
results below.

## What the model represents

The [Rust event model](../rust/ftu-stream/examples/timing_model.rs) is an offline
executable. It independently models IN/OUT slot ownership, matching feedback
sequence, ordered capture publication, immutable in-flight packet counts,
64 ms count replay, and the 32-entry feedback history. Packet counts cycle
5/6/7 frames per USB service interval. Every completed run must conserve those
counts and release all slots/history entries. This is not a hardware emulator.

The model uses 125 microsecond ticks. The admission and post-submission checks
use integer USB milliseconds and require more than one millisecond of lead.
It sweeps 1/2/4/8 ms blocks and depths 2/3/4/6/8. For each configuration it shifts
one owner-thread stall across two complete block periods at 125 microsecond
steps, covering both parities of the callback-delay pattern: 4,800 runs total.

| Scenario | Even-sequence callback delay | Owner stall | Time spent on each submission |
|---|---:|---:|---:|
| Clean | 0 | 0 | 0 |
| Bounded disturbance | 2 ms | 8 ms | 1 ms |
| Reordering | 10 ms | 0 | 125 us |
| Beyond budget | 2 ms | 32 ms | 1 ms |

These are chosen test inputs, not measured worst-case execution times. Delayed
callback delivery does not shift packet timestamps. USB scheduling and capture
publication are distinct; the model measures both. It has no HAL audio callback
or PCM ring, operating-system scheduler, clock drift, USB errors, hardware mixer
or converter model. Its lead figures are not raw monitoring latency. Direct
production-pool tests below cover actual feedback and payload ownership.

## Results and choice

| Block / depth | Clean minimum submit lead | Bounded phases passed | Reordered phases passed | Interpretation |
|---|---:|---:|---:|---|
| 1 ms / 4 | unavailable | 0 / 16 | 0 / 16 | Even clean runs exhaust the unchanged feedback history |
| 2 ms / 4 | 6 ms | 0 / 32 | 0 / 32 | Insufficient deadline margin under these scenarios |
| 4 ms / 4 | 12 ms | 45 / 64 | 64 / 64 | Phase-dependent deadline failures |
| 4 ms / 6 | 20 ms | 64 / 64 | 64 / 64 | Worth a later transfer-size/capacity experiment |
| 8 ms / 2 | 8 ms | 62 / 128 | 0 / 128 | Insufficient deadline margin |
| **8 ms / 3** | **16 ms** | **128 / 128** | **128 / 128** | Selected next profile |
| 8 ms / 4 | 24 ms | 128 / 128 | 128 / 128 | Measured baseline geometry |

The baseline clean model reproduces the observed 24 ms submission lead and
1151–1153-frame output/capture cursor distance. The three-slot profile has
768 frames of clean cursor distance and retains at least 3 ms submit lead in
the bounded scenario, 6 ms in the reordering scenario. Both three- and four-slot
profiles reject every 32 ms stall phase. This preserves an explicit failure
boundary rather than presenting arbitrarily long stalls as supported.

Shorter blocks improve clean capture publication age in the model (about 4 ms
versus 8 ms), but require changing the native packet-count ABI and its fixed
64-packet completion records. More frequent submissions also consume the
chosen execution budget: queue duration alone does not determine feasibility.
The selected three-slot profile keeps 64 packets per transfer and the current
native ABI. Capture publication cadence remains 8 ms, so input safety stays
1024. Future 4 ms work must revisit slot count, feedback capacity, worker budget,
ABI layout and the native sanitizer tests together.

## Output-safety calculation

Reducing the queue by one 8 ms block lets the HAL publish playback later relative
to USB preparation. Use a conservative 383-frame gain: the baseline model's
minimum 1151 minus the candidate's maximum 768, allowing for packet-count phase.
For each accepted measured checkpoint run, let `M` be the smaller minimum
write/preparation lead or output high-water margin and `B` the actual callback:

```
predicted margin = M + 383 - 512
remaining        = predicted margin - B - 832
```

The 512-frame term is the output-safety reduction, 3072 → 2560. The same chosen
extra-callback plus protocol-maximum-block budget used in the previous experiment
leaves at least **538 frames** across its thirteen runs. This is a counterfactual
availability screen, not proof of contiguous PCM or a scheduling guarantee.
The model and measured baseline play different roles; actual missing samples
remain authoritative in hardware qualification.

The pre-hardware prediction was 512/48 = **10.667 ms**, from 89.206 to about
**78.539 ms** at 64 frames. The later sweep confirmed this latency at 64 frames,
but failed a separate submission-headroom gate. This remains high for software
monitoring. Expected unloaded OUT submission lead changes from 23–24 ms
to roughly 15–16 ms. Only the physical experiment can establish those values.

## Runtime implementation and validation

Rust `PoolSession` now has a validated, immutable depth within the existing
four-slot storage capacity. Polling, reuse and submission all use that depth.
The frozen candidate resolves three live slots and passes the same count to native
allocation. The original standalone probes retain four slots and their existing
evidence schema. No native production code or ABI changed.

Offline validation passed:

- 109 library/integration Rust tests and four independent model tests.
- Production-pool tests at three slots across sixteen coarse-clock phases,
  including bounded stalls, reordering, feedback conservation, immutable payload
  checksums, zero loop allocations, failure cleanup and stale-ticket retention.
- The recycled live journal exercises over 5,000 capture transfers at the new
  depth without loop allocations. Fake native operations reject access beyond
  the prepared count; live thread activation verifies that count.
- Native callback/token tests exercise three allocated slots in both directions
  and reject the fourth under AddressSanitizer/UndefinedBehaviorSanitizer.
- HAL boundary and loopback callback sanitizer checks, 13 Python analysis tests,
  full legacy probe regressions, bundle validation, Rust formatting and Clippy.

Logs retain the initial model iterator lint finding and corrected rerun under
`build/validation/queue-model-20261018/`. `model.json` records every profile's
phase acceptance count, failures and extrema; `comparison.json` hashes that
model and the measured baseline. The [model guide](../tools/timing-model/README.md)
provides commands that require no interface or administrator access.

Frozen executable SHA-256:
`7addd9920baa1f44f78e3178792fcaf1f47ba9c8595e689f9edf123364d568bc`.
The bundle, verified source-input archive and manifest are in
`build/candidates/queue-depth-3-20261018/`. Hash-checked install and rollback
helpers are in `build/validation/queue-depth-3-20261018/`; the latter restores
the measured 89.206 ms checkpoint. Fresh physical plans are under
`build/trials/2026-10-18-queue-depth-3/`.

## Hardware gate

Preserve the 89.206 ms checkpoint and freeze this candidate with an exact hash.
After installation, verify loaded identity and safety properties, then use the
existing rear output-3 → rear input-4 cable for a quiet silence preflight and
two independent chirp starts at 64/128/256/512/1024/4096 frames. Retain actual
callbacks, both latency reference planes, timing snapshots and every failure.

Require the existing waveform/epoch/cleanup/restoration gates, zero steady
misses/drops, sampled pending counts no greater than three, and at least 8 ms
observed steady OUT submit lead in this unloaded qualification sweep. The latter
is an additional experiment gate, stricter than the generic >1 ms analyzer rule.
If the model's expected preparation distance does not appear, investigate before
attributing a latency change to it. Follow a successful sweep with owner listening
and two-client checks, then longer application sessions. Rollback targets the
preserved measured build; no lower-latency qualification is inherited from it.

## Hardware outcome — qualification stopped

The installed executable and loaded identity matched the frozen candidate.
Seven of thirteen planned runs completed: one silence run and two independent
chirp starts each at 64, 128 and 256 frames. The general waveform/timing analyzer
accepted all seven runs and all twelve chirps. The experiment-specific gate
accepted six runs, then rejected `06-buffer-256-run-2`: one of its 891 steady
OUT submissions had only **6 ms** of lead against the predeclared **8 ms** minimum.
The remaining six runs, listening and two-client checks were not performed.

| Actual buffer | Raw round trip across both starts |
|---|---:|
| 64 | 78.539126–78.539129 ms |
| 128 | 81.205792–81.205794 ms |
| 256 | 86.539127–86.559962 ms |

At 64 frames the measured reduction is the predicted 10.667 ms. At 256 frames,
the two starts differ by about one sample; that spread is retained. Timestamp
RTL is about 1.206–1.227 ms; raw minus timestamp RTL remains exactly twice the
actual callback size plus 1024 + 2560 safety frames. A lower raw result does not
waive the separate headroom gate.

The rejected run had zero steady input/output missing frames, zero dropped
observations and no clock-query errors. All requested callback sizes matched,
sample timestamps were continuous and all stops were clean. Startup misses
remain recorded separately. The driver ended idle at epoch 7 with empty queues,
no fault and the original 512-frame host buffer restored.

The normal output/capture preparation distance was 767–769 frames, matching the
model's approximately 768-frame prediction. In the rejected run it briefly fell
to 384 frames. The pre-submission check and post-submission return both reached
6 ms; capture availability remained positive (minimum 413 frames), and playback
availability remained positive (minimum 1633 frames). The low lead first appears
between cumulative timing snapshots with 304 and 439 steady OUT submissions.
These aggregates cannot pair every minimum to one transfer or identify the cause.
An earlier scheduling delay is consistent with the evidence; callback delivery,
worker descheduling, polling and earlier work are not distinguished.

This is a **failed qualification margin**, not an observed audio dropout or proof
that three slots cannot work. The model itself allows lower lead under its
injected disturbances. Do not lower the experiment gate retrospectively or rerun
until a passing minimum replaces this record. The next investigation needs
bounded event timing across completion, worker wake, preparation and submission,
plus the end-to-end HAL/PCM placement model described in [ASFW reuse](ASFW-REUSE.md).
Keep the four-slot checkpoint for normal use while that work proceeds.

Evidence is retained under `build/trials/2026-10-18-queue-depth-3/`: `summary.json`,
each attempted run's original waveform/log/callback data, generic analysis and
separate `experiment-assessment.json`. `hardware-evidence.json` in the validation
directory hashes the evidence and runners. The candidate metadata records
`hardware_qualified: false` and `qualification_status: stopped_submission_headroom`.
The owner ran the prepared rollback helper on 18 October. Read-back then confirmed
`dev-2820c37b9ed14d39` loaded with the exact preserved executable hash, safety
1024/3072, idle epoch 0, empty queues, no fault and the 512-frame host buffer.
`rollback-verification.json` and `rollback-hal-check.txt` in the validation
directory record that confirmation separately from the original rejected-trial
evidence. No audio was started for rollback verification.

## Offline follow-up: identify the delayed stage

Source review after rollback found useful existing timestamps, but no complete
per-transfer timing record in the live HAL diagnostics:

- The native completion callback records `callback_ticks` with
  `mach_absolute_time`; it runs on the USB worker's run loop. This is delivery
  time to userspace, not the hardware completion instant
  ([native boundary](../tools/ftu-probe/duplex.c)).
- The worker uses `CFRunLoopRunInMode(..., 0.002, true)` between polling/submission
  passes. The 2 ms argument is a requested run-loop wait, not a measured upper
  bound on wake latency. The Rust-configured thread policy likewise is not a
  measured execution-time guarantee ([live worker](../rust/ftu-stream/src/live.rs)).
- The pool checks the bus clock before zeroing/packing the output and computing
  its payload diagnostics/CRC. `admission_ticks` is recorded only after that
  preparation, immediately before native submission. Existing check/return
  extrema therefore do not isolate preparation from submission cost
  ([pool](../rust/ftu-stream/src/pool.rs)).
- Completion records and submission timestamps exist in a recycled worker journal,
  but the HAL timing property exports aggregate counters rather than that journal.
  The independently recorded minima may belong to different transfers.
- The loopback tool's `now_host` column copies CoreAudio's supplied `now` timestamp;
  it does not call a host clock at callback entry. Treat it as an API timestamp,
  not proof of when the client thread actually executed
  ([loopback callback](../tools/hal-loopback/main.c)).

The next instrumentation should preserve one coherent bounded record per selected
late transfer, identified by epoch, direction, sequence and slot generation.
Include the old slot's callback and consumption times, relevant feedback delivery,
worker/run-loop entry and exit, admission-clock check, preparation start/end,
submission start/return and final bus-frame lead. Retain raw ticks with their
explicit timebase and distinguish USB frame numbers, packet timestamps and host
ticks. Keep unavailable values distinct from zero and distinguish IN from OUT.

Use a fixed-capacity observation buffer with explicit overwritten/dropped counts,
no samples, allocation, formatting or file writes on the worker/audio path.
Format a coherent snapshot only on the read-only diagnostics path. Keep policy
in Rust; native code should expose only timestamps the OS boundary must collect.
Measure instrumentation overhead before using its timings to change scheduling.

Regression scenarios should independently delay callback delivery, consumption
after callback, payload preparation and native submission, then verify correct
stage attribution, epoch reset, slot reuse, snapshot consistency and bounded
memory. Run an observation-only build with the qualified four-slot geometry
before another depth reduction. The 8 ms qualification gate and the rejected
candidate stay preserved. The subsequent [worker instrumentation](WORKER-TIMING.md)
implements these stage records at the four-slot geometry; its hardware
qualification is separate from both this experiment and the restored checkpoint.

## Source defaults restored after rollback

The source now selects four live slots and 3072-frame output safety again; input
safety remains 1024. Configurable-depth ownership and explicit three-slot fault
regressions are retained. The HAL boundary assertion again checks output safety
3072. The rejected candidate's frozen bundle, source archive and measurements
were not overwritten.

The rebuilt source is `dev-3d1d78ee70713e15`. All 82 stream tests, HAL boundary
AddressSanitizer/UndefinedBehaviorSanitizer checks, formatting, driver build and
bundle validation passed. Logs and the rebuilt identity are retained under
`build/validation/rollback-defaults-20261018/`. This rebuild was **not installed
or hardware-qualified**; the running binary remains the exact restored
`dev-2820c37b9ed14d39` checkpoint. No further administrator action is needed for
the completed rollback.
