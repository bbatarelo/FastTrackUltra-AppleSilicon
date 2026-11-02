# Capture delivery: batching fix and owner-unavailability limit

20 October 2026. Follow-up to the [callback-dispatch model](CALLBACK-DISPATCH-MODEL.md).
The [fixed-time replay and architecture decision](FIXED-STALL-REPLAY.md)
now complete the next experiment proposed below; this report preserves its
original results.
This is an offline investigation. The new dispatcher is a **test-only hypothesis**
inside the real Rust pool fixture; it is not wired into the native ABI or any
build profile. Normal build/install remains 8 ms / four slots. No driver bundle,
installation, audio run, daemon restart or commit is produced.

## What the first missing sample shows

The bounded trace records physical readiness, callback delivery, Rust consumption,
staging, ordered publication and the HAL deadline for the first missing sample.
It allocates its fixed 8,192-event capacity before streaming and prints afterward.
HAL reads are evaluated at their scheduled time before newly delivered capture
is published; later data cannot retrospectively repair a missed read.

The original zero-submission-cost failure at buffer 64 / phase 0 is a publication
batching problem:

| Event | Virtual time (us) |
|---|---:|
| IN sequence 373 physically ready | 1,612,000 |
| Owner returns from injected stall | 1,629,000 |
| IN callback delivered | 1,629,050 |
| HAL needs first missing sample 71,615 | 1,629,334 |
| Callback batch finishes; Rust publishes | 1,629,500 |

The callback arrived 284 us before the read, but publication waited another
450 us for the batch. All seventeen failures in this zero-cost row disappear
when ownership is reconciled and capture published between dispatches.

## Bounded dispatch experiment

The hypothesis drains already-ready sources at the existing two-attempt / 2 ms
submission checkpoints and at the regular pump. After each dispatch it runs the
existing pool poll, validation, staging and ordered-publication path before more
callback or submission work. It does not publish unvalidated or out-of-order data.

Each group permits at most ten dispatch calls and a cooperative 2 ms service
budget. The first optional idle wait is outside that service budget, matching the
retained fake; subsequent entry/callback work consumes it. An individual native
call cannot be preempted. Empty nonblocking probes terminate immediately, and a
regular pump still waits if no callback is ready. Stop/fault checks bound groups.
The ten-submission catch-up cap is not reset by an intermediate callback drain.
A drain yields after a dispatch when an eligible submission reaches its work
allowance (OUT lead 16 + 2 bus frames; IN lead 2). At the ten-attempt boundary it
always probes ready callbacks, then resumes eligible queued work without an idle
wait. The full 8 ms idle interval is retained when no eligible work can proceed.
This avoids sleeping with a submission ready, while preserving the hard cap.

The nominal hypothesis delivers one callback per source dispatch, at the same
50 us service cost. This is a chosen input, not a measured native cost; the
fixture does not separately charge Rust capture-copy/decoding time. That is an
additional measurement limit for a native candidate. The source granularity is
also an explicit model assumption: a native run-loop
source can deliver multiple USB callbacks. The sensitivity screen retains source
batch sizes 2 and 10 as well as paid entry/service cost. A native implementation
must measure and bound that behavior before this hypothesis can be promoted.

An earlier attempt returned to submissions after just one callback. It removed
the zero-cost publication failures but introduced 15-frame OUT lead failures.
Draining ready work before resuming submissions removes that regression. The
full screen then caught an independent delayed-HAL / 4096-frame regression:
after ten submissions, the worker entered an 8 ms idle interval with another
eligible OUT request queued. It returned too late for the 2 ms preparation/call
cost, leaving 15 bus frames. Probing ready callbacks and resuming queued work
fixes that scheduling error; a targeted regression retains the exact workload.
Another
variant counted the initial idle wait against the callback-service budget and
prematurely stopped the drain; its rejected results are retained too.

## Acceptance and the remaining failure

The final nominal screen accepts **2,096/2,112 positive cases**, compared with
2,077 previously. It rejects the remaining sixteen, all in the 12.5 ms stall +
1 ms per-submission workload: eight phases at buffer 64 and eight at buffer 512.
Buffer 4096 passes that row. Maximum missing input is 96 frames. Positive cases
have no transport faults, no output loss and at least 16 bus frames OUT lead.
All **1,152 deliberate negative cases** remain rejected, including paid 2 ms
per-direction saturation and the 32 ms owner stall. Regression-test success
includes recognition of those rejections; it does not mean the matrix qualifies.

| Workload | Previous accepted / 192 | New accepted / 192 |
|---|---:|---:|
| 12.5 ms stall, zero submission cost | 175 | 192 |
| 12.5 ms stall + 1 ms each submission | 174 | 176 |
| Delayed HAL + 0.5 ms preparation, 0.5 ms IN / 1.5 ms OUT | 192 | 192 |
| Each other positive workload | 192 | 192 |

