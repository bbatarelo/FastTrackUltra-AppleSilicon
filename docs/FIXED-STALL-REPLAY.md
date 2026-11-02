# Fixed-time stalls: capture independence is necessary, but not sufficient

20 October 2026. Offline follow-up to [capture delivery](CAPTURE-DELIVERY.md).

Follow-up: [independent completion, slot return and joint scheduling](INDEPENDENT-COMPLETION.md)
are now implemented in the closed-loop fixture. This report retains the earlier
projection and its original limits.

**Decision:** reject the current single-owner 4 ms / ten-slot prototype under
the unchanged combined-absence workload. Capture completion and publication
must be able to progress while the transport owner is unavailable. A capture-only
split is not yet a complete solution: some fixed stall placements also violate
the output submission deadline. The next design must qualify capture delivery,
slot return, count feedback and output scheduling together. Do not prepare an
installer or repeat timeout/drain tuning as a fix for the frozen absence.

This is a decision about the retained stress contract, not a claim that macOS
always deschedules a worker for 20.5 ms or that USB 2.0 cannot achieve the latency
goal. The installed driver, native adapter and all build/install profiles remain
unchanged. The new execution paths are test-only.

## What was replayed

The production-pool fixture now has an optional absolute wall-time absence.
Active processing pauses during it; a wall-clock timeout continues to expire.
Wake-on-ready is separate from per-dispatch work and lateness after the nominal
wake. A wake delay overlapping the same absence is counted once. Independently
specified work after the absence can still add time. The shared virtual clock
cannot move backwards when the HAL fixture charges preparation or capture work.

The original first-qualifying-pump injector remains available. Its buffer-64,
phase-7 reference has an 8 ms idle followed by an injected stall at
1,630,800–1,643,300 us. Replaying just that injected interval at fixed time,
with the preceding idle retained, reproduces every submission, callback,
publication and missing-read tuple exactly, as well as its terminal reason,
output loss and minimum lead. It still loses 72 input frames with zero output
loss and 16 bus frames minimum OUT lead.

A second arm freezes the **whole** 1,622,800–1,643,300 us owner absence.
This is the retained combined-unavailability control. It can preempt work at
the idle-start boundary and is not claimed to reproduce every original tuple.
Changing to wake-on-ready cannot cancel this imposed interval. The distinction
matters: replacing the earlier voluntary idle with an earlier wake is a useful
separate workload, not a pass of the frozen full-absence control.

The decisive request, IN 376, is physically ready at 1,624,000 us. Its first
missing sample is needed at 1,641,480 us, before the owner returns. It was
submitted before the absence. Queued IN coverage extends to 1,656,000 us in the
coarse-wait witness and 1,660,000 us in the wake-on-ready witness. No publication
reordering on the unavailable owner can satisfy that read.

## Same gates, explicit processing costs

All original settings remain: 48 kHz; input/output safety 1024/2560 frames;
64 ms count replay; PCM offset 3072; worker policy 8/1/2 ms; cutoff 26 bus frames;
returned OUT lead at least 16 integer bus frames; zero steady IN/OUT loss;
ordered counts, immutable pending payloads, no streaming allocation and clean
drain or quarantine. The original delayed-HAL disturbance remains buffer + 832
frames and steady classification begins at 48,000 captured frames.

Submission work costs 1 ms per direction from virtual time 1.616 s in the new
screens. Callback service costs 50 us. Capture processing is charged separately
for validation/copy/decoding/publication; these costs are chosen inputs, not
native measurements. They use active-work semantics. A blocking native operation
with a different wait/work decomposition requires separate measurement and
modelling; this experiment does not identify that decomposition.

The 1,536-case paired screen covers two absence scopes, coarse/event-driven
waits, capture processing of 0/100 us, buffers 64/512/4096 and 64 HAL phases.
Entries are accepted cases out of 64. A passing regression test preserves the
rejections; it does not qualify the overall design.

