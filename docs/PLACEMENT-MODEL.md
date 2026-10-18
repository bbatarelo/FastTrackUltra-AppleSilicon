# Sample placement and the output-safety reduction

The preceding qualified reference is `dev-517459af36927152`: four 8 ms USB
slots per direction, input safety 1024, output safety 3072, native 48 kHz.
The now-qualified `dev-8387395854108015` changes **only output safety to 2560**. USB scheduling,
packet counts, worker policy, clock mapping and input safety retain the reference
behavior. Offline checks and short desktop-Ultra hardware qualification passed.

The pre-test prediction was **512 frames / 48 = 10.667 ms**, taking the observed
64-frame-buffer round trip from 89.206–89.227 ms to **78.539–78.560 ms**.
This remains high for live monitoring. The earlier three-slot experiment reached
the same latency at the same output safety, but failed its USB headroom gate.
It remains rejected; this experiment retains four slots.

## Hardware result, 18 October 2026

All thirteen attempts passed: one silence preflight and two starts at each
64/128/256/512/1024/4096-frame buffer. All 24 chirps matched the predeclared
latency range. Raw round trip was **78.539 ms at 64**, **97.206–97.227 ms at 512**
and **246.539 ms at 4096**. All 29,438 callbacks matched the expected sample-index
distance. Steady input/output misses were zero; every stop was clean and restored
512 frames. The two-client checks at 64 and 4096 and owner-confirmed five-pair
listening check at 512 also passed.

Across those checks, 39,687 worker observations had no drops or attention events.
Minimum OUT submission lead was 22 ms against the predeclared 16 ms gate; the
minimum additional availability screen was 150 frames. This confirms the
one-constant latency prediction on this Mac without reducing queue depth.
It does not establish long-session stability or an OS scheduling bound.

The first read-only enumeration after installation timed out before audio began.
A later read completed and CoreAudio CPU settled before the sweep. The client
stack waited in CoreAudio system-object initialization; root cause remains
unresolved. Original diagnostics and all hardware attempts are preserved.
See [current status](STATUS.md) for exact scope and the final checkpoint.

## Coordinates and where the time goes

Let `I(k)` be the cumulative frames before input block k, and `c(k)` its actual
captured count. The production pool replays those same counts in OUT block k.
Its USB start is IN's scheduled start plus 64 ms. `AudioIo::output` reads PCM
at `I(k) + PLAYBACK_OFFSET`, where the offset is 3072 frames (64 ms at 48 kHz).
Thus the count replay and PCM placement shift together. Reducing both replay
and offset would not by itself reduce application round trip.

This is a nominal shared sample coordinate, not proof that the USB bus and device
crystal agree exactly. Packet-count phase and clock error over the replay interval
can leave a small residual. The measured restart spread is about one sample.

In all 29,438 recorded callbacks of the qualified sweep, the output sample index
minus the input sample index was exactly:

```
2 * actual_buffer + input_safety + output_safety
```

At a 64-frame buffer the measured round trip decomposes as follows:

| Term | Frames | Time |
|---|---:|---:|
| Input safety | 1024 | 21.333 ms |
| Output safety | 3072 | 64.000 ms |
| Two client-buffer spans | 128 | 2.667 ms |
| Timestamp-relative remainder | measured separately | about 1.206–1.227 ms |

The remainder is not calibrated ADC/DAC latency. The 16384-frame zero-timestamp
publication period is not another buffering delay. In particular, do not add
the 64 ms packet-count replay to this table a second time.

The whole path is HAL publication into a sample-tagged playback ring → packed
PCM copied before USB submission → immutable submitted native payload → scheduled
USB service → physical return → input batch completion and ordered publication →
HAL input read. Each boundary has a different deadline and ownership rule.

## Choosing one change

Four 8 ms slots normally provide about 24 ms of submission lead. Reducing the
queue lets PCM be prepared later but also reduces that USB reserve. With the
sample coordinates held consistent, queue depth alone does not shorten physical
round trip; a lower safety offset is what moved the measured latency previously.

