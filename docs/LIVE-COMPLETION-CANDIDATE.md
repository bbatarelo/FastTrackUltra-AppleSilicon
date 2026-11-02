# Experimental 4 ms driver candidate

21 October 2026. The independent completion path is now wired into an actual
HAL driver build. Its first hardware start failed before USB configuration;
the corrected build subsequently passed its first three short hardware runs. “Opt-in live adapter” meant this explicitly selected test build,
not another application or a setting the user would have to configure.

## What changed

`independent-4ms` uses 4 ms transfers and twelve slots per direction. The USB
owner still opens/configures the interfaces and makes every IOUSBLib submission
and abort call. After the established 48 kHz rate write/readback, the native
bridge moves the two async sources to a dedicated CFRunLoop thread. Rust on
that thread copies capture PCM, publishes it in order, updates the device clock,
and returns count feedback plus generation-specific slot acknowledgements.
The owner uses those counts for deadline-ordered IN/OUT scheduling, with up to
ten submissions per round and bounded PCM-readiness deferral. Idle owner waits
are one millisecond; completion dispatch uses its own run loop and a one
millisecond retry timer. Neither interval is an OS scheduling guarantee.

Setup allocations and dispatcher creation happen before applying the owner's
8/1/2 ms time-constraint policy. Rust explicitly selects user-interactive QoS
for the completion thread, applied by the thin C boundary before startup is
reported ready. It is not a second time-constraint scheduling reservation.

Stop closes admission, aborts on the owner, drains callbacks, joins the dispatcher,
retires matching Rust generations and restores the original interface settings.
A lost callback has a bus-clock watchdog (64 ms beyond the scheduled start).
Uncertain ownership or a caught panic retains the native and Rust contexts.
The existing terminal quarantine prevents restarting that HAL handle. Clock
invalidation is performed once by the completion thread, then by the owner only
after a successful join. Capture and output have separate scratch storage and
share only the existing atomic audio timelines; neither AudioIo has a Drop hook.

The native bridge rejects legacy buffer/submission/cleanup/close/rate operations
once ownership has moved to the route. Failed attach restores sources to the
original owner. Failed disposal is terminal and cannot be retried against released
resources. Callback-before-submit-return still cannot enable premature slot reuse.

## Bounds retained

- 48 kHz, 64 ms count replay and PCM offset 3072.
- Input/output safety 1024/2560 frames.
- IN checked at least two integer bus frames before start, and submission returns
  before its start; OUT returns at least sixteen bus frames before start.
- PCM-readiness cutoff 26 ms; pending OUT buffers are immutable.
- Zero steady PCM loss from frame 48000; count order and ownership remain strict.

The 16 ms OUT lead alone prevents sub-10-ms round trip. This candidate establishes
real-device behavior of the new transport before later margin reductions. The
previous six-frame latency discrepancy and first-start-after-reboot fault remain
open. A controlled experimental comparison is authorized; it is not a release
qualification or a claim that either issue has been resolved. Reboot testing
remains deferred.

## Offline checks

The production `Owner` and `Completer` are exercised with actual native callback
routing and injected USB interfaces, including normal 64/128-frame HAL processing,
a 21 ms owner stall, native clock slopes, packet faults/removal, synchronous
rejection, callback-before-return, lost aborts, late drain, callback panic,
missing-completion watchdog and retained first packet-failure evidence.
Fourteen groups pass normally, with Rust+C ThreadSanitizer, and with Rust
AddressSanitizer plus C UndefinedBehaviorSanitizer. The realtime scopes check
Rust allocations; native-owned allocations are instrumented separately. Opaque
CoreFoundation/IOKit internals are outside that allocation claim.

Five additional native live-bridge cases verify source migration, real event
conversion, QoS startup, abort drain, attach/policy failure and terminal cleanup
failure under C address/undefined and thread sanitizers. The existing 32 native
route cases and 201 Rust regression tests also pass. These linked tests check
mechanisms; the earlier paid-cost/stall model screens keep their original scope
and are not newly claimed as exhaustive simulations of this live worker.

Existing HAL PCM/missing-frame, retry, output lead, startup and first-failure
properties remain available. Output lead is measured using bus-frame reads on
both sides of the real submission. Owner-pump observations are not evidence for
this profile: its callbacks run on a different thread, so the old pump/delivery
observer records are not populated by the new streaming path.

## Build and hardware comparison

```sh
make independent-driver
make test-independent
```

