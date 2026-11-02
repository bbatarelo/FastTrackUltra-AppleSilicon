# Retry diagnostics and combined-cost tests

20 October 2026. Offline follow-up to the [PCM-readiness prototype](PCM-READY-PROTOTYPE.md).
The cost/stall coverage gaps are now exercised and retry counters are exposed.
The subsequent [deadline-order experiment](DEADLINE-SUBMISSIONS.md) retains these
rows and improves the 8 ms combined-stall result; this report remains the input-first reference.
This is not hardware qualification or a latency reduction. Normal build/install
still selects 8 ms / four slots; the 4 ms / ten-slot ready profile stays opt-in.

## What changed

Rust publishes a fixed, coherent retry snapshot to the new read-only `ftuR`
property. The C HAL layer only serializes it. Existing status/timing properties
and native pool ABI v4 are unchanged. There are no new locks, allocation, waits
or USB operations in audio callbacks. Only the ready policy collects owner-side
retry data; the eager profile publishes its policy and empty counters.

The production fixture now keeps microsecond time across pump returns, native
submissions and simulated preparation, including a 12.5 ms owner stall. HAL
callbacks catch up against that elapsed time before input publication; submission
work cannot disappear from the HAL clock. Preparation cost is charged after
reading PCM, so later HAL writes cannot repair the owned snapshot. Native IN
and OUT costs are independently configurable, as are uniform versus alternating
callback delays. These are synthetic costs, not claims about measured CPU time.

The existing lead/PCM screens remain at their original settings: 48 kHz,
safety 1024/2560, count replay 64 ms, PCM offset 3072, worker 8/1/2 ms,
26-bus-frame deferral cutoff and 16-bus-frame returned-OUT lead requirement.
The unchanged delayed-HAL disturbance is buffer + 832 frames. Acceptance also
requires zero steady IN/OUT missing frames, count conservation, immutable
pending payloads, no streaming allocations, and clean drain/quarantine.
Integer USB-frame lead may exceed physical lead by almost one millisecond.

## Reproduce offline

```sh
make test-prototype-4ms-ready
cargo run --release --locked --offline --example placement_model -- --retry-cost-study
make test-hal test-loopback
```

The new diagnostic client option is `build/hal-check --retry`. It reads the
installed driver's property without starting audio; it requires a build that
exposes `ftuR`. It was built but not run against the installed driver in this
work. The C/Rust boundary was tested using the hardware-free boundary harness.
`tools/hal-loopback/retry_analysis.py` validates a saved property JSON object.
An unavailable property, torn snapshot or epoch mismatch must not become an
empty successful result.

## New acceptance measurements

The new production matrix covers **6,528 cases**: eager and ready policies,
buffers 64/512/4096, all 64 relative HAL phases, and 17 workloads at an 8 ms
pump interval. Each policy accepts **1,464/3,264** cases and rejects **1,800**.
These totals include the explicit startup and 32 ms rejection controls; the
individual rows remain authoritative. The complete matrix is not a pass.

| Workload (ready policy) | Accepted / cases |
|---|---:|
| 1 ms per IN and OUT call, recurring after steady boundary | 192/192 |
| 1 or 2 ms per OUT call only | 384/384 |
| 2 ms per IN and OUT call | 0/192 |
| 8 ms owner stall plus 1 ms per IN and OUT call | 120/192 |
| 12.5 ms owner stall plus 1 or 2 ms per IN and OUT call | 0/384 |
| Uniform 5 ms callback delay plus 1 or 2 ms per IN and OUT call | 0/384 |
| Delayed HAL plus 1 ms per IN and OUT call | 192/192 |
| Delayed HAL plus 0.5 ms preparation, 0.5 ms IN / 1.5 ms OUT calls | 0/192 |

The recurring 1 ms two-direction screen retains at least 19 bus frames of
returned lead. With 2 ms on every IN and OUT call, native work alone consumes
the whole 4 ms transfer period; startup, catch-up bursts and other work add to
that load. An OUT-only 2 ms cost is a different budget and passes these rows.
The 8 ms stall plus 1 ms calls rejects 24/64 phases at every buffer, despite
passing the stall and ordinary service-cost cases separately. Under ready
policy the below-gate session stops, preserving/draining successfully submitted
requests; eager can continue with a recorded acceptance violation.

The independent model adds **864 cases**, with both policies accepting
332/432 each. Its original preparation-study matrix retains **501/504** positive
cases and all three combined-stall rejections; permanent-hole and 32 ms controls
remain rejected. The original production PCM sweep still passes all **2,304**
ready cases at 1/4/8 ms pumps, and retains the eager ten-slot 381/384 delayed-HAL
rejections. The new sweep does not replace or weaken those references.

The model and production harness use different owner-wake semantics. The model
uses periodic owner opportunities while HAL/hardware advance every sample;
the fake native pump advances by a relative interval and the production owner
can immediately process multiple available requests. They intentionally remain
independent. The prior three model failures (8 ms cadence plus 12.5 ms stall)
remain recorded even where the production fixture passes that combination.
No failed workload is relabeled as a successful acceptance case.

Startup-cost rows apply cost from the first submit. Other new cost rows apply
recurring per-call cost after the steady window begins; this isolates sustained
service from priming without hiding startup failures. The 32 ms stall is the
retained over-budget rejection control. Measured minimum-returned-lead excludes
a request stopped before native submit, so a large minimum plus a fault is
still a failure. Final status and loss counters are always part of acceptance.

