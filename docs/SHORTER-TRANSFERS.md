# Shorter USB transfers: measurement and design decision

20 October 2026. **Decision: build the shorter-transfer prototype offline.**
The ten-minute session completed with zero steady audio losses, but failed its
unchanged-latency comparison. No new driver was installed, no margins changed,
and no second long session was started.

## Latest follow-up

The [ten-slot offline experiment](TEN-SLOT-PROTOTYPE.md) now passes the retained
lead screens but fails delayed-HAL sample availability. It remains unapproved
for hardware. The following eight-slot report and original results are preserved
as the comparison. The subsequent [PCM-readiness prototype](PCM-READY-PROTOTYPE.md)
implements the separate preparation policy offline.

## Offline implementation and decision (20 October)

**Implemented and tested; do not install this prototype.** The production Rust
pool and native callback adapter now accept validated 8/4/2/1 ms geometry. The
opt-in live profile is **4 ms, eight slots per direction, 32 packet entries and
64 feedback-history entries**. A normal `make` / `make install` still selects
8 ms / four slots. The installed binary was not changed during this work.

Run the offline prototype from the repository root:

```sh
make test-prototype-4ms
```

This builds a separate Cargo target, links and ad-hoc signs an experimental
bundle, and runs Rust plus native/HAL sanitizer tests. It never opens the audio
device, installs the bundle or restarts CoreAudio. `make prototype-4ms` builds
without rerunning the tests. Outputs are in `build/prototype-4ms-eight-slot/`.
The bundle has a profile-specific content identity and `FTUTransportProfile`
metadata. Both normal and prototype bundle builds read back geometry from their
actually linked Rust library and reject the wrong profile before bundling.

The first prototype still uses 64 ms count replay, PCM offset 3072, safety
1024/2560 and the 8/1/2 ms worker policy. It does **not** lower measured latency.
The private pool ABI is now v4; the standalone one-shot ABI is unchanged.
Allocation occurs before streaming. The native side allocates exact active
packet/data extents and transfers ownership only on a matching callback.
Inactive completion entries must be zero; malformed geometry or ABI versions
stop admission. OUT lengths always come from ordered IN completions.

The diagnostic `ftuW`, `ftuD` and `ftuF` records now use schema v2 and carry active
geometry. Analyzers retain v1 support for historical captures. Last-packet time,
slot/predecessor identity and packet validation use the configured geometry;
inactive packets are not reported as audio. `ftuT` and `ftuP` remain unchanged.
The `ftuW` late-event collection threshold becomes 24 ms for eight 4 ms slots;
it is an observation threshold, **not** a change to the 16 ms hardware gate.

### Test result and retained rejection

The real production pool was run through 240 delay/phase/drift scenarios. The
fake bus clock has integer-millisecond resolution: 4.364 ms measured callback
latency is rounded up to 5 ms, and the observed 7.826 ms outer-pump / 7.523 ms
empty-timeout durations to an 8 ms owner stall. These are separate screens;
they are not treated as statistically independent, additive measured delays.
Each row covers 16 millisecond phases and -200/0/+200 ppm packet-count drift.

| Injected condition | Minimum returned OUT lead | Result |
|---|---:|---|
| 5 ms callback delay | 23 ms | 48/48 above 16 ms |
| 8 ms owner stall | 20 ms | 48/48 above 16 ms |
| 9 ms callback delay | 19 ms | 48/48 above 16 ms |
| 9 ms delay plus 4 ms owner stall | **15 ms** | **6/48 below the gate**; transfer completion is not acceptance |
| 9 ms delay plus 32 ms owner stall | — | 48/48 stop with stale schedule and drain cleanly |

The independent 125 µs timing model agrees in direction and minimum integer
lead: at the matching 4 ms/eight-slot/64-history geometry, its retained envelope
plus 4 ms stall rejects 16/64 phases and reaches 15 ms bus-frame lead (14.625 ms
in exact model time). That model includes serialized work and a different
phase grid; its rejection count is not expected to equal the production fake's.
No acceptance threshold was relaxed, and no failing phase was omitted.

The remaining validation passed:

