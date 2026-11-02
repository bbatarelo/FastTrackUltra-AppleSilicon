# Bounded PCM preparation with ten USB slots

The later [retry/cost follow-up](RETRY-COST-OBSERVATION.md) closes the test and
counter-export gaps listed below, and records additional scheduling rejections.
This document retains the first prototype's results and review history.

20 October 2026. **Implemented; retained offline acceptance screens pass. An added
combined-stall screen exposes a scheduling limit. Hardware qualification remains pending.**

The [eager ten-slot experiment](TEN-SLOT-PROTOTYPE.md) improved USB lead but
packed some output before the HAL supplied it. This opt-in revision separates
slot capacity from the decision to read, pack and submit PCM. The installed
driver and both earlier prototype evidence directories are preserved.

## Reproduce offline

```sh
make test-prototype-4ms-ready
```

The isolated output is `build/prototype-4ms-ready/`. `make prototype-4ms-ready`
builds without rerunning tests. These targets do not open the device, install
a bundle or restart audio. Normal `make install` retains 8 ms / four slots;
the eager eight-slot and ten-slot targets remain unchanged comparisons.
Geometry and admission policy are read back from the linked Rust library before
bundling. Metadata/content identities distinguish all four profiles, including
the two with identical ten-slot USB geometry. Multiple prototype features are
rejected at compilation; a failed rebuild removes its stale result marker.

## Runtime policy

Only the `prototype-4ms-ready` feature selects the new policy. Geometry remains
4 ms / ten slots / 64 history entries, 48 kHz, 64 ms count replay, PCM offset
3072, safety 1024/2560 and worker policy 8/1/2 ms. Native ownership and pool ABI
v4 are unchanged. A small read-only export identifies the selected policy.

For the next ordered OUT request, Rust first checks its bus-frame lead. Below
16 ms, admission stops with a distinct `output_lead_deadline` reason (55).
Otherwise it tries one bounded read of the exact requested frame tags into
preallocated staging. Every sample comes from the existing atomic timeline
reader; a high-water mark cannot stand in for contiguous valid samples.

- If all samples are present, pack that owned snapshot and submit immediately.
  A subsequent HAL rewrite cannot alter the snapshot. There is no second ring
  read between successful staging and packing, and submitted USB data stays
  immutable.
- If samples are missing and more than **26 ms** remains, leave the native
  slot, feedback record and sample cursor untouched. Resume polling input and
  pumping callbacks; retry on a later owner iteration. Even a finite run with
  no requests currently pending can wait for this bounded OUT deadline.
- At 26 ms or less, use the existing read/zero-fill/missing-frame accounting.
  A missing HAL writer or permanent hole cannot stall capture indefinitely.
  Any steady zero-filled frames still fail acceptance. Readiness retries alone
  do not count a frame as lost, because no PCM has been submitted yet.

The 26 ms cutoff reserves the unchanged **16 ms returned lead**, an **8 ms
modeled retry interval**, and **2 ms for preparation/submission work**. The
enum value and these timing constants are separate. Neither the retry interval
nor work allowance is an OS guarantee. After a successful native submit,
a fresh clock check must still show at least 16 ms; otherwise the session stops
and drains the request it now owns. Clock-query failure also fails closed.
Neither the precheck nor a missed deadline permits freeing pending storage.

The owner never sleeps or waits on the HAL thread. Each readiness attempt is
bounded by the existing timeline read limits and one active transfer's sample
count. No hot-loop allocation or pending USB mutation is introduced. Capture
uses the same scratch storage only outside the staging-to-packing interval and
invalidates any abandoned staging tag before reuse.

## Retained acceptance criteria and results

All comparisons retain the 16 ms returned-OUT-lead gate, zero steady input and
output missing frames, exact ordered count replay, immutable payloads, zero
streaming allocations, and clean drain or quarantine when ownership is unknown.
The late-HAL disturbance remains **one HAL buffer plus 832 frames**.

