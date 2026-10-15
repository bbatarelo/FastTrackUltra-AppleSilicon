# Installing and using Fast Track Ultra

This is an early development driver for Apple Silicon Macs. Start with the
[current status](STATUS.md): desktop Ultra playback is confirmed at 48 kHz;
Ultra 8R is a joint target but is not hardware-qualified.

## Packages: no build tools needed

Maintainers can run `make package` to create an install package and a matching
uninstall package in `build/packages/`. Open the **install** package in Finder
and follow macOS Installer. It requests an administrator password. Quit audio
applications first because installing or removing the driver restarts CoreAudio.

The current local artifacts are **unsigned development packages**, and the
packaged install path has not yet been exercised on the live system. They are
not a notarized public release. Do not disable Gatekeeper or SIP to run a
package. A signed, notarized and tested release is a separate publication step;
see [packaging](PACKAGING.md).

After installation:

1. Connect USB and turn on the interface's external power supply.
2. Open **System Settings → Sound** and select **Fast Track Ultra** for output.
3. Turn monitor volume down, then play audio and raise it gradually.
4. Use **48 kHz** in your DAW. The driver currently offers no other native rate.

No separate driver download is needed for Ultra 8R, but do not treat its
inclusion in the bundle as evidence that it has been tested.

## Building from source

Use a native Apple Silicon terminal, not a terminal running under Rosetta.
Prerequisites are Xcode Command Line Tools, Rust/Cargo and Python 3. This build
has no third-party Rust dependencies and uses Cargo's offline mode. Ripgrep,
Homebrew, a paid developer certificate and a full Xcode installation are not
required by the build.

```sh
xcode-select --install
```

Install Rust using the instructions at [rustup](https://rustup.rs). Check Python
with `python3 --version`. Once the prerequisites are available:

```sh
cd FastTrackUltra-AppleSilicon
make doctor
make
make install
make check
```

Run `make` as yourself; only deployment requests administrator rights. Do not
run `sudo make`. `make install` rebuilds as needed, validates the bundle, stages
it outside the HAL directory, preserves the existing driver, and replaces it.
CoreAudio restarts afterward. Installation does not choose a default device.

`make` builds the driver and diagnostics. `make help` lists the commands.
`make version` shows the source version and content identity; `make check`
reports the identity actually loaded by CoreAudio. They can differ until you
install a new build. The version is maintained in the root `VERSION` file;
the content identity also distinguishes uncommitted code changes.

## Updating and removing

After obtaining updated source, run `make` and `make install` again. With a
prebuilt package, open the newer install package. Replaced bundles are kept in:

```text
/Library/Audio/Plug-Ins/FastTrackUltra-Backups/
```

For removal, use the matching uninstall package, or from the source folder:

```sh
make uninstall
```

This removes only `FastTrackUltra.driver`, keeps a backup and restarts audio.
Select another output afterward. Backups are never automatically deleted.
See [packaging](PACKAGING.md#deployment-and-backups) for developer recovery details.

## Troubleshooting

**No sound:** check the external power supply, selected output device, monitor
connections and main-output knob. Confirm the application is using the Ultra
at 48 kHz. Listening tests so far cover physical outputs 1 and 2.

**Device missing:** run `make check` if you have the source build. If it reports
no published device, check USB and power and reopen the audio application. Keep
the complete output when reporting the problem; do not assume that installing
files means the driver loaded successfully. Changing USB ports, sleep/wake and
automatic recovery remain unqualified, so report those events.

**An installer is blocked by macOS:** the current packages are local development
artifacts. Use a trusted signed release when available; do not reduce system
security settings. Source builds are currently the documented development path.

**An installation lock is reported:** another install may still be running.
Allow it to finish. If an earlier attempt was interrupted, keep its error output
and ask a maintainer to inspect `.FastTrackUltra-install.lock` in the Plug-Ins
folder before retrying; do not remove locks while an installer is active.

**Audio stops after a fault:** retain `make check` output. Automatic recovery is
not yet qualified. Close applications using the device before a deliberate
restart or reinstall. Reinstalling restarts audio for every application.

For a useful report, include the interface, Mac model, macOS and app versions,
loaded build identity, selected rate, connections and steps to reproduce. No
recorded audio is needed unless you choose to share it.
