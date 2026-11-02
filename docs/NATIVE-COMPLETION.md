# Native completion routing and shutdown

21 October 2026. **The native boundary is implemented and tested offline.**
It can dispatch IN and OUT completions on a dedicated macOS run loop while the
submitter is inside an IOUSBLib call, and it retains resources until callbacks,
consumer release, owner acknowledgement and thread exit are accounted for.
This closes the native-mechanics step after
[the independent completion study](INDEPENDENT-COMPLETION.md). It is an opt-in
prototype, not a new driver build or an installation candidate.

The next step described in this original report is now completed offline in
[the linked native/Rust integration](INTEGRATED-COMPLETION.md). Its evidence and
remaining live-adapter/hardware limits are recorded separately.

## Implemented boundary

`tools/ftu-probe/completion_route.{h,c}` accepts already prepared FTU interfaces,
LowLatency-compatible buffers and exclusive async sources. Endpoint 0 is IN81
on interface 2; endpoint 1 is OUT01 on interface 1. It performs no discovery,
open, alternate-setting change, rate write or device activation. The existing
`duplex.c`, pool ABI v4, live Rust runtime and normal build targets are unchanged.

The C layer is limited to OS mechanics and native lifetime. PCM preparation,
count order, sample-clock publication, scheduling, safety margins and admission
remain Rust responsibilities. The new complete/poll hooks are the boundary for
wiring that Rust implementation; this study uses an injected consumer and does
**not** claim that the two have already been connected into a live transport.

- A dedicated pthread attaches both sources to its own default run-loop mode,
  dispatches callbacks, retries consumer publication via a poll hook, and removes
  the sources on that same thread before exiting.
- Every IOUSBLib method call, including both directions' submissions and aborts,
  stays on the creating submitter thread. The completion thread dispatches
  notifications and consumes already completed buffers; it makes no interface
  method calls. It currently uses ordinary scheduling, an indefinite CFRunLoopRun and one
  repeating 1 ms timer allocated at startup to retry retained publication.
  This avoids repeatedly constructing finite run-loop waits. Neither the timer
  interval nor the owner wake notification establishes a measured wake bound or
  the Rust worker's real-time policy.
- Callback refcons are process-unique monotonically increasing integer tokens,
  never context or slot pointers. Tokens are atomically paired with slot state,
  never wrap, and cannot identify another generation or a later session. Frame
  identity must match before the consumer sees any data.
- Native state follows FREE → PENDING → CALLBACK → HELD → RETURNED → FREE.
  The slot is published before calling IOUSBLib, so a callback can arrive before
  the submit call returns. Release runs on the completion thread; ACK and reuse
  run on the submitter after the synchronous call returns. A returned token must
  be ACKed even for a synchronous rejection. Callback/error conflicts fault the
  session rather than overwriting either terminal outcome.
- A pointer to the event struct is valid only during the callback. The consumer
  may retain its immutable data/frame pointers until release, for example while
  count publication is backpressured. The owner cannot obtain writable storage
  during that interval. Wrong-thread, stale, duplicate and overflowing ACKs do
  not release another request.

The source must not already be scheduled elsewhere. The constructor checks the
creating thread's default/common modes and a process-local registry prevents two
routes from claiming the same sources/interfaces. Quarantine keeps those claims.
The registry uses a mutex only during creation/disposal, never during streaming;
it cannot discover arbitrary external source scheduling. It retains its own source references; the backend dispose hook owns
and eventually releases the original references and interface/buffer storage.

## Shutdown and quarantine

A stop request closes admission. An already admitted synchronous call may
finish; owner serialization prevents abort or ACK from reentering that call.
The owner then aborts each pipe once while the independent thread continues to
dispatch. Abort success alone never frees a slot. Stop consumes returned ACKs,
so a future Rust adapter must retire its count/ACK queues as part of stop.

Only when every slot is FREE may the dispatcher detach its sources and exit.
The owner observes exit, joins it, then permits backend disposal. Disposal
failure retains the remaining backend and route; it is not retried as if partial
cleanup were a fresh operation. A callback fault or abort error permanently
marks the session unfit for clean disposal.

A wait timeout retains **the live dispatcher, consumer context, interfaces,
sources and buffers**. A later matching abort completion or consumer release
can finish a clean drain on retry. If that proof never arrives, destroy remains
refused and storage stays retained for process lifetime. A blocked consumer
therefore cannot cause a use-after-free. Invalidated sources fault the session;
the startup-created timer keeps publication retry and source checking paced,
even with both input sources invalidated. A test bounds poll growth in that case.

`ever_timed_out` is historical. `poisoned` identifies permanent failure;
`quarantined` means permanent failure or an unfinished drain after timeout.
`joined` independently reports thread quiescence. A zero
millisecond stop poll is not a permanent fault. A wrong-thread orphan callback
has no safe context pointer and increments a global counter: conservatively,
all overlapping sessions then fail clean disposal. This prototype's tests use
separate processes for deliberate faults, including intentional retention.

Cross-thread stop callers must retire before destroy; the C handle is not
reference-counted. Rust must enforce that lifetime with its shared owner guard.
Hooks must be bounded, nonallocating and non-unwinding; they must not run nested
run loops that reenter the same Rust consumer.

The timeout bounds the polling wait, **not** a blocking native submit/AbortPipe,
thread startup or OS scheduling. Thread creation waits for readiness. Those
operations cannot be given real-time bounds by the offline fake.

## Reproducible offline tests

From the repository root:

