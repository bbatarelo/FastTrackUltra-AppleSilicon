# Completion-delivery observations

The observation-only candidate `dev-b683adc88b54ab87` is installed and verified
loaded. All 13 isolated short runs and both ten-minute duplex sessions passed
with unchanged latency, zero steady audio losses and valid telemetry. The
five-pair tone test passed its instrument checks and **the owner confirmed all
five pairs clean and correctly routed**. The minimum OUT lead reached the unchanged 16 ms gate exactly.

The initial enumeration stall recovered without another restart; its cause
remains unresolved. An early sweep attempt was rejected when Chrome changed the
epoch between diagnostic reads. All attempts remain preserved. Both saved Mac
audio selections were restored after the isolated tests.

The preceding qualified checkpoint and exact rollback is `dev-8387395854108015`.
The [offline investigation](COMPLETION-STUDY.md) is committed as `67286e5`.

This implements callback/run-loop correlation, bounded timing distributions and
worker CPU/policy sampling. Rust owns recording, accounting and interpretation;
C supplies native timestamps and read-only Mach queries. Native 48 kHz, four
8 ms slots per direction, the 64 ms packet-count replay, 3072-frame PCM offset,
input/output safety 1024/2560 and zero-timestamp period 16384 are unchanged.
The worker still requests period/computation/constraint 8/1/2 ms. USB
`updateFrequency=0`, callback-owned storage and the 16 ms OUT lead gate remain.

## Reading a completed run

The additive read-only `ftuD` CFString property has schema version 1. Existing
`ftuW` and `ftuT` formats are unchanged. With this candidate installed:

```sh
build/hal-check --delivery > /path/to/trial/delivery.log
python3 -B tools/hal-loopback/delivery_analysis.py /path/to/trial/delivery.log \
  --expected-build dev-b683adc88b54ab87
```

The path must be an existing trial directory. These commands do not start audio.
Read after the client has stopped, before starting another epoch; retain the
matching status, worker and timing logs too. The analyzer requires the expected
loaded build, a clean stopped state, matching epoch and internally consistent
statistics. `--allow-empty` is only for pre-audio inspection, not qualification.
`hal-observe` does not collect this new property automatically.

The recorder uses fixed atomic storage and a single writer. Readers make at
most two snapshot attempts and never lock the writer. No recorder allocation or
logging occurs in the USB loop. Reset/configuration requires the existing
quiescent lifecycle boundary. Failed observations are counted; they do not
change transport decisions or become audio faults. An incoherent snapshot,
epoch mismatch or dropped observation is unacceptable measurement evidence.

## What the measurements mean

Each pump invocation has an ID, host entry/exit ticks and the actual
`CFRunLoopRunInMode` result. A callback retains the invocation that delivered
it, even if its completion is consumed after another pump. Reusing a slot clears
old metadata. A callback outside the pump, or after ID exhaustion, has unavailable
pump metadata. This correlation never reads pending frame lists or payloads.

There are two directions, eight metrics, 18 histogram buckets per metric, two
worst packet-to-callback records, eight recent completions and two worst-lead
admissions. A worst OUT admission retains both the previous same-slot callback's
pump and the capture-feedback callback's pump. Either prerequisite can delay
admission; one must not be blamed simply because its record is present.

Steady observations start when the capture cursor reaches 48000 frames.
Completion totals include startup and drain. Draining completions are explicitly
flagged and counted, and excluded from steady histograms and worst-delivery
selection. Admissions are a separate population: a final completion need not
lead to another submission.

| Metric | Interval and interpretation |
|---|---|
| `packet_to_callback` | Last packet timestamp to callback entry; includes an unresolved native delivery interval, not a proven OS scheduler delay |
| `callback_to_consumed` | Callback entry to owner consumption; may include remaining run-loop work |
| `callback_pump_duration` | Entry to return of the callback's whole pump invocation; overlaps other measurements |
| `preparation` | Admission-check end to preparation end; elapsed time, not pure PCM CPU cost |
| `submission` | Preparation end to native submission return; can include descheduling |
| `post_submit_observation` | Submission return to the final lead-clock observation; OUT only |
| `callback_to_resubmission` | Previous same-slot callback to submission/observation end |
| `consumed_to_admission` | Previous same-slot consumption to the next admission-check start |