- **142 Rust tests per profile**, plus native and HAL AddressSanitizer /
  UndefinedBehaviorSanitizer checks; the three explicitly ignored observer
  microbenchmarks remain excluded. Both profiles pass Clippy with warnings
  denied and formatting checks.
- 2,048 sequences per direction for each 8/4/2/1 ms profile: exact count replay,
  slot/history reuse, unchanged OUT CRCs, valid clock fit and zero hot-loop
  allocations. Faults cover submissions, packet/request errors, malformed
  completion geometry, poisoned inactive entries, callbacks, stale tickets and
  abort/drain. Stale ownership is quarantined rather than freed.
- The real four-millisecond pool, packed PCM code and HAL rings together:
  **384 buffer/phase combinations** (64–4096 frames, all 64 distinct relative
  HAL/USB sample phases), 600 transfers per direction with a 5 ms callback-delay
  fixture. Zero steady missing samples, correct input/output values, conserved
  frames, clean drain and zero hot-loop allocations. Startup is excluded from
  the steady assertion; input callbacks after the finite IN stimulus ends are
  outside its capture window. This is a simulated HAL clock, not a recording.
- Separate packing tests exercise every sample offset against each HAL buffer
  across ring wrap for all four transfer sizes, minimum/maximum packet lengths,
  and signed 24-bit values. Existing hole/late-repair and lifecycle tests remain.
- 120 probe Python tests, 54 loopback/diagnostic Python tests, ten installer tests
  using temporary destinations, and two profile identity/isolation tests.
  Independent timing/placement model tests and saved study outputs also pass;
  their nominal analog fixture is not a physical latency measurement.

Claude reviewed the nine authorized transport/build files and reported no
blocking issue. Its inactive-tail and build-provenance suggestions resulted in
additional rejection tests and the linked-profile check. A clock-validation
failure also no longer advances the completed-sequence cursor. The review did
not include hardware behavior or the entire repository.

**Decision:** retain this as the completed eight-slot offline reference. The
next design step is to test additional queue headroom (for example, ten 4 ms
slots) against the same stall, PCM-availability and CPU-cost screens; do not
promote the eight-slot profile by rerunning only easier cases. No such revised
live profile is selected here. Effective worker CPU cost and delivery behavior
still need hardware measurement, and the six-frame same-configuration baseline
shift must be characterized before any hardware candidate. Lowering safety
margins remains a later, separate change toward the under-10-ms target.

Local evidence: `build/validation/shorter-transfer-offline-20261020/`, including
source/binary hashes, command results, model outputs, Claude's authorized review
and both offline bundles. Source was built in a temporary mirror and copied back
after checking that the active files still matched their starting hashes.
Baseline bundle: `dev-0c2a1a7615db9485`; prototype: `dev-a426a1f536fdefc4`.
These are offline identities, not a replacement for the installed
`dev-673cc272caa79014`. No hardware audio session, installation, daemon restart
or commit was performed.

## Ten-minute hardware result

One eight-second silence check at 128 frames passed, followed by the requested
600-second duplex run at the same actual buffer. Native 48 kHz; rear output 3 →
rear input 4 (Rear); 0.01-peak chirps every five seconds; all other outputs zero;
no input monitoring. The device remained on the direct Mac USB connection.

Installed build: **dev-673cc272caa79014**; binary SHA-256
`e8b420efc0156e334137caa6870fc45e4c47e58abc6d4ffcaf7d09037831594c`.
It includes the all-pump observers and the later 48 kHz initializer. It is not
the original all-pump binary rejected at startup on 19 October.

| Check | Result |
|---|---|
| Captured duration / callbacks | 28,800,000 frames / 225,000 callbacks, all 128 frames |
| Chirp signal checks | 120/120 accepted |
| Steady playback / capture missing samples | 0 / 0 |
| Startup playback misses | 1912, retained separately; silence preflight had 2007 |
| Raw analog round trip | 81.330641–81.330655 ms |
| Prior same-buffer baseline | 81.205797–81.205799 ms |
| Comparison gate | **Failed**: shift 5.9924–5.9932 frames, outside ±2 frames |
| Minimum returned OUT submission lead | 20 ms, above the unchanged 16 ms gate |
| Additional availability screen | 137 frames remaining; a heuristic, not contiguous-coverage proof |
| Observation integrity | 150,038 completions/admissions, 308,270 pumps; no dropped/invalid observations |
| Worker sampling | 601 samples; estimated 9.53% of one CPU, max sampler duration 39.125 µs, no sampled policy changes |
| Teardown | Idle epoch 4; zero pending requests, fault, reason and cleanup error; buffer restored to 512 |

