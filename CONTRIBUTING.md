# Contributing

Read the [status](docs/STATUS.md), [roadmap](docs/ROADMAP.md) and
[architecture](docs/ARCHITECTURE.md) first. The clean repository contains the
active implementation, diagnostics and selected regression fixtures. Full
experiments, failed trials and reviews remain in the research archive linked
from [provenance](docs/PROVENANCE.md).

## Build and verify

Use Apple Silicon, Xcode Command Line Tools, Rust/Cargo and Python 3.
No third-party Rust crates are required; Cargo builds offline. NumPy is an
additional dependency only for loopback analysis and its Python tests.

```sh
make doctor
make
make test
make test-hal
make test-loopback
make test-install
make test-app
make test-fixtures
cargo clippy --workspace --all-targets --release --offline -- -D warnings
make package
```

These commands do not start hardware audio or install a system driver. Native
HAL and loopback callback tests use AddressSanitizer/UndefinedBehaviorSanitizer;
Rust is not sanitizer-instrumented by those targets. Installation tests use
signed dummy bundles in temporary filesystem roots without administrator rights
or restarting CoreAudio. Package construction extracts and validates its own
outputs but does not deploy them.

`make check` reads the installed driver's status. A build's `make version` can
differ from the loaded identity until installation. Do not run raw USB probes,
`live-check` or `hal-host-check` while the installed HAL may own the device.
Hardware runs require an explicit test setup; arrange low levels and a listener
for audible tests, and request input-recording permission where needed.

## Repository map

| Path | Responsibility |
|---|---|
| `driver/` | CoreAudio HAL adapter and bundle metadata |
| `rust/ftu-protocol/` | Profiles, controls, packed PCM and packet accounting |
| `rust/ftu-stream/` | Sessions, feedback, timing, buffer ownership and scheduler |
| `shared/` | Narrow C ABI between Rust and native transport/HAL |
| `tools/ftu-probe/` | Native USB boundary, bounded diagnostic, offline oracles and analyzers |
| `tools/hal-check/`, `tools/hal-loopback/` | External CoreAudio diagnostics and measurements |
| `tools/live-check/`, `tools/hal-host-check/` | Standalone engine/host bring-up tools |
| `scripts/`, `packaging/` | Build identity, install/remove, packages and experiment tooling |
| `tests/fixtures/` | Only retained regression inputs, with archive paths and hashes |
| `docs/` | Current guides, findings, architecture, roadmap and baseline |

Protocol, timing and ownership policy belongs in Rust. Native code translates
macOS APIs, owns OS handles and bridges callbacks according to Rust's lifecycle.
Original portable C modules in the probe are offline acceptance oracles, not
a second production scheduler. Ultra and Ultra 8R share one implementation;
model differences require measured profiles rather than duplicate folders.

## Evidence and changes

Describe the problem, behavior, validation and remaining limits. Hardware claims
must include loaded build identity, device/OS, rate, physical routing, requested
and granted buffer, actual callback spans and raw observations. Keep failed
trials and listening reports separate from transport counters. See
[the loopback guide](tools/hal-loopback/README.md) for both latency reference planes.

Store new local experiments under `build/trials/`; curate small regression
fixtures deliberately instead of committing whole build trees. Preserve source
and hash provenance. `make test-fixtures` checks the retained fixture bytes.
Tests must work without an adjacent copy of the research archive.

Update status and user instructions with behavior changes. Add meaningful
regressions for timing, ownership, lifecycle or deployment bugs. `VERSION` is
the public version; content identity includes runtime/build inputs and dirty
files. Do not reuse an old build's audio qualification for a new binary.

## Releases and licensing

A public release needs live install/update/remove and audio tests plus appropriate
signing/notarization. `make package` does not publish or submit to Apple.
See [packaging](docs/PACKAGING.md). Intel support requires a separate implementation
and test decision; the current target is arm64.

Project code is MIT. Preserve the license and copyright notices. Reference
projects have their own licenses; see [provenance](docs/PROVENANCE.md) before
copying external code. Do not upload commercial music or private sessions as
playback demonstrations.
