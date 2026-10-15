# Provenance and repository handoff

Extracted on 16 October 2026 from the frozen M-Audio research repository.

- Archive: https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon
- Final archive commit: `9c87ae0fc82fbe995641436e4841a720c519fee0`.
- Local archive folder: `M-Audio-FastTrackUltra`, adjacent to this checkout.
- Contributor folder: `FastTrackUltra-AppleSilicon`, fresh Git history.
- [MIGRATION.json](../MIGRATION.json) inventories copied source hashes and changes.
- [Fixture manifest](../tests/fixtures/manifest.json) records exact archive paths,
  SHA-256 hashes and sizes for the curated test inputs.

The archive's final commit is local until the owner pushes it; pinned archive
links may not resolve remotely until then. An adjacent archive is useful for
research but is not a build, test, package or runtime dependency. No remote is
assumed for the new repository and no publication is part of this extraction.

## Scope

The Rust crates, HAL implementation, shared ABI and native transport were copied
byte-for-byte. Changes in the clean repository concern contributor documentation,
fixture locations, test targets and provenance. The full history, repeated
binaries/source snapshots, debug symbols, reviews and raw experimental sessions
remain in the archive. The selected legacy inspector binary is inert test data
whose bytes are verified, never run by the regression suite.

The installed measured build is `dev-57287986d44956d9`, with executable SHA-256
`1be5a06cc93e4c8fdfc950eaa3f688171fd7d769a3558d160e324290387c8af5`.
Build/install metadata changes mean a new checkout may have a different content
identity despite unchanged engine logic. Offline checks do not transfer the
installed binary's hardware qualification. Repeat loaded identity, listening,
loopback and clean-stop checks before qualifying a new installed build.

## References and licensing

Project code is MIT; see [LICENSE](../LICENSE). No E-MU, Linux or ASFireWire
implementation code is linked/copied into this driver. Their source was used
as a protocol, architecture or measurement reference and keeps its own terms.

- E-MU clean project: `5921d33cec07f80c4f8e0e4c158e0f65c795779f`; workflow,
  CoreAudio/USB design and measurement comparison. No E-MU files were changed.
- Linux USB audio: `4056b3c9d3c92ea45955641dbafeb9b960e146ef`; profiles,
  packet/feedback associations and control references. GPL material stays in
  the research cache and is not vendored in this checkout.
- ASFireWire: `0b8a23a65e0d6c35f3f59c913d7402a39c172356`; Apache-2.0/NOTICE.
  [Specific lessons](ASFW-REUSE.md) cover timestamps, latency, geometry and
  ownership. Any later code reuse must retain the applicable notices.
- Apple SDK headers informed the native ABI and lifecycle requirements; the
  checkout uses the installed SDK rather than redistributing it.

The [full research provenance](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/provenance.md) records original inputs,
URLs, revisions and review history. The
[E-MU backport assessment](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/EMU-BACKPORT-CANDIDATES.md) is preserved for
later separate work. User-supplied blueprints and third-party driver/reference
caches were not copied to this clean repository.
