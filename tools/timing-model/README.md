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

## Completion-delivery investigation

The [completion study](../../docs/COMPLETION-STUDY.md) links retained worker
timestamps, varies feedback capacity and serialized work, and compares shorter
transfers at unchanged sample offsets. Both Rust commands below run entirely
offline and require no private traces:

```bash
mkdir -p build/validation/completion-study
cargo test --release --locked --offline -p ftu-stream --example timing_model
cargo test --release --locked --offline -p ftu-stream --example placement_model
cargo run --release --locked --offline -p ftu-stream --example timing_model -- --completion-study > build/validation/completion-study/transport.json
cargo run --release --locked --offline -p ftu-stream --example placement_model -- --completion-study > build/validation/completion-study/placement.json
python3 -B -m unittest discover -s tools/timing-model -p 'test_*.py'
```

The transport study has 90 rows / 5,280 phase/scenario runs; the placement
study has 105 rows / 840 runs. Expected failures are retained. The transport
report separates `transport_completed` from `out_screen_passed` at the
16 ms comparison threshold and reports exact model lead beside bus-frame lead.
Only completed runs can pass the screen; extrema from failed runs may precede
the failure. Elapsed work allowances are not measured CPU time or OS bounds.
The placement report measures missing PCM separately and is not gated by that
transport screen.

The new transport mode interrupts simulated work when an owner stall begins.
The historical no-argument mode preserves its boundary-only stall convention
and original output, including that limitation. Use the new mode for the
completion investigation; do not treat the historical sweep as a model of
preemption inside submission work.

For the retained development-Mac logs only:

```bash
python3 -B tools/timing-model/completion_evidence.py --expected-build dev-8387395854108015 build/trials/2026-10-18-long-session/01-soak-64/worker.log build/trials/2026-10-18-long-session/02-soak-512/worker.log > build/validation/completion-study/linked-evidence.json
```

This read-only analyzer requires the version-1, four-slot/8 ms worker schema,
matching build and epoch, coherent recorder banks and clean final stop. It
rejects duplicate epochs rather than double-counting observations. It retains
unavailable linked intervals as null and log hashes for provenance. Detailed
retained records are biased windows, not a full timing distribution. Its
queue-horizon counterfactual is arithmetic, not a new hardware measurement.

The original investigation evidence and review are kept separately under
`build/validation/completion-study-20261018/`; reproduction commands above
write to a different directory.