The 1,728-case sensitivity screen uses the same eight representative phases,
three buffers and eight workload subset as the previous study. It does not
replace the full 64-phase screen. Each row below has 192 cases, including 24
saturated-service negative controls. Paid entry/service cost and callback batching
still materially affect results. Free callback service remains an optimistic
control; phase improvements from added cost are not a scheduling guarantee.

| Callback cost (us) | Entry delay (us) | Callbacks per source | Positive accepted / 168 | Overload rejected / 24 |
|---:|---:|---:|---:|---:|
| 0 | 0 | 1 | 168 | 0 |
| 50 | 0 | 1 | 162 | 24 |
| 50 | 250 | 1 | 162 | 24 |
| 50 | 1000 | 1 | 24 | 24 |
| 50 | 8000 | 1 | 0 | 24 |
| 250 | 0 | 1 | 161 | 24 |
| 1000 | 0 | 1 | 144 | 24 |
| 50 | 0 | 2 | 162 | 24 |
| 50 | 0 | 10 | 161 | 24 |

These columns separate the classes previously combined as “accepted / 192”.
The free-callback control accepts all 24 paid-submission saturation cases too;
it does **not** satisfy their expected rejection. Every paid-callback sensitivity
row rejects those 24 controls. The full nominal matrix separately rejects all
1,152 negative cases. This is a reporting clarification from the sealed rows,
not a changed test result or acceptance gate.

The group bound is ten source dispatch calls, with publication between them.
For the nominal one-callback source this is also ten callbacks. A batched source
is indivisible: a larger source can consume more callback work before Rust can
publish or check its deadline. The sensitivity rows do not establish a ten-USB-
callback bound for the real native API. Retry intervals exceeding the nominal
8 ms allowance remain reported; neither those counters nor input losses are
suppressed to improve acceptance.


All gates remain unchanged: 48 kHz; input/output safety 1024/2560 frames; worker
policy 8/1/2 ms; count replay 64 ms; PCM offset 3072; cutoff 26 bus frames; returned
OUT lead at least 16 integer bus frames; zero steady input/output PCM loss;
ordered count replay; immutable pending payload; zero streaming allocations;
clean drain or quarantine. The delayed HAL disturbance remains buffer + 832
frames and steady classification starts at 48,000 captured frames. Integer
bus-frame lead is not a sub-millisecond physical-lead guarantee.

A remaining buffer-64 / phase-7 / 1-ms-per-submission failure has a different cause:

| Event | Virtual time (us) |
|---|---:|
| Empty pump begins its full 8 ms idle interval | 1,622,800 |
| IN sequence 376 becomes physically ready | 1,624,000 |
| Idle ends; full 12.5 ms stall starts | 1,630,800 |
| HAL needs first missing sample 72,191 | 1,641,480 |
| Owner returns from stall | 1,643,300 |
| Callback delivered and capture immediately published | 1,643,350 |

The required request becomes ready **after** the owner stops running, and the
HAL deadline expires **before** the owner returns. Even zero callback/publication
cost cannot repair that read: the owner returns 1,820 us late. The callback and
publication are now at the same virtual time. Regression assertions preserve
this causal ordering, not just the terminal failure count.

This is a concrete counterexample for this owner schedule, not a proof that USB
2.0 cannot achieve low latency. The 1024-frame input allowance is 21.333 ms.
A 4 ms capture batch already consumes part of it; an additional full 8 ms idle
interval plus a 12.5 ms owner stall can exhaust the remaining availability
window. Increasing queue capacity preserves USB ownership but does not make
samples available to the HAL while their publisher cannot run.

## Decision and next implementation boundary

**The unchanged complete stress matrix still rejects this design.** Keep the
batch-publication fix as an offline experiment and stop treating callback order
alone as a route to qualification. Do not prepare an installer from this result.

The next design question is the owner's service/wake bound. An event-driven
wait may end when a new callback becomes ready, whereas the retained fake pays
the entire coarse interval. Those are different workloads. A paired future
experiment should model wake-on-ready separately from independent involuntary
entry lateness and retain the old idle-plus-stall counterexample. Passing an
earlier-wake model would not qualify the existing 8 ms unavailability workload.
If that full combined owner absence remains mandatory, the architecture must
make capture publication progress independently of it, or the design stays
rejected at the current safety. Merely reducing a requested native timeout,
adding slots or moving sample coordinates is insufficient evidence.

The baseline six-frame analog RTL shift and first-start-after-reboot issue remain
separate hardware/release gates. Reboot testing remains deferred. This experiment
does not lower latency or establish a macOS scheduling guarantee.

## External review and bounded next experiment (20 October)

Anthropic's Claude reviewed the timing summary after the owner's explicit sharing
approval. It did not inspect source or run tests. Its response and the exact
request are preserved separately in
`build/validation/capture-delivery-review-20261020/`; the original experiment's
sealed evidence remains unchanged.

The review agrees that the traced owner-routed publication cannot meet that
read: readiness-to-deadline is 17,480 us, while the owner returns 1,820 us after
the deadline and publication follows another 50 us later. We retain this
specific counterexample. A nominal safety-minus-transfer calculation is useful
for designing a sweep, but is not a universal bound across sample phases,
packet counts and HAL read blocks without validating those coordinates.

