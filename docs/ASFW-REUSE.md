# ASFireWire: reusable upper-layer work

Inspected upstream commit:
`0b8a23a65e0d6c35f3f59c913d7402a39c172356` (14 October 2026).
Selected source references and hashes remain in the frozen research archive;
this contributor repository does not vendor ASFW. See [provenance](PROVENANCE.md).

The overlap is substantial: a hardware clock must drive CoreAudio, callbacks
and transport progress on different schedules, buffers have lifetimes, and
reported latency must match what a cable measures. ASFW uses AudioDriverKit;
FTU uses `AudioServerPlugIn`. Reuse the measurement methods
and invariants while implementing the different host API correctly.

For the existing E-MU driver, see the separate
[backport candidate assessment](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/EMU-BACKPORT-CANDIDATES.md). It compares the
specific ASFW mechanisms with what E-MU already implements and gives a
validation experiment for each candidate.

## Highest-value adoption candidates

| Reference | Idea worth adopting | FTU adaptation |
|---|---|---|
| `tools/halprobe/` | Inspect HAL read-back, fit the published clock with numerically stable math, record callback spans, sweep granted buffer sizes | Use as an independent client against FTU; retain nulls for unread properties |
| `tools/rtl/` | Separate physical callback-count RTL from sample-time RTL and declaration residual | Keep alongside E-MU's tone/channel/phase tests; explicitly cover 88.2 kHz |
| `tools/rtl/rtl_core.h` and README | Reject trials crossing lost frames, re-anchors, overloads, missing timing, or weak/edge signals | Report acceptance ratio and reason counts, not only successful measurements |
| `Runtime/ResolvedTimingGeometry.hpp` and `TimingGeometryTests.cpp` | One validated configuration drives both declarations and runtime behavior | Resolve USB intervals, channels, capacities, timing margins, and ZTS coherently |
| `Runtime/HardwareSampleTimeline.hpp` and tests | Explicit clock source/epoch, stale-observation rejection, bounded transition behavior | Replace FireWire bus correlation with measured USB sample/time observations |
| `Runtime/HostClockAnchor.hpp` and tests | Coherent latest-anchor publication with generation and epoch | Keep same-process implementation small; audit bounded reads and memory ordering |
| `Runtime/PlaybackRingRange.hpp` and tests | Explicit valid frame interval and discontinuity/overrun detection | Prevent old ring laps from masquerading as new audio |
| `tools/tx_payload_ownership_sim.py` | Separate missing PCM payload from transport deadline failure | Model HAL writes, USB slot exposure/submission, transfer deadline, and recycling |
| `tools/asfw_timing_geometry_sim.py`, `tx_data_horizon_burst_sim.py` | Inject phase, burst, and stall conditions before hardware debugging | Replace all FireWire cadence/ring constants; use USB descriptor-derived geometry |
| `tests/audio/ZtsCharacterizationTests.cpp` | Named clean, loss, gap, no-clock, and restart traces | Create USB trace replays when captures exist |

