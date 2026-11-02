# All-pump observation candidate

**20 October update:** the requested ten-minute 128-frame session has now run
on the later `dev-673cc272caa79014` initializer build. Signal and observation
checks passed, but the physical latency shifted by about six frames and failed
the ±2-frame comparison. The valid all-pump records support an offline
shorter-transfer design; no new hardware candidate is approved. See
[the measured outcome and decision](SHORTER-TRANSFERS.md). The candidate identity,
startup failure and original plan below are retained as historical evidence.

Prepared on 19 October 2026. **Observation only; not hardware-qualified.**
Candidate: **dev-27ce3330fc59d37e**, binary SHA-256
`bca637500ebaa0922778600eeb4c0efaacbd8abea75a3b5b2d419ddb6cb98830`.
The qualified rollback is `dev-b683adc88b54ab87` (binary SHA-256
`da9106f061e19e37362e334fcaf5f038676b71cc6adf8b13af92b16efcb6d4da`).
The frozen candidate manifest, source snapshot, checks and install/rollback scripts
live under `build/candidates/all-pump-20261019` and
`build/validation/all-pump-20261019`.

Installation update: the owner installed this exact binary on 19 October.
The first read-only HAL query timed out before audio testing. A normal reboot
reset logged AirPlay registrations from 4,096 to one; HAL now responds promptly.
The candidate then failed its startup preflight: its first post-login start
returned an input USB overrun, with clean abort/drain.
See the [CoreAudio restart investigation](STARTUP-RESTART.md).
**At that point, the planned long latency sessions had not started.** The qualified rollback
was restored and passed two controlled starts. The later
[first-failure diagnostic](STARTUP-EVIDENCE.md) includes these
observations and passed two short warm checks, then reproduced the first-IN
overrun after reboot with complete packet metadata retained. It is rejected for
continued testing and has been rolled back. The qualified baseline then reproduced
the same first-IN babble after reboot in one controlled silent start: the newer
observers are not required. Shared initialization is now the priority. The kernel independently logged
the candidate failure as FTU endpoint-0x81 babble. The root cause remains open;
startup reliability takes priority over further latency changes.
No latency or observation-overhead conclusion follows from this startup failure.

## The decision this experiment must support

This closes the measurement gap in [the run-loop study](RUNLOOP-STUDY.md).
Completion-associated pumps omitted all returns that delivered no callback.
We now count every call, retain its real return reason, and correlate adjacent
calls with the actual worst completion and minimum submission-lead events.

We will make a development decision after **one focused hardware session, or
at most two**. This is a stopping rule for this latency investigation, not a
claim that two sessions qualify a general release.

1. **Session 1:** one ten-minute, 128-frame full-duplex loopback run, after a
   short silence/level check. Rear output 3 to rear input 4, channel 4 Rear.
   Quiet 0.01-peak chirps on output 3 only; input monitoring off. This targets
   the buffer size that previously reached the unchanged 16 ms OUT gate.
   Save ftuT/ftuW/ftuD/ftuP from the same stopped epoch.
2. **Session 2 only if needed:** one ten-minute run at 64 frames with the same
   binary and conditions. This changes HAL callback workload, not USB geometry.
   Use it only if session 1 lacks a representative tail, gives mixed evidence,
   or needs confirmation. No automatic retries or third measurement campaign.

A representative tail is a steady last-packet-to-callback interval of at least
4 ms. This is a diagnostic selection threshold, not an OS guarantee. For each
retained worst IN/OUT event, intersect that exact interval with its preceding
pump, the gap between pumps, and its own pump. Report any earlier interval
outside those two pumps as uncovered; do not assign it to a cause.

| Evidence after an accepted session | Decision |
|---|---|
| At least 90% of a representative interval is covered; at least 80% lies inside pumps and the between-pump part is at most 1 ms; other representative retained events do not contradict it | Stop optimizing Rust work between pumps. Keep current production margins and move to an offline design of a coherent shorter-transfer candidate, with packet feedback, queue depth, sample placement and safety budget considered together. Do not infer Mach readiness or change timeout/policy from this measurement alone. |
| At least 90% is covered; the between-pump part is at least 2 ms and at least half the interval, with consistent linked preparation/submission evidence | Address the identified worker operation first. Propose one specific optimization with a before/after test; do not reduce safety offsets as a substitute. |
| No representative tail, incomplete coverage, or mixed evidence | Use the second session once. If still inconclusive, retain the qualified geometry and close this measurement phase as unresolved. Prioritize stable 48 kHz usability/release work; keep deeper scheduler tracing and shorter-transfer work explicit future tasks. |
| Audio, ownership, startup, epoch or observation-integrity failure | Stop the run, retain the failed evidence and use the exact qualified rollback. Decide on the concrete regression before any latency change. A failing test is not discarded or silently rerun. |

An empty timeout overlapping the critical interval is evidence of where time was
spent, not proof that a ready USB message failed to wake the worker. A handled
source after 2.1 ms is likewise a literal outer-entry timestamp observation.
Kernel waiting and descheduling remain indistinguishable here.

## Unchanged behavior and acceptance gates

48 kHz; four 8 ms slots per direction; 64 ms packet-count replay; PCM offset
3072 frames; input/output safety 1024/2560; zero-timestamp period 16384;
worker time constraint 8/1/2 ms; USB update frequency 0;
`CFRunLoopRunInMode(default, 0.002, true)`; minimum OUT lead 16 ms.
No wait observers, busy polling, queue changes, mixer writes or new ownership rules.