The eight-second preflight retained 384,000 frames / 3,000 callbacks in epoch 3.
The immutable first-failure record remains epoch 1, not a new failure in either
session. All diagnostic readbacks refer to their respective stopped epoch.
Normal input/output/alerts stayed on built-in devices. The daemon PID, boot and
USB registry identity were unchanged; the bounded kernel log contained cleanup
aborts but no babble. No reboot, daemon restart, automatic retry or rollback was
performed. Earlier startup-after-reboot failures remain unresolved.

### Why the result is not a full qualification pass

Every chirp has essentially the same six-frame shift. The chirp reference bytes,
channel pair, rate and safety values match the saved baseline. An independent
integer dot-product check found a peak at frame 3904 for the new first/middle/last
chirps, versus 3898 in the previous recording. This is not just a sub-sample fit
or a display rounding difference. The paired HAL scheduling distance remains
3840 frames throughout; the timestamp-relative remainder moved from about
57.878 to 63.871 frames.

Six samples at 48 kHz are one 125 µs high-speed USB microframe. That is a useful
hypothesis for a session/transport phase difference, not a demonstrated cause.
The current direct topology, initializer/build, session and power history differ
from the older baseline. There is no controlled comparison here that isolates
which matters. No correction was subtracted and the frozen ±2-frame gate was
not widened. The runner exited 1 with the sole rejection
`latency_outside_two_frame_gate`; all raw evidence remains intact.

This small unexplained shift does not account for the roughly 80 ms scheduling
latency, but it prevents calling the whole session an unchanged-latency pass.
Resolve or characterize it with a matched same-configuration baseline before
installing a shorter-transfer candidate or reducing margins. There is no second
automatic ten-minute run in this task.

## What the all-pump records establish

The exact retained worst steady events partition as follows:

| Direction | Last packet → callback | Previous pump | Between pumps | Current pump | Uncovered |
|---|---:|---:|---:|---:|---:|
| IN | 3.912125 ms | 0 | 0 | 3.912125 ms | 0 |
| OUT | 4.364125 ms | 3.924708 ms | 0.413791 ms | 0.025625 ms | 0 |

The OUT event meets the predeclared representative threshold of 4 ms. Coverage
is 100%; 90.52% lies inside pumps, with less than 1 ms between them. IN falls
just below the representative threshold and does not contradict that pattern.
The OUT current pump alone was short; its preceding IN pump accounts for most
of the interval. Across the steady run the largest between-pump gap was
0.698 ms; an empty timeout lasted 7.523 ms despite a requested 2 ms wait.

These observations support ending the search for a large Rust operation between
pumps. They do **not** identify kernel-message readiness, distinguish kernel
waiting from worker descheduling, or justify changing run-loop timeout/policy.
The timing records pass their integrity/link checks independently of the failed
physical-latency comparison. Accordingly, the architectural decision below is
limited to offline work; it is not approval of a hardware candidate under the
original accepted-session rule.

## Design decision

Proceed with a **configurable shorter-transfer transport**, starting with an
**offline 4 ms / eight-slot profile per direction**, alongside the existing
8 ms / four-slot baseline. The design must support 2 ms and 1 ms batches without
another native ABI rewrite. This closes the all-pump measurement phase; another
observation-only installation is not the next task.

The first 4 ms prototype keeps 64 ms count replay, the matching 3072-frame PCM
coordinate offset, input/output safety 1024/2560 and the 16 ms OUT lead gate.
It is a transport/ownership experiment with **no expected RTL reduction**.
It is not ready to install. Eight slots are selected to retain 32 ms queued
coverage; four 4 ms slots already failed the model's headroom screen. The
previous corrected model gave eight slots 19 ms minimum lead, but an additional
4 ms owner stall still made 16/64 phases fail the 16 ms gate. A production-pool
simulation must therefore replay the new measured stalls before this profile
can become a hardware candidate. Failure means revising this profile offline,
not lowering its acceptance threshold.