The first three metrics account against steady completions; the others against
steady admissions. For every histogram, count + unavailable + invalid + overflow
must equal that population. Missing endpoints and non-applicable intervals both
use unavailable: IN post-submit observations are normally all unavailable.
Unexpected missing measurements still require review even if the analyzer
accepts the arithmetic. Null extrema mean no valid observations, not zero cost.
Negative ordering and conversion overflow are reported separately.

Histogram values are integer nanoseconds converted using the recorded Mach
timebase; record timestamps remain raw host ticks. Exclusive bucket upper
bounds are 1, 2, 4, 8, 16, 32, 64, 128, 256 and 512 microseconds, then 1, 2, 4, 8,
16, 32 and 64 milliseconds; the final bucket is unbounded. Exceedance counts use
inclusive thresholds of 1, 4, 8 and 16 ms. These event-conditioned distributions
do not establish OS bounds. Do not add overlapping intervals.

## Worker sampling and observer cost

A read-only sample brackets `thread_info(THREAD_BASIC_INFO)` and
`thread_policy_get(THREAD_TIME_CONSTRAINT_POLICY)`. It retains CPU user/system
totals, basic policy, time-constraint values, the default flag, both return
statuses and sampler begin/end ticks. API failures produce unavailable fields.
There is an initial sample before anchoring, at most one periodic sample per
second after polling/admissions and before pumping, and a final sample before
policy restoration/drain. Cadence uses an independent host-clock read, so a
failed sampler cannot cause sampling on every loop.

First and last samples are retained, with total/error/default counts, maximum
sampler duration and changes between adjacent valid policy observations.
Transient changes or changes during failed reads can be missed. Matching
time-constraint parameters do not prove the thread continuously ran realtime.
Apple's [policy implementation](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/thread_policy.c)
can return saved realtime parameters; its
[basic thread information](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/thread.c)
distinguishes timeshare from round-robin, but round-robin is not unique to realtime.
The [get-default contract](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/thread_policy.h)
also matters. These sources were inspected on 18 October 2026, not matched to a
proven source revision of the running kernel.

A histogram counts intervals overlapping its most recent relevant sampler
window. Completion records retain the sampler context from their callback pump;
admissions use the latest sampler window. This flags possible direct
contamination, not every indirect effect or all older overlapping samples.
Overlap counts cannot be subtracted from individual buckets.

CPU percentage is an epoch-average estimate using first/last CPU counters and
sample midpoint elapsed time. It includes instrumentation; the underlying CPU
API has microsecond precision. It does not measure per-transfer CPU deadlines.

Offline on this Mac, the recorder benchmark's completion-plus-admission batch
means were 150.1 ns median, 163.0 ns p99 and 167.2 ns maximum. These are percentiles
of batch means, not individual-event tail costs. They exclude native pump/clock
work, Mach queries and reader contention. The native sampler averaged 1726 ns
with a 2333 ns maximum over 100 calls on a sanitizer test thread, without audio.
Neither measurement establishes realtime overhead bounds. Pump bookkeeping also
scans the fixed slot metadata (eight slots total in this HAL configuration).
The hardware comparison and its limits are recorded below.

## Hardware results, 18–19 October 2026

The installed and frozen binary SHA-256 is
`da9106f061e19e37362e334fcaf5f038676b71cc6adf8b13af92b16efcb6d4da`.
All 37 build inputs still match the frozen source manifest. The loaded build was
checked before each trial. Physical routing was rear output 3 → rear input 4,
channel 4 Rear; only input 4 was retained.

The isolated suite completed 13 short trials (one silence preflight and two
starts per actual buffer) and two ten-minute duplex sessions. All 264 chirps and
530,438 callbacks passed. It retained 326,574 completions, 326,574 admissions and
1,332 worker samples, with no dropped, invalid or overflowed observations,
unmatched steady pumps, sampler errors or sampled policy changes. IN post-submit
observations are inapplicable and were explicitly reviewed as unavailable;
no unexpected missing measurements were accepted. Steady audio losses were
zero, timestamps continuous and callback geometry unchanged. All stops were
clean with zero pending transfers. Startup playback misses remain in the logs.

| Actual buffer | Short-run raw RTL | Minimum OUT lead |
|---:|---:|---:|
| 64 | 78.539 ms | 22 ms |
| 128 | 81.206 ms | 16 ms |
| 256 | 86.539 ms | 23 ms |
| 512 | 97.206 ms | 23 ms |
| 1024 | 118.539 ms | 23 ms |
| 4096 | 246.539 ms | 19 ms |

