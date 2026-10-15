# Extraction validation — 16 October 2026

The clean source was committed to a temporary local Git repository, cloned with
`--no-local` into a fresh directory and built/tested there. A macOS sandbox
explicitly denied reads beneath the original research repository. A deliberate
read probe failed as expected before any checks ran. No adjacent archive,
existing target directory or inherited build outputs were needed.

| Check | Result |
|---|---|
| `make` | Driver and diagnostics built |
| `make test` | 97 Rust and 120 Python tests passed, plus C/Rust ABI, profile and native callback checks |
| `make test-hal` | HAL boundary passed under AddressSanitizer/UndefinedBehaviorSanitizer |
| `make test-loopback` | Callback sanitizer checks and six analysis tests passed |
| `make test-install` | Ten temporary-root install/update/remove/package-definition tests passed |
| `make test-app` | Five application-audit regressions passed |
| `make test-fixtures` | All 38 retained fixture files matched archive hashes and sizes |
| `cargo clippy --workspace --all-targets --release --offline -- -D warnings` | Passed |
| `make package` | Install/uninstall packages built, extracted and validated |
| Production-source comparison | Rust, HAL, shared ABI and USB transport matched the final archive byte-for-byte |
| Documentation links | Local targets and pinned archive paths verified against the final archive tree |

The verified source builds content identity `dev-11c2d28e1cc0cf54`, version
`0.1.0`. It differs from the installed measured build `dev-57287986d44956d9`.
The differences in this extraction are fixture paths, test targets, documentation
and provenance; metadata/build inputs also changed during the preceding work.
See [MIGRATION.json](../MIGRATION.json) and [provenance](PROVENANCE.md).

Full local logs are retained in `build/validation/` after handoff and are ignored
by Git. Installer packages and their manifests are generated under
`build/packages/`. Generated binaries, packages and debug symbols do not belong
in the contributor repository's Git history.

These are offline checks. They do not install a driver, restart CoreAudio,
record inputs or perform a fresh hardware qualification. The previously
installed driver remains in place. Live installation/update/removal, listening
and loopback of a build from this repository remain release gates.
