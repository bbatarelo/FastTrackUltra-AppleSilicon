# Deadline-ordered submissions — offline prototype

20 October 2026. Follow-up to [retry/cost testing](RETRY-COST-OBSERVATION.md).
The new scheduler improves the combined-stall screen, but does not pass the
complete acceptance matrix. It remains an offline comparison, at unchanged
latency and margins. Normal build/install remains 8 ms / four slots.

## Scheduling and bounds

The opt-in `prototype-4ms-deadline` profile keeps 4 ms transfers, ten slots per
direction and exact PCM readiness. Rust chooses between the next eligible IN
and OUT request after each submission. The effective deadlines are IN start
minus two integer USB frames (the existing native scheduling guard) and OUT
start minus sixteen (the existing returned-lead gate). OUT wins ties. A request
is eligible only with a free matching slot and, for OUT, ordered captured count
feedback. Per-direction sequences, count replay and sample coordinates do not
change. A choice never authorizes touching pending USB memory.

Catch-up is bounded in two ways:

- After two attempts or 2 ms elapsed work, check stop and reconcile already
  delivered completions/capture before choosing again. Update the worker event's
  poll boundary. This check does not dispatch new native callbacks.
- After at most ten attempts in total, return to the existing native pump. The
  hard count includes readiness attempts; rechecks do not reset it. Incomplete
  OUT PCM that may defer returns to the pump immediately, so subsequent IN
  catch-up cannot consume that retry's allowance.

Ten attempts are one direction's queue generation, shared across both directions,
not ten for each. The 2 ms interval bounds when work is reconsidered, not total
work before a pump. A synchronous native call cannot be preempted: an overlong
call is checked on return and a successfully submitted request retains ownership
through failure/drain. Invalid or backward host time requests an early recheck.
There is no new native scheduling policy, spin wait or HAL-thread work.

The old input-first eager and ready profiles remain available. The new linked
read-only discriminator `ftu_live_submission_order()` is 1 only for the deadline
profile, 0 otherwise. Build identity, bundle profile metadata and transport
readback distinguish otherwise identical ten-slot ready geometries. `ftuR`
still describes readiness policy 1 for both; it does not identify submission
order by itself. No native function-table or retry-property ABI changes.

## Reproduce without hardware

```sh
make test-prototype-4ms-deadline
cargo run --release --locked --offline --example placement_model -- --deadline-study
```

Run the heavy profile suites one at a time to avoid host CPU contention in the
wall-clock watchdog tests. The builder writes only `build/prototype-4ms-deadline/`. It never installs or
starts audio. All prototype feature pairs are rejected, and the profile checker
rejects the wrong linked geometry, readiness policy or submission order. No
installer for this prototype is prepared.

## Acceptance results

The gates remain 48 kHz; input/output safety 1024/2560; worker 8/1/2 ms; replay
64 ms; PCM offset 3072; deferral cutoff 26 bus frames; returned OUT lead at least
16 bus frames; zero steady missing IN/OUT PCM; ordered count conservation;
immutable pending payload; no streaming allocation; clean drain or quarantine.
The delayed-HAL disturbance remains buffer + 832 frames. Startup is classified
separately at the existing 48,000-frame boundary. Integer bus-frame lead can
exceed physical lead by almost one millisecond.

The production cost sweep runs 17 workloads at buffers 64/512/4096, all 64
relative phases, with 8 ms pumps: **3,264 cases per policy**. The new order accepts
**1,815**, versus **1,464** for the ready input-first reference. The remaining
**1,449** candidate cases are rejections, including explicit negative controls.
A test that correctly reproduces those failures is not a matrix pass.

| Workload | Ready input-first | Deadline order |
|---|---:|---:|
| 1 ms each IN/OUT call | 192/192 | 192/192 |
| 8 ms owner stall + 1 ms each call | 120/192 | **192/192** |
| 12.5 ms owner stall + 1 ms each call | 0/192 | 120/192 |
| Uniform 5 ms callback delay + 1 ms each call | 0/192 | 159/192 |
| 2 ms each IN/OUT call | 0/192 | 0/192 |
| Delayed HAL + 0.5 ms preparation, 0.5 ms IN / 1.5 ms OUT | 0/192 | 0/192 |

