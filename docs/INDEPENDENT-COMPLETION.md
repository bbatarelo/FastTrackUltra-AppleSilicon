# Independent completions and joint scheduling

21 October 2026. Offline implementation following [fixed-time replay](FIXED-STALL-REPLAY.md).

**Decision:** retain the independent completion/capture architecture and reject
4 ms / ten slots for the retained owner-absence envelope. The new closed-loop
fixture fixes capture delivery while that owner is stalled, but output coverage
still runs out at some placements. A separately labelled **twelve-slot-per-direction**
comparison passes the main phase screen and the uniform 5 ms callback-delay sweep
at the same margins. It is a useful next design reference, not an installation
candidate or a measured latency improvement. Compounded callback delays still
reject it. Native threading and lifecycle qualification remain ahead.

All new transport execution is under `cfg(test)`. Native ABI, production runtime,
feature selection and installed binary are unchanged. Normal `make install`
continues to use the 8 ms / four-slot implementation. No hardware, recording,
playback, daemon restart, installation or commit was performed.

## What is implemented

`rust/ftu-stream/src/independent_completion.rs` supplies reusable experimental
ownership primitives. A ticket includes epoch, endpoint/slot identity and
monotonically increasing generation. Atomic transitions separate preparation,
USB ownership, callback completion, exclusive copying, publication, release and
the owner's acknowledgement. Stale, duplicate, wrong-slot and wrong-epoch
callbacks cannot touch a newer payload. A callback and a synchronous submission
rejection compete for the same terminal transition; the loser cannot release or
overwrite the winner's data.

No method lends a shared payload reference across a transition. Release/acquire
operations publish data and fence reuse. The count channel is bounded, allocates
before streaming, and has one non-clonable producer and consumer. Full queues
retain records and apply backpressure; cursor exhaustion rejects instead of
wrapping. The owner also stops consuming when its ordered count history is full.
Slot-return timers retain the original ticket, and the owner acknowledges the
submitted ticket, not whichever generation happens to be in the slot now.

Stable boxed arenas retain their storage on destruction unless the contexts are
quiescent and every slot has been acknowledged. The fixture waits for already
entered submission/copy work and paid handoffs before its scripted abort/drain.
An omitted abort callback leaves ownership unknown and the arena quarantined.
This tests the retention policy, not real IOKit abort latency or shutdown behavior.

`rust/ftu-stream/src/independent_tests.rs` drives those same primitives in a
causal, two-context event fixture. Future submissions are chosen from current
acknowledgements, count feedback, actual HAL PCM readiness and IN/OUT deadlines.
There is no replay of a parent's future submission times. The submission owner
uses any free slot, chooses between eligible direction heads with the existing
deadline policy, and limits catch-up to ten submissions. It rechecks channels
after each returned call. A missing OUT payload can defer at the existing cutoff
without permanently blocking eligible input work. Idle waits allow count/ACK
notification or an 8 ms timer; there is no assumed HAL-driven notification.

The completion context independently dispatches callbacks, validates/copies
capture, restores sequence order, publishes actual PCM through `AudioIo`, updates
the sample clock/timestamp path, and sends counts before releasing the slot.
It owns clock publication. A separate `AudioIo` instance handles output staging;
its output placement uses the shared ring and fixed offset, not an independently
cached clock. Capture, output and HAL buffers are real code paths, not arithmetic
availability estimates. PCM identities, ordered packet counts, pending-output
CRC and no streaming allocations are checked. The synthetic HAL still advances
on the original nominal time coordinate; this does not measure analog RTL.

## Retained gates and explicit assumptions

The gates stay at 48 kHz, input/output safety 1024/2560 frames, 64 ms count replay,
3072-frame PCM offset, at least **16 integer USB bus frames of returned OUT lead**,
zero steady IN/OUT missing PCM, count conservation, immutable pending payload,
no streaming allocation and clean drain or quarantine. Steady checking starts at
48,000 captured frames. The HAL disturbance remains exactly buffer + 832 frames.
The twelve-slot comparison changes slot capacity in both directions; block size,
64-record history, replay, offset, safety, lead gate and ten-submission catch-up
cap are asserted unchanged. Production thread policy remains 8/1/2 ms; simulated
availability is not an OS scheduling guarantee for either future thread.

Default paid work is 25 us per owner probe/preparation, 1 ms per native submission
in each direction from virtual time 1.616 s, 50 us callback service, 100 us IN
validation/copy/ordered-publication/clock work, and 25 us per return handoff.
These are selected aggregate costs, not measurements of the proposed native
split. Deferred probes also pay owner work. Handoffs serialize with callback/copy
work; publication retries cannot overlap them or rewind the service clock.
Costs and absence windows compose as active work, while timeouts keep expiring.

The owner remains unavailable during the exact 20.5 ms interval starting at
1,622,800 us (plus each swept offset). Independent completion service is an
explicit assumption. A correlated-absence control withdraws that assumption and
retains the resulting capture failures. Wake lateness, paid work, callback delay,
reordering and delayed HAL writes have separate screens. Default equal-time
ordering makes HAL reads win against publication. An alternate owner/completion
order and a one-microsecond HAL service delay exercise the other tie direction,
including output packing before a coincident HAL write. These sampled variations
are not an exhaustive scheduler model.

## Results

The main screen uses six HAL buffers (64, 128, 256, 512, 1024, 4096), all 64
relative sample phases, and four fixed stall offsets. Each accepted case completes
600 transfers per direction, publishes 115,200 input frames, conserves output
counts and drains safely.

