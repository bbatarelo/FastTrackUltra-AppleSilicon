# Integrated native/Rust completion path

Follow-up: the [actual driver candidate](LIVE-COMPLETION-CANDIDATE.md) now implements
the live connection described as the next step below. This page retains the
original offline study and its bounded conclusions.

21 October 2026. **The native callback boundary is connected to the real Rust
capture, clock publication and playback-packing paths in an offline transport.**
The submitter and completion consumer run on separate real threads. The native
router uses actual Mach-port sources; only USB execution and scheduling time
are injected. Normal builds and the installed driver retain the earlier runtime.

This completes the integration step following [native routing](NATIVE-COMPLETION.md).
It does not establish IOUSBLib concurrency on hardware or qualify an installer.
The decision remains: reject ten-slot coverage, retain twelve slots as the
explicit experimental comparison, and preserve the compounded-delay failures.

## What runs together

`rust/ftu-stream/src/integrated_tests.rs` is enabled only by the
`native-integration` test feature. The runner links the unchanged
`completion_route.c` with `integration_native.c`, an injected IOUSBInterface300
backend. No discovery, interface open, rate write, driver factory activation or
CoreAudio call starts hardware. The fixture does not decode real IOKit messages.

- The owner prepares a Rust admission before native submission. The native
  token and Rust generation/key travel together in one return record. Both
  releases precede publication of that record; only the owner can ACK and reuse
  the slot after the synchronous call returns. The return queue holds 32 records
  for at most 24 slots, so a full queue is an invariant failure.
- Native callbacks reach Rust on the actual dedicated CFRunLoop thread. Rust
  snapshots the bounded completion record, retains the native buffer while it
  is HELD, copies packed S24 input, publishes ordered capture and observes USB
  timestamps through `SampleClock` and `AudioIo.clock`. Input counts become
  available to the submitter only after that publication. Count backpressure
  retains the slot and never republishes the same PCM/clock observation.
- A separate owner-side `AudioIo` stages and packs playback from the real tagged
  HAL ring. The two instances share atomic audio storage and telemetry, not
  mutable scratch buffers. Deadline ordering, bounded catch-up and the existing
  PCM-readiness policy choose eligible input/output submissions together.
- Shutdown marks Rust stopping before requesting native abort. It retires known
  completed buffers even with a full count queue. Successful native drain/join
  precedes destruction of the Rust callback context. Lost completion, native
  identity failure or a caught Rust panic retains both lifetimes. A late real
  callback can finish a timed-out drain; timeout alone never invents an ACK.

The test context is stable behind a raw pointer; only the completion thread
borrows its mutable consumer. The owner reads atomic summaries and bounded
channels. Summary version checks use atomic fields, with a physical liveness
watchdog rather than a fixed spin count. Rust panics are caught before the C ABI;
a fault freezes that context, so cleanup cannot continue through partially
mutated state. This is tested with unwind enabled; panic-abort builds terminate
instead and cannot promise recoverable quarantine.

## Timing fidelity and results

The virtual coordinator preserves the previous fixture's HAL-first tie order,
paid callback/copy/handoff/submission work, fixed owner absence and independent
completion progress. Each command and its barrier are one Mach message; at most
one command is outstanding. Abort messages do not modify command serials.
A separate real-timer test services Rust publication and slot return without
owner pumping. Neither test proves a physical scheduling or wake-time bound.

The earlier pure fixture remains byte-identical. Reference outcomes are asserted
in the linked tests, then compared with the sealed reference results:

| Screen | Ten slots | Twelve slots |
|---|---:|---:|
| Main phase/offset screen | 768/1536 accepted; minimum OUT lead 14 ms | 1536/1536; minimum 22 ms |
| Uniform 5 ms callback delay | 0/384; minimum 9 ms | 384/384; minimum 17 ms |
| Uniform 5 ms plus 5 ms reordering, cost screen | 0/12 | 0/12 |
| Correlated owner/completion absence, cost screen | 0/12 | 5/12; small buffers lose capture |

All main/uniform-delay prefixes retain zero steady input/output PCM loss;
rejected prefixes are not complete successful streams. The 32 ms absence and
sustained 2 ms-per-call saturation remain negative controls. The full cost matrix
retains its exact accepted/fault/missing-frame counts, including overload and
HAL disturbance. The unchanged 16 **integer USB bus-frame** output-return lead
is enforced separately from sample availability.