Before each session, verify installed binary hash and loaded build identity.
Temporarily move normal output/alerts off FTU if needed to isolate the diagnostic;
restore selections with the existing guarded restoration helper. Confirm clean
Idle and the initial buffer size. Abort on driver fault, steady loss, lead below
16 ms, exhausted availability screen, invalid timestamps or instrument deadline.
Each attempted run gets a fresh directory; preserve failures.

Require clean Idle, zero pending debt, original buffer restored, same epoch
across all readbacks, zero steady audio losses and all chirps accepted.
Compare measured RTL to the existing same-buffer baseline within two frames.
Reject dropped/invalid/overflowed pump data, ID discontinuities, missing context,
mismatched delivery/admission links or sampler failures. The availability screen
is still a heuristic; it does not prove contiguous sample coverage.
A later listening check and broader lifecycle/app regressions remain separate
requirements before calling this candidate generally qualified.

## Implementation and ownership

The private native pool ABI v3 adds an optional `last_pump` copy-out hook.
The original one-shot ABI and ftuD v1/ftuW/ftuT schemas are unchanged.
Native C records only the outer enter/return ticks, literal CF result, callback
count, first/last callback ticks and rejected callback count. It does not read
pending packet lists or payloads. Callback counts include rejected stale/duplicate
tokens, making an invalid run visible. A fresh native owner starts IDs at 1;
exhaustion reports ID 0 rather than wrapping.

Rust observes exactly once after every ordinary or drain pump. A missing hook
is explicit unavailable telemetry and never a transport error. Startup means
capture cursor below 48000 frames; steady means at least 48000; drain is separate
and runs under the existing restored normal policy. Phase is chosen before the
next completion poll advances the captured cursor.

Storage is fixed. One writer uses a nonwaiting atomic guard; readers try at most
twice and report valid=0 on a torn read. Counter/generation exhaustion and missing
context remain visible. No logging, allocation or policy/syscall sampling occurs
in the Rust recorder. JSON formatting runs on the property reader. Additional
native timestamp/count bookkeeping and Rust work are observer overhead, not free.

## ftuP version 1 layout

Read with `build/hal-check --pumps`. Snapshot size: **11,448 bytes**.
All numbers are unsigned 64-bit words. Compact JSON arrays have the same fixed
order as the shared C header and Rust snapshot.

- Header keys: version, valid, epoch, timebase_numer, timebase_denom, steady_after,
  calls, unavailable, invalid, dropped, overflow, id_discontinuities, link_misses.
- Phase order: startup, steady, drain.
- Per-phase counts: calls, valid, unavailable, invalid, finished, stopped,
  timedout, handled, callbacks, empty, multiple, late_first_callback.
  The last count uses first callback minus outer entry >=2.1 ms.
- Metric order: whole duration, empty timed-out duration, empty timed-out
  overshoot above 2 ms (clamped to zero), entry to first callback,
  last callback to return, between consecutive same-phase pumps.
- Histogram: count, minimum ns, maximum ns, 18 buckets. Exclusive upper limits:
  1/2/4/8/16/32/64/128/256/512 microseconds,
  1/2/4/8/16/32/64 ms, then unbounded. Empty extrema are zero storage with count=0,
  interpreted as unavailable, not a measured zero.
- Record (13 words): ordinal, phase, captured cursor, native ID, enter ticks,
  exit ticks, CF result, latest sampler begin/end ticks, callbacks,
  first/last callback ticks, invalid callbacks. An all-zero record is absent.
- `worst[phase][metric]`: current record followed by its actual preceding record.
  The preceding record may be from another phase. A gap metric never crosses
  phases. It includes local worker work, sampler and observer work.
- `recent`: 32 records in ring storage order; sort by ordinal to read chronology.
- `links`: four banks: worst IN delivery, worst OUT delivery, minimum IN lead,
  minimum OUT lead. Each is 30 words: present, sequence, ticket, comparison value,
  current record, previous record. Delivery compares exact tick deltas, matching
  ftuD before nanosecond rounding; lead is signed milliseconds stored as uint64.
  Missing/expired context increments link_misses. Pinned pairs survive ring
  overwrite. Sampler timestamps allow same-event overlap checks.

The analyzer validates shapes, totals, extrema, exact current/previous IDs,
phase coverage, rolling retention and ftuD event identity. It can partition each
worst delivery interval without combining unrelated extrema:

```sh
python3 tools/hal-loopback/pumps_analysis.py /path/to/pumps.log \
  --expected-build dev-... --delivery-log /path/to/delivery.log
```

## Offline validation scope

Rust tests cover timeout overshoot, delayed/multiple callbacks, phase separation,
missing observations, ID exhaustion/discontinuities, conversion/counter overflow,
bounded concurrent readers, pinned context across ring overwrite and epoch reset.
The simulated transport verifies every normal/drain pump is observed with zero
loop allocations. Native sanitizer tests exercise real CFRunLoop dispatch,
zero-callback timeout, two callbacks, rejected duplicates and exhausted IDs.
HAL sanitizer tests check read-only property shape, ABI and empty/populated JSON.
Python tests reject corrupt, mixed-epoch and unrelated-event evidence.
A release recorder microbenchmark is recorded with the candidate; it excludes
native bookkeeping, concurrent property reads and scheduling interference.
The final offline checks passed: 129 Rust tests, 120 standalone Python tests,
44 loopback/analyzer tests, Clippy with warnings denied, HAL and native ASan/UBSan.
The all-pump plus link recorder benchmark reported batch means of 98.25 ns median,
112.79 ns p99 and 114.88 ns maximum; these are not individual-call bounds and
exclude native bookkeeping/property readers. Qualification of this original candidate did not proceed past startup; the later
measurement and its failed latency gate are recorded above.