The candidate was selected using this reference-build availability screen:

```
M = min(minimum write/preparation lead, minimum output high-water margin)
remaining = M - 512 - actual_buffer - 832
```

`actual_buffer + 832` is a chosen **additional** disturbance reserve, not an API
requirement. The metrics already refer to the requested block end; 832 is an
extra maximum-size block. A positive high-water mark does not prove contiguous
coverage, and independent metric minima do not identify one event.

The remaining screen is at least 145 frames over the thirteen physical runs,
130 after also including the two-client and listening tests. A 1024-frame cut
would leave −382 frames over that combined evidence, so it is not selected.
Actual missing-sample counters remain the decisive hardware loss check.

The reproducible evidence calculation is:

```bash
python3 tools/timing-model/placement_evidence.py
python3 tools/timing-model/placement_evidence.py --reduction 1024
```

It reads the preserved sample-index CSVs and metric summaries, checks physical
CSV hashes, and reports a counterfactual margin shift. It does not read recorded
audio, start a device or claim to replay callback execution. Those raw evidence
files are local, ignored artifacts from this development Mac.

## Independent model

[placement_model.rs](../rust/ftu-stream/examples/placement_model.rs) uses one
sample frame per simulation tick. Hardware and HAL continue while the USB owner
is paused. It models input/output slot ownership, matching feedback, ordered
capture publication, tagged rings, copied output payloads, per-block physical
service, delayed HAL publication and the return of two distinct sample markers.
Submission checks use floored USB milliseconds, as the native observation does.

The model has an explicit nominal six frames per microframe, fixed 48 kHz,
58-frame analogue fixture and a 48-frame common anchor-bias fixture. These are
assumptions, not converter or clock calibration. The bias tests also exercise
−48 and zero. Actual 5/6/7-frame counts and protocol maxima are covered separately
by production tests; this independent model is not the production `PoolSession`,
macOS HAL, a drift model, an OS scheduler or an event replay of the recordings.

It evaluates five profiles, six client sizes and six scenarios at eight selected
boundary phases: 1,440 runs. Phase selection samples the relative HAL/block
alignment, rather than exhaustively covering all possible timing histories.
Each ordinary scenario below has 48 runs per profile (six buffers × eight phases).

| Profile | Clean | 6 ms callback pause | 8 ms owner / 2 ms callback | HAL publication paused for B+832 | 12.5 ms owner pause |
|---|---:|---:|---:|---:|---:|
| 8 ms × 4, output 3072 | 48/48 | 48/48 | 48/48 | 48/48 | 48/48 |
| **8 ms × 4, output 2560** | **48/48** | **48/48** | **48/48** | **48/48** | **48/48** |
| 8 ms × 4, output 2048 | 48/48 | 48/48 | 48/48 | 4/48 | 48/48 |
| 4 ms × 6, output 2560 | 48/48 | 48/48 | 48/48 | 48/48 | 48/48 |
| 4 ms × 4, output 2048 | 48/48 | 48/48 | 48/48 | 48/48 | 0/48 |

“Pass” here means the chosen scenario completed with no modelled steady input or
output misses and both markers returned. It is not hardware qualification or the
candidate's separate unloaded 16 ms headroom gate. Both four-slot 8 ms profiles
retain 11 ms minimum lead under the 12.5 ms pause; the 4 ms / six-slot profile
retains 6 ms. All profiles fail the separate 32 ms beyond-budget owner pause.
The 12.5 ms pause is a synthetic challenge; a measured 12.418 ms run-loop interval
includes waiting and cannot be equated to that much OS descheduling.

The former transport-only model used different costs and disturbance patterns;
its failures are not superseded by this model. A 4 ms candidate still needs native
packet-array/ABI work, more frequent callbacks, capacity/policy review and joint
validation. It gives less submission lead at six slots than today's four-slot
8 ms geometry, so the one-constant candidate was selected first.

```bash
cargo test --release --locked --offline -p ftu-stream --example placement_model
cargo run --release --locked --offline -p ftu-stream --example placement_model > build/placement-model.json
```