The newly passing 8 ms combined-stall rows retain at least 19 bus frames of
returned OUT lead. The uniform-delay row still loses capture data in 33 of 64
phases at the 64-frame HAL buffer, despite completing without a transport fault.
At buffers 512 and 4096 it passes all phases. This is why terminal status and
OUT headroom alone cannot qualify a scheduler. The 12.5 ms plus cost cases
retain 24/64 rejections at each buffer, including below-gate output returns.
2 ms on every IN and OUT call consumes the entire 4 ms transfer period before
pump, packing or other work; ordering cannot manufacture that missing capacity.

The original production PCM sweep still passes all **2,304** ready input-first
cases, and the candidate passes the same **2,304** cases at 1/4/8 ms pumps. The
eager ten-slot delayed-HAL reference still rejects 381/384 cases. All 60 retained
PCM rows and all 102 retained cost rows match the prior sealed values exactly,
including loss, lead and retry measurements; they were not retuned.

The independent nominal model retains the earlier input-first rows and adds
bounded bursts with callback dispatch only at periodic owner wakes. It accepts
**278/432** candidate cost/stall cases versus **332/432** reference cases. With
1 ms calls it improves the 8 ms cadence / 12.5 ms stall row from 0/24 to 15/24;
with 2 ms calls its bounded catch-up rejects rows that the old model's work/
wake behavior accepted. The 8 ms cadence / 12.5 ms stall with zero cost still
rejects one phase. These different wake semantics are explicit; do not merge
model and production totals or use one to erase the other's failures. Separate
physical-marker tests retain the same round-trip sample coordinates and reject
permanent PCM holes and over-budget stalls. This is not an RTL reduction.

## Fixture fidelity and verification

Rechecking completion state during catch-up exposed a fake-native assumption:
previously merely advancing simulated time made a callback available. The fake
now records the most recent pump's dispatch frame. Submission work and repeated
completion queries cannot dispatch a callback. A direct negative test verifies
this, and all retained input-first acceptance totals are compared with the
previous sealed evidence. Native mechanics are unchanged.

The production fixture independently counts submissions between pumps, so the
ten-attempt bound is checked against actual native calls as well as session
counters. Stop tests cover both two quick attempts and one call exceeding 2 ms.
Ownership tests run both orders over 2,048 transfers, verify payload CRC and
packet-count replay, inject errors, and retain unknown-ownership quarantine.
Capture-only tests cover all 24 pump/delivery phases for each order, plus an
OUT call that crosses the return gate after ownership has transferred. Streaming
allocation counters stay zero. Retry snapshots remain checked against the actual
session's counters; their validity does not override audio-loss gates.

Validation covers **157 Rust tests per profile** across all five builds, with
three intentionally ignored observer benchmarks, plus seventeen model tests.
Three full profile invocations passed directly. Two initial parallel invocations
had one failure each during the unchanged wall-clock watchdog checks; each failed
test was rerun in isolation with one test thread. The failed logs and successful
reruns remain in the evidence. Runtime watchdogs and acceptance gates were not
changed. A profile's passing test coverage must not be confused with its acceptance
matrix: the latter still includes the rejections above.

Native and HAL AddressSanitizer/UndefinedBehaviorSanitizer checks, 120 probe
Python tests, 60 loopback/diagnostic tests, ten temporary-destination installer
tests, three build-contract tests, formatting and all five Clippy configurations
passed. Six conflicting feature pairs are rejected; all 25 cross-profile linked
readbacks enforce geometry/readiness/order identity. Each final bundle is checked
against its source manifest and ad-hoc signature. Read-only retry JSON fixtures
are emitted by the hardware-free C boundary tests for all profiles.

Evidence is frozen under `build/validation/deadline-offline-20261020/`, including
source hashes, incremental changes, command logs, profile bundles, independent
models and separate acceptance results. Earlier offline evidence and the installed
binary are reverified. No source was sent externally for this follow-up.

## Decision

Keep deadline ordering as an opt-in research profile; do not install it or lower
margins. It closes the 8 ms combined-stall failure and preserves the original PCM
screens, but larger combined delays, capture delivery and saturated service
budgets remain real limits. The next bounded experiment is to separate dispatch
of already-ready callbacks from idle waiting, while retaining the hard catch-up
bound and these exact workload rows. Model that distinction before changing
native run-loop mechanics; a shorter requested wait alone does not guarantee
an earlier owner wake.

The unresolved six-frame baseline RTL shift and first-start-after-reboot failure
remain hardware/release gates. The installed `dev-673cc272caa79014` binary, ordinary
build profile and safety margins remain unchanged. This work starts no audio,
installs no bundle, restarts no daemon and makes no commit.
