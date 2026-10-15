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

The source workflow follows the E-MU driver. Install Xcode Command Line Tools,
Rust from [rustup](https://rustup.rs), and Python 3, then run from this checkout:

```sh
make
make install
make check
```

`make install` asks for an administrator password, backs up the previous driver,
and restarts CoreAudio. Quit audio applications first. Then select **Fast Track
Ultra** in System Settings → Sound or your application's audio settings. Start
with monitor volume low. Keep the interface's external power supply connected.

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

## History and license

This is the clean contributor repository extracted from the frozen
[M-Audio research archive](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon).
[Provenance](docs/PROVENANCE.md) records the exact source commit and fixture
hashes. The archive is not needed to build or test this project.

The [E-MU project](https://github.com/bbatarelo/emu-apple-silicon) informed the
workflow; [ASFireWire lessons](docs/ASFW-REUSE.md) informed timing and measurement.
No E-MU or ASFW implementation was copied into the driver.

MIT — [LICENSE](LICENSE). Copyright © 2026 Bruno Batarelo.
