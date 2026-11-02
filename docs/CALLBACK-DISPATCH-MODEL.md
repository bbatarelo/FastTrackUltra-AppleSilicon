# Callback dispatch and idle waiting — offline model

20 October 2026. Follow-up to [deadline-ordered submissions](DEADLINE-SUBMISSIONS.md).
This experiment separates callback availability, callback service and idle waiting
in two offline models. It adds no native pump, ABI, driver profile or installer.
Normal build/install remains 8 ms / four slots. The installed driver is unchanged.

## Question and model contract

The previous fake advanced its clock by the full pump interval, then made all due
callbacks visible together. It could distinguish submission work from a pump,
but could not charge callback service independently or represent a ready queue
that could be serviced without first waiting. Its behavior remains available as
`Legacy`, and every retained PCM/cost result is checked against sealed evidence.

The new fake has three states for each request: not ready, ready but undelivered,
and delivered but not yet consumed by Rust. Clock advancement and completion
queries do not dispatch callbacks. A pump marks individual deliveries after
charging their service cost. Pending USB storage remains owned until the matching
completion is consumed; the existing payload and count checks still apply.

Two policies use the same callback cost and idle interval:

- Wait first: pay the idle interval on every pump, then service ready callbacks.
- Ready first: service already-ready callbacks without an idle wait. If none are
  ready, pay the same idle interval before checking again. Delivered but unconsumed
  requests cannot cause an empty pump to spin.

The main production-code comparison charges 50 microseconds per delivered
callback, including OUT, with at most ten callbacks and a cooperative 2 ms service
budget per pump. The existing ten-submission catch-up cap remains separate.
One slow callback may exceed the time budget; subsequent work is stopped on
return. Callback service advances the independent HAL clock before input is
published, so a callback batch can itself cause capture loss. These are chosen
model inputs, **not measured native callback costs or OS latency guarantees**.
An independent entry cost is paid even when ready work avoids an idle wait.

The full screen retains 8 ms idle waits, 17 workloads, buffers 64/512/4096, all
64 HAL/USB phases and 600 transfers per direction: 3,264 cases per policy.
The one-shot stall is injected at the first qualifying pump boundary, including
entry, return from idle, and callback service. It cannot disappear because a
ready-first pump skips a wait. Successful stalled runs assert exactly one full
stall charge. This boundary-injection convention is distinct from the independent
sample model's fixed wall-time stall window; results must not be merged.

The sensitivity screen uses eight representative phases per row and explicitly
labels the reduced coverage. It varies callback service (0/50/250/1,000 us), entry
delay (0/250/1,000/8,000 us) and callback batch limit (1/2/10), using specified
combinations rather than a full Cartesian product. Zero callback cost is only
an optimistic control. All combinations retain the same admission and loss gates.

## Acceptance results

The ready-first model accepts **2,077/3,264 cases**, versus **1,541/3,264** for
wait-first at the same 50 us callback cost. The preserved legacy deadline screen
accepts 1,815/3,264, but does not charge callback service: it is a historical
reference, not a cost-matched comparison.

| Workload (192 cases each) | Legacy deadline | Wait first, 50 us | Ready first, 50 us |
|---|---:|---:|---:|
| 1 ms each IN/OUT submission | 192 | 192 | 192 |
| 8 ms stall + 1 ms each submission | 192 | 192 | 192 |
| 12.5 ms stall, zero submission cost | 192 | 175 | 175 |
| 12.5 ms stall + 1 ms each submission | 120 | 22 | 174 |
| Uniform 5 ms delivery delay + 1 ms each submission | 159 | 0 | 192 |
| Delayed HAL + 0.5 ms preparation, 0.5 ms IN / 1.5 ms OUT | 0 | 0 | 192 |
| 2 ms each IN/OUT submission | 0 | 0 | 0 |

The six deliberate overload/negative-control workloads remain rejected in all
1,152 cases for each paid-service policy. Of the 2,112 intended positive cases,
ready-first still rejects **35**: seventeen in the zero-submission-cost 12.5 ms
stall row and eighteen when adding 1 ms per submission. All 35 complete without
transport faults and with adequate OUT lead; they fail because of missing input
PCM. The maximum loss in those rows is respectively 17 and 112 sample frames.
The 4096-frame buffer passes both rows; smaller buffers retain the failures.
This rules out qualifying the approach from terminal status or output lead alone.

The ready-first model also passes all **2,304** original PCM-phase cases at
1/4/8 ms idle intervals and all six original HAL buffer sizes. Every one of the
96 retained PCM rows and 153 retained cost rows matches the prior sealed values
exactly, including failed controls, lead measurements and retry counters.

Accepted runs can still report retry intervals above the nominal 8 ms allowance
because callback service itself consumes time (for example 8.1–8.3 ms). Those
counters remain visible; they do not silently redefine the lead/loss acceptance
gates or establish a new timing guarantee.

The 1,728-case sensitivity screen confirms that free callback service can hide
saturation and that small timing changes shift the failing phases. Each row below
has 192 cases (eight phases, three buffers, eight workloads); these totals are
not interchangeable with the full-phase screen.

| Callback cost (us) | Entry delay (us) | Callback limit | Accepted / 192 |
|---:|---:|---:|---:|
| 0 | 0 | 10 | 192 |
| 50 | 0 | 10 | 161 |
| 50 | 250 | 10 | 164 |
| 50 | 1,000 | 10 | 163 |
| 50 | 8,000 | 10 | 104 |
| 250 | 0 | 10 | 154 |
| 1,000 | 0 | 10 | 144 |
| 50 | 0 | 1 | 159 |
| 50 | 0 | 2 | 162 |

