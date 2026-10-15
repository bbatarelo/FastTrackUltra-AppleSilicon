# Current status

Baseline: 15–16 October 2026, desktop Fast Track Ultra on Apple Silicon,
macOS 26.7.1, installed driver `dev-57287986d44956d9`. This repository preserves
the implementation but its build/package metadata can produce a new identity.
A new build has not inherited the installed binary's hardware qualification.

| Area | Verified baseline | Remaining limits |
|---|---|---|
| Desktop Ultra | 48 kHz audio, eight Float32 inputs/outputs published | More units/Macs/OS versions and all-channel tests |
| Ultra 8R | Product ID recognized by shared driver | Hardware not tested |
| Playback | Outputs 1/2 clean in VLC and GarageBand; owner reports Spotify working; output 3 measured | Longer sessions and other outputs |
| Recording | Input 4 physical return in diagnostic and GarageBand | Other inputs; input-3 same-channel loop saturation unresolved |
| GarageBand | Playback, recording, play/stop, Cycle, seek, saved-project reopen | One unclassified 128-frame transition loss; broader DAW tests |
| Buffers | Six independent runs at actual 64/128/192 frames | External range 15–192 differs from source 64–1024 |
| Latency | Raw round trip 110.54/113.21/115.87 ms | High for software monitoring; timing optimization needed |
| MIDI | Planned | System handling and bidirectional CoreMIDI behavior unqualified |
| Mixer/DSP | Audio used existing hardware configuration | Control read-back, routing app and effects not implemented/qualified |
| Install/package | Offline build/extraction and temporary-root deployment tests | New deployment/package path needs live/fresh-Mac testing |

The timestamp-relative remainder is about 1.206 ms, **not** the usable monitoring
round trip. See [qualification](LOOPBACK-QUALIFICATION.md) for reference planes,
rejected trials, sample sizes and cleanup.

Only native 48 kHz is supported. VLC's 44.1 kHz file playback and GarageBand's
44.1 kHz recording exercise application conversion. Other rates, clock modes,
MIDI, hotplug/sleep/wake, automatic recovery and multiple Ultras are roadmap work.
The macOS 14 deployment target is not a tested per-version compatibility matrix.

Next implementation priorities are in [ROADMAP.md](ROADMAP.md). Exact archive
identity and extraction scope are in [PROVENANCE.md](PROVENANCE.md).
