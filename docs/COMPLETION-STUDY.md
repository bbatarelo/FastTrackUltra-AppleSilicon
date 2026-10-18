# Completion delivery and shorter USB transfers

Offline investigation, 18 October 2026, following checkpoint `75b1a75`
(`Qualify longer duplex sessions and GarageBand transitions`).

**Keep the qualified driver and its margins.** The retained events point to
completion delivery as the next boundary to measure. Four-millisecond transfers
may improve scheduling margin or capture publication cadence, but do not shorten
round trip with the current sample coordinates and safety offsets. No new driver
candidate is selected by this study.

This work changes offline examples, analysis and documentation only. It performs
no hardware access, installation, CoreAudio restart or audio test. The qualified
configuration remains `dev-8387395854108015`: 48 kHz, four 8 ms slots per
direction, input safety 1024 and output safety 2560. Its measured 64-frame-buffer
round trip remains **78.539 ms**. See [long-session qualification](LONG-SESSION.md).

## What the retained events establish

The two ten-minute logs contain epoch totals of **300,064 observations** but
retain only **36 unique detailed records**: the last 16 plus the worst IN/OUT
events from each epoch. Worst means minimum steady submission lead, not maximum
duration of every stage. These biased selections cannot supply percentiles or a
worst-case scheduling bound.

The [linked-stage analyzer](../tools/timing-model/completion_evidence.py) validates
build, epoch, recorder integrity and clean final stop before comparing records.
It measures callback-to-return directly between timestamps, including unlabelled
gaps, rather than adding possibly overlapping stages.

| Worst OUT event | Actual 64-frame run | Actual 512-frame run |
|---|---:|---:|
| Epoch / ordinal | 20 / 142948 | 21 / 93704 |
| Post-submit lead | 18 ms | 17 ms |
| Predecessor's last packet stamp → callback | 6.465 ms | 7.351 ms |
| That callback → post-submit clock observation | 0.305 ms | 0.360 ms |
| PCM preparation, within that local interval | 0.109 ms | 0.132 ms |
| Native submission, within that local interval | 0.102 ms | 0.115 ms |

Thus the larger elapsed interval occurs before callback delivery in these two
events. This does **not** locate the delay inside the USB stack, run loop or
thread scheduler. The most recent run-loop call at these OUT admissions lasted
only 0.013/0.025 ms; the adjacent IN records retained 13.409/11.148 ms calls.
The OUT predecessor callback can belong to an earlier pump invocation. Joining
a callback to the latest pump duration would misattribute the delay.

Across the 36 records, the largest linked local intervals were 0.351 ms for IN
and 0.603333 ms for OUT. They are elapsed intervals, not CPU execution time.
IN ends at submission return; OUT includes the subsequent bus-clock observation.
The synthetic model rounds these upward to 375/625 microseconds. Its 8 ms
delivery delay likewise rounds the selected events upward; it is **not an
empirically established upper bound**.

## Queue arithmetic before simulation

With block duration `B` and depth `D`, nominal slot-recycle lead is
`(D - 1) * B`. For a fixed completion delay, serialized local work and no
additional stall, that lead minus delay/work largely predicts the result.
The event model adds phase, feedback capacity, conservation and failure checks;
the simple envelope numbers are not a new discovery from simulation.

Holding the measured worst event's delay unchanged gives this limited arithmetic
screen:

| Block / depth | Nominal recycle lead | Shifted 17 ms observation |
|---|---:|---:|
| 8 ms / 4 | 24 ms | 17 ms |
| 8 ms / 3 | 16 ms | 9 ms |
| 4 ms / 4 | 12 ms | 5 ms |
| 4 ms / 6 | 20 ms | 13 ms |
| 4 ms / 7 | 24 ms | 17 ms |
| 4 ms / 8 | 28 ms | 21 ms |

Changing geometry also changes callback frequency, cost and feedback behavior,
which this arithmetic omits. It does not qualify any profile or replace the
earlier three-slot hardware failure.

## Corrected transport study

Run the independent transport example with `--completion-study`; reproduction
commands are in the [tool guide](../tools/timing-model/README.md). It covers
15 geometry/history profiles, six scenarios and **5,280 phase/scenario runs**.
Each models about 2.2 seconds on a 125-microsecond grid, with a synthetic 5/6/7
packet-count pattern, 64 ms replay and a one-second warm-up.

The following rows all use 32 feedback entries and an 8 ms delay on **every**
completion, plus 375/625 microseconds of serialized work per IN/OUT submission.
The 16 ms OUT comparison screen is kept separate from merely completing transport.

