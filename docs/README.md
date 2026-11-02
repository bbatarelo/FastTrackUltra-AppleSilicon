# Documentation

| Guide | Start here when… |
|---|---|
| [Installation](INSTALLATION.md) | Building, installing, updating or removing the driver |
| [Status](STATUS.md) | Checking what is actually supported and tested |
| [Roadmap](ROADMAP.md) | Choosing implementation work, including MIDI/DSP/the companion app |
| [Contributing](../CONTRIBUTING.md) | Running tests, reporting issues or proposing changes |
| [Architecture](ARCHITECTURE.md) | Understanding Rust, native APIs, timing and ownership |
| [USB reconnect candidate](HOTPLUG-RECONNECT.md) | Reviewing port changes, safe session retirement and the pending physical reconnect test |
| [Disconnect retirement candidate](DISCONNECT-RECOVERY.md) | Reviewing safe drained removal, ftuQ shutdown evidence and pending hardware validation |
| [Disconnect quarantine investigation](DISCONNECT-QUARANTINE-20261030.md) | Understanding the silent device after reconnect, retained USB error and safe retirement work |
| [Failed recovery startup](FAILED-RESTART-RECOVERY.md) | Reviewing the retained-client silence fix, bounded retries and E-MU recovery comparison |
| [Runtime fault record and recovery](RUNTIME-RECOVERY.md) | Reading ftuE/ftuA, the bounded automatic recovery policy and what is still unproven on hardware |
| [E-MU reconnect audit](EMU-HOTPLUG-AUDIT.md) | Comparing port-move handling and tracking separate E-MU lifetime risks |
| [Timing geometry candidate](TIMING-GEOMETRY.md) | Reviewing the buffer/timestamp change, measured sweep and remaining latency work |
| [Timing observations](TIMING-OBSERVATION.md) | Understanding measured playback/capture margins and the first latency reduction |
| [USB queue model](QUEUE-MODEL.md) | Reviewing block-size/depth tradeoffs and the rejected three-slot experiment |
| [Worker timing](WORKER-TIMING.md) | Separating callback delivery, worker processing, preparation and submission |
| [Findings](FINDINGS.md) | Looking up measured protocol and HAL behavior |
| [GarageBand and loopback](LOOPBACK-QUALIFICATION.md) | Reading the reproducible 48 kHz hardware baseline |
| [Sample placement and safety reduction](PLACEMENT-MODEL.md) | Understanding physical latency, the independent model and the qualified four-slot 2560 reduction |
| [Longer sessions and app transitions](LONG-SESSION.md) | Reviewing the ten-minute duplex runs, GarageBand cycle/recording checks and preserved evidence |
| [Completion delivery study](COMPLETION-STUDY.md) | Reviewing linked timing evidence, shorter-transfer models and the next observation plan |
| [Completion-delivery observations](DELIVERY-OBSERVATION.md) | Reviewing the observation-only candidate, measurement semantics and completed hardware results |
| [CoreAudio restart investigation](STARTUP-RESTART.md) | Reviewing progressive restart stalls, AirPlay registration growth and the separate USB startup failure |
| [First USB failure evidence](STARTUP-EVIDENCE.md) | Reviewing retained packet evidence, the matching baseline failure and the next initialization experiment |
| [48 kHz startup candidate](STARTUP-48K.md) | Reviewing dock/direct first-boot failures and the subsequent successful warm recovery after power removal |
| [Startup sequence audit](STARTUP-AUDIT.md) | Comparing early bring-up with Linux, reviewing the reset experiment and choosing the next startup comparison |
| [All-pump candidate](ALL-PUMP-OBSERVATION.md) | Reviewing the original observation plan, startup rejection and later measurement outcome |
| [Shorter-transfer decision](SHORTER-TRANSFERS.md) | Reviewing the ten-minute 128-frame result, failed latency comparison and concrete path toward under 10 ms |
| [Ten-slot offline experiment](TEN-SLOT-PROTOTYPE.md) | Reviewing improved USB lead, failed late-HAL sample availability and the next preparation-window design |
| [PCM-readiness prototype](PCM-READY-PROTOTYPE.md) | Reviewing bounded staging, independent submission deadlines and the offline acceptance results |
| [Retry and combined-cost study](RETRY-COST-OBSERVATION.md) | Inspecting retry counters and combined submission-cost/stall acceptance results |
| [Bounded deadline submissions](DEADLINE-SUBMISSIONS.md) | Comparing deadline order, catch-up bounds and remaining offline rejections |
| [Callback dispatch model](CALLBACK-DISPATCH-MODEL.md) | Separating ready callback service, idle waits, callback costs and capture loss |
| [Capture delivery and owner availability](CAPTURE-DELIVERY.md) | Tracing missing samples, evaluating bounded publication and understanding the remaining rejection |
| [Fixed-time stall replay](FIXED-STALL-REPLAY.md) | Reviewing the capture-independence decision, output deadline limits and slot-return modelling |
| [Independent completion and joint scheduling](INDEPENDENT-COMPLETION.md) | Reviewing slot acknowledgements, concurrent ownership tests, ten-slot rejection and the twelve-slot comparison |
| [Native callback routing and shutdown](NATIVE-COMPLETION.md) | Reviewing real-thread Mach-port tests, slot lifetime and late aborts |
| [Experimental live driver](LIVE-COMPLETION-CANDIDATE.md) | Building and comparing the actual 4 ms/twelve-slot driver, with lifecycle tests and rollback |
| [Integrated native/Rust transport](INTEGRATED-COMPLETION.md) | Reviewing linked capture/clock/scheduling, exact timing comparisons, sanitizer scopes and the subsequent real-device candidate |
| [Run-loop control study](RUNLOOP-STUDY.md) | Separating whole-pump duration from synthetic message delivery and planning wait observations |
| [ASFireWire lessons](ASFW-REUSE.md) | Evaluating timing, ownership and measurement methods |
| [Diagnostics](../tools/README.md) | Choosing a read-only inspector or an explicit hardware experiment |
| [Packaging](PACKAGING.md) | Preparing installers and future signed/notarized releases |
| [Provenance and handoff](PROVENANCE.md) | Tracing this checkout to the frozen research source |
| [Extraction validation](VALIDATION.md) | Checking the independent build/test/package baseline |
| [Fixture inventory](../tests/fixtures/README.md) | Understanding the selected regression inputs |

Full bring-up diaries, reviews, source snapshots and experiments live in the
research archive. This repository keeps the current implementation and the
material needed to build, test and contribute without that archive present.
