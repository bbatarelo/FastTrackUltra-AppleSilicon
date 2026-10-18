# Current status

As of 18 October 2026, the installed build is `dev-2820c37b9ed14d39`, tested on
a desktop Fast Track Ultra, Apple Silicon, macOS 26.7.1. The first output-safety
reduction passed all thirteen physical loopback runs and two silent two-client
scenarios. Raw round trip at 64 frames is **89.206 ms**, down from 110.539 ms.
That is a measured 21.333 ms improvement, but remains high for software monitoring.
See [exact builds, margins and evidence](TIMING-OBSERVATION.md).

Current source matches this measured driver's hashed inputs. On 18 October the
owner also confirmed all five output-1/2 tone pairs clean and correctly routed.
The exact binary, source archive and qualification record are preserved as the
next comparison checkpoint. Qualification remains specific to each binary and
test; earlier application results do not automatically transfer.

| Area | Verified behavior | Remaining limits |
|---|---|---|
| Desktop Ultra | Native 48 kHz, eight Float32 inputs/outputs published | More units/Macs/OS versions and all-channel tests |
| Ultra 8R | Product ID recognized by shared driver | Hardware not tested |
| Playback | Current build: output 3 physical return; five output-1/2 tone pairs heard clean and correctly routed | Longer sessions and other outputs |
| Recording | Current build: input 4 physical diagnostic return | Other inputs; input-3 same-channel loop saturation unresolved |
| Buffers | Current build: two accepted loopback starts each at actual 64/128/256/512/1024/4096 | 192/2048 passed on earlier geometry build; external minimum 15 differs from declared 64; below 64 unqualified |
| Latency | Current build: raw 89.206 ms at 64 frames, 107.872 ms at 512, 257.206 ms at 4096 | Still high for software monitoring; input/output safety 1024/3072 |
| Two clients | Current build: silent A → A+B → B at 64 and 4096; clean final stop | Longer sessions and application-transition tracing |
| Applications | Research build: VLC/GarageBand clean playback, Spotify owner report, GarageBand recording and transport | Not repeated on current build; one earlier unclassified 128-frame transition loss |
| MIDI | Planned | System handling and bidirectional CoreMIDI behavior unqualified |
| Mixer/DSP | Audio uses existing hardware configuration | Effective control read-back, routing app and effects unqualified |
| Install/package | Current frozen bundle installed through shared helper with backup/restart and loaded-hash verification | Fresh-Mac setup and live `.pkg` installation remain untested |

The current sweep accepted one silence run and twelve chirp runs, with 24
accepted returns and no rejected attempt. Every actual callback matched its
requested size, sample timestamps were continuous, final stops were idle with
no pending USB work, and each run restored the 512-frame host setting.

The steady observations recorded zero input/output misses, zero dropped
observations and 23–24 ms post-submit USB lead. Startup output-miss totals are
nonzero and retained. The two-client tests also had flat sampled counters,
zero invalid callbacks/gaps and valid final timing observations. These are
short isolated checks, not long-session or arbitrary-application guarantees.
The twelve-second listening run at 512 frames also had zero invalid callbacks
or timestamp gaps, flat sampled counters, valid timing observations and clean
teardown; the owner confirmed the audible result.

The timestamp-relative remainder is about 1.206 ms, **not** the usable monitoring
round trip. Raw minus timestamp RTL equals twice the client buffer plus the
1024/3072-frame safety offsets. The explicit extra-callback/USB-block availability
budget retained 667 frames at its minimum across the sweep; it is a chosen
screening assumption, not a measured worst-case bound.

The [earlier geometry qualification](TIMING-GEOMETRY.md) records actual buffers
through 4096, owner listening, two-client checks, and a rejected Chrome-overlap
attempt with a successful isolated repeat. The [observation comparison](TIMING-OBSERVATION.md)
retains the unchanged-safety instrumented sweep and its one-sample restart
spread. The research build established the [GarageBand and physical-loopback
baseline](LOOPBACK-QUALIFICATION.md).

Only native 48 kHz is supported. Earlier VLC/GarageBand 44.1 kHz sessions exercised
application conversion. Other rates, clock modes, MIDI, hotplug/sleep/wake,
automatic recovery and multiple Ultras are roadmap work. The macOS 14 deployment
target is not a tested per-version compatibility matrix.

Next priorities are in [ROADMAP.md](ROADMAP.md). Archive identity and extraction
scope are in [PROVENANCE.md](PROVENANCE.md).
