# Timing geometry candidate — 16 October 2026

**Offline checks, the contributor baseline comparison, and the short hardware
geometry qualification passed.** The candidate tested here was
`dev-4130a04a1146d999`; see [current status](STATUS.md) for the installed build. The unchanged contributor build
`dev-11c2d28e1cc0cf54` (source `a127cbc`) was installed through the shared deployment
script and measured on 16 October. Its executable hash and loaded identity
matched the preserved bundle at `build/baselines/a127cbc/`. The previous research
bundle `dev-57287986d44956d9` was preserved by deployment. These generated local
builds and trial directories are evidence, not repository dependencies.

## Problem and change

The previous 512-frame zero-timestamp period was below the 10923-frame minimum
specified by the local macOS SDK's `AudioServerPlugIn.h`, in the documentation
for `kAudioDevicePropertyZeroTimeStampPeriod`. The observed HAL buffer ceiling
was 192 frames. E-MU's empirical `min(4096, period * 3/8)` relationship predicts
that result; it is not a documented Apple guarantee.

Simply increasing the period would introduce another failure: the old playback
admission window followed the most recently published zero timestamp. While
that pair stays fixed, the device and HAL keep advancing. Valid future writes
would eventually exceed the old admission window.

The Rust [geometry module](../rust/ftu-stream/src/geometry.rs) now supplies the
HAL configuration and timeline capacities together. Playback admission follows
the latest completed capture cursor; zero timestamps keep their stable-pair
semantics and epoch checks. Absolute frame tags and bounded atomic copies still
protect the timelines. No submitted USB payload is modified.

| Setting | Previous source | Candidate |
|---|---:|---:|
| Zero-timestamp period | 512 frames | 16384 frames |
| Maximum HAL call / declared buffer | 1024 frames | 4096 frames |
| Capture and playback ring capacity, each | 16384 frames | 32768 frames |
| Default buffer | 256 frames | 256 frames |
| Declared minimum buffer | 64 frames | 64 frames |
| Input / output safety offsets | 1024 / 4096 frames | 1024 / 4096 frames |
| Capture-feedback replay delay | 64 ms | 64 ms |
| Playback timeline offset | 3072 frames | 3072 frames, derived from delay and rate |

The timestamp publication period is not a queue delay or a monitoring-latency
setting. The predicted 4096-frame external HAL ceiling was confirmed by read-back
and actual callbacks on this Mac. The HAL still exposes a lower minimum than
the source declaration; sizes below 64 are not qualified. Neither a source range
nor a successful set request alone proves the delivered callback size.
The diagnostics now accept requests through 4096 and retain actual spans and
read-back. `make check` also reports buffer-range and timestamp-period reads,
including their error status if a property is unavailable.

This candidate does **not** reduce monitoring latency. Its measurements reproduce
the earlier 110.54–115.87 ms result at 64/128/192 frames. Safety offsets, USB
schedule depth and payload ownership policy have not been optimized.

## Offline validation

- All 100 Rust tests passed. New regressions exercise full 4096-frame live IO
  near the end of a held timestamp period, ring wrap, rejected oversized/empty
  calls, future/past admission bounds and foreign epochs. Timestamp tests span
  twenty publication periods with jitter and reject regressing pairs.
- Driver and all diagnostic tools compile; the bundle validates and its ad-hoc
  signature verifies.
- HAL boundary and loopback callback tests passed under AddressSanitizer and
  UndefinedBehaviorSanitizer. Coverage includes maximum-size calls, rejection
  above capacity, partial final buffers and channel isolation.
- Seven offline loopback analysis tests passed, including 4096-frame callbacks
  and partially filled final capture buffers.