| Absence | Wait | Capture work | Buffer 64 | Buffer 512 | Buffer 4096 |
|---|---|---:|---:|---:|---:|
| Injected 12.5 ms | Coarse | 0 us | 0 | 0 | 64 |
| Injected 12.5 ms | Coarse | 100 us | 64 | 64 | 64 |
| Injected 12.5 ms | Wake on ready | 0 us | 64 | 64 | 64 |
| Injected 12.5 ms | Wake on ready | 100 us | 64 | 64 | 64 |
| Full 20.5 ms | Coarse | 0 us | 0 | 0 | 64 |
| Full 20.5 ms | Coarse | 100 us | 0 | 0 | 0 |
| Full 20.5 ms | Wake on ready | 0 us | 0 | 0 | 64 |
| Full 20.5 ms | Wake on ready | 100 us | 0 | 0 | 64 |

The full-absence/coarse/100-us rows fault on OUT lead in all 192 cases; minimum
lead reaches 9, which remains a rejection. Other failed rows include capture
loss. Extra processing can move work into a more favorable phase before a fixed
stall; the apparently improved 12.5 ms coarse row is not a monotonic cost margin
or scheduling guarantee. Buffer 4096 passing some rows does not qualify the
smaller-buffer cases or establish exhaustive arbitrary-time alignment coverage.

A separate 256-case screen sweeps 32 start offsets at 125 us spacing across one
4 ms request, at buffer 64 / HAL phase 7, wake-on-ready and 100 us capture work:

| Absence duration | Accepted / 32 | Parent transport faults |
|---:|---:|---:|
| 12.5 ms | 32 | 0 |
| 16 ms | 32 | 0 |
| 17 ms | 32 | 0 |
| 17.5 ms | 29 | 0 |
| 18 ms | 25 | 0 |
| 20.5 ms | 0 | 15 |
| 24 ms | 0 | 32 |
| 32 ms | 0 | 32 |

These are results for this phase/cost screen, not a universal 17 ms service
bound. The 20.5 ms row reaches 15 bus frames OUT lead in some placements. A
further 96-case sensitivity screen pays 250/1000/8000 us wake lateness and uses
one/ten callbacks per source. All full-absence cases remain rejected. The
requested sleep duration is not assumed to be a native wake guarantee.

## Independent capture: useful bound, not a concurrent driver

A trace-derived model uses only requests actually submitted, completed and
validated by the real pool fixture. It retimes their publication on an
independent service context, preserves sample order, and reconstructs missing
samples against the original HAL reads. Actual publication times reproduce
**each read's** missing count; trace completeness and ring capacity are checked.
No samples are invented for unsent requests.

With a frozen owner schedule, a capture copy can meet every read yet finish
later than the next use of its USB slot. Those cases are retained as slot-return
conflicts, not called feasible implementations. Charging copy work in the
parent changes its schedule too; a zero-conflict paid parent is still a limited
counterfactual, not evidence of a free independent path.

The 144-case paid-parent subset covers three buffers, eight representative
phases, both wait modes and 25/100/250 us copy work. Of these, 48 parent runs
fault and are excluded from counterfactual success. The remaining 96 have zero
independent read misses and zero premature reuse in the frozen schedule. When
the capture context shares the full absence, small-buffer reads still fail.
A 4,050 us per-request service overload also remains rejected.

A second, causal slot-return projection uses the zero-copy parent, a nonzero
handoff cost and the same requested USB start frames. Slot reuse waits for copy
completion plus handoff; output also waits for the corresponding count record.
Those waits propagate through serialized IN/OUT submission calls. The model
stops at the first missed IN bus deadline or below-16 OUT lead and makes no
capture-success inference beyond that rejection. A zero-service/zero-handoff
control reproduces the parent's native submission return times.

This projection deliberately omits a full concurrent transport implementation,
other owner processing, native wake costs and HAL output repacking. Its passes
show a timing possibility for the frozen submission plan, not qualification.
It identifies back-pressure rather than declaring every reuse conflict fatal.
For the phase-7 witness, service costs 75–1,050 us and handoffs 1/25/100 us can
meet the projected deadlines; 4,050 us service is rejected. The broader offset
projection at 150 us service and 25 us handoff yields:

