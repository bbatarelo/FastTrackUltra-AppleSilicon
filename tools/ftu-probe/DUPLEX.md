# Bounded Rust duplex experiment

```sh
make -C tools/ftu-probe
tools/ftu-probe/build/ftu-probe duplex-silence --output NEW_DIRECTORY --product 2080 --duration-ms 256
python3 tools/ftu-probe/analyze_duplex.py NEW_DIRECTORY
```

Select a particular device with `--location HEX_LOCATION`. Product 2081 is
recognized by the same code; the 8R has not been hardware-tested. An existing
output directory is rejected before hardware access. Silence duration is
8–2048 ms in multiples of 8, default 256 ms. This is a standalone diagnostic,
not an installed CoreAudio driver.

## Fixed hardware contract

Both rate GETs must already report 48,000 Hz, connection speed must be high,
and cached descriptors must match both supported streaming profiles. Both
interfaces must initially be at alternate 0. The adapter opens interface 2
(IN81) and interface 1 (OUT01), allocates all buffers, then activates their
alternate 1 in that order. It verifies native pipe geometry after activation.
The companion IN82 pipe is checked but receives no transfers.

There are no mixer or sample-rate SET requests, device open/seize/reset,
configuration changes or firmware operations. Interface alternate selection
is the required USB state change; cleanup verifies restoration of both.

## Rust/native boundary and packet planning

`rust/ftu-protocol` supplies profile validation, controls, PCM and portable
accounting. `rust/ftu-stream` owns scheduling, per-direction queues, capture
feedback, waveform generation, stop/drain decisions and evidence. The small
C function table in `shared/ftu_native.h` exposes native operations only.
`duplex.c` owns IOKit handles and wired buffers and bridges same-thread callbacks.

Each request contains 64 microframes (8 USB milliseconds). At most four requests
are pending per direction; every request gets unique memory that is never
recycled during this finite run. Capture starts 16 ms ahead of the initial bus
frame. OUT block i starts exactly 64 ms after IN block i and uses each accepted
input packet's actual byte count. Counts must be positive, at most 312 and
divisible by the 24-byte eight-channel frame. Missing, empty or invalid input
stops both queues; there is no nominal-six-frame fallback or late rebasing.

OUT packets are packed consecutively using their requested lengths; IN slots
have a fixed 312-byte stride. Rust fills OUT memory before submission. No payload
or frame list is read while owned by a pending request. Only a matching callback
returns ownership. A failed submit creates no callback debt. A bad callback,
packet, native fault or stale schedule stops further submissions.

On a signal or error, both queues stop admission, only pending pipes are aborted,
and callbacks are dispatched for at most one second. Resources are released
only after debt is drained. If Rust or native bookkeeping still indicates
pending ownership, exit 4 terminates the standalone process without freeing
live buffers/refcons. This fallback is unsuitable for a long-lived HAL host.

## Listening command

Run this only with the user ready and monitor level low:

```sh
tools/ftu-probe/build/ftu-probe playback-tone --output NEW_DIRECTORY --product 2080
```

The duration is fixed at 2048 ms; any other duration is rejected. The output is
eight-channel packed signed little-endian 24-bit PCM. Relative to output frame 0:

| USB channel | Signal window at 48 kHz | Signal |
|---|---|---|
| 1 | 0.1–0.7 s | 440 Hz |
| 2 | 0.9–1.5 s | 660 Hz |
| 3–8 | Entire stream | Digital zero |

Both tones have a 0.01 peak ceiling (-40 dBFS) and 20 ms raised-cosine ramps.
All other samples are zero. Mixer state is unchanged. The intended listening
result is left then right through outputs 1/2. The first desktop trial confirmed
this with the existing mixer state; other routes and states remain unverified.
No sound requires routing investigation; do not raise the digital
ceiling or treat uniform zero GETs as a trustworthy restore image.

## Evidence

`metadata.json` is written by the CLI before opening streaming interfaces.
`plan.json` describes geometry and policy. `duplex-config.bin` is the validated
cached descriptor. A flushed `duplex_ready` event precedes clock sampling and
submissions; other events are written after the stream. This marker allows an
external runner to time SIGINT without filesystem work in the transfer loop.

`observations.jsonl` retains all planned slots, including unsubmitted ones:
direction (0=IN81, 1=OUT01), block/slot, bus start, queue state, submission and
completion statuses, callback identity/ticks, planned/requested/actual sizes,
packet timestamps, acceptance and raw offset. Aborted payload is retained but
never accepted as valid feedback or completed audio.

`input-packets.bin` and `output-packets.bin` contain a full 19,968-byte reserved
block for every matched callback. Offsets select the actual packet bytes; they
are not flat continuous PCM files. `summary.json` records counters, first stop
reason, cleanup, restored/open masks, original alternates, device identity,
clock anchor and rate reads. Rate arrays explicitly use endpoint order [1,129],
independent of the transfer-direction convention. `output_planned_frames` also
includes a failed OUT submission attempt; it is not a delivered-frame claim.

The offline analyzer independently checks request/slot/byte/frame conservation,
input-to-output packet mapping, raw strides, all output samples, the tone bounds
and channel windows, and successful cleanup/rates. Gzip archives are supported.
`audit:passed` means evidence is internally consistent, including incomplete
runs; inspect `experiment_status`. It does not verify analogue output or latency.

Exit 0 means complete and restored, 3 means incomplete (including a drained
signal), 1 means setup/evidence/cleanup failure, 2 means invalid usage, and 4
means native ownership could not be safely released. A native completion
contract error may also retain debt and cause exit 4; the original reason is
kept separately from `drain_failed`.

## Limits

The 64 ms delay is a conservative diagnostic scheduling choice, not an analogue
latency measurement or final HAL setting. This finite test does not establish
continuous drift tracking, rate changes, external-clock recovery, physical
channel mapping, effects isolation, MIDI coexistence or 8R compatibility.
The full tone is generated in the standalone loop; a production realtime path
needs separately bounded processing and recovery before this engine enters HAL.