The production pool, packed PCM and HAL rings pass **all 2,304 ready-policy
cases**: 64 distinct relative phases at buffers 64, 128, 256, 512, 1024 and 4096,
with 600 transfers per direction, for both the retained alternating 5 ms callback delay (even sequences, to
exercise reordering) and
the unchanged late-HAL disturbance. Each screen now runs at **1, 4 and 8 ms
pump intervals**. There are zero steady missing input/output frames, conserved
frame counts, zero streaming allocations and clean drain.

| Pump interval | Ready cases passing | Minimum steady OUT lead | Minimum whole-run lead |
|---:|---:|---:|---:|
| 1 ms | 768/768 | 31 ms | 26 ms |
| 4 ms | 768/768 | 28 ms | 24 ms |
| 8 ms | 768/768 | 24 ms | 20 ms |

Whole-run minima include startup fallback. Counted startup silence/missing
samples remain outside the unchanged steady window; zero steady loss does not
mean zero startup loss. The same fixture reruns 1,536 eager comparison cases,
retaining the previous ten-slot **381/384 late-HAL rejections**. The regression
suite also tests a missing HAL writer at pump intervals 1/4/8 ms and callback
delays 0–7 ms: **24/24 cases** complete with counted fallback, at least 20 ms
lead, conserved counts, no allocations and clean drain.

The independent sample-placement model now advances HAL and hardware at every
sample while waking the owner at one sample, 4 ms or 8 ms. Of 504 cases in the
expanded positive-stress matrix, **501 pass and three reject**. All original
one-sample-cadence positive cases pass. Delayed HAL (buffer + 832 frames),
delayed HAL plus an 8 ms owner stall, and ordinary 4/8 ms owner cadences pass.
All 144 permanent-hole and 32 ms stall controls remain rejected; a permanent
64-frame hole remains 64 missing samples despite later publication.

The **three additional failures** combine an **8 ms owner cadence with a
12.5 ms owner stall**: one of eight phases fails for each of buffers 64, 512
and 4096. The eager ten-slot comparison fails the same three cases, returning
14 ms lead; the ready policy stops with `output_lead_deadline`. The 64-frame
case also loses 80 capture frames. The model's minimum-returned-lead column
excludes the rejected request, so its 16 ms value must not be read as a pass.
This exceeds the modeled retry allowance and exposes an existing geometry/
service-delay limit. It is retained as a rejection and a regression test;
**the expanded matrix is not a complete pass**. No acceptance threshold was
lowered and these cases are not relabeled as successful negative controls.

Analog fixture RTL is unchanged. This is a model reference, not a physical
latency measurement; neither four-millisecond transfers nor waiting to pack
PCM reduce the current declared latency by themselves.

The production pool's retained callback/owner-stall matrix covers 48 phase/drift
cases per row. Under the ready policy, the alternating 9 ms callback-delay plus 4 ms and
8 ms owner stalls retain minimum OUT leads of **23 and 19 ms**, respectively.
All selected in-budget cases complete and conserve frames. All 48 cases with
a 32 ms owner stall stop and drain; no below-gate completion becomes a pass.
The eager profiles' original failures remain in the same regression output.

Additional production-path controls verify that a HAL rewrite after successful
staging cannot change the packed snapshot; an actual ring hole remains counted
at fallback; a missing writer makes bounded progress; a native submit that
consumes the reserved work budget fails the return check while its successful
request remains pending until drained; and stale callbacks remain quarantined.

All four build profiles pass **147 Rust tests each**, with the three explicitly
ignored observer microbenchmarks still excluded. Native/HAL AddressSanitizer
and UndefinedBehaviorSanitizer checks pass. The 2,048-sequence ready-policy
simulation preserves frame/count replay, OUT CRCs, slot reuse and zero hot-loop
allocations; injected submission, packet, callback and geometry failures drain,
while stale ownership is quarantined.

Also passed: 120 probe Python tests, 54 loopback/diagnostic Python tests, ten
temporary-destination installation tests, three profile identity/isolation
tests, sixteen independent model tests, formatting, and Clippy with warnings
denied for all four profiles. All three conflicting feature pairs and all twelve
incorrect geometry/policy read-back combinations reject. All four bundles
pass signature verification. The final ready bundle identity is recorded in
`build/validation/pcm-ready-offline-20261020/identities.json`.