Source links: [HAL probe](https://github.com/mrmidi/ASFireWire/tree/0b8a23a65e0d6c35f3f59c913d7402a39c172356/tools/halprobe),
[RTL instruments](https://github.com/mrmidi/ASFireWire/tree/0b8a23a65e0d6c35f3f59c913d7402a39c172356/tools/rtl),
[runtime models](https://github.com/mrmidi/ASFireWire/tree/0b8a23a65e0d6c35f3f59c913d7402a39c172356/ASFWDriver/Audio/Runtime),
[ownership simulator](https://github.com/mrmidi/ASFireWire/blob/0b8a23a65e0d6c35f3f59c913d7402a39c172356/tools/tx_payload_ownership_sim.py).

## Improve measurement before optimizing the engine

ASFW's RTL tool distinguishes:

- `RTL_raw`: delivered-frame distance from writing a signal to observing it
  return, independent of the driver's sample-time numbering.
- `RTL_ts`: distance between those events in sample-time coordinates.
- Scheduling distance: their paired difference.
- Residual: timestamp RTL minus declared device/stream hardware latency,
  withheld if declarations could not be read.

This is complementary to E-MU's existing chirp, phase, drift, and channel tests.
Keep both instruments until their reference planes and failure handling are
understood. Same-interface timestamp correlation alone is not an independent
proof of the published clock or total monitoring latency.

An especially useful experiment changes only a declared hardware latency by a
known amount. Neither physical RTL nor timestamp RTL should move; the residual
should change by the opposite amount. Safety offsets and client buffer size
are unsuitable controls for that experiment because they can change scheduling.

ASFW records all trials, rejection reasons, and build/configuration metadata,
and warns that one stable session can hide restart-dependent latency classes.
Use repeated session baselines. A good median from a few survivors among many
rejected trials is not a clean small-buffer result.

## First concrete FTU application of these methods

The [loopback instrument](../tools/hal-loopback/README.md) now retains delivered-
frame and timestamp reference planes, actual callback spans, unread declaration
states, rejected trials and exact tool/installed-driver hashes. Its independent
analyzer is tested with known delays and bad signals before hardware results
are accepted. Six independent output-3/input-4 sessions accepted all twelve
chirps at actual 64/128/192-frame buffers. Raw round trip was 110.54–115.87 ms,
while timestamp RTL was about 1.206 ms. Reporting only the latter would hide
the large scheduling lead. Their difference was exactly twice the client
buffer plus the 1024/4096-frame safety offsets. This is concrete value from
keeping both reference planes; it does not justify changing the driver before
an ownership/queue-lead review.

Repeated starts showed a consistent short-run result. Rejected permission and
input-level trials remain beside the accepted sessions. A later GarageBand
transition increased the output-miss counter by 128 frames outside the sampled
listening interval; preserving that uncertainty defines the next lifecycle-
instrumentation task. These are useful methods to evaluate in E-MU, not evidence
that E-MU has the same defects.

The read-back-first method already exposed a useful discrepancy: FTU's source
originally declared a 64–1024 range, while this Mac's external HAL reported
**15–192**.
The E-MU project's documented timestamp-period cap predicts 192 from FTU's
512-frame period. This supports keeping host-visible geometry in diagnostics
and the eventual companion app, rather than displaying only driver constants.
See [the qualification findings](LOOPBACK-QUALIFICATION.md). No ASFW source was
copied and no E-MU files were changed.

## Single configuration applied to the next candidate

The [measured timing candidate](TIMING-GEOMETRY.md) puts HAL limits, timestamp
period, ring capacity and admission bounds in one Rust geometry module with
capacity assertions and maximum-buffer regressions. This applies the
`ResolvedTimingGeometry` method to AudioServerPlugIn. Diagnostics retain external
read-back errors and actual callbacks because source constants alone did not
predict the original host range. Hardware now confirms a 16384-frame timestamp
period, a 4096-frame ceiling, and actual callbacks through that size. The host
selected 512 after installation despite the source default of 256: another
reason to show measured values in a future diagnostic app.

The same strict trial rejection caught a Chrome client overlapping an isolated
measurement; logs identified the shared session, and a fresh repeat passed.
A one-sample difference between 1024-frame starts remains in the reported range.
These are concrete benefits of retaining rejected attempts and restart spread.
The measured [observation build](TIMING-OBSERVATION.md) separates publication and
preparation margins, actual missing PCM, and submission lead separately. Its
regression with a positive high-water margin but a missing payload makes the
ownership distinction concrete. Thirteen physical runs now retain minima across
independent starts, count actual misses, and separate startup from the steady
bank. A counterfactual shift with an explicit callback/block disturbance budget
selected a smaller output-safety step for hardware testing. The subsequent
physical sweep measured the predicted 21.333 ms reduction at 64 frames, with
zero steady misses and 23–24 ms post-submit lead. Keeping reference planes and
actual payload loss separate made that comparison reviewable; it did not
require copying any FireWire timing constant.

For a later E-MU review, the reusable pieces are: publish queue/copy boundaries
explicitly; record signed lead and capture availability alongside actual missing
samples; retain coherent final epoch snapshots; and state a disturbance budget
separately from measured extrema. The FTU USB constants and the resulting
3072-frame experiment are not E-MU recommendations. These observations do not
establish an E-MU defect or backport need.

## Boundaries on reuse

FireWire cycle counters, CIP/SYT presentation timestamps, AMDTP blocking cadence,
DMA-ring geometry, and device profiles do not define USB behavior. Even though
high-speed USB can use 125-microsecond intervals, equal duration does not make
the transports equivalent.

The ownership simulator compares particular PUSH/PULL scheduling assumptions.
It is not proof that direct output binding is universally wrong or that a deep
USB schedule necessarily adds equivalent signal latency. Instrument slot
coverage, payload availability, and recycle races in our own model.

Do not copy ASFW's latency/safety constants or assume its fixtures validate FTU.
Some source comments reference historical documents/labs no longer present in
the checked main tree. The current tools and tests above were inspected; a
historical `ADKVirtualAudioLab` checkout was not obtained or evaluated.

The pinned ASFW repository carries Apache-2.0 and a NOTICE. No ASFW code has
been integrated into a product here. Preserve the relevant license/attribution
if a later task reuses its tools or implementation.
