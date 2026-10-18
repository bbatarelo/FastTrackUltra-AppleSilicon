# Current status

As of 18 October 2026, **`dev-8387395854108015` is installed and verified loaded**
on the desktop Fast Track Ultra, Apple Silicon, macOS 26.7.1. This
[output-safety reduction](PLACEMENT-MODEL.md) passed thirteen physical loopback
runs, two silent two-client scenarios and the owner's five-pair output-1/2
listening check. The installed binary matches the frozen bundle; current source
uses the same configuration.

Native 48 kHz, four 8 ms USB slots per direction and input safety 1024 are
unchanged. Output safety is now **2560 frames**, down from 3072. Raw round trip
measured **78.539 ms at a 64-frame buffer**, about 10.67 ms less than the preceding
qualified build. Monitoring latency remains high.

## Measured behavior

The physical sweep used rear output 3 → rear input 4, channel 4 Rear: one silence
preflight and two independent starts at each buffer size. All 24 chirps passed.
All 29,438 recorded callbacks had the requested frame count and the expected
output-minus-input sample distance, `2 * buffer + 1024 + 2560`.

| Actual buffer | Raw round trip |
|---:|---:|
| 64 | 78.539 ms |
| 128 | 81.206 ms |
| 256 | 86.539 ms |
| 512 | 97.206–97.227 ms |
| 1024 | 118.539 ms |
| 4096 | 246.539 ms |

Timestamps remained continuous, steady input/output misses stayed zero, and
every run stopped cleanly and restored 512 frames. Startup playback-miss counts
remain nonzero and are retained; zero steady loss does not mean zero lifetime
misses. The roughly 1.206–1.227 ms timestamp-relative remainder is not calibrated
converter delay or application monitoring latency.

The silent A → A+B → B checks passed at 64 and 4096 frames. The owner heard all
five lower-left/higher-right tone pairs cleanly at 512 frames. Across the sweep,
two-client checks and listening, the recorder observed **39,687** transfer
admissions with no dropped observations or attention events below 16 ms.
Minimum OUT post-submit lead was **22 ms**, exceeding the predeclared 16 ms gate.
The additional availability screen retained at least **150 frames** after
reserving one client buffer plus a maximum-size USB block; this is a heuristic,
not proof of contiguous sample coverage.

## Scope and remaining limits

| Area | Verified behavior | Remaining limits |
|---|---|---|
| Desktop Ultra | Native 48 kHz, eight Float32 inputs/outputs published | More units/Macs/OS versions and all-channel tests |
| Ultra 8R | Product ID recognized by shared driver | Hardware not tested |
| Playback | Current build: owner-confirmed clean output-1/2 tones and output 3 physical return | Longer sessions and other outputs |
| Recording | Current build: input 4 physical diagnostic return | Other inputs; input-3 same-channel loop saturation unresolved |
| Buffers | Current build: two loopback starts each at actual 64–4096 frames | External minimum 15 differs from declared 64; below 64 unqualified |
| Latency | Current build: 78.539 ms at 64 frames | Still high for monitoring |
| Two clients | Current build: silent A → A+B → B at 64 and 4096; clean final stop | Longer application-transition tracing |
| Applications | Research build: VLC/GarageBand clean playback, Spotify owner report, GarageBand recording and transport | Not repeated on this build; one earlier unclassified 128-frame transition loss |
| MIDI | Planned | System handling and bidirectional CoreMIDI behavior unqualified |
| Mixer/DSP | Audio uses existing hardware configuration | Effective control read-back, routing app and effects unqualified |
| Install/package | Current installation and earlier checkpoint rollback verified by loaded identity and installed hash | Fresh-Mac setup and live `.pkg` installation remain untested |

After this installation, the first read-only device-enumeration query timed out
at 15 seconds while CoreAudio showed high CPU use. A later query completed,
reported the correct idle build, and daemon CPU settled before the first audio
test. The sampled client was waiting in CoreAudio system-object initialization.
The cause is unresolved; this does not establish either a driver fault or an OS
fault. Diagnostics and the original timeout are retained. Restart/startup
reliability remains work to qualify.

## Preserved checkpoints and next work

The preceding four-slot `dev-517459af36927152` (output safety 3072) remains the
exact rollback target, with its successful [worker-timing checks](WORKER-TIMING.md)
preserved. The older `dev-2820c37b9ed14d39` also remains available. The rejected
three-slot `dev-a70335afdd0f3a22` is still rejected: one run reached 6 ms against
its predeclared 8 ms gate. None of its seven attempts was replaced. See
[the queue experiment](QUEUE-MODEL.md).

The current bundle and all 36 source inputs are frozen under
`build/candidates/output-safety-2560-20261018/`; hardware evidence is under
`build/trials/2026-10-18-output-safety-2560/`. The final
`qualification-checkpoint.json` records the subsequent lifecycle and listening
success; the immutable sweep-only summary retains their earlier pending status.
Offline checks passed 116 Rust tests, six placement-model tests, four transport-
model tests, HAL/native/callback sanitizer checks, 18 Python tests, Clippy and
bundle validation. Claude independently reviewed the design and new model.

Next: qualify longer sessions and trace application transitions, then investigate
completion delivery, worker scheduling and shorter USB batches. Extend the model
with production-pool behavior and drift, and characterize capture constraints
before lowering input safety. The [ASFW comparison](ASFW-REUSE.md) records concrete
methods to evaluate later in E-MU; no E-MU source has changed.

Only native 48 kHz is supported. Earlier VLC/GarageBand 44.1 kHz sessions exercised
application conversion. Other rates, clock modes, MIDI, hotplug/sleep/wake,
automatic recovery and multiple Ultras are roadmap work. This is short
qualification on one unit and Mac, not a long-session guarantee or a per-version
macOS compatibility matrix.

See [ROADMAP.md](ROADMAP.md) for priorities and [PROVENANCE.md](PROVENANCE.md)
for archive identity and extraction scope.