```sh
make -f tools/ftu-probe/completion_route.mk test
make -f tools/ftu-probe/completion_route.mk OUT=build/native-completion-tsan SANITIZER=thread test
```

These targets build only a test executable. They do not discover USB devices,
start CoreAudio, create a driver bundle or install anything. The fixture injects
IOUSBInterfaceInterface300 function tables into the actual new C boundary. Its
sources use real CFMachPort messages and two real threads, with exactly one
completion per message and an explicit dropped-message case. The fake port has
capacity 128 so the 32-slot test is not bounded by the usual small port queue;
this does not establish the real IOUSBLib port's queue capacity. One additional
version-0 source control checks source scheduling/removal explicitly. The fake
emulates completed frame/data writes before invoking the normal IOAsyncCallback1
signature. Distinct per-packet patterns and nonuniform OUT lengths verify packed
prefix offsets; entries × 312 is only the slot capacity, not packet stride. It does not emulate the kernel's actual isochronous engine or decode
real IOKit completion messages.

**32 native routing/lifecycle cases pass at -O2 under Address/UndefinedBehavior
Sanitizer and Thread Sanitizer**, including 24,000 matched completions, releases
and ACKs per stress run. The legacy native adapter's sanitizer tests also pass.
Clang static analysis reports no diagnostics; the normal object compiles for
arm64 and x86_64 (only arm64 executes here).

The **201 existing Rust tests pass**, including the eleven joint-fixture tests
and six independent ownership/channel tests. Three pre-existing observer
benchmarks remain ignored. Formatting and workspace Clippy pass. All 870
retained cost/PCM/dispatch/capture rows match the prior sealed study, as do the
fixed-time replay groups and joint-screen result rows. No Rust or existing
production source was changed.

The cases cover IN/OUT routing, 8 ms/four-slot and 4 ms/ten/twelve-slot extents,
1 ms/32-slot extents, a 24,000-completion reuse stress, callback-before-return,
synchronous rejection, callback/error conflict, reject-then-late-callback,
backpressure, out-of-order ACKs, clean abort, missing/late abort callbacks,
missing release, blocked consumer, duplicate/stale/cross-session callbacks,
wrong thread/frame identity, source invalidation, stop admission, token
exhaustion, invalid extents, duplicate source ownership, zero-timeout polling,
lost messages, thread/timer creation failure and disposal failure.

Allocation wrappers count calloc/malloc/realloc/free calls made by the native
boundary translation unit. Snapshots taken after setup must remain unchanged through stream and drain;
clean disposal frees the route. Failed startup/duplicate-claim tests separately
check ownership returns to the caller. This excludes allocations inside
opaque CoreFoundation/IOKit functions, and is not a whole-process allocation
claim. The existing Rust fixture's allocation gate is unchanged.

Claude reviewed the initial source and a follow-up snapshot. The first review
led to live-dispatch retention on timeout, a zero-timeout regression, Mach-port
fixtures, buffer extents and explicit rejection/source-ownership contracts.
The follow-up also prompted the single startup timer, packed-packet patterns,
source claims, explicit permanent-fault telemetry and one-message/one-callback
fixture. It suggested verifying arg0; the local SDK explicitly documents the
frame-list pointer at IOUSBLib.h lines 3139 and 3210 and abort callbacks at 2439.
It also suggested TSan, already part of this suite. Review requests/responses and
a disposition record are preserved; final follow-up edits are tested here and
are not represented as a third Claude-reviewed snapshot.

## Unchanged acceptance and next decision

No buffer size, lead threshold or safety margin was changed. The prior model
still requires 48 kHz, IN/OUT safety 1024/2560, PCM offset 3072, 64 ms count replay,
16 integer bus frames of OUT lead, a 26-frame readiness cutoff, zero steady
capture/playback loss, count conservation, immutable pending PCM and clean drain
or retained ownership. Steady starts at 48,000 captured frames; the HAL disturbance
remains buffer + 832 frames. Worker policy remains 8/1/2 ms in that model.
The 32 ms absence and sustained 2 ms/call overload remain negative controls.

The unchanged twelve-slot comparison passes the 1,536-case main screen and
384-case uniform 5 ms delay screen; ten slots still fail OUT coverage. Compounded
callback delay remains rejected. These are deterministic model results,
**not timing measurements of this native thread**. Neither native sanitizer
success nor the twelve-slot comparison qualifies reduced safety margins.

**Next: connect this boundary to the existing Rust independent capture/clock,
bounded count and ACK handoffs, and joint submission scheduler in an isolated
integrated transport fixture.** Drive fake native completions through actual
Rust publication and shutdown, with the same loss/lead/negative gates. Include
out-of-order completions and backpressure across that full boundary. Then assess
whether an isolated hardware prototype is justified. Do not change normal
`make install` first.

Hardware still has to establish concurrent IOUSBLib submit/dispatch behavior,
actual abort/notification behavior, async-port queue capacity, OS wake and
processing costs, priority policy, and
first-start reliability. The SDK documents Mach-port/run-loop delivery, frame-list callback identity
and callbacks for aborted asynchronous requests. It does not promise the required
concurrency or scheduling bounds. The six-frame baseline RTL
shift and deferred reboot/startup failure remain separate open gates. The
under-10 ms analog RTL goal is unchanged; this step makes no latency reduction
claim. No hardware, audio, installation, restart or commit was performed.

Evidence: `build/validation/native-completion-offline-20261021/`, including final
commands, sanitizer/model logs, exact source, incremental patch, Claude reviews,
SDK excerpts and SHA-256 manifest. All prior evidence and the installed binary
are preserved.