| Geometry | Accepted / cases | Minimum returned OUT lead | Capture result |
|---|---:|---:|---|
| 4 ms / 10 slots | 768 / 1536 | 14 bus frames | No misses before fault or through completion |
| 4 ms / 12 slots | 1536 / 1536 | 22 bus frames | No misses through completion |

Every case makes independent publication progress inside the owner absence.
All rejected main-screen cases are output-lead failures. A zero missing-sample
count on a faulted prefix is not a full-session capture pass.

A separate 5 ms uniform callback-delay screen covers three buffers, four HAL
phases and 32 offsets at 125 us spacing: ten slots accept **0/384**, twelve slots
**384/384**, with minimum returned lead 17 for the latter. A 5 ms reordering delay
on every seventh request passes the sampled twelve-slot screen by itself; adding
it on top of the uniform 5 ms delay rejects all twelve sampled twelve-slot runs.
Do not hide that compounded-delay limit or call the entire stress space qualified.

The dense duration screen retains 32 ms absence rejection at both depths. Sustained
2 ms submissions in both directions also reject. A short 600-transfer run can
survive that overload temporarily; the negative control now permits up to 6,000
transfers, counts/checks HAL data through the longer window and requires an actual
output deadline fault after the shorter test would have ended. The observed
faults occur around 3.621 s (ten slots) and 4.265 s (twelve) on the fixture clock.
Copy-service overload and small-buffer correlated absence remain rejected.

Queue coverage is sampled **before** the frozen absence. An optimistic earliest
OUT return is computed from that state and any in-flight owner work, granting OUT
the next possible call. If even this earliest return lacks 16 bus frames, rejection
is necessary. Actual returns must not precede the bound. This is a one-sided
necessary bound, not a prediction that all other cases will pass; wake/eligibility
and earlier input work can delay them further. Both pre-call deadline rejections
and returned calls remain in the result set. The original observed-lead classifier
was circular and is preserved only as an exploratory result, not evidence.

## Native boundary and next step

The checked local SDK describes asynchronous USB notifications delivered through
a Mach port/run-loop source. That supports separating callback routing from an
owner's idle wait; it does **not** establish concurrent submission safety or a
wake-time bound. The current adapter in `tools/ftu-probe/duplex.c` attaches sources
to the caller's run loop and uses thread-local context/token state and ordinary
slot fields. It cannot adopt this split by moving a source alone.

Next implement an isolated native boundary prototype around the Rust ownership
contract: completion-run-loop ownership, explicit ticket/refcon routing, stable
storage, submission-return races, stop/abort/join and quarantine. Establish whether
legacy IOKit permits the required cross-thread interface calls and measure their
contention/cost before a hardware driver candidate. Keep twelve slots as a labelled
capacity comparison while qualifying delayed completion limits; no more tuning
of a single unavailable callback owner can repair the earlier capture deadline.

The under-10-ms RTL target still requires later reductions in replay/placement,
safety and native lead requirements. Neither increasing queue capacity nor this
completion split changes the current declared sample coordinates. The historical
six-frame hardware RTL shift and first-start-after-reboot failure remain separate
release gates; reboot testing stays deferred.

## Review and verification

Claude reviewed the architecture and then the exact supplied source snapshot.
Its code review found overlapping handoff work, self-derived acknowledgement
generations, incomplete count backpressure, the circular bound classifier and
missing tie/long-run checks. Those issues were corrected, with focused regressions.
A further identity audit added endpoint/slot keys. The final revisions are tested
here; they are not represented as another completed external review. Real-thread
stress is run on this host, with bounded failure detection and allocation tracking
on both threads. This is not an exhaustive weak-memory proof; Loom was not cached
and no dependency was fetched for this offline task.

Final validation passes **201 Rust tests** (three pre-existing observer benchmarks
remain intentionally ignored), twenty standalone-model tests, formatting and
Clippy across all five build profiles. The new portion is six ownership/channel
tests and eleven joint-fixture tests. The 20,000-generation real-thread exchange
runs on arm64 with allocation tracking enabled on both participants.

All 870 retained cost/PCM/dispatch/capture result rows match the earlier sealed
study exactly. All eight groups of fixed-time replay results and all four
standalone-model JSON results also match. The earlier 2,096/2,112 positive
acceptances and rejection of all 1,152 negative cases are retained, rather than
being relabelled by this new architecture. Exploratory compile/lint failures and
the initially too-short saturation assertion remain in the evidence; they are
not final qualification passes.

Reproduce the new fixture and protocol tests:

```sh
cargo test --release --locked --offline -p ftu-stream independent_completion:: -- --nocapture --test-threads=1
cargo test --release --locked --offline -p ftu-stream live::tests::independent_tests:: -- --nocapture --test-threads=4
```

Evidence is sealed under `build/validation/independent-completion-offline-20261021/`.
It includes the exact Claude payloads/responses, exploratory failures, final logs,
source snapshot, incremental patch and hashes. Older evidence is preserved.

## Native boundary follow-up

The [native routing and shutdown study](NATIVE-COMPLETION.md) now implements the
OS boundary with real Mach-port/run-loop fixtures, generation-fenced callbacks,
release/ACK, retained live dispatch on timeout, and cleanup only after join.
It remains separate from this Rust joint fixture; integrating the two is the
next step. This does not change the timing results or hardware limitations above.
