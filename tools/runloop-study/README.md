# Standalone run-loop delivery study

This macOS-only diagnostic sends pairs of small Mach messages to two ports on a
dedicated consumer thread. It uses the driver's current
`CFRunLoopRunInMode(default, 0.002, true)` call, with either normal scheduling or
the same requested 8/1/2 ms time-constraint policy. It does not open USB, load a
driver, start audio, alter CoreAudio or require an administrator password.

Build and run from the repository root:

```sh
make -C tools/runloop-study
make -C tools/runloop-study test
mkdir -p build/runloop-results
build/runloop-study normal plain 250 > build/runloop-results/normal.json
build/runloop-study realtime observer 250 > build/runloop-results/realtime.json
python3 -B tools/runloop-study/analyze.py build/runloop-results/normal.json
python3 -B tools/runloop-study/analyze.py build/runloop-results/realtime.json
```

Arguments are `normal|realtime`, `plain|observer`, and 1–2500 message pairs.
Use `idle:20` instead of the pair count for a 20-second no-producer control
(allowed idle duration: 1–30 seconds). The sources stay attached but no messages
are sent; this measures the timeout path independently of sender scheduling.
Each pair is scheduled 8 ms apart, starting after 100 ms. `observer` records
before/after-wait timestamps; `plain` omits those callbacks. Compare all four
combinations across independent runs with varied order. Each fresh process
creates its own run loop, ports and threads. Policy changes affect only these
test threads; the consumer restores timesharing before cleanup and exits.

The producer explicitly requests timesharing. Its planned deadline, send entry
and send return are retained separately. A callback can occur before the send
returns, so enqueue-to-callback delay is bounded by
`max(0, callback - send_return)` and `callback - send_entry`. Producer lateness
is separate and must not be called consumer delivery delay. Both messages in a
pair have the same planned deadline, but separate actual send times.

All records use fixed, pre-touched arrays; the callbacks only timestamp and
record metadata. Formatting/output happens after threads stop. CoreFoundation
may allocate internally. Message send timeout, missed/duplicate messages,
ordering failures, unexpected pump results, incomplete waits, sample errors and
policy read-back mismatch fail analysis. The test is bounded by pair count,
elapsed deadline and pump capacity. A timeout is not permission to reuse pending
USB storage; there is no USB storage here.

The JSON preserves every send, callback and pump, including pumps with no
callback. Columns:

- `sent`: sequence, endpoint (0/1), planned ticks, send entry, send return, status.
- `received`: sequence, endpoint, callback ticks, zero-based pump index.
- `pumps`: index, entry, exit, return code, last before-wait, last after-wait,
  completed wait count, total observed wait ticks, callback count.

The analyzer uses nearest-rank empirical percentiles, not the driver's histogram
buckets. Wait brackets include waiting and wakeup/descheduling time; they are
not kernel sleep-time measurements. CPU is the consumer thread average over the
sample interval. Initial/final policy values cannot exclude transient changes.
Comparing observer/plain runs does not isolate observer cost from system noise.

This is a diagnostic control, **not a USB-stack emulator or a driver latency
qualification**. It omits USB interrupt delivery, IOKit wrappers, PCM work,
CoreAudio clients and device drift. The producer also has its own scheduler.
A clean run cannot exonerate any of those boundaries or prove an OS bound.
Timing results from sanitizer builds must be kept separate.

Analysis revision 2 also reports pump phase (before producer, crossing its start,
active or after final send; idle mode separately), timeout overshoot, worst-pump
context and the prior pump for the worst delivery. `handled_after_2_1_ms_count`
means callback entry occurred more than 2.1 ms after the outer pump entry. It is
a descriptive threshold, not an OS deadline or evidence of timeout-worker failure.
Per-endpoint producer lateness and endpoint-1 send after endpoint-0 return keep
pair ordering explicit. Older version-1 records without idle metadata remain
readable. No recordings are involved.