The bundle is `build/independent-4ms-twelve-slot/FastTrackUltra.driver`.
Ordinary `make install` still builds the 8 ms/four-slot baseline. For this trial,
use the frozen install/rollback scripts in
`build/validation/live-completion-20261021/`; they verify the exact binary and
helper before deployment. Installation preserves the previous driver and
restarts CoreAudio. No automatic Mac reboot is part of this trial.

First retain a same-session 128-frame baseline: silence preflight, then two
short OUT3 → rear IN4 loopbacks. Install the frozen candidate, verify its identity,
then repeat silence and the same loopbacks at actual 128 frames. Record raw and
timestamp-relative latency separately, startup versus steady misses, output
lead, driver faults and teardown. A failure stops the progression and retains
logs; rollback restores the exact prior binary. Only a clean short comparison
advances to 64-frame/longer sessions and a separately confirmed listening check.
No hardware pass or latency improvement is claimed by the offline build.

## Fresh baseline retained before installation

The installed `dev-673cc272caa79014` passed an eight-second silence preflight
and two independent eight-second loopbacks at actual 128 frames: 9,000 callbacks,
four accepted chirps, zero steady PCM losses, no faults and clean teardown.
Raw round trip across the four chirps was 81.330642–81.330645 ms; timestamp-relative
round trip was 1.330642–1.330645 ms. Thus the later 81.331 ms measurement repeats
within this session; the difference from the older 81.206 ms baseline is still
unexplained. Startup playback misses were 2024, 1764 and 2004 respectively;
zero steady loss is not zero lifetime loss. Buffer size was restored to 512.
The original candidate was subsequently installed; its first-start result follows.


## First hardware attempt and startup correction

The installed `dev-901b05fda865ee4f` matched the frozen binary (SHA-256
`b6f6fe6294f75af33b68792bfff9fb3d61e8f7cae525e61b90bf404af88be39f`).
The eight-second, 128-frame silence preflight could not start: CoreAudio returned
2003329396, with zero callbacks and zero captured frames. The worker ended in
terminal quarantine, fault 11, epoch 1. The startup snapshot remained at stage
zero, before configuration, preparation, activation or rate writes. The test
restored the 512-frame buffer. No chirps or monitor listening tests followed;
no latency measurement exists for this candidate. Zero pending counters do not
establish successful native cleanup after this early panic.

The cause is a software integration error: the independent worker used
`PoolSession` for diagnostic/teardown reporting, but also called its legacy
ready-policy and deadline-order setters. Those setters deliberately accept the
4 ms/ten-slot geometry only. The twelve-slot live profile therefore panicked in
`with_output_policy` before publishing startup stage one. This is distinct from
the earlier first-IN USB overrun after reboot.

The correction leaves those legacy guards intact and initializes a reporting
session with the actual twelve-slot geometry and correct retry metadata. The
independent `Owner` retains the same readiness policy, joint order and timing
gates. Legacy scheduling setters are compiled for the default runtime and tests;
the independent production runtime does not use them.

The linked runner previously selected only the 14 Owner/Completer test groups.
It now also runs the real worker's startup-failure and control-cancellation
suites. The new selection reproduced the exact old assertion before the fix.
With the correction, all 16 groups pass normally, under Rust+C ThreadSanitizer,
and under Rust AddressSanitizer plus C UndefinedBehaviorSanitizer. The startup
suite exercises fifteen failures through the public HAL lifecycle, including
preconditions, preparation, activation, SET status/length and readback. Its
legacy fake context cannot be passed to the real native attach; the success
case remains in the separate native attach and Owner/Completer tests. These
checks do not substitute for a successful hardware start.

Replacement installation is prepared under
`build/validation/live-completion-startup-fix-20261021/install.sh`. It can replace
the stopped candidate directly; the owner chose to defer rollback. Its rollback
restores the exact known baseline `dev-673cc272caa79014`, not the failed candidate.
The original sealed candidate/evidence remain unchanged. New hardware failure,
red/green regression, sanitizer results and replacement source/bundle hashes
are retained in the new validation directory.

Repeat the silence preflight first, then the two short 128-frame loopbacks only
if it passes. All original acceptance criteria remain in force. The replacement
was built and checked offline before installation. The first hardware result follows.


## Corrected candidate: short 128-frame hardware comparison passed

On 21 October, installed `dev-d180f6efb5393f27` matched SHA-256
`48358f0eee994ff8d958935dd38a23e854e02300673a57c2d5bbd8ffec8f99ad`.
It passed an eight-second silence preflight and two independent eight-second
loopbacks, rear OUT3 → rear IN4, channel 4 Rear, with all other outputs zero.
All three runs used actual 128-frame callbacks at 48 kHz: 9,000 callbacks total,
four accepted chirps, zero steady input/output loss and clean teardown.