Negative controls remove a middle publication despite future high-water, choose
an incorrect PCM offset (which adds the replay delay), reduce input safety until
capture misses occur, overrun the ring tags, and exceed the transport budget.
A common anchor bias changes availability margins while leaving round trip
unchanged. This exposes an important blind spot of round-trip checks alone.

## Production checks and independent review

New tests exercise actual `ftu_live_write` → `AudioIo::output` → staged capture →
`AudioIo::ordered` → `ftu_live_read`, across all six client sizes, ring wrap,
5/6/7-frame packets and maximum packet sizes. They include negative and near-full-
scale packed samples and a missing frame despite later high-water publication.
A fresh packing operation sees a later repair; missing counts remain cumulative.
These tests check conversion/index handling, not native DMA ownership or measured
physical latency. Existing pool/native CRC and callback-generation regressions
cover in-flight ownership. Separate assertions pin replay, PCM offset and block
schedule relationships.

Claude reviewed seven FTU source files plus the measurement summary, then the
new model and production tests. It independently recommended the 2560 candidate
and found no blocking model error in the second review. We removed vacuous
immutability assertions, strengthened bias/negative-control checks and added
signed/full-scale PCM fixtures following that review. Original reviews and test
failures are preserved under `build/validation/placement-model-20261018/`.

Review limitations and decisions:

- The current sample clock pairs a group's starting frame with its final
  processing timestamp. This convention can carry common timing bias; precise
  timestamp semantics are not established here. Retiming it is a separate change.
- `captured` is updated only by the same USB worker, so loading it after submit
  does not introduce an intervening capture-publication skew in this design.
- Host ticks at HAL entry, contiguous extra headroom, worker CPU time and thread
  policy read-back are future observations. We do not claim they were measured.
- We do not demand event-for-event cross-build margins or “indistinguishable”
  distributions from unrelated extrema. The existing recordings cannot establish
  that. No E-MU source or recordings were sent to Claude.

## Candidate and predeclared hardware gates

Candidate `dev-8387395854108015`, SHA-256
`87a3378b39beceedca5c223b906d7d7c90cffc703fe67b51194507851d8c2780`.
Its frozen bundle and 36 source inputs are under
`build/candidates/output-safety-2560-20261018/`. Runtime source comparison against
the qualified archive confirms only `OUTPUT_SAFETY` changes; the other changed
inputs are the new offline model and tests. Offline checks passed 116 Rust tests,
six placement-model tests, four transport-model tests, 18 Python tests, HAL and
loopback callback sanitizer checks, Clippy and bundle validation.

The predeclared sweep used output 3 → input 4 (Rear), one silence preflight and two
independent starts at each actual 64/128/256/512/1024/4096-frame buffer. Its gates were:

- Exact installed/loaded identity, healthy isolated startup and restored buffer.
- Existing waveform, timestamp continuity, callback-size and zero-steady-loss
  checks; zero dropped or invalid timing observations and clean ownership teardown.
- At least **16 ms unloaded OUT post-submit lead**, the four-slot nominal 24 ms
  minus one 8 ms block. This is declared before the experiment; the rejected
  three-slot experiment's original 8 ms gate remains unchanged.
- Raw round trip inside the reference range shifted down by 512 frames, allowing
  two extra sample frames on either side for phase variation. Scheduling distance
  must be exactly twice the actual buffer plus 1024 + 2560 (within numeric tolerance).
- Nonnegative measured `M - B - 832` availability screen. It is explicitly a
  heuristic alongside the exact missing-sample checks, not a contiguity guarantee.

Stop at the first failed gate and preserve the attempt. Only after the sweep is
reviewed should two-client and owner listening checks proceed. Installation
backs up the existing bundle; the prepared rollback selects the exact qualified
`dev-517459af36927152` bundle. The sweep, lifecycle and listening checks now passed;
the final qualification checkpoint preserves their results separately from the
immutable preparation and sweep-only records.
