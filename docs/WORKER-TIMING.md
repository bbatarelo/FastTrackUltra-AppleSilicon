# USB worker timing observations

This observation-only development build keeps four 8 ms USB slots per direction,
1024-frame input safety and 3072-frame output safety. It follows the
[rejected three-slot experiment](QUEUE-MODEL.md): audio stayed clean, but one
submission retained only 6 ms against that experiment's 8 ms headroom gate.
Aggregate minima did not identify which stage was delayed.

The installed build `dev-517459af36927152` has now passed the bounded hardware
checks below. The previous qualified `dev-2820c37b9ed14d39` remains preserved for
comparison and rollback. Matching geometry alone does not qualify a new binary.

## Read and decode

After installing a build that supports the new `ftuW` property:

```bash
build/hal-check --worker-timing > build/worker-timing.log
python3 tools/hal-loopback/worker_analysis.py build/worker-timing.log
```

These commands do not start audio, record samples or change a device setting.
Use `--allow-empty` on the analyzer for a never-started session. Older drivers
without `ftuW` report it unavailable. Existing `--timing` / `ftuT` version 1 is
unchanged and remains the source of all/steady payload-loss counters.

`worker timing` is a version-1 JSON snapshot. Its epoch must match the associated
status/read-back and the stopped measurement being analyzed. `valid: 0` means a
bounded snapshot race; it must not be interpreted as an empty successful trace.
Timebase numerator/denominator convert raw host ticks to nanoseconds. USB bus
frame numbers are integer milliseconds; PCM cursors are sample frames.

## What is retained

Rust owns the policy and fixed storage. The native USB callback already records
its delivery tick; no change to the USB function-table ABI is needed. Each
admission attempt that reaches the bus-clock check can emit a 32-word record,
identified by direction (0 IN, 1 OUT), sequence, slot and ticket. The snapshot
epoch supplies the session generation; a slot's predecessor is identified
separately. Early exits during payload preparation do not emit a completed-stage
record; existing driver fault/status evidence remains authoritative for them.

Storage retains:

- The latest 16 records, with an explicit `recent_overwritten` count.
- The first eight attention records, with `late_seen`, `late_retained` and
  `late_overflow`; later good transfers cannot erase the first event.
- The lowest observed steady lead record for each direction. Startup is excluded
  using the same 48000-captured-frame boundary as the existing timing counters.

The attention trigger is nominal submission lead minus one 8 ms block:
`(queue_depth - 2) * 8`, which is **16 ms at four slots**. A failed clock query or
submission is also retained, including during startup. This is a diagnostic
selection threshold, **not a new scheduling rule or qualification threshold**.
The rejected candidate's original 8 ms gate and result remain unchanged.

IN records use pre-submission bus lead. OUT records use the post-submission bus
query when available. No extra successful-path IN bus query was added. The
analyzer reports this distinction as `lead_reference`; do not combine those
reference planes as one measurement.

Every word in the observation storage is atomic. A single worker writes under a
fail-fast writer gate, and a generation protects the whole snapshot. Readers
make at most two attempts; reset/configuration happen while that worker is
quiescent. Storage exhaustion and contention are explicit, never a reason to
block audio. There are no sample copies, allocation, JSON formatting or file
writes in this recorder. Snapshot formatting runs only on the property path.

## Record fields and their limits

The C ABI consists of 32 unsigned 64-bit words in the order declared by Rust's
`worker_timing::Event` and the native formatter. Status words are sign-extended
native 32-bit statuses and formatted as signed integers. ABI size is 256 bytes
per record and 6760 bytes per snapshot. Rust/C boundary tests pin both sizes.

| Field group | Meaning |
|---|---|
| `ordinal`, `direction`, `sequence`, `slot`, `ticket`, `captured`, `flags` | Observation identity, captured cursor at publication, availability |
| `start_frame`, `checked_frame`, `returned_frame` | Scheduled USB start, admission bus-frame check, optional post-submit bus frame |
| `previous_sequence`, `previous_start_frame`, `previous_last_packet_ticks`, `previous_callback_ticks`, `previous_consumed_ticks` | The matching completion that released this reused slot; raw final-packet timestamp, native callback delivery and worker consumption |
| `feedback_sequence`, `feedback_last_packet_ticks`, `feedback_callback_ticks`, `feedback_consumed_ticks` | Capture feedback used by this OUT transfer; intentionally older because of 64 ms count replay |
| `loop_enter_ticks`, `poll_end_ticks`, `pump_enter_ticks`, `pump_exit_ticks` | Current worker pass and most recent completed run-loop call |
| `check_begin_ticks`, `check_end_ticks`, `prepare_end_ticks` | Admission query interval and payload preparation, including packing, diagnostics and CRC |
| `submit_begin_ticks`, `submit_end_ticks`, `query_end_ticks` | Native submit call boundaries and completion of post-submit observation |
| `check_status`, `submit_status`, `return_clock_status` | Native statuses where the corresponding operation was attempted |