Why start at 4 ms: it halves publication batching and doubles the completion
rate while preserving the ownership mechanism. This gives a bounded way to
validate geometry changes before the approximately eightfold completion-rate
increase of 1 ms batches. A smaller batch is useful only if CPU cost, delivery
tails and actual sample availability improve together.

### Implementation boundary

Keep protocol, timing, sample placement and ownership in Rust. Native C should
validate geometry, allocate the configured USB storage and expose bounded
submission/completion operations.

1. Introduce one resolved live-transport geometry: microframes per transfer,
   duration, slots per direction, feedback-history capacity, count-replay delay,
   sample offset, packet capacities and observation units. Validate 8/4/2/1 ms
   profiles (64/32/16/8 high-speed microframes) explicitly. Keep the existing
   standalone one-shot probe ABI intact; version the private live-pool ABI.
2. Replace live-pool fixed-size/deadline assumptions in `native.rs`, `pool.rs`,
   `live.rs`, `geometry.rs`, `ftu_native.h` and `duplex.c`. Use bounded allocation
   before streaming, with no allocation during audio processing. All native
   lengths, slot counts and returned completion sizes must agree with Rust.
3. Retain capture-derived counts and ordered feedback: never substitute nominal
   packet lengths. Derive history capacity from replay age, in-flight work and
   out-of-order completion allowance. At 4 ms, 64 ms replay already spans 16
   sequences; at 1 ms it spans 64. The current 32-entry history cannot simply
   be carried into the 1 ms profile. Assert capacity and test wrap/overwrite.
4. Preserve ticket/epoch identity, immutable submitted OUT payloads and exactly
   one matching completion before slot reuse. Exercise duplicates, stale
   callbacks, failed submissions, reordering, abort/drain and undrained teardown.
   Generation checks do not by themselves permit accessing freed or in-flight
   storage.
5. Run the actual production pool with drifted packet counts, all HAL phases,
   observed delivery stalls plus explicit additional disturbance, ring holes,
   maximum HAL buffers and lifecycle changes. Assert frame conservation,
   count replay, coordinate mapping, actual PCM misses and clean ownership.
   Update diagnostic geometry/schema validation rather than leaving 8 ms
   assumptions hidden in the analyzers.
6. Measure effective worker policy and CPU cost for each geometry. Preserve the
   current policy for the first comparison only if its requested budget and
   measured cost are compatible. Any change needs a named rationale and
   validation; setting a time-constraint policy is not a scheduling guarantee.

Before installation, resolve or characterize the six-frame shift with a matched
baseline, and require native/HAL boundary sanitizer checks, Rust
ownership/lifecycle tests, independent placement-model agreement and a frozen
candidate/rollback. The first hardware gate remains isolated silence plus raw
loopback, same-epoch counters and clean teardown at unchanged margins. Lower
input/output safety only in a subsequent candidate backed by measured
publication and submission deadlines. Startup-after-reboot reliability is a
separate unresolved release gate; the owner deferred further reboot testing.

## The budget for under 10 ms

The target is **measured analog round trip at native 48 kHz**, with the actual
HAL buffer stated. It excludes extra plug-in delay. Start qualification at
64 frames and also evaluate 128; passing at 64 does not imply passing at 128.
A 512-frame callback already contributes about 21.33 ms for two buffers.

For the currently observed reference planes:

`raw RTL frames ≈ 2 × HAL buffer + input safety + output safety + about 64`

The approximately 64-frame remainder is measured timestamp-relative path, not
isolated converter delay and not guaranteed to remain unchanged after a
transport redesign. The 64 ms packet-count replay is compensated by the PCM
coordinate offset; adding it again would double-count it.

| Actual HAL buffer | Two buffers | Required combined safety budget for <10 ms, assuming a 64-frame remainder |
|---:|---:|---:|
| 64 frames | 2.67 ms | Less than about 288 frames / 6.00 ms |
| 128 frames | 5.33 ms | Less than about 160 frames / 3.33 ms |

