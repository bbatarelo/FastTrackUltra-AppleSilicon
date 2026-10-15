# Build and distribution infrastructure

The source workflow follows E-MU: `make`, `make install`, `make check`, and
`make uninstall`. This project also builds native macOS Installer packages so
recipients do not need a compiler, Rust, Python or Terminal.

**Current state:** local unsigned development packages build and can be
extracted/validated. The shared deployer passes temporary-root tests. The
new package path has not yet been installed on a live system, notarized or
published. The existing installed audio driver was left in place while this
infrastructure was developed.

## Local packages

```sh
make
make test-install
make package
```

Artifacts are placed in `build/packages/`:

- `FastTrackUltra-<version>-<build-id>-install.pkg`
- `FastTrackUltra-<version>-<build-id>-uninstall.pkg`
- `FastTrackUltra-<version>-<build-id>-manifest.json`

Both device models share the installer. Its welcome text states that Ultra 8R
is untested and that current playback operates at 48 kHz. The distribution
requires arm64 and macOS 14 or later; the scripts also reject installations to
another volume. The minimum OS target is not a tested compatibility matrix.

`VERSION` supplies bundle/package versions. `scripts/build_identity.py`
produces `build/build_identity.h`, `build/Info.plist` and a source-hash manifest.
The content identity includes build inputs and dirty source; a build-system
change can therefore produce a new identity without changing audio logic.
The packaged binary carries the same identity in its metadata and CoreAudio
property. Debug symbols are kept outside the driver bundle.
The root MIT `LICENSE` is included in the driver bundle and both packages.

`package.py` builds a scripts-only component using `pkgbuild --nopayload`, then
wraps it with `productbuild`. The signed driver is included alongside the
postinstall script, which invokes the shared deployer. This permits staging,
backup and rollback before the HAL directory changes; macOS Installer never
copies a partially built driver directly into HAL. A separate scripts-only
package removes the driver. Receipts do not inventory the deployed bundle;
use the supplied uninstaller rather than treating `pkgutil --forget` as removal.

The builder extracts each resulting product with `pkgutil --expand-full`,
parses its Distribution XML, checks the exact deployment script, and validates
the extracted driver signature/architecture. The manifest includes source
hashes, packaged executable hash, deployer hash and package hashes. Package
construction does not require root and does not touch CoreAudio.

## Deployment and backups

`scripts/deploy_hal.sh` is the single implementation behind `make install`,
`make uninstall`, the packages and the compatibility Python entry point.
It uses only macOS system tools on the receiving computer.

For installation it validates identity, arm64 code and signature; rejects
symlink/special-file bundles and symlink destination components; serializes
changes with a lock; stages outside HAL; fixes ownership/modes; and validates
again before moving the previous driver into a timestamped backup directory.
If replacement fails, the previous bundle is restored. A completed replacement
is not rolled back merely because an audio restart fails; the error explains
that the files changed and a restart is still needed.

Destination:
`/Library/Audio/Plug-Ins/HAL/FastTrackUltra.driver`

Backups:
`/Library/Audio/Plug-Ins/FastTrackUltra-Backups/`

Backups are retained on upgrade and removal. To restore one, use the common
deployer with the exact backup path as the source; it backs up the current
bundle in turn. This is a maintainer operation, not an automatic recovery claim.
The legacy command remains supported:

```sh
sudo python3 scripts/manage_hal.py install --bundle /absolute/path/to/FastTrackUltra.driver --restart
```

The `--root` option of the shell deployer exists for isolated filesystem tests.
It must point to an existing test directory and cannot be combined with
`--restart`. Tests never use `/` or administrator rights. Fault-injection tests
force a final rename failure and verify that the real rollback trap restores
the previous signed bundle. Other tests exercise wrong identity, tampering,
symlinks, locking, removal, backups and preserving another driver's files.

## A future public release

An ordinary local build uses an ad-hoc driver signature and unsigned packages.
Do not present those artifacts as a frictionless public download or advise
users to lower macOS security settings.

Apple describes the distribution process in
[Packaging Mac software for distribution](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution)
and [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).
A public package needs appropriate Developer ID code and installer signatures,
notarization and installation testing on the supported systems.

When the maintainer has the appropriate certificates, the build accepts explicit
identities; it never selects one silently:

```sh
SIGN_IDENTITY='Developer ID Application: ...' \
INSTALLER_SIGN_IDENTITY='Developer ID Installer: ...' make package
```

The staged driver is signed with hardened runtime and a secure timestamp;
`productbuild` signs both products. This signing path is implemented but was
not exercised with a Developer ID certificate during this change. The builder
does not submit anything to Apple. Notarization, stapling, Gatekeeper assessment,
fresh-Mac install/update/remove testing and loaded-build/audio verification are
remaining release steps. Signatures alone do not establish notarization.

Before publication, select a version, retain
the package manifest and test evidence, document the qualification limits, and
obtain the repository owner's approval to publish. No release tag, GitHub
upload, certificate changes or E-MU edits are part of `make package`.
