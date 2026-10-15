# Selected regression fixtures

These are byte-identical selected inputs from the research archive. They are
real observations except the original synthetic playback WAV, which was used
in an actual VLC experiment. They are not new hardware runs in this repository.

| Fixture | Why it is retained |
|---|---|
| `descriptors/ultra-config-0.bin` | Real 340-byte USB descriptor, C/Rust profile tests and bounded mutation checks |
| `capture/run-*` | Five input captures covering complete, interrupted and restarted sessions; byte/frame accounting and corruption rejection |
| `hal-app/run-004-vlc-melody-repeat` | Startup admission versus later loss, input faults and teardown debt, with actual application observations and an original quiet waveform |
| `hal-app/hal-check.bin` | Historical inspector bytes checked by the archived audit; never executed by tests |
| `rust/ftu-protocol/tests/fixtures/config-0.bin` (repository-relative) | Same descriptor retained at the compile-time Rust fixture path |

`manifest.json` records every source path, exact archive commit, SHA-256 and size.
`make test-fixtures` verifies the bytes. Capture files stay compressed; their
analyzers read them without requiring any external folder. Tests mutate temporary
copies, not these originals.

The 4.8 MiB selection excludes historical driver binaries, repeated source
snapshots, debug symbols, complete playback/loopback sessions and review
transcripts. Those stay in the archive. Add small, purposeful fixtures with
provenance when a regression needs them; do not import whole experiment trees.