| Block / depth | Transport completed | Minimum OUT lead | Passed 16 ms screen |
|---|---:|---:|---:|
| 8 ms / 4 | 128/128 | 15 ms | 0/128 |
| 8 ms / 3 | 128/128 | 7 ms | 0/128 |
| 4 ms / 4 | 64/64 | 3 ms | 0/64 |
| 4 ms / 6 | 64/64 | 11 ms | 0/64 |
| 4 ms / 7 | 64/64 | 15 ms | 0/64 |
| 4 ms / 8 | 64/64 | 19 ms | 64/64 |

The qualified geometry's synthetic 15 ms result is lower than its measured
17 ms minimum. The rounded, every-completion disturbance is deliberately
different from the measured run. **This is neither a replay nor a retroactive
hardware rejection**, and passing the model would not qualify a new driver.

For 4 ms / 8 with 32 feedback entries:

| Added disturbance | Transport completed | Minimum OUT bus-frame lead | Passed screen |
|---|---:|---:|---:|
| None | 64/64 | 19 ms | 64/64 |
| One 4 ms owner stall | 64/64 | 15 ms | 48/64 |
| One 8 ms owner stall | 64/64 | 11 ms | 0/64 |
| Local work increased to 1 ms per direction | 64/64 | 18 ms | 64/64 |
| 32 ms owner stall plus 1 ms local work per direction | 0/64 | Not a successful-run result | 0/64 |

Eight feedback entries overflow in every simple-envelope phase; 12, 16, 24 and
32 complete that scenario. This is a capacity sensitivity check, not permission
to reduce production history. Deeper queues cannot rescue unbounded stalls.

Callback eligibility and owner availability are separate absolute times. Their
overlapping intervals combine through availability, not by summing the same
wall-clock interval twice. An owner stall interrupts in-progress work in this
new mode. No statistical independence, correlation law, run-loop emulation or
CPU budget is inferred. An additive sequence of separate delays would be another
scenario, not an established interpretation of the retained events.

The report includes exact model lead at 125-microsecond resolution alongside
the integer USB-frame lead used by the driver. With frame-aligned start times,
the latter can exceed exact remaining model time by less than 1 ms. For example,
the 4 ms stall row has a 15 ms bus lead but 14.625 ms exact minimum. Keep the
existing bus-frame gate; do not dismiss or relax it as rounding noise.

Extrema in failed rows may describe progress before failure. Only complete runs
can pass the screen. The output/capture frame horizon, maximum OUT lead, capture
age and feedback occupancy are also retained. The model's slot-scan
`reordered` flag describes consumption order, not observed callback ordering;
the new study does not use it as evidence of OS delivery order.

Claude identified a missed-stall case when simulated work jumped across a stall
boundary. The new mode now preempts that work and has boundary regressions.
The no-argument historical mode deliberately retains its old boundary-only
stall convention and byte-identical output for reproducibility. **Use the new
mode for this investigation; historical stall results do not model preemption
inside submission work.** Initial and corrected outputs are preserved separately.

## Sample placement: smaller batches alone do not lower RTL

The separate `placement_model --completion-study` uses five profiles, actual
buffer choices 64/512/4096, seven disturbances and eight selected phases:
**840 phase/scenario runs**. It retains HAL publication, ring tags, USB service,
capture publication and marker return. This is a nominal six-sample-per-packet
model with a chosen 58-frame analogue fixture and anchor bias; it is not converter
calibration or a replay of real clock drift.

All profiles pass the six bounded scenarios, 720 runs total. All 120 deliberately
over-budget runs fail, including capture loss. These are sample-coverage results,
not passes of the separate 16 ms transport screen: some successful coverage rows
still have insufficient transport margin.

At unchanged input/output offsets, all clean 8 ms / 4 and 4 ms / 6, 7, 8 profiles
return the same marker distance:

```
raw_RTL_frames = 2 * buffer + 1024 + 2560 + 58
```

The additional 4 ms / 8 profile with output safety 3072 increases that model
distance by 512 frames. Four 8 ms and eight 4 ms slots both cover 32 ms of
transport; finer recycling can raise nominal lead and shorten publication
batches without itself changing the HAL reference distance. Another latency
reduction requires a justified offset or placement change.

Neither independent model exercises the production pool, modern macOS dispatch,
device drift, lifecycle races or all host-publication phases.

## Native boundary audit