The silence preflight had 22 ms minimum lead. Every raw measurement was within
the predeclared two-sample comparison allowance. The 128-frame run **met the
unchanged 16 ms gate exactly**, leaving no observed reserve above that gate.

| Ten-minute run | Chirps / callbacks | Raw RTL range | Minimum OUT lead | Worker CPU estimate |
|---|---:|---:|---:|---:|
| 64 frames | 120 / 450,000 | 78.539121–78.539134 ms | 19 ms | 8.966% |
| 512 frames | 120 / 56,250 | 97.205789–97.205799 ms | 20 ms | 9.638% |

The additional availability heuristic retained at least 160 frames in the short
sweep, 156 in the long 64-frame run and 151 at 512. It is not proof of contiguous
sample coverage. The long sessions each collected 601 worker samples; maximum
sampler duration was 34.916 and 36.833 microseconds respectively. Across the full
isolated suite the maximum was 36.833 microseconds. CPU figures measure the whole
worker epoch; no matched uninstrumented CPU control was collected, so they are
**not instrumentation overhead**.

First/last samples reported period/computation/constraint 8/1/2 ms, preemptible
true, basic policy 2 and get-default false, with no changes observed at the
one-second sampling cadence. This does not exclude transient scheduling changes.

The separate output-1/2 test completed five -40 dBFS lower-left/higher-right pairs
at 512 frames. Its 1,133 callbacks had no reported gaps or steady losses; final
worker/delivery analysis passed and minimum OUT lead was 23 ms. **The owner
confirmed all five pairs clean and correctly routed.** The normal output and alert selections were restored
and verified after both the automated suite and the tone test.

### What the correlation revealed

In `isolated/04-buffer-128-run-2`, IN sequence 592 and OUT sequence 584 share
the same last-packet timestamp. Input pump 2586 lasted **13.277 ms** and delivered
its callback 8.335 ms after that timestamp. Output pump 2587 lasted only
**0.021 ms**, yet the output packet-to-callback interval was **8.650 ms**:
8.629 ms had already elapsed before that output pump began. The OUT callback
was consumed about 1.292 microseconds after entry.

At the corresponding worst OUT admission, sequence 588, preparation took
0.112 ms, submission 0.172 ms and post-submit observation 0.0125 ms; lead was
16 ms. The retained sampler window ended about 754 ms before this tail, with
no direct overlap. Other events do overlap sampling and remain flagged.

This is why a short output pump cannot stand in for prompt completion delivery.
The delay can accumulate before its invocation, while the same owner is in an
input pump. The record does not distinguish kernel dispatch, waiting inside the
call or owner descheduling; the 2 ms requested run-loop wait is not an elapsed
upper bound. No pending frame-list access or ownership change follows from this.

The longer runs show the ordinary distribution and rarer delays separately:

| OUT steady completion metric | 64-frame session | 512-frame session |
|---|---:|---:|
| Completion count | 74,887 | 74,891 |
| Packet → callback p50 bucket | [0.256, 0.512) ms | [0.256, 0.512) ms |
| Packet → callback p99 bucket | [0.512, 1) ms | [0.512, 1) ms |
| Packet → callback maximum | 4.7715 ms | 4.371208 ms |
| Count ≥1 ms / ≥4 ms | 65 / 2 | 51 / 1 |
| Callback → consumption maximum | 21.208 µs | 22.000 µs |
| Preparation maximum | 274.416 µs | 275.083 µs |
| Submission maximum | 697.125 µs | 548.916 µs |
| Packet → callback sampler overlaps | 126 | 121 |

Quantiles are bucket intervals, not exact percentiles or time-weighted measures.
Maxima on different rows need not belong to the same event. Sampler overlaps
cannot be subtracted from individual buckets. These observations support
investigating the completion-delivery boundary before changing policy or margins;
they do not prove an OS scheduling bound or identify a fix.

### Retained failures and scope

Two read-only startup enumeration queries timed out with high CoreAudio CPU.
The later privileged stack sample showed mostly waiting, and a later successful
query found the same daemon at low CPU without another restart. Exact recovery
time and cause are unresolved. No startup-reliability pass is claimed.

The first sweep retained a passing silence run and a rejected 64-frame attempt:
audio passed, but Chrome opened a new epoch between diagnostic readbacks.
The subsequent isolated suite used guarded temporary routing of ordinary Mac
playback/alerts to built-in speakers and verified restoration. These are fresh
trials, not replacements for the failed evidence.