For scale, 256 combined safety frames would predict about 9.33 ms at 64 frames;
144 would predict about 9.67 ms at 128. These are **budget examples, not safe
constants or promised results**. Current combined safety is 3584 frames, and
current OUT submission alone requires 16 ms of lead. Neither a declaration
change nor a modest queue cut can meet the target within that geometry.

The proposed low-latency path targets approximately 1 ms publication/submission
units and a much shorter verified output lead. Shorter units do not ensure a
sub-millisecond worker wake. If the current USB API and immutable submission
model cannot meet that budget without steady loss, document the measured floor
and choose a supported transport/scheduling change rather than silently
weakening the gates.

## Which ASFireWire mechanisms we will use

The inspected implementation separates a deep descriptor schedule from late PCM
filling. `HandleOutputWriteEnd` fills exposed packets on the CoreAudio IO thread;
preparation runs separately. A projected hardware cursor plus a two-packet
finality guard limits writable packets. The shared geometry has 1 ms completion
groups, 63 ms hardware TX coverage and 126 ms total preparation coverage. That
coverage is not 63–126 ms of PCM latency because audio arrives later.

For FTU, adopt the separation of concerns: transport reservation, PCM availability,
submission deadline, capture publication and safe reuse must have independent
coordinates and counters. Keep the actual missing-frame counters alongside
headroom; a positive future high-water does not prove contiguous PCM coverage.
Use coherent geometry and explicit valid ring intervals. These are also useful
review items for E-MU; no E-MU change follows automatically from this experiment.

**Do not transplant ASFW's late writes into submitted USB payloads.** The current
FTU native boundary grants write access only before submission. Apple's
`LowLatencyReadIsochPipeAsync` documentation explicitly permits inspecting
completed frame status/counts and processing completed input data before the
final callback; update frequency 1–8 requests updates every N milliseconds,
while 0 updates at transfer end. This is a possible future capture-publication
path. It still requires a sound ARM64 publication/visibility protocol, completed
range validation, same-slot lifetime protection and abort coordination. A
volatile read, acquire fence or double read alone is not that proof. It neither
licenses pending OUT rewrites nor retires the USB request before its callback.

The first shorter-transfer profile therefore keeps callback-owned buffers. Early
capture observation is a separately gated extension if shorter completion units
cannot meet the input budget. This follows the useful ASFW ownership distinction
while respecting the different native transport contract.

The Fireface result provides a relevant comparison: the owner's build-20 notes
record **25.912 ms at 48 kHz / 512 frames / S400**, versus FTU's earlier measured
**97.206 ms at the same rate and buffer**. Fireface's measured scheduling distance
was 1152 frames: 1024 from two callbacks plus 64+64 safety. The remaining path
was about 1.912 ms. Its sustained recordings also reported input faults, and
later profile changes increased FF800 input safety to 256 frames; the initial
latency result is not a clean endurance qualification of the current build.
This comparison supports studying buffer architecture rather than attributing
the difference to FireWire 400 versus USB 2.0 bandwidth.

See [ASFW reuse](ASFW-REUSE.md) for source identity and the concrete E-MU review
checklist. Inspected references are frozen in the local evidence directory;
no ASFW implementation has been copied into FTU.

## Evidence and validation

Local evidence is under
`build/validation/all-pump-decision-20261020/`: frozen plan and inputs,
per-run audio/callback data, same-epoch property logs, analyzer results,
`latency-shift-audit.json`, `decision-evidence.json`, source snapshots and a final
read-only state capture. `verify_decision.py` verifies the rejected comparison
explicitly, recomputes timing/worker/delivery/pump/failure analyses, checks all
callback geometry and preserves the original acceptance gates. The evidence
seal hashes the files after completion.

All 41 production inputs still match the frozen installed candidate. The 21
pre-run inputs and prior warm-recovery, direct-path, dock-path and offline-audit
seals were reverified (120, 146, 127 and 29 files respectively). No runtime code
changed during that hardware-measurement phase. The subsequent offline changes
are documented above. This report covers one unit, one Mac and one warm
session, with no new owner listening check or GarageBand transitions.