Availability bits: previous completion 1, feedback 2, successful admission clock
4, completed preparation 8, submit attempted 16, successful return clock 32,
previous run-loop call 64, return-clock query attempted 128. Unavailable fields
are JSON null, including a first-use slot's predecessor and IN's absent return
query. A zero raw packet timestamp is retained but is unusable for packet-relative
analysis. Missing evidence is not a measured zero delay.

The analyzer converts differences into milliseconds and validates generation,
flags, monotonic stage order, counters, retained-window consistency and epoch.
One event present in several banks must be identical. It rejects native status
errors and dropped observations. Retention overflow is reported rather than
hidden; it limits completeness even when the retained records are valid.

Interpret the intervals carefully:

- Packet timestamp → callback delivery includes timestamp service-point semantics
  and delivery delay; it is not a pure hardware-completion-to-dispatch measurement.
- Callback delivery → consumption is separate from processing later in that poll.
- `poll_pass_ms` includes completion validation, capture publication and associated
  work; it can include work for several slots, not just the record's predecessor.
- `run_loop_ms` includes waiting and callbacks dispatched inside the most recent
  run-loop call. A record's callback may have arrived in an earlier call.
- `preparation_ms` covers the interval after the admission-clock query through
  packing and payload diagnostics. `submission_ms` surrounds only the native
  submit call. `post_submit_observation_ms` includes bookkeeping and the bus query.
- Elapsed intervals can include OS descheduling. They locate where elapsed time
  accumulated; they do not prove CPU execution time or the scheduler's cause.
- Feedback age includes the deliberate count-replay offset. It must not be
  presented as an avoidable worker delay or an extra measured audio delay.

The external loopback client's `now_host` is a CoreAudio-supplied timestamp, not
an independently sampled callback-entry clock. Do not use it to override these
worker observations or to claim exact client-thread execution time.

## Validation and hardware gate

Production-pool tests separately inject delayed callback delivery, delayed
consumption after callback, delayed PCM preparation and native-submit cost.
They check the correct intervals, slot generations, monotonic stages, final
ownership and zero loop allocations. The existing long recycled-journal test
also enables the recorder through more than 5000 transfers. Recorder tests cover
startup separation, fixed retention, overflow, worst-record preservation,
contention, reset and concurrent snapshots across epochs. The C sanitizer test
covers read-only property behavior, ABI sizes, null fields, signed errors and
large JSON output. Independent Python tests reject malformed traces and verify
timebase conversion and stage attribution.

The release benchmark measures only recorder publication, in batches. It
excludes host-clock calls, native IOKit work, snapshot formatting and scheduler
latency; it cannot establish a worst-case execution-time guarantee. Hardware
must still compare raw latency, timestamp continuity, payload loss and submission
margins against the exact four-slot checkpoint. Preserve every attempt.

The prepared sweep uses the existing quiet rear-output-3 → rear-input-4 loop,
one silence preflight and two independent starts per actual 64/128/256/512/1024/
4096-frame buffer. After each clean stop it saves both timing properties and
checks that epochs/builds still match. A low diagnostic-selection margin is
retained for analysis; the general transport gate remains greater than 1 ms.
Require clean waveform/callback/ownership results and unchanged scheduling
distance (twice the actual client buffer plus 1024 + 3072). No smaller queue or
safety setting is enabled by successful instrumentation qualification.

## Prepared checkpoint — 18 October 2026

Candidate: `dev-517459af36927152`, binary SHA-256
`1e44ce22f98f16e18fea2502a3181356e98c63c23279c6b4b37d3304493ee425`.
The exact bundle, identity manifest and hash-verified archive of all 35 build
inputs are in `build/candidates/worker-timing-20261018/`. Validation logs,
installer, rollback and sweep runner are in
`build/validation/worker-timing-20261018/`. Those local artifacts are ignored by
Git; source, analyzer and this specification remain in the contributor tree.

Offline validation passed 113 Rust tests, four timing-model tests, Clippy,
HAL and loopback callback sanitizer checks, the native probe tests, 18 Python
analysis tests and signed-bundle structural validation. The first Clippy attempt
found a style lint; its failure and successful corrected rerun are both retained.
The long production-pool simulation completed with zero loop allocations while
recording. Recorder-only release benchmark batch means were median 20.8 ns,
p99 21.8 ns and maximum 22.2 ns per publication (100 batches of 1000 events).
These are measurements on this Mac, not an end-to-end overhead bound. Recorder
storage is 6744 bytes; its exported snapshot is 6760 bytes.

