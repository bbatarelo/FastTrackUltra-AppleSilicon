# Diagnostics

| Tool | Purpose | Starts hardware audio? |
|---|---|---|
| `make check` / `build/hal-check` | Loaded build, device geometry and health | No, without explicit tone/silence options |
| `build/hal-scenarios` | Bounded buffer/read-back and multi-client experiments | Yes, silent streaming |
| [`hal-loopback`](hal-loopback/README.md) | Output 3 → selected input 3/4; raw/timestamp RTL and actual callbacks | Yes, explicit prepared run; saves selected input |
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
