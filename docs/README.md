# Documentation

| Guide | Start here when… |
|---|---|
| [Installation](INSTALLATION.md) | Building, installing, updating or removing the driver |
| [Status](STATUS.md) | Checking what is actually supported and tested |
| [Roadmap](ROADMAP.md) | Choosing implementation work, including MIDI/DSP/the companion app |
| [Contributing](../CONTRIBUTING.md) | Running tests, reporting issues or proposing changes |
| [Architecture](ARCHITECTURE.md) | Understanding Rust, native APIs, timing and ownership |
| [Timing geometry candidate](TIMING-GEOMETRY.md) | Reviewing the buffer/timestamp change, measured sweep and remaining latency work |
| [Timing observations](TIMING-OBSERVATION.md) | Understanding measured playback/capture margins and the first latency reduction |
| [USB queue model](QUEUE-MODEL.md) | Reviewing block-size/depth tradeoffs and the rejected three-slot experiment |
| [Worker timing](WORKER-TIMING.md) | Separating callback delivery, worker processing, preparation and submission |
| [Findings](FINDINGS.md) | Looking up measured protocol and HAL behavior |
| [GarageBand and loopback](LOOPBACK-QUALIFICATION.md) | Reading the reproducible 48 kHz hardware baseline |
| [Sample placement and safety reduction](PLACEMENT-MODEL.md) | Understanding physical latency, the independent model and the qualified four-slot 2560 reduction |
| [ASFireWire lessons](ASFW-REUSE.md) | Evaluating timing, ownership and measurement methods |
| [Diagnostics](../tools/README.md) | Choosing a read-only inspector or an explicit hardware experiment |
| [Packaging](PACKAGING.md) | Preparing installers and future signed/notarized releases |
| [Provenance and handoff](PROVENANCE.md) | Tracing this checkout to the frozen research source |
| [Extraction validation](VALIDATION.md) | Checking the independent build/test/package baseline |
| [Fixture inventory](../tests/fixtures/README.md) | Understanding the selected regression inputs |

Full bring-up diaries, reviews, source snapshots and experiments live in the
research archive. This repository keeps the current implementation and the
material needed to build, test and contribute without that archive present.
