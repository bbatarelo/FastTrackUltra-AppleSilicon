# Four-millisecond, ten-slot offline experiment

20 October 2026. **Implemented; rejected for hardware installation.** Ten slots
provide enough USB submission lead in the retained stall screens, but cause
missing output samples when the HAL publishes late. This is a useful boundary:
adding queue capacity alone does not solve both deadlines. The sixteen-ms lead
requirement and zero-steady-missing-sample criterion remain unchanged.

The eight-slot reference and its original evidence are preserved in
[shorter transfers](SHORTER-TRANSFERS.md). Normal `make install` still builds
8 ms / four slots. No device was opened, driver installed, audio started,
CoreAudio restarted or commit created in this experiment.

The subsequent [PCM-readiness prototype](PCM-READY-PROTOTYPE.md) implements the
separate preparation policy. This report retains the eager ten-slot experiment
and its failed acceptance unchanged.

## Reproduce offline

```sh
make test-prototype-4ms-ten-slot
```

This builds and ad-hoc signs the ten-slot bundle and runs Rust plus native/HAL
sanitizer tests in `build/prototype-4ms-ten-slot/`. For the eight-slot comparison,
use `make test-prototype-4ms`; its output stays in a separate directory. The
build-only targets omit `test-`. Neither target installs anything. Passing
regression tests includes preserving expected rejection cases; it does not
mean the profile meets the acceptance criteria.

Both profiles use 32 microframe entries per 4 ms transfer, 64 feedback-history
entries, 64 ms count replay, PCM offset 3072, input/output safety 1024/2560 and
the existing 8/1/2 ms worker policy. Ten slots give 40 ms queued coverage instead
of 32 ms. There is no expected round-trip latency reduction at these margins.
The private pool ABI remains v4 and the one-shot ABI is unchanged.

The selected Rust feature is `prototype-4ms-ten-slot`. Selecting both prototype
features is a compile error. Linked-library read-back checks the geometry before
bundling; source identities and bundle metadata distinguish all three profiles.
A failed rebuild removes that profile's stale success marker. The native
adapter already supports validated non-power-of-two depths; its allocation,
callback and ownership tests now explicitly exercise ten slots.

The geometry-derived `ftuW` late-event collection threshold is 32 ms here,
compared with 24 ms for eight slots. That threshold controls diagnostic event
collection; **it does not replace the 16 ms acceptance gate**.

## Transport lead

The production pool fake uses integer bus milliseconds. Each row covers 16
stall phases and -200/0/+200 ppm packet-count drift, 48 cases per profile.
Five-ms callback delay rounds the retained 4.364 ms measured tail upward;
eight-ms owner stall separately rounds the measured pump/timeout tail upward.
Combined disturbances are explicit stress cases, not an assertion that the
measured delays are independent and additive.

| Condition | Eight-slot minimum OUT lead | Ten-slot minimum OUT lead | Ten-slot result |
|---|---:|---:|---|
| 5 ms callback delay | 23 ms | 31 ms | 48/48 accepted |
| 8 ms owner stall | 20 ms | 28 ms | 48/48 accepted |
| 9 ms callback delay | 19 ms | 27 ms | 48/48 accepted |
| 9 ms delay + 4 ms owner stall | 15 ms | 23 ms | 48/48 accepted |
| 9 ms delay + 8 ms owner stall | 11 ms | 19 ms | 48/48 accepted |
| 9 ms delay + 32 ms owner stall | Stops in all cases | As low as 2 ms | 0/48 accepted |

In the ten-slot beyond-budget row, 42 cases stop on stale schedule and six
complete below the lead gate. All drain cleanly. The native one-ms schedule
guard is distinct from the sixteen-ms acceptance screen: completion alone is
not acceptance. The original eight-slot rejections remain recorded.

The independent 125 µs timing model also passes all 64 ten-slot phases for its
retained callback/work envelope plus 4 ms or 8 ms owner stall: minimum integer
lead is 23/19 ms, respectively (22.625/18.25 ms in exact model time). Its 32 ms
beyond-budget cases all stop. It includes serialized work and a different phase
grid, so failure counts need not match the production fake.

## Sample availability decides the outcome

The independent placement model retains the original late-HAL disturbance:
**one buffer plus 832 frames**, not a smaller disturbance derived from the new
4 ms batch. At buffers 64, 512 and 4096, ten slots lose up to **112 output
frames**, with all eight phases at each buffer rejected. Input misses are zero
and native transport completes. Clean and the selected delivery/owner-stall
cases have zero missing samples, which does not erase the delayed-HAL failure.