The complete release suite has **21 passing groups**; all 201 existing Rust tests also pass.
Seventeen focused groups pass with Rust+C TSan and with Rust ASan/C UBSan;
eighteen pass in the native ASan/UBSan build (including the cost matrix). Focused sanitizer runs
cover the actual C/Rust callback and ownership paths, initial clock publication,
count backpressure, synchronous and pre-admission rejection, physical callback
before submit return (including joint admission), generation reuse, stop during
work, device removal, missing/late aborts, and poisoned context retention.
Native-fed clock tests start with no HAL clock and recover the expected slopes
at -250, 0 and +250 ppm. The main timing comparison retains the reference's
pre-stream clock seed and requires capture to replace it.

A first Thread Sanitizer run found a publication race in the injected backend's
route handle. It is now atomic; the failure log is retained. Review also caught
abort classification before Rust's stop flag, missing exact result assertions,
pre-admission retirement and stale Cargo linkage of a rebuilt external archive.
The runner now explicitly rebuilds the Rust harness after compiling C and records
source hashes plus compiler versions. It uses isolated processes for intentional
retained-thread faults. No sanitizer finding is suppressed.

## Reproduce without hardware

From the repository root:

```sh
python3 tools/ftu-probe/test_integrated_completion.py --jobs 3
python3 tools/ftu-probe/test_integrated_completion.py --out build/integrated-tsan --sanitizer thread --quick --jobs 2
python3 tools/ftu-probe/test_integrated_completion.py --out build/integrated-rust-asan --sanitizer address --quick --jobs 2
python3 tools/ftu-probe/test_integrated_completion.py --out build/integrated-native-asan --sanitizer native-address --quick --jobs 2
```

The first command runs every timing and lifecycle test. `--quick` omits four
broad timing sweeps; it is used for focused sanitizer coverage, not as a
replacement for full release acceptance. `thread` instruments Rust, its rebuilt
standard library and C with TSan. `address` instruments Rust/standard-library
memory accesses and C undefined behavior. `native-address` instruments C with
Apple ASan/UBSan and links ordinary Rust. These complementary ASan builds avoid
mixing Apple's and Rust's incompatible ASan version guards; guards are never
aliased or disabled. Framework internals are not instrumented. Intentional
quarantine disables leak detection, not access/race checks.

Rust sanitizer builds need an installed nightly toolchain, `rust-src` and cached
standard-library build dependencies. Cargo runs with `--offline --locked`.
Normal source installation still needs only the stable prerequisites in README.
Rust allocations are checked separately on the owner and completion threads;
native wrappers audit allocation/free calls from the router translation unit.
Setup/disposal and opaque CF/IOKit allocations are outside that streaming claim.
The callback thread remains ordinary scheduling, not a qualified real-time policy.

## Acceptance and next step

Unchanged: 48 kHz; 4 ms / ten-slot reference and twelve-slot comparison; input/
output safety 1024/2560 frames; 64 ms count replay; PCM offset 3072; readiness
cutoff 26 ms; input start check at least 2 ms; output return lead at least 16
bus frames; zero steady loss from frame 48000; HAL disturbance buffer+832;
count order/conservation, immutable pending output and known ownership at free.

Next promote the now-linked consumer and owner lifecycle into an **opt-in live
adapter** using the real prepared interfaces/sources. Keep the default factory
unchanged, test start/stop/error propagation offline, then prepare one explicit
hardware candidate with matching rollback and telemetry. This study's harness
still supplies virtual service costs and is not that live adapter. Hardware must
verify actual IOUSBLib submission/dispatch concurrency, wake policy, drain and
physical latency before reducing margins. There is no further abstract-model
step required to establish the C-to-Rust handoff itself.

The six-frame analog RTL discrepancy and first-start-after-reboot failure stay
open. Reboot testing remains deferred by the owner. The current 16 ms output
lead alone prevents a below-10-ms round trip; this integration provides the
ownership foundation for later margin/transfer reductions, not that latency
result. Nothing was installed, played, restarted or committed in this step.
Evidence is sealed under `build/validation/integrated-completion-offline-20261021`.