## External review and corrections

Claude reviewed the nine explicitly authorized FTU files and test summary.
The first review found that the original 18 ms cutoff left no allowance for
coarse pump returns. The cutoff is now 26 ms, and both the production fixture
and independent model test coarse retries. An isolated 18 ms mutant fails the
new capture-only regression with `output_lead_deadline`; the final version
passes. This negative control demonstrates that the new test detects the bug.

The first review also found that preparation telemetry began after staging.
`playback_requested_end` now publishes the requested end **before the first
staging read**, including unsuccessful attempts. A regression checks the marker
and confirms that HAL rewrites cannot alter the owned staged snapshot.
The reviewed payload, hashes, responses and disposition are retained with the
evidence. The original review refers to the superseded 18 ms implementation.

Deferral/fallback counters remain private pool diagnostics, exercised by offline
tests; exposing them for hardware qualification is still required. Live capture
keeps IN requests pending during deferral. The finite-tail waiting path is only
exercised by the offline harness: native one-shot tools still use eager policy.
If that path is later exposed to native finite runs, test idle run-loop progress
and CPU use explicitly. No sleep was added to the HAL thread. Claude's follow-up accepted the three
corrections and found no new offline implementation blocker; the full response
and the disposition distinguish fixes from remaining qualification work.

The **16-frame gate is deliberately fatal in this opt-in qualification
prototype**, including when PCM was already ready. Eager packing may continue
through the same transient. This is an availability tradeoff, not a production
rollout. Lead figures throughout this experiment are **integer USB bus-frame
differences** (nominal milliseconds); physical lead can be almost one millisecond
lower. Preserve the existing integer gate for comparisons, then use subframe
measurements when choosing a physical safety margin.

A fallback counter records the readiness decision, not necessarily emitted
silence: the subsequent read may see a concurrent HAL write. Actual missing-frame
counters remain authoritative. Before hardware, export deferrals, fallbacks,
maximum retry interval and over-budget retry count. Also close the remaining
combined-cost coverage gap: the coarse production PCM sweep uses zero native
submit cost, the independent model uses 0.5 ms prepare/submit cost, and the
separate return-boundary control uses 11 ms. The combination of an 8 ms retry
with 1/2 ms submission costs still needs an explicit production-fixture screen.

Evidence: `build/validation/pcm-ready-offline-20261020/`, with command logs,
source snapshots/hashes, explicit acceptance results, independent model output,
profile read-backs and offline bundles. The old eight-slot and eager-ten-slot
seals are reverified separately. The installed binary remains
`dev-673cc272caa79014`, SHA-256
`e8b420efc0156e334137caa6870fc45e4c47e58abc6d4ffcaf7d09037831594c`.
No audio session, driver installation, daemon restart or commit was performed.

## What this establishes and what comes next

This is a transport-correctness step at unchanged latency settings. Queue
capacity can now absorb completion delays without forcing every available slot
to consume PCM immediately. This adopts ASFireWire's useful separation of
transport coverage and PCM readiness while keeping the USB callback-ownership
contract; it does not copy FireWire's late writes into pending DMA payloads.

Offline clocks and workloads cannot establish native scheduling bounds or CPU
cost. Before a hardware comparison, characterize the existing six-frame
same-configuration baseline shift, expose deferral/fallback counters, and
resolve or explicitly bound the combined owner-stall limit. Then freeze the
candidate and rollback. A
bounded silence/loopback comparison must retain the same margins and verify
startup, exact active build/epoch, returned lead, missing-frame counters,
worker cost and teardown. Deferral/fallback counters currently remain private
pool diagnostics used by offline tests; hardware observability should be
reviewed before that comparison.

Lower safety margins toward under 10 ms only in a later, separately qualified
change. Startup-after-reboot reliability remains an open release gate; no
initialization changes or automatic hardware retries are included here.