| Absence | Parent faults / 32 | Evaluated | Projection passes |
|---:|---:|---:|---:|
| 17 ms | 0 | 32 | 32 |
| 17.5 ms | 0 | 32 | 32 |
| 18 ms | 0 | 32 | 32 |
| 20.5 ms | 13 | 19 | 19 |
| 24 ms | 32 | 0 | 0 |
| 32 ms | 32 | 0 | 0 |

This table uses zero-copy parents; the earlier offset table charges 100 us in
the owner, so their fault counts are intentionally different. Thirteen parent
faults are not thirteen rescued cases. The whole 20.5 ms envelope still fails.

## Concrete next boundary

Proceed toward an independently serviced completion/capture path, with a
joint IN/OUT scheduling and ownership fixture before changing the native ABI.
A second publisher that still waits for callbacks on the blocked owner is not
independent progress. A standalone capture-only patch is not ready for hardware.

The current native adapter registers endpoint run-loop sources on the calling
thread, uses thread-local callback context/token state, and reads/writes ordinary
slot fields. Moving a source to another thread without changing that contract
would break its assumptions. Establish completion-routing/thread-affinity
feasibility, then model a preallocated handoff with explicit states:

`Submitted → Completed → Claimed → Published → Released → Submitted`

Keep submission/control ownership explicit. Require ticket generations,
release/acquire visibility, slot reuse only after acknowledgement, ordered count
handoff, bounded back-pressure and deadline rejection. Assign clock/timestamp
publication ownership too. Stop/drain must wait for both contexts or quarantine
unknown ownership; a callback must not retain a pointer into freed storage.
Qualify output submission alongside capture rather than hiding the OUT failures.
If the stress contract blocks every capture-capable context over the same
readiness-to-deadline interval, the current safety cannot satisfy it.

## Review, validation and evidence

Claude reviewed two complete test files, three test-only patches and the timing
summary with tools disabled, under the owner's existing sharing approval. Its
review prompted the monotonic-clock guard, per-read/completeness/ring checks,
explicit handoff latency and the slot-return projection. Its wake-lateness and
single-witness concerns were also addressed by the new sensitivity and sampled
screens. The review predates those follow-ups and is not a review of their final
code. No external opinion is counted as a test pass.

Final validation passes **184 Rust tests** (three existing observer benchmarks
remain intentionally ignored), twenty standalone-model tests, formatting and
Clippy across all five profiles. Non-live tests run serially; independent live
fixtures run with four test threads. All eleven new tests were then repeated
after completing the per-read assertion; the result rows remain identical.
All final commands pass. The original
153 cost, 96 PCM, 102 dispatch, 216 sensitivity, 36 paid-dispatch PCM, 51 capture
and 216 capture-sensitivity rows match the sealed study exactly; all four
standalone-model JSON results are unchanged. The original 2,096/2,112 positive
acceptances and rejection of all 1,152 negative cases are therefore retained.

The new checks cover absolute absence boundaries, monotonic time, paid processing,
no owner actions inside the absence, each HAL read, trace completeness, ring
capacity, causal slot release and deadline rejection. Existing real-pool tests
retain immutable payload CRC, ordered counts, zero streaming allocations,
stop/drain and stale-ticket quarantine. The independent projections are analysis
code, not implementations of concurrent memory ordering or native cleanup.
Initial failures are preserved: freezing the whole interval was incorrectly
assumed to reproduce every original boundary event, zero-copy reuse conflicts
were initially asserted away, some paid parents correctly faulted, and a test
binding briefly shadowed its helper function. Clippy also caught the initially
incomplete per-read assertion, which was completed and the eleven new tests
rerun. Those exploratory failures are not qualification passes.

Reproduce the new experiments with:

```sh
cargo test --release --locked --offline -p ftu-stream fixed_replay_ -- --nocapture --test-threads=1
```


Evidence is saved in `build/validation/fixed-replay-offline-20261020/`, including
exact review payload/response, final and exploratory logs, per-class result rows,
source hashes and an incremental patch. Earlier evidence remains immutable.
No driver bundle, installation, audio run, daemon restart or commit is produced.
The six-frame hardware RTL comparison and first-start-after-reboot behavior
remain separate unresolved release gates; reboot testing remains deferred.