| Run | Raw round trip | Startup playback misses | Minimum OUT return lead: whole run / steady |
|---|---:|---:|---:|
| Silence | — | 1815 | 25 / 37 ms |
| Loopback A | 81.309811, 81.330644 ms | 1774 | 25 / 29 ms |
| Loopback B | 81.330645, 81.330646 ms | 1864 | 25 / 38 ms |

The four raw results differ from the fresh baseline median by at most 1.000017
frames, inside the unchanged ±2-frame comparison gate. One chirp returned about
one frame earlier; this is not a claimed latency reduction. Timestamp-relative
RTL was 1.309811–1.330646 ms, with a 3840-frame scheduling distance. Neither
that remainder nor these tests calibrate converter delay. Startup playback
misses remain nonzero; all steady loss counters stayed unchanged.

Every observed OUT return retained at least 25 ms, above the 16 ms gate;
minimum steady lead was 29 ms. Final startup stage 7/outcome 1 records both
48 kHz writes and readbacks. Final epoch 3 is idle, pending `[0,0]`, with no
fault, cleanup error or first-failure record, and the buffer restored to 512.
The final retry snapshot has no over-budget retry; its steady requests were
all ready with no deferrals/fallbacks. Old pump/delivery records remain outside
this runtime's observation scope. The existing hal-check binary lacked the
retry option; a freshly compiled copy of the unchanged diagnostic source read
that property successfully, without another audio start.

The decision is to advance this installed transport to short 64-frame tests
and longer duplex sessions before reducing margins. These three short starts
establish initial hardware operation, not reboot reliability, long-session
stability, monitor listening or the below-10-ms goal. The earlier six-frame
baseline discrepancy and cold-start USB failure remain open.

Raw captures, callback timestamps, analyser results, the comparison calculation,
final diagnostic readbacks and a SHA-256 evidence manifest are retained in
`build/trials/2026-10-21-live-completion/`. Both earlier candidate evidence sets
remain sealed and unchanged. No driver source or timing gate changed during
this hardware validation.


## 64-frame comparison rejected; ten-minute 128-frame session passed

The subsequent 21 October session kept `dev-d180f6efb5393f27`, all driver
source and safety margins unchanged. The test plan froze both references
before playback: the older measured 64-frame baseline and the fresh same-session
128-frame baseline, with ±2 frames in both cases. No six-frame correction or
inferred 64-frame baseline was substituted.

| Run | Actual callbacks | Chirps | Raw RTL | Result |
|---|---:|---:|---:|---|
| Eight seconds, 64 frames | 6,000 | 2 | 78.663978–78.663978 ms | Latency comparison failed |
| Ten minutes, 128 frames | 225,000 | 120 | 81.309810–81.330649 ms | All retained gates passed |

The 64-frame recording had zero steady losses, clean signal/timestamps, no
fault and clean teardown. Its latency was 5.992815 frames above the older
78.539 ms reference, outside ±2 frames. The second short 64-frame start and the
64-frame soak were therefore skipped. Its 3712-frame scheduling distance and
approximately 63.871-frame timestamp remainder are consistent with the current
128-frame measurements. This supports tracking the already-known baseline shift;
it does not establish its cause or qualify the rejected comparison.

The 128-frame soak used the already-passing fresh comparison. It retained
28,800,000 input-4 frames, with all 120 output-3 chirps accepted and
zero steady input/output losses. Maximum deviation from the fresh reference
was 1.000070 frames. Minimum OUT return lead was 25 ms across the
whole run and 28 ms steady, against 16 ms. Startup playback misses
were 1855, with no later increment; startup misses remain part of the
record. The final driver state is idle at epoch 5, pending `[0,0]`, no
fault/cleanup error/first-failure record, with the buffer restored to 512.

The decision is mixed: the independent transport now has a ten-minute hardware
pass at 128 frames, but 64-frame unchanged-latency qualification remains open.
A matched fresh 64-frame baseline from the preserved prior driver is needed
before resuming that progression. The older failure remains recorded even if
a later matched comparison passes. These tests do not approve lower margins,
reboot reliability, DAW transitions or a below-10-ms latency claim.

Evidence is under `build/trials/2026-10-21-live-completion-duration/`, including
the pre-run plan, raw input-4 captures, callbacks, all analyser and acceptance
results, final startup/retry/fault records and a SHA-256 manifest. The first
wrapper mistakenly compared integer callback-size keys to string keys, falsely
reporting a geometry error; the retained callback file proves all 6,000 callbacks
were 64 frames. The corrected report still rejects latency. Both wrapper
versions and the original erroneous report are retained; no audio was rerun
and no threshold changed.