The zero-cost control even accepts the otherwise saturated 2 ms-per-direction
row. It is not a realistic qualification result. Every paid-service configuration
rejects that row, and none passes all of its other 168 positive cases. A smaller
callback batch alone therefore does not close the gap. Minor improvements from
added cost are phase effects in this deterministic screen, not evidence that
padding waits improves the design. Entry delay is charged independently of idle,
so ready-first dispatch cannot evade an owner that is itself unable to run.

The gates remain 48 kHz; safety 1024/2560 frames; worker policy 8/1/2 ms; count
replay 64 ms; PCM offset 3072; deferral cutoff 26 bus frames; returned OUT lead
at least 16 integer bus frames; zero steady IN/OUT PCM loss; ordered count replay;
immutable pending payload; no streaming allocations; clean drain or quarantine.
The delayed HAL disturbance is still buffer + 832 frames. Startup classification
still ends at 48,000 captured frames. Integer bus-frame lead can overstate physical
lead by almost 1 ms. No safety, clock coordinate or acceptance tolerance changed.

## Independent placement model

The independent model now runs a pump state machine alongside hardware and HAL
progress: entry, idle, individual callback work, then return to bounded submission
work. Completion polling cannot deliver a callback. It rounds the chosen 50 us
service cost up to three nominal sample frames (62.5 us); it does not import the
production fake or Rust pool implementation. Its one-tick submission floor,
nominal six-frame packets and fixed stall windows remain explicit approximations.

The paired relative-wait study accepts **288/432 cases for each policy**. It
provides no aggregate acceptance improvement from skipping an idle wait. Both
policies reject the 2 ms-per-direction saturation rows; the other cost/stall rows
pass at the eight sampled phases. The earlier periodic-wake deadline model stays
at 278/432, byte-for-byte unchanged. Its wake rule differs, so the ten-case
increase is not evidence that ready-first dispatch caused an improvement.

The physical marker coordinate is unchanged wherever observed. Sample-model
acceptance also requires both markers to be captured, zero input/output loss,
completion/count conservation and returned OUT lead of at least 16 bus frames.
Its results do not override the production-code fixture's capture losses.

Physical-marker tests retain the exact round-trip sample coordinates and reject
permanent PCM holes, 32 ms owner stalls and saturated 2 ms-per-direction service.
Changing callback delivery alone is not a latency reduction. The three older
model studies are rerun and compared with their sealed JSON without modification.

## Reproduce and verification

```sh
cargo test --workspace --release --locked --offline -- --nocapture --test-threads=1
cargo test --workspace --examples --release --locked --offline
cargo run --release --locked --offline --example placement_model -- --dispatch-study
```

Validation covers **166 Rust tests** (three observer benchmarks remain
intentionally ignored), twenty standalone-model tests, doc tests, formatting and
Clippy across all five existing feature configurations. The first workspace test
invocation passed 165 tests and failed one newly written sensitivity expectation:
it incorrectly required the explicit zero-callback-cost control to reject 2 ms
per-direction submissions. With no callback/entry overhead, that model can fit
exactly and pass the unchanged gates. The expectation was corrected to require
rejection when callback work is paid, and the full sensitivity test was rerun.
Both the failed log and successful correction remain in the evidence. No timing
or loss gate was relaxed to repair the test.

Direct checks cover ready-but-undelivered callbacks, completion queries that
cannot dispatch work, empty pumps that always wait, count/time bounds, a slow
callback exceeding its cooperative budget, a stall during callback service,
independent entry cost, out-of-order readiness, missing HAL writers, immutable
payloads, count replay, streaming allocations, clean drain and stale-ticket
quarantine. The sample model retains markers under the original delayed-HAL
workload as well as clean runs. Old model study JSON is unchanged.

All source changes are in the test-only fake, the `live.rs` test module, the
standalone example and repository documentation. The non-test portion of
`live.rs`, every other runtime/native source, and all build/install scripts are
compared byte for byte with the starting snapshot. This turn produces no driver
bundle. Evidence in `build/validation/dispatch-model-offline-20261020/` includes
source hashes, command logs, separate acceptance results and the incremental
patch. Five previous evidence sets and the installed binary are reverified.
No source files were sent externally and no commit was made.

## Decision

**Do not promote this model to an installation candidate under the unchanged
gates.** Ready-first dispatch is worth retaining: at identical callback cost it
recovers 536 cases over wait-first. It also exposes a capture-publication limit
that the older free-callback fake hid. Neither bounded callback count alone nor
skipping idle waiting solves that limit, and the independent model offers no
aggregate confirmation of a readiness-policy gain.

The next bounded step is to trace the first missing input sample through
callback readiness, delivery and publication, then test whether publication can
happen before further owner work without violating output deadlines or ownership.
Keep the failed phases, paid service, entry-delay sensitivity and the 16-frame
lead gate. Do not compensate with lower safety, changed coordinates or a claim
that the regression suite passing qualifies the stress matrix. Native pump
implementation and an installer remain subsequent work, not outputs of this turn.

The baseline's six-frame analog RTL shift and first-start-after-reboot failure
remain separate hardware/release issues. Reboot testing remains deferred by the
owner. An offline pump model cannot establish a macOS scheduling guarantee or
resolve either issue.