The current native adapter submits 64 high-speed packet entries with
`updateFrequency=0`. It exposes completion and buffer access only after the
callback. The owner pumps `CFRunLoopRunInMode(..., 0.002, true)`; the requested
2 ms wait is not an observed upper bound on elapsed call time.

Apple documents frame-list updates before callback delivery, with zero update
frequency updating at transfer end and values 1–8 selecting a millisecond update
cadence. Callback completion still covers the whole transfer. The rationale
explicitly describes variable completion-delivery delay. See Apple's
[LowLatencyReadIsochPipeAsync API](https://developer.apple.com/documentation/iokit/iousbinterfaceinterface192/1559722-lowlatencyreadisochpipeasync)
and [source header](https://github.com/apple-oss-distributions/IOUSBFamily/blob/main/IOUSBFamily/Headers/IOUSBLib.h).
The inspected local Xcode 27 SDK header, lines 3081–3216, is hashed in the evidence.

This suggests measuring frame-list visibility separately from callback ownership.
It does not establish a complete modern ARM64 publication, reuse and abort
protocol. A user-side acquire fence, volatile read or two matching reads cannot
by itself prove coherent publication or safe lifetime. Keep pending storage
immutable and callback-owned until its matching completion; do not introduce a
polling thread or early PCM access without resolving that contract.

A coherent 4 ms prototype would touch more than one constant:

- Native packet-count ABI, frame-list/payload allocation, completion copying and
  slot capacity: 32 packet entries instead of 64 and enough slots for the profile.
- Rust scheduling arithmetic, pool arrays, replay/history indexing, packet/count
  validation, capture publication and maximum PCM scratch size.
- Worker timing schema and analyzers whose existing thresholds assume 8 ms.
- Worker policy, effective read-back and measured CPU cost. The present request
  is period 8 ms, computation 1 ms, constraint 2 ms. Applying it successfully does
  not prove a CPU-time bound. More frequent work needs measurement before any
  choice of a new policy period or budget.

## Decision and next experiment

First prepare observation of the existing geometry, before selecting a latency
candidate:

1. Attach a pump invocation ID, entry/exit ticks and return code to the callbacks
   delivered in that invocation. Preserve generation, direction and epoch so
   a later slot admission can refer to the correct callback and pump.
2. Collect bounded, allocation-free histograms and threshold counts over every
   steady event, with invalid/overflow/drop counts and retained worst exemplars.
   Separate packet-stamp → callback, callback → consumption, preparation,
   submission and post-submit observation. Do not sum overlapping intervals.
3. Measure worker CPU time and effective scheduling policy outside realtime
   callbacks; account for sampler overhead. Pump membership and elapsed duration
   alone still cannot distinguish kernel dispatch from owner descheduling.
4. Resolve the native frame-list publication/lifetime contract before designing
   any visibility probe. A separate observer also needs slot pinning, teardown
   synchronization and an overhead comparison; generation checks alone do not
   make plain shared reads safe.
5. Only then choose a coherent shorter-transfer experiment or publication
   change, with production-pool/drift/lifecycle tests. Keep the current margins
   for the first observation candidate. Predeclare physical loopback, steady
   loss, ownership, teardown and the unchanged 16 ms lead checks before install.

Claude reviewed the offline model, selected native source and sample-free timing
summary. Accepted findings include work/stall preemption, missing horizon output
and clearer model scope. The proposal to poll shared frame lists using only
volatile/double reads was not adopted. Neither the elapsed work allowance nor
the review justifies an automatic scheduling-policy change.

These are also useful questions for a later [E-MU/ASFW comparison](ASFW-REUSE.md):
match measurements to ownership events, distinguish data visibility from lifetime,
and test sample placement separately from transport margin. No E-MU source or
ASFW implementation was changed.

## Reproduction and retained evidence

Local, ignored evidence: `build/validation/completion-study-20261018/`.
It contains the pre-change examples/output, linked log hashes, initial and
corrected model results, source/SDK hashes, checks, Claude request/review and
dispositions. Source logs remain in
`build/trials/2026-10-18-long-session/{01-soak-64,02-soak-512}/worker.log`.
No recording is copied into this study or sent for review.

Validation: seven transport-example tests, seven placement-example tests and
five linked-evidence tests pass; both Rust examples pass Clippy with warnings
denied. Legacy no-argument model output remains byte-identical to the preserved
pre-change output. The model sweep retains expected failures rather than treating
a high pass percentage as hardware qualification.