The prepared runner has a `--check-only` mode that verifies hashes and all 13
fresh plans without accessing hardware. After installation, its normal mode
requires the exact candidate identity, a healthy idle device, and the expected
512-frame pre-test buffer. It saves the worker snapshot after every attempt,
even if waveform analysis fails, and requires the stopped measurement and
read-back to identify the same unchanged session. It reports raw-latency
differences from the qualified checkpoint in sample frames; these still require
review and do not silently become a statistical acceptance threshold.

## Hardware results — 18 October 2026

The exact candidate was installed and its loaded identity verified. All thirteen
prepared loopback runs passed, with 24 accepted chirps and continuous timestamps.
Actual callback sizes matched each request, scheduling distance remained twice
the actual buffer plus 4096 frames, and steady input/output loss stayed zero.
Each run stopped cleanly and restored 512 frames. Startup output misses remain
in the raw evidence. No attempt was discarded or repeated.

| Actual buffer | Raw round trip, ms | Independent starts |
|---|---|---|
| 64 | 89.205794–89.226628 | 2 |
| 128 | 91.872458–91.872465 | 2 |
| 256 | 97.205793–97.205798 | 2 |
| 512 | 107.872461–107.872463 | 2 |
| 1024 | 129.205795–129.226629 | 2 |
| 4096 | 257.205794–257.226627 | 2 |

Differences from the preserved checkpoint are within approximately one sample
(0.020833 ms). Timestamp RTL remains about 1.206–1.227 ms; it excludes the
scheduling distance and is not the usable monitoring round trip. The sweep's
minimum OUT post-submit lead was 19 ms, versus 23–24 ms in the previous short
checkpoint sweep. This variation is retained; neither short run characterizes
worst-case scheduler behavior or isolates instrumentation overhead.

The sweep recorded 26,548 worker events. Silent A → A+B → B scenarios at actual
64 and 4096 frames passed and added 10,120 events. The owner heard all five
lower-left/higher-right pairs cleanly at 512 frames; that test added 3,038 events.
Across all 39,706 observations, there were zero drops, zero attention events,
zero steady input/output misses and no native submission-clock errors. The
two-client and listening tests retained at least 23 ms of OUT submission lead.

The listening wrapper initially rejected its count check because it expected ten
status samples from a tool that explicitly runs for twelve seconds. The frozen
tool source, all twelve samples, stopped state, callback counts and timing
properties were checked offline. Counters were unchanged and the native test
returned zero. The original result is preserved alongside
`listening-assessment-corrected.json`; the wrapper was corrected and no audio
was replayed to replace the result. Owner confirmation is recorded separately
from instrument checks.

### What the linked record adds

In `06-buffer-256-run-2`, OUT ordinal 640 / sequence 315 had the minimum 19 ms
lead. Its predecessor's last packet timestamp preceded callback delivery by
5.568167 ms; delivery-to-consumption was 0.001458 ms, consumption-to-check
0.079542 ms, PCM preparation 0.109250 ms and native submission 0.106250 ms.
The neighboring IN ordinal 639 / sequence 323 retained a 12.418292 ms run-loop
call and a 5.271417 ms packet-to-callback interval. These are linked records,
not unrelated histogram minima. OUT's own most recent run-loop interval was
0.034208 ms, illustrating why its predecessor callback must not be assumed to
belong to that last call.

Most elapsed delay in this retained OUT event is before callback delivery,
not in PCM packing. The packet timestamp's service point and OS descheduling
still limit interpretation. This is evidence about this four-slot event; it does
not reproduce or establish the cause of the rejected three-slot 6 ms event.
The 234 unique retained sweep records are a biased subset selected for lowest
lead and recent history, not a full latency distribution.

### Evidence and qualification scope

All physical attempts, raw captures, worker snapshots, independent analyses,
summary and SHA-256 evidence manifest are in
`build/trials/2026-10-18-worker-timing/`. The sweep summary was frozen before the
subsequent lifecycle/listening checks; its pending fields remain historical.
`build/validation/worker-timing-20261018/qualification-checkpoint.json` records
the completed short qualification, and is copied beside the frozen bundle.
The installation receipt identifies the verified project rollback bundle; the
automatic administrator-owned backup was not independently readable.

The 16 ms attention trigger only selects retained records. It does not relax the
rejected three-slot experiment's original gate or enable a smaller queue/safety
configuration. This result covers one desktop Ultra and one Mac at native 48 kHz,
with short sessions only. Next investigate completion delivery/worker scheduling
and extend the HAL-to-USB placement model using this evidence.