The new candidate's GarageBand and two-client transitions were not repeated.
Those results remain qualified only for the preceding binary; this observation
comparison covers one unit/Mac, the stated loopback sessions and the separate
tone test. It establishes no broader reliability guarantee.

The machine-readable `isolated-summary.json` and per-run assessments live under
`build/trials/2026-10-18-delivery-observation/`. A separate final
`qualification-checkpoint.json` beside the frozen candidate records the hardware
outcome; the original pre-install `candidate.json` remains unchanged.

## Private ABI

The C/Rust snapshot is 784 64-bit words / 6272 bytes, in this order:

| Section | Shape |
|---|---|
| Header | 28 words |
| Histograms | 2 directions × 8 metrics × 29 words |
| Worst delivery | 2 × 18 words |
| Recent completions | 8 × 18 words |
| Worst lead | 2 × 44 words |
| Worker samples | 2 × 12 words |

Header order: version, valid, epoch, timebase numerator/denominator, steady
threshold; completed IN/OUT; steady completed IN/OUT; admissions IN/OUT; steady
admissions IN/OUT; dropped; unmatched pump IN/OUT; sample count, CPU errors,
policy errors, policy defaults, invalid samples, maximum sampler ns, overflow;
drained IN/OUT; basic-policy and constraint-policy change counts.

Histogram order: count, minimum, maximum, unavailable, invalid, overflow;
18 buckets; four exceedance counts; sampler overlaps. Completion order:
ordinal, direction, sequence, slot, ticket, capture cursor, last packet ticks,
callback ticks, consumed ticks, pump ID/entry/exit/result, status, matched,
draining, sampler begin/end. Worst lead is the existing 32-word worker event
followed by two six-word pump records (previous callback, then feedback).
Pump order: ID, entry, exit, result, sampler begin/end. Sample order:
begin/end, CPU user/system ns, CPU status, policy status/default, period,
computation, constraint, preemptible, basic policy.
Statuses use signed values in JSON; the raw ABI stores their bit patterns.

The native pool completion grows to 1096 bytes: ticket + the unchanged 1040-byte
one-shot completion + 48-byte pump metadata. A worker sample is 96 bytes.
Native and HAL tests check the layouts and actual C serialization.

## Checks and hardware qualification

Offline checks pass: 121 Rust tests, Clippy with warnings denied, 120 standalone
Python/fixture tests, 32 loopback/analyzer Python tests, native and HAL boundary
tests with AddressSanitizer/UndefinedBehaviorSanitizer, and the full build.
Coverage includes callback identity across pumps and slot reuse, drain exclusion,
sampler failure/cadence, policy changes, overlap accounting, bounded concurrent
snapshots and zero allocation in the production-pool simulation.

Claude reviewed the initial implementation and sample-free context. The final
revision incorporates explicit drain accounting, both admission prerequisite
pumps, sampler placement/overlap handling, policy-change observations and unified
saturating drop accounting. That review predates these fixes; it is not an
independent review or hardware qualification of the final binary.

The hardware comparison passed these predeclared gates, including owner listening:

1. Verify installed hash and loaded identity, then unchanged geometry and a
   quiet output-3 → rear-input-4 silence preflight.
2. Run two independent starts at each actual buffer 64, 128, 256, 512, 1024 and
   4096. Capture status, timing, worker and delivery evidence from each epoch.
   Require continuous timestamps, zero steady loss, clean callback debt/stop and
   at least the unchanged 16 ms OUT post-submit lead.
3. Require valid histogram accounting, no dropped/overflow/invalid observations
   or unmatched steady pumps. Review unexpected unavailable measurements,
   sampler errors, overlapping tails and policy changes separately from audio
   faults. Never convert missing telemetry into a passing zero.
4. Repeat ten-minute duplex sessions at 64 and 512, comparing raw latency with
   78.539 and 97.206 ms respectively (initial tolerance two samples at 48 kHz).
   Preserve every attempt, including failures. Roll back on material regression.
5. Check owner listening when available. Do not infer an audible pass from
   counters or claim that observing a delay explains its cause.

Local ignored evidence and the predeclared plan live in
`build/validation/delivery-observation-20261018/`. The frozen bundle is
`build/candidates/delivery-observation-20261018/FastTrackUltra.driver`;
its source manifest and validation inputs are preserved beside it. The install
script backs up the current bundle. Its separate rollback script targets the
qualified `dev-8387395854108015` bundle, not the rejected three-slot experiment.
No audio was played, captured or restarted during the earlier offline preparation.
The subsequent installed-candidate tests are reported above.
