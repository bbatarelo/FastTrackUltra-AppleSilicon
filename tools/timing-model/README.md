# Offline USB timing model

Run from the repository root; no device, microphone access or administrator
permission is needed:

```bash
cargo test --release --locked --offline -p ftu-stream --example timing_model
mkdir -p build/validation/queue-model
cargo run --release --locked --offline -p ftu-stream --example timing_model > build/validation/queue-model/model.json
```

The implementation is [a standalone Rust example](../../rust/ftu-stream/examples/timing_model.rs).
It emits JSON for 1/2/4/8 ms blocks, depths 2/3/4/6/8, and four explicit
callback/stall/submission-cost scenarios, across two block periods of stall phase.
The fixed test inputs and scope are described in [the queue decision](../../docs/QUEUE-MODEL.md).
Changing the example does not change the running driver or install anything.

Each row retains `phases`, `passed`, failure `reasons`, minimum submission lead,
minimum/maximum playback-preparation versus capture-cursor distance, maximum
capture-publication age, and maximum feedback-history occupancy. Unobserved
lead values are null. Extrema can include progress made before a failed phase;
only `passed == phases` means that whole scenario completed. A zero capture age
on an early failure means it never reached the measurement window.

`min_submit_ms` includes both USB directions. Successful rows conserve all
captured/replayed packet counts and leave no pending flights or feedback entries.
The standalone model intentionally allocates; realtime allocation regressions
exercise the production pool separately. Model success does not prove PCM
coverage or hardware latency. The chosen stress values are not measured OS
bounds, and the synthetic packet-count pattern is not a recording from a device.

## End-to-end sample placement

`placement_model` is a separate nominal-sample model with HAL publication,
ring tags, USB service and physical marker return. Run it with:

```bash
cargo test --release --locked --offline -p ftu-stream --example placement_model
cargo run --release --locked --offline -p ftu-stream --example placement_model > build/placement-model.json
```

See [scope, results and candidate gates](../../docs/PLACEMENT-MODEL.md).
The independent model does not exercise the production pool or emulate macOS.
`placement_evidence.py` separately verifies the preserved hardware callback
coordinate identity and computes the explicitly limited margin screen. It needs
the ignored measurement artifacts on the development Mac; the Rust models and
production tests run without those recordings or any device.