The most useful additional criticism is that the first-qualifying-pump stall
moves when the tested schedule changes. The existing matrix is a regression
screen, not an exhaustive worst-alignment proof. The next offline step is a
small paired absence-replay experiment before another native implementation:

1. Keep the original injector and its exact failure cases. Record realized
   absence windows and replay the same wall-time windows in comparisons. First
   reproduce the phase-7 read/deadline ordering and ensure unchanged controls;
   report any disagreement instead of retuning the fixture to pass.
2. Separate wakeable waiting, delay after a wake, per-dispatch work and externally
   imposed absence. Retain the complete 8 ms plus 12.5 ms owner-absence control.
   A wake-on-ready arm is a distinct experiment and cannot qualify that control.
   Add a focused readiness-offset sweep over one transfer and an absence-duration
   sweep around the observed deadline boundary, rather than relying only on HAL
   phase. Keep all sample, lead, loss and ownership gates unchanged.
3. Charge capture validation/copy/decoding explicitly and retain batched native
   sources. Report total time without publication, actual in-flight IN coverage,
   OUT lead and the causal first missed read. Separate expected-positive passes
   from expected-negative rejections, as clarified in the table above.
4. If the same complete owner absence is mandatory, evaluate independent capture
   progress only as an offline ownership hypothesis. Include both owner-only
   and correlated absence of every capture-capable context. A second context
   cannot pass when it is also unavailable over the required interval. A passing
   owner-only arm does not silently qualify the correlated case.

**Decision rule:** faster service or an earlier wake cannot repair the frozen
counterexample. If independent capture progress cannot meet the unchanged
workload with paid processing, in-flight coverage and safe lifetime rules,
reject this geometry under that workload. If it can, first establish a native
completion-routing and ownership contract; only then consider a candidate.
There is no installation decision from this review.

Several review suggestions need qualification. The existing entry-delay variable
is charged per dispatch, not per wake, so its sensitivity rows cannot be read as
measured scheduler wake latency. Overlapping descriptions of the same absence
must be counted once; independently justified sequential delays may add. The
2-bus-frame IN threshold is an urgency check, not a promise of two queued
transfers or a measured coverage guarantee. Calling Rust publication inside a
native callback would require a reentrancy/aliasing audit of the currently
borrowed owner state; it is not a drop-in fix. A capture split must preserve
ordered validation, preallocated handoff, ticket generations, slot reuse only
after publication, and quiescence before teardown. Capture cost remains
unmeasured, regardless of the review's expectation that it may be small.

This follow-up changes documentation and saves the review only. No additional
Rust/native behavior or test matrix is claimed; the previously completed
173 Rust and twenty standalone-model tests retain their original scope.

## Verification and evidence

Verification covers **173 Rust tests** (three observer benchmarks remain
intentionally ignored), twenty standalone-model tests, doc tests, formatting
and Clippy across all five existing profiles. The five new matrix/trace/ownership
tests ran together; the remaining workspace tests ran separately with those five
excluded, avoiding a duplicate long sweep. All final invocations passed.

All 153 retained cost rows, 96 original PCM rows, 102 earlier dispatch-cost rows,
216 earlier sensitivity rows and 36 paid-dispatch PCM rows match their sealed
values exactly. All four independent model JSON studies are unchanged. Tests
cover immutable payload CRC, ordered counts, zero streaming allocations, clean
drain, stale-ticket quarantine, stop responsiveness, empty nonblocking probes,
count bounds and a nonpreemptible callback exceeding the cooperative time budget.

Exploratory failures remain visible. They include the one-callback scheduling
regression, the premature service-budget variant, the delayed-HAL output failure,
compile/style corrections, and a fixture snapshot-initialization error exposed
by 8 ms entry delays before any OUT admission. The fixture now publishes its
initial retry snapshot as the real live worker does; no counter or loss gate is
waived. Earlier logs are diagnostic history, not qualification results.

```sh
cargo test --workspace --release --locked --offline -- --nocapture --test-threads=1
cargo test --workspace --examples --release --locked --offline
```

For the new full matrix, sensitivity, causal trace and ownership tests alone:

```sh
cargo test --release --locked --offline -p ftu-stream capture_dispatch_ -- --nocapture --test-threads=1
```

The independent sample-placement model is unchanged. Its fixed-wall-time stall
convention differs from the production fixture's first-qualifying-pump-boundary
injection. Its previous results are retained rather than used to override this
counterexample. No new hardware claim follows from either model.

Evidence lives in `build/validation/capture-delivery-offline-20261020/`, including
traces, acceptance rows, command logs, unsuccessful exploratory variants, the
incremental source patch and hashes. Six earlier evidence sets and the installed
binary are verified unchanged. The source changes that affect execution are all
under `cfg(test)`; the native ABI, driver profile and build/install scripts stay
unchanged.
