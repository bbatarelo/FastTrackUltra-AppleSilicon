# Run-loop control experiment and next measurement

Offline follow-up, 19 October 2026, to the qualified
[completion-delivery observations](DELIVERY-OBSERVATION.md).

**Keep the installed driver and all margins unchanged.** The standalone control
confirms that total run-loop duration and message-delivery delay must be measured
separately. It does not identify the cause of the FTU's 8.650 ms completion delay.
The next proposed driver experiment records every pump; wait observers are an
optional later condition. Neither is implemented or installed by this study.

## Why this control

The real FTU's worst 128-frame example associated an IN callback with a 13.277 ms
pump, followed by a 0.021 ms OUT pump. Both completions carried the same last-packet
timestamp, but the OUT callback arrived 8.650 ms after it. Most of that interval
preceded its own pump. Existing telemetry retains completion-associated pumps,
not every timeout or no-callback pump.

The native adapter attaches both interface sources to the worker's default run
loop and calls `CFRunLoopRunInMode(default, 0.002, true)`. Apple documents that
the final argument returns after a source is handled; a zero timeout performs
one pass, not an exhaustive drain. See
[Apple's API reference](https://developer.apple.com/documentation/corefoundation/cfrunloopruninmode(_:_:_:)).
A requested timeout is not a measured upper bound on wall-clock return time.

For historical context, Apple's published
[CFRunLoop implementation](https://github.com/apple-oss-distributions/CF/blob/main/CFRunLoop.c)
contains a dispatch timer for finite activations and checks handled-source
termination before the timeout condition. It can wait on Mach ports while a
separate timer path arranges a wakeup. This is 2015-era source, **not a verified
implementation of this Mac's CoreFoundation**. It motivates measuring the wait;
it does not prove that a particular queue, timer or scheduler caused our event.
No Apple or ASFW implementation code was copied into the diagnostic.

## Reproducible standalone tool

The [run-loop study tool](../tools/runloop-study/README.md) creates two Mach
ports on a dedicated consumer thread. A producer sends a pair of small messages
every 8 ms, after a 100 ms initial interval. The consumer uses the driver's
requested 2 ms run-loop call, with normal scheduling or its 8/1/2 ms time-constraint
request. The producer explicitly requests timesharing.

Each profile runs with wait observers disabled or enabled. Observers retain
before/after-wait timestamps, completed wait counts and total elapsed wait
brackets. All send/callback/pump records use fixed pre-touched arrays. Formatting
and output happen after the threads stop; CoreFoundation's own internal
allocation is outside the tool's control.

Send entry and return bracket unknown kernel enqueue time. The delivery lower
bound is `max(0, callback - send_return)`; its upper bound is
`callback - send_entry`. A callback may precede send return. Producer
deadline lateness is retained separately, not attributed to consumer delivery.

The experiment accessed no USB device, audio input/output, HAL bundle or
CoreAudio process. Only the diagnostic's own threads had scheduling requests.
The consumer restores timesharing before cleanup. Native-boundary code is C;
analysis and validation are standalone Python. Production driver inputs and its
Rust/native architecture are unchanged.

## Results

The predeclared sequence used seed 20261019 to shuffle the four profiles within
each of three repetitions. Each fresh process sent 500 pairs, about four seconds
of traffic. No external load was deliberately imposed; normal desktop activity
was uncontrolled.

All 12 runs passed measurement-integrity checks: **12,000 messages and 24,850
pump calls**. No message loss/duplicates, invalid ordering, send errors,
observer-pairing errors or policy read-back mismatch were found. Nine analyzer
tests passed, as did separate AddressSanitizer/UndefinedBehaviorSanitizer smoke
runs under normal and requested realtime scheduling. Sanitizer timing is excluded
from the comparisons.

The following ranges span the three runs of each profile. Delivery values use
the send-entry upper bound; percentiles are nearest-rank empirical values.

| Consumer / observer | Per-run delivery p99 | Per-run maximum delivery | Per-run maximum whole pump |
|---|---:|---:|---:|
| Normal / off | 0.241–0.423 ms | 0.985–1.499 ms | 6.170–17.872 ms |
| Normal / on | 0.634–0.745 ms | 1.870–4.862 ms | 7.986–23.244 ms |
| Requested realtime / off | 0.046–0.057 ms | 0.069–0.336 ms | 7.987–8.236 ms |
| Requested realtime / on | 0.046–0.057 ms | 0.082–0.268 ms | 7.994–8.861 ms |

One realtime/observer call lasted 8.861 ms, of which 8.845 ms lay within recorded
wait brackets. Another returned `timedOut` after 7.994 ms with no message
callback. Long whole pumps therefore occur independently of slow delivery after
a message is sent. The largest normal/observer call, 23.244 ms, was also a
timeout and occurred in the initial interval before message production.

A separately labelled, post-hoc sensitivity check excludes callbacks before one
second and pumps starting before one second. It retains 774 callbacks per run.
The largest remaining whole pump is 17.872 ms for normal scheduling and 8.571 ms
for requested realtime. The realtime delivery maxima remain 0.069–0.336 ms.
This check does not replace the full-run results or change an acceptance gate.

Whole-consumer CPU estimates were 1.018–1.186%. This lightweight workload is very
different from the actual USB/PCM worker. Observer/on versus off differences do
not isolate observer overhead from scheduling noise. Initial/final policy
read-back cannot establish uninterrupted realtime scheduling.

The producer itself was late by as much as 14.393 ms in one run. This is a reason
to retain actual send timestamps: planned generation time would overstate
consumer delivery delay. These short runs neither establish scheduling bounds
nor qualify a new driver policy.

## Review and timeout-only follow-up

Claude reviewed the three diagnostic source files and sample-free timing summary.
The accepted changes use the same integer truncation as the native policy
request, check the normal policy's default flag, and report pump phase, exact
worst-pump context, timeout overshoot, late handling, per-endpoint producer
lateness and the previous pump. All 12 original raw runs pass the revised
analyzer. The original source and analyses remain preserved.

The analyzer now has 16 regression tests, including non-divisible Mach timebases,
positive realtime read-back, multiple wait brackets, timeout/callback mismatch,
callback accounting and the idle mode. Updated native sanitizer checks cover
both policies in message and idle modes.

The review motivated two **newly predeclared 20-second runs without a producer**,
with observers off. All pumps returned timed-out and all integrity checks passed:

| Consumer policy | Pump count | Duration p50 / p99 | Maximum duration | Maximum overshoot above 2 ms |
|---|---:|---:|---:|---:|
| Requested realtime 8/1/2 ms | 6,718 | 3.044 / 6.026 ms | 9.478 ms | 7.478 ms |
| Normal | 6,711 | 3.043 / 5.998 ms | 13.409 ms | 11.409 ms |

This directly shows that the requested 2 ms return interval can be exceeded on
the current Mac without USB or message production. It does **not** verify the
historical dispatch-timer mechanism or explain the real USB delay. A late timeout
alone does not establish that a ready USB message was prevented from waking the
owner. Direct Mach-receive or event-correlated tracing controls remain possible
follow-ups, not driver replacements selected here.

In the original message runs, 170–217 callbacks per run began more than 2.1 ms
after their enclosing pump's outer entry timestamp. The 0.1 ms allowance is a
descriptive screen, not an acceptance gate or exact measurement of CoreFoundation's
internal deadline. These are late handling observations, not proof that messages
rescued a failed timeout wakeup.

The review also suggested that the worst normal-policy delivery delays occurred
between pump invocations. Correlation **does not support that location**:
those inter-pump gaps were approximately 0.04–0.21 microseconds. In the largest
4.862 ms example, 4.821 ms elapsed before the current pump, predominantly while
the preceding pump was still active; that prior pump delivered endpoint 0 only
about 3.458 microseconds before returning. This still cannot distinguish a native
wait from descheduling. Three earlier worst events cluster near 1.33 seconds
after process start; no cause is assigned to that clustering.

## What remains unknown

The control omits USB interrupt processing, IOKit wrappers, device clocks, PCM
preparation, CoreAudio clients and the daemon's scheduling environment. Short
synthetic delivery under the requested policy cannot exonerate any of those
boundaries. The current driver already requests that policy; this result does
not select a scheduling change.

Before/after-wait brackets include both blocking and wakeup/descheduling time.
They do not reveal kernel message-ready time. Neither a short pump nor a long
wait proves where a USB completion was delayed. The separate startup enumeration
stall also remains unresolved.

## Proposed next observation-only candidate

Start with **all-pump accounting**, keeping wait observers optional:

1. Have the native boundary return each pump's begin/end ticks, result and
   callback count to a fixed Rust recorder, including pumps without a completion.
   Retain current actual callback/pump association and the previous pump context.
   Separate startup, steady operation and drain; record gaps between invocations,
   invalid/missing records and overflow.
2. Summarize timed-out pumps by duration/overshoot, and handled pumps by entry
   to first callback and callback to return. Count late handling with the exact
   threshold recorded. Keep representative worst records with epoch/pump IDs;
   do not join unrelated extrema or infer message-ready time.
3. Preserve the current diagnostic contract and qualified binary. Add an
   explicit version or additive property with bounded coherent reads. Test
   no-callback/timeouts, multiple callbacks, late consumption, drain, teardown,
   ID exhaustion and unavailable/overflowed observations.
4. If these records leave an important interval unresolved, add before/after-wait
   observations as a separate optional condition. Keep callback timestamps,
   summed brackets, wait count and the last matched pair. No allocations,
   logging, formatting or Mach policy/CPU queries belong in observer callbacks.
   Compare observer-on/off impact; do not assume the callouts are free.
5. Keep 48 kHz, four 8 ms slots, 64 ms count replay, 3072-frame PCM offset,
   safety 1024/2560, scheduling 8/1/2 ms, the requested 2 ms pump wait and
   `updateFrequency=0`. Preserve callback-owned USB storage; observation grants
   no access to pending frame lists or payloads.
6. Compare frozen baseline and candidate identities, silence preflight, repeated
   actual buffers, long duplex runs and independent listening against the
   existing **16 ms OUT lead gate**. Retain failures and observer costs. If the
   diagnostic materially changes behavior, revise the measurement rather than
   lower the gate.

Kernel-ready versus thread-runnable attribution would require a separate tracing
plan. Observer timestamps cannot supply either event. Continuous polling, shorter
timeouts, new transfer geometry and a second USB owner are not selected here.

Local ignored evidence is under `build/validation/runloop-study-20261019/`:
predeclared plan, source hashes, randomized run order, environment, raw records,
per-run analyses, sanitizer logs and the subsequent review. The diagnostic is
reproducible from its tracked source; these measurements remain specific to this
Mac and run.