## Property layout and interpretation

`struct ftu_retry_snapshot` is 256 bytes: twelve `u64` header words, then two
banks of ten `u64` words. The single writer publishes atomic words under a
sequence counter. A read makes one bounded attempt; `valid=0` means concurrent
publication/reset, and its other fields are unavailable. Lifecycle joins the
old worker before a new epoch resets the snapshot. Stop/fault retains the last
published epoch. Read it with matching `ftuS` status/build; this property alone
cannot establish audio health.

| Header index | JSON field | Meaning |
|---:|---|---|
| 0–3 | `version`, `valid`, `epoch`, `policy` | v1; policy 0 eager / 1 ready |
| 4–5 | `wait_cutoff_bus_frames`, `lead_gate_bus_frames` | 26 / 16, unchanged integer-frame gates |
| 6–7 | `retry_budget_ns`, `work_budget_ns` | 8,000,000 / 2,000,000; modeled allowances |
| 8 | `steady_after` | 48,000 ordered capture frames |
| 9–10 | `timebase_numer`, `timebase_denom` | Host tick conversion; zero before configuration |
| 11 | `waiting` | One incomplete OUT deferral is still awaiting a retry; not USB ownership |

The two banks are JSON `all` and `steady`. A retry is classified by the ordered
capture cursor at its next policy check, even if its deferral happened during
startup. Consequently steady retries can exceed steady deferrals.

| Bank index | Field | Meaning |
|---:|---|---|
| 0 | `checks` | OUT readiness-policy checks, including a check rejected by its lead gate |
| 1 | `attempts` | Completed staging attempts with a decision |
| 2 | `ready` | Exact PCM interval available at staging |
| 3 | `deferrals` | Incomplete PCM; retry permitted by the unchanged cutoff |
| 4 | `fallbacks` | Incomplete at the decision; normal read/loss accounting follows |
| 5 | `retries` | Policy checks following a deferral |
| 6 | `over_budget` | Measured retry interval strictly greater than 8 ms |
| 7 | `invalid_time` | Backward ticks, invalid timebase or unrepresentable converted interval |
| 8–9 | `max_retry_ns`, `last_retry_ns` | Maximum / last successfully measured interval; not measured when no valid retries exist |

The interval starts at the deferral decision after staging and ends at the next
eligible OUT policy check, before its staging read. It includes intervening
input work, pumping and descheduling. It does not measure time before OUT
eligibility or a terminal wait that never reaches another policy check (including
a prior stale-schedule/clock/IN failure); `waiting=1` exposes that unresolved
interval. Check `ftuW`, `ftuD`, `ftuP` and terminal status for those delays.
Counters saturate; invalid timing never changes scheduling or substitutes zero
into a measured interval. Snapshot validation fails closed on inconsistent or
saturated relationships.

A fallback is not necessarily lost PCM: a concurrent HAL write can fill the
interval before the normal fallback read. `playback_missing` and `OutputMissing`
remain the audio-loss measures. Similarly, an over-budget retry is an observation;
only the existing lead and audio criteria determine acceptance. A deliberate
9 ms pump fixture reports 117/117 retries over budget and a 9,000,000 ns maximum,
with identical frames, completions and clean drain with publication off/on.

## Validation and decision

All four profiles passed the full regression suites (151 Rust tests each,
three intentionally ignored observer benchmarks), then the added observer
on/off equivalence test passed in all four configurations: **152 tests covered
per profile**. The final bundles were rebuilt after that test-only addition.
Native and HAL AddressSanitizer/UndefinedBehaviorSanitizer checks, 120 probe
Python tests, 60 loopback/diagnostic tests (including six new retry-validator
tests), ten temporary-destination installer tests, three build/profile tests,
sixteen model tests, formatting and four Clippy configurations passed.

C/Rust ABI tests verify the 256-byte layout, read-only property behavior,
short-buffer rejection and numeric serialization. The JSON validator consumes
both idle and nonzero C-generated fixtures from each profile. Atomic publication
stress checks coherent reads and explicit invalid snapshots; lifecycle checks
cover retention and reset. The production matrix checks the published snapshot
against the actual session counters, including decisions that fail the lead gate.

Evidence is frozen under `build/validation/retry-cost-offline-20261020/`, with
source hashes, incremental changes, command logs, all four final profile bundles,
separate acceptance results and validation provenance. The three previous offline
evidence seals and installed binary hash are reverified. No source files were
sent externally for this follow-up; the earlier Claude review remains in its
original evidence directory.

The new counters and tests close the identified observability/coverage gaps.
The scheduling rejection remains real. Keep this profile offline: do not lower
safety margins or treat the synthetic cost allowance as a measured OS bound.
A concrete next experiment is deadline-ordered admission with bounded catch-up
work, so filling free IN slots cannot postpone an urgent OUT submission through
an entire batch. Compare it against the same rejected rows and retained eager
reference; do not change the lead, loss or latency gates to obtain a pass.

Before hardware, the six-frame baseline phase discrepancy and startup-after-reboot
failure remain open. Any later comparison also needs a frozen candidate/rollback,
active build/epoch verification and measured native cost. The installed binary
remains `dev-673cc272caa79014`; this work starts no audio, installs no bundle,
restarts no daemon and makes no commit.
