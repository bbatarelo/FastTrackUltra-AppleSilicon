# Fast Track Ultra / Ultra 8R for Apple Silicon

An independent userspace macOS driver for the M-Audio Fast Track Ultra family.
One codebase supports both models, with a Rust audio engine and a thin native
CoreAudio/USB boundary. It installs no kernel or system extension and requires
no SIP or startup-security changes.

**Early development.** Desktop Ultra playback and recording have worked at
48 kHz, including a short GarageBand test. Monitoring latency is still high
(about 111–116 ms). Ultra 8R hardware and MIDI are not yet qualified.
See [current status](docs/STATUS.md) before relying on it for a session.

## Compatibility

| Requirement | Current scope |
|---|---|
| Mac | Apple Silicon (arm64); Intel builds are not supported |
| macOS | Builds target 14 or later; hardware tested on 26.7.1 |
| Fast Track Ultra | Outputs 1/2 heard cleanly; output 3 → input 4 loopback measured |
| Ultra 8R | Recognized by the shared driver; hardware testing needed |
| Format | Eight inputs and outputs, 48 kHz only; other rates are roadmap work |

## Install

Open **Terminal** on your Apple Silicon Mac (without Rosetta). Paste each block
below in order, waiting for it to finish before continuing. These are one-time
prerequisites; you can skip an installer if that tool is already set up.

**1. Install Apple's command line tools:**

```sh
xcode-select --install
```

Click **Install** in the macOS dialog and wait for installation to finish.
If Terminal says the tools are already installed, continue.

**2. Install Rust and Cargo using [rustup](https://rust-lang.org/tools/install/):**

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal &&
. "$HOME/.cargo/env"
```

**3. Install Python using the official [Python 3.13 installer](https://www.python.org/downloads/release/python-31316/):**

```sh
ftu_python_dir="$(mktemp -d)" &&
curl --proto '=https' --tlsv1.2 -fL \
  https://www.python.org/ftp/python/3.13.16/python-3.13.16-macos11.pkg \
  -o "$ftu_python_dir/python.pkg" &&
sudo /usr/sbin/installer -pkg "$ftu_python_dir/python.pkg" -target / &&
export PATH="/Library/Frameworks/Python.framework/Versions/3.13/bin:$PATH"
```

Enter your Mac login password when asked; Terminal does not display it as you
type. The Rust and Python blocks make the tools available in this same Terminal
window. No Homebrew, `pip` packages or manual shell-file edits are needed to
build and install the driver.

**4. Build and install the driver:**

In Terminal, enter the project folder. You can type `cd ` (including the space),
drag the folder from Finder into Terminal, and press Return. Quit audio
applications, then paste:

```sh
make install
```

This command checks the prerequisites, compiles and validates the driver, then
asks for an administrator password to install it. A separate `make` step is
optional. Do not run `sudo make install`.

Installation backs up the previous driver and restarts CoreAudio. Select
**Fast Track Ultra** in System Settings → Sound or your application's audio
settings. Start with monitor volume low. Keep the interface's external power
supply connected. To check that CoreAudio loaded the driver, run:

```sh
make check
```

To remove the driver:

```sh
make uninstall
```

For nontechnical users, maintainers can build double-clickable install and
uninstall packages using `make package`. Current packages are unsigned development
artifacts; there is no signed/notarized public release. The new package deployment
path still needs a live-system trial. See [installation](docs/INSTALLATION.md)
for setup, removal and troubleshooting, and [packaging](docs/PACKAGING.md)
for release preparation.

## What works and what comes next

VLC and GarageBand playback on outputs 1/2 were confirmed clean; the owner also
reported Spotify working. GarageBand recorded a physical input-4 return.
Six loopback sessions used actual 64/128/192-frame buffers. These are short
baseline results from the archived installed build, not qualification of every
channel, device model, Mac or new build from this repository.

The 16 October [timing build](docs/TIMING-GEOMETRY.md) additionally passed physical
loopback at actual 64–4096-frame buffers, clean left/right listening, and silent
two-client checks. The 17 October [output-safety reduction](docs/TIMING-OBSERVATION.md)
measured **89.2 ms round trip at 64 frames**, down from 110.5 ms. All twelve
chirp runs across six buffer sizes passed. Monitoring latency is still high;
these are short measurements on one desktop Ultra. See [current status](docs/STATUS.md)
for exact build identities and remaining limits.

The [roadmap](docs/ROADMAP.md) covers lower latency and timing, reliability,
MIDI input/output, a diagnostics/routing/mixer app, onboard DSP, other rates,
clock/S/PDIF modes, Ultra 8R and distribution. The companion app will remain
separate from realtime streaming; audio should work while it is closed.

## Contribute

Start with [CONTRIBUTING.md](CONTRIBUTING.md). Reports from another Mac or an
Ultra 8R are useful: include the model, macOS/app versions, physical wiring,
what you tried and heard, and `make check` output. Redact personal information.

| Guide | Purpose |
|---|---|
| [Installation](docs/INSTALLATION.md) | Build, install, update, remove and troubleshoot |
| [Status](docs/STATUS.md) | Verified behavior and limits |
| [Roadmap](docs/ROADMAP.md) | Development priorities and acceptance criteria |
| [Architecture](docs/ARCHITECTURE.md) | Rust/native boundaries and ownership |
| [Baseline](docs/LOOPBACK-QUALIFICATION.md) | GarageBand, buffers and measured latency |
| [Documentation](docs/README.md) | Findings, diagnostics and source provenance |

## License

The [E-MU project](https://github.com/bbatarelo/emu-apple-silicon) informed the
workflow; [ASFireWire lessons](docs/ASFW-REUSE.md) informed timing and measurement.
No E-MU or ASFW implementation was copied into the driver.

MIT — [LICENSE](LICENSE). Copyright © 2026 Bruno Batarelo.