The production pool plus real PCM/ring fixture confirms the rejection. Each
row covers all 64 distinct relative HAL/USB phases and 600 transfers per
direction. Its integer-ms fake clock, variable capture packet counts and pause
phase differ from the nominal independent model, so exact missing counts differ.

| Actual HAL buffer | Ten-slot phases with missing OUT frames | Maximum missing frames |
|---:|---:|---:|
| 64 | 63/64 | 63 |
| 128 | 64/64 | 127 |
| 256 | 64/64 | 64 |
| 512 | 63/64 | 190 |
| 1024 | 64/64 | 127 |
| 4096 | 63/64 | 63 |

Thus **381/384 delayed-HAL cases fail**. All 384 eight-slot comparison cases
have zero missing frames. The original 5 ms callback-delay screen also passes
all 384 combinations for each profile. In all 1,536 cases, input misses remain
zero, frame counts are conserved, no streaming allocations occur, and requests
drain with no pending ownership or transport fault. These transport successes
do not cancel the missing output samples.

The fixture uses the actual pool, HAL ring readers/writers and packed 24-bit
PCM path. Frame tags verify all eight output channels; every zero-filled frame
must agree with the missing-frame counter. Capture uses an exact constant
signal. The HAL clock advances independently of USB completion service. The
late-HAL pause starts after 72,000 nominal frames so its affected output samples
are inside the steady accounting window; startup and callbacks after the finite
input stimulus are excluded explicitly. No future publication high-water is
used as proof that a missing sample exists.

This result explains the tradeoff: submitting more immutable output requests
requires packing audio earlier. The added lead consumes the time previously
available for late HAL writes. Neither model is a hardware CPU or scheduling
measurement, and its analog fixture is not a measured RTL.

## Validation and evidence

All three profiles pass 142 Rust tests each (the three explicitly ignored
observer microbenchmarks remain excluded). Native/HAL AddressSanitizer and
UndefinedBehaviorSanitizer checks pass, including the ten-slot allocation and
callback matrix. The 2,048-sequence geometry/fault screen retains count replay,
immutable OUT CRCs, history/slot wrap, clean drain/quarantine and zero hot-loop
allocations. The 1,536-case production PCM screen explicitly preserves the
381 ten-slot sample-availability rejections as regression expectations.

Also passed: 120 probe Python tests, 54 loopback/diagnostic Python tests,
ten installation tests against temporary destinations, three build-profile
identity/isolation tests, the independent model tests, formatting and Clippy
with warnings denied for all three profiles. Feature-conflict compilation and
all six incorrect linked-profile expectations reject as intended. All three
bundles pass ad-hoc signature verification. Actual CPU cost and native callback
delivery still require hardware measurement; offline test duration is not that
measurement.

Local evidence is under `build/validation/ten-slot-offline-20261020/`, including
source snapshots and hashes, per-command results, both independent model
outputs, phase-screen results, explicit failed acceptance and three offline
bundle identities. The old eight-slot evidence seal is reverified separately.
The installed binary remains `dev-673cc272caa79014`, SHA-256
`e8b420efc0156e334137caa6870fc45e4c47e58abc6d4ffcaf7d09037831594c`.

## Next bounded design

Separate available slot capacity from **when an OUT request is packed and
submitted**. Model a bounded preparation window that waits for actual contiguous
PCM coverage while retaining at least 16 ms returned submission lead under the
same stalls. Test the intersection of those deadlines before changing runtime
policy. An empty interval is a design rejection, not permission to silently
increase safety margins or weaken the lead/sample gates.

This follows the useful ASFireWire distinction between transport coverage and
PCM readiness, but the FTU boundary still forbids modifying submitted USB data.
Any future implementation must keep ordered capture-derived counts, immutable
pending payloads, preallocated storage, bounded work and callback-owned reuse.
Do not implement FireWire-style late writes into pending USB buffers.

That scheduling experiment is the next task; it is not included in this profile.
If it passes offline, characterize the existing six-frame baseline shift before
a frozen hardware comparison, then measure CPU cost and delivery behavior at
unchanged margins. Safety reduction toward under 10 ms remains separate.
Reboot/startup reliability stays an open release gate.
