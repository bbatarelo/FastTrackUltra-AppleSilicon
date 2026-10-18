# Diagnostics

| Tool | Purpose | Starts hardware audio? |
|---|---|---|
| `make check` / `build/hal-check` | Loaded build, device geometry and health | No, without explicit tone/silence options |
| [`build/hal-check --worker-timing`](../docs/WORKER-TIMING.md) | Read coherent, sample-free USB-worker stage records when `ftuW` is available | No |
| [`build/hal-check --delivery`](../docs/DELIVERY-OBSERVATION.md) | Read sample-free completion histograms, callback/pump links and worker CPU/policy observations when `ftuD` is available | No |
| `build/hal-check --timing` | Read-only playback/capture margins when the installed build exposes `ftuT` | No |
| `build/hal-scenarios` | Bounded buffer/read-back and multi-client experiments | Yes, silent streaming |
| [`hal-loopback`](hal-loopback/README.md) | Output 3 → selected input 3/4; raw/timestamp RTL and actual callbacks | Yes, explicit prepared run; saves selected input |
| [`build/hal-soak`](hal-loopback/README.md#extended-sessions-and-application-transitions) | Opt-in duplex measurements bounded to ten minutes | Yes, explicit prepared run; saves selected input |
| [`build/hal-observe`](hal-loopback/README.md#extended-sessions-and-application-transitions) | Continuously sample driver status/timing around application transitions | No |
| [Offline timing model](timing-model/README.md) | USB block/depth, callback/stall, feedback-capacity and deadline experiments | No |
| [`ftu-probe`](ftu-probe/README.md) | Descriptors, controls and bounded raw-USB experiments | Depends on the explicit subcommand |
| `build/live-check` | Standalone Rust live-session bring-up | Yes; direct USB ownership |
| `build/hal-host-check` | Standalone host for a local HAL bundle | Yes; direct USB ownership through the bundle |

Never run raw USB probes, live-check or hal-host-check while the installed HAL
may own the device. External CoreAudio tools are the normal choice with an
installed driver. None of these tools should change system default devices as
a side effect of inspection.

Start with loaded identity and status, then choose a bounded experiment. Keep
new local captures in `build/trials/`, retain failures and record physical wiring,
rate, actual buffers, permissions and tool hashes. Follow the relevant command
guide before sending audio or recording inputs. Source-level API/CLI options
are authoritative for each development tool; old experiment references link
to the frozen archive, not an active second driver branch.

For bounded ten-minute physical sessions and continuously sampled GarageBand
transitions, see the [extended loopback tools](hal-loopback/README.md#extended-sessions-and-application-transitions).
These are external clients/observers; building them does not replace the driver.

The [standalone run-loop study](runloop-study/README.md) compares two-port Mach
message delivery with normal/requested realtime scheduling and optional wait
observers. It does not access USB, audio or the driver. Its results are a control
experiment, not hardware qualification.