- `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

Local native/lint logs, including the initial test failure and corrected rerun,
are in `build/validation/timing-geometry-offline/`. No synthetic test is counted
as a hardware measurement.

## Contributor baseline comparison

The shared deployment script installed the unchanged contributor bundle,
preserved the research bundle, and restarted CoreAudio successfully. The installed
SHA-256 was `d490dd4a823f7ac84ccb5bb9d3c5c3c5e236ed5f2e0f9bd08a7b621cb8dfb487`;
CoreAudio reported `dev-11c2d28e1cc0cf54`. This exercises the live deployment
helper used by `make install`, but is not a fresh-Mac bootstrap or `.pkg` test.

Physical wiring was rear output 3 → rear input 4, with input 4 set to Rear.
The diagnostic retained only input 4, emitted -40 dBFS chirps only on output 3,
and kept software monitoring and all other outputs off. An initial eight-second
silence run at 64 frames passed with input peak 0.00005484. Four subsequent
eight-second runs passed, each containing two chirps:

| Actual callback frames | Independent starts | Chirps | Mean raw round trip | Mean timestamp-relative remainder |
|---|---:|---:|---:|---:|
| 64 | 2 | 4 | 110.5391 ms | 1.20579 ms |
| 192 | 2 | 4 | 115.8725 ms | 1.20580 ms |

Every callback in each run had its requested size. Sampled playback-missing
counters were constant after startup; capture-missing stayed zero. Timestamp
continuity, correlation, epoch stability, final Idle state, empty pending queues,
zero cleanup/fault status, and buffer restoration all passed analysis. Startup
playback-missing totals were nonzero; these results do not claim loss-free
startup or continuously traced application transitions. The raw round trips
reproduce the research baseline; the small timestamp remainder is not monitoring
latency or calibrated converter latency.

Local evidence, including raw input-4 captures, callback records, invocation
hashes, analysis, and an aggregate `summary.json`, is retained under
`build/trials/2026-10-16-geometry/baseline/`. Installation logs are under
`build/validation/timing-hardware-20261016/`. Temporary microphone access was
restored off after the baseline stage. No new listening test was performed.

## Candidate hardware qualification

The owner installed the candidate through the same deployment helper. Installed
executable SHA-256:
`3f442f9065de125287bdb349e3c65cb3fe29d8124bd07e81aa447305fbcade3b`.
CoreAudio reported `dev-4130a04a1146d999`, period 16384, and external range
**15–4096**. The post-install host buffer was 512, despite the source default of
256. Each test restored that observed 512-frame setting. This is a useful reason
to expose read-back in diagnostics instead of displaying configuration alone.

The initial eight-second silence run at 64 frames passed with input peak
0.00005794. The same output-3/input-4 wiring and bounded chirps then produced:

| Actual callback frames | Accepted / attempted runs | Accepted chirps | Raw round trip across accepted chirps |
|---|---:|---:|---:|
| 64 | 2 / 2 | 4 | 110.539 ms |
| 128 | 2 / 2 | 4 | 113.206 ms |
| 192 | 2 / 2 | 4 | 115.872 ms |
| 256 | 2 / 3 | 4 | 118.539 ms |
| 512 | 2 / 2 | 4 | 129.206 ms |
| 1024 | 2 / 2 | 4 | 150.539–150.560 ms |
| 2048 | 2 / 2 | 4 | 193.206 ms |
| 4096 | 2 / 2 | 4 | 278.539 ms |

All callbacks in each accepted run had the requested size, including the final
callback whose retained samples were capped at eight seconds for 2048/4096.
All 16 accepted runs had continuous input/output sample timestamps, stable
sampled post-start loss counters/epoch, no capture misses, clean isolated
teardown, zero faults/cleanup errors, and restored buffers. These are short
runs, not a long-session or startup-loss guarantee. Nonzero startup
playback-missing counters remain visible in the logs.

One additional 256-frame run was rejected as `unclean_or_shared_stop` and remains
in the evidence. CoreAudio's log showed an overlapping Google Chrome Helper
client (PID 3236) from 22:07:50.890 to 22:08:00.899 local time; the diagnostic
stopped at 22:07:59.832. The driver reached Idle when the remaining client
stopped. A fresh isolated repeat passed. No rejection criterion was relaxed,
and the rejected chirps are excluded from latency aggregates.

The timestamp-relative remainder was approximately 1.206 ms, except for one
1024-frame start at 1.227 ms: a one-sample restart difference retained in the
reported range. For every accepted chirp, raw minus timestamp RTL remained
`2 * actual_buffer + 1024 + 4096` frames. This supports prioritizing measured
scheduling lead next; it does not identify separate ADC/DAC delay or independently
calibrate the absolute host clock.

A twelve-second output-1/2 test delivered all five quiet lower-left/higher-right
pairs. The owner confirmed all five sounded clean and correctly routed. There
were no invalid callbacks or timestamp gaps, sampled counters stayed flat, and
teardown was clean. It ran at the restored 512-frame host setting. GarageBand
and Spotify have not been repeated on this candidate.

Two silent lifecycle scenarios also passed at requested/read-back 64 and 4096:
A alone for five seconds, A+B for five seconds, then B for ten seconds after A
stopped. Both clients had zero invalid callbacks/gaps, sampled counters and epoch
stayed constant, and final stop left Idle with no pending transfers, fault or
cleanup error. Callback totals matched the requested sizes. The 512-frame
setting was restored after each scenario. This sampled test is not continuous
lifecycle tracing of arbitrary applications.

Raw captures, callback records, all analyses (including the rejection), hashes,
and `summary.json` remain in `build/trials/2026-10-16-geometry/candidate/`.
The rejected run also retains `coreaudio-lifecycle.log`. Installation, tone and
two-client logs/metadata are in `build/validation/timing-hardware-20261016/`.
ChatGPT's Microphone switch was restored off after captures, and a fresh helper
reported denied before the final silent lifecycle tests. No mixer writes,
default-device changes, native-rate changes or E-MU edits were made.

## Next implementation and validation

1. Instrument HAL write coverage versus USB payload preparation/deadlines and
   capture publication. Model phase and delivery stalls before lowering margins.
2. Lower one timing margin at a time, then repeat raw/timestamp loopback with
   independent starts, actual buffer read-back and unchanged rejection criteria.
3. Repeat GarageBand playback/recording and longer sessions on the measured
   candidate. Capture lifecycle events continuously before attributing any new
   miss count, including the earlier research-build 128-frame transition loss.
4. Qualify more inputs/outputs, other Macs and an actual Ultra 8R separately.

This applies ASFW's single-configuration and read-back-first methods described
in [the reuse notes](ASFW-REUSE.md). No ASFW or E-MU source was copied or changed.
