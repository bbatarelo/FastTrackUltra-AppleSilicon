# First USB failure evidence

19 October 2026. **First-post-login failure reproduced on both this diagnostic
and the restored qualified baseline; further audio tests stopped.** Two earlier
short warm diagnostic checks passed, but the candidate is rejected and rolled
back. At the close of this 19 October comparison, the baseline
`dev-b683adc88b54ab87` was installed, faulted and fully drained. See
[current status](STATUS.md) for the later installed candidate.
The newer observers are not required for the shared failure. This diagnostic
includes all-pump observations as well as the first-failure record; it is not a
change applied only to the qualified baseline. See the
[baseline outcome and next experiment](#qualified-baseline-after-reboot-shared-failure).

| Artifact | Identity |
|---|---|
| Candidate | `dev-c46bc0d543e8d11f` |
| Candidate binary SHA-256 | `55fd5ac2c48c093dbf3d9a48cadcfc381f5cef2449afde85431552ba1ea17de9` |
| Qualified rollback | `dev-b683adc88b54ab87` |
| Rollback binary SHA-256 | `da9106f061e19e37362e334fcaf5f038676b71cc6adf8b13af92b16efcb6d4da` |

The [startup investigation](STARTUP-RESTART.md) found a first IN request overrun
on the first post-login start, corroborated by a kernel endpoint-0x81 babble
message. The completed request's individual packet results did not survive
teardown. Two subsequent warm starts on the qualified build passed; that does
not establish a cause or a controlled cold-boot comparison.

## What changes

Rust preserves the **first request/packet validation failure that stops
transport, in existing pool poll order**. This is not necessarily the earliest
callback by timestamp. It copies existing completed, callback-owned metadata
only; the exported snapshot contains no PCM samples.

A fixed immutable primary record is published before policy restoration, abort,
drain or cleanup. A second immutable record adds the terminal outcome if teardown
returns. Release/acquire publication prevents partially written snapshots.
A later start cannot overwrite either record within the same live-driver
instance. Destroying that instance or its host process loses the record: collect
it before restart or rollback.

The additive read-only HAL property is `ftuF`, exposed through
`build/hal-check --first-failure`.

An empty record does **not** establish health. Setup/submission failures, missing
callbacks, stale native tokens, clock faults and a panic before publication are
outside this completion-only record. Always retain ordinary `ftuS` status.
An unmatched completion explicitly has no packet array evidence; the diagnostic
never reads pending or unowned buffers.

Native 48 kHz, four 8 ms slots per direction, activation order, 312-byte input
packet capacity, input/output safety 1024/2560, 3072-frame PCM offset, deadlines,
thread policy, error acceptance and retry behavior are unchanged. No startup
delay, discarded packet, additional USB query, rate/mixer write or automatic
recovery is introduced. The extra record copy occurs on a fatal validation
branch, not on every successful completion.

## Reading the evidence

The version-1 C ABI is 415 64-bit words (3320 bytes). The C formatter and Rust
layout are covered together by the boundary test.

| Field/group | Meaning and limitations |
|---|---|
| `version`, `valid`, `present` | Schema, publication completeness and failure presence. Unavailable is not zero. |
| `epoch`, `reason`, `direction`, `sequence`, `ticket`, `slot` | Failed record identity; reason 22 is packet validation, 23 request completion. Direction 0 is IN, 1 OUT. |
| `start_frame`, `checked_frame`, `returned_frame`, `admission_ticks` | Existing admission observations. Successful IN submissions do not measure `returned_frame`; zero there is unavailable, not an enormous lead. |
| `submit_status`, `completion_status`, `matched`, `callback_ticks` | Original submission/completion result and ownership match. |
| `requested[64]`, `packets[64][4]` | Original requests plus raw [status, requested, actual, ticks]. Failed lengths are evidence, never accepted PCM. Array status words are sign-extended 64-bit values. |
| `record_valid`, `activated` | Internal identity checks and activation success. No new activation clock measurement. |
| `finished` | Zero: primary before teardown; one: terminal after teardown returned, including failed cleanup. |
| `drain_failed`, `cleanup_status`, `post_available`, `close_available`, `close_status` | Terminal context. Primary zeros are placeholders, not successful cleanup. Availability flags control interpretation. |
| submitted/completed/pending IN/OUT counts | Counts at the selected publication point, not a later session's counters. |
| `rates[2][12]` | Existing pre-start and post-cleanup reads: status[2], hz[2], actual[2], raw[6]. Endpoint order OUT 0x01, IN 0x81. Second bank unavailable without `post_available`. |
| `info[2][17]` | Cached native information before teardown and existing post-cleanup information. Not a new read-back at failure time. |
| `pump[6]` | Existing pump ID, enter/exit ticks, result and sampler begin/end ticks, with the same meanings as `ftuD`. |

Each info bank orders: timebase numerator/denominator, location, vendor/product,
stage, original alternates IN/OUT, pipes IN/OUT, restored/open masks, callback
fault, restore statuses IN/OUT and cleanup statuses IN/OUT. Context status words
also retain signed errors in their 64-bit representation.

The SDK low-latency sentinel `0x6c6c6974` (`kUSBLowLatencyIsochTransferKey`,
“llit”) is retained verbatim and classified as untouched/unavailable packet
status. Do not count it as a normal error, successful service or valid PCM.
Packet timestamps and mapped statuses cannot reveal the controller's full
internal babble reason.

`tools/hal-loopback/failure_analysis.py` checks shape, identity, native widths,
raw statuses and publication availability. Valid failure evidence always has
`audio_accepted: false`. An older retained epoch is allowed and explicitly
distinguished from current status. A future record epoch or ambiguous readback
is rejected.

## Bounded hardware plan

1. Install the frozen candidate once, retaining the qualified rollback.
2. Capture read-only properties before diagnostic audio. A fault, retained
   failure, timeout, mismatched identity or changing epoch stops the test.
3. If cleanly idle, use an isolated eight-second silence preflight at 512 frames
   followed by an eight-second quiet loopback at 128 frames: rear output 3 to
   rear input 4, channel 4 Rear. Other outputs remain zero, with no input
   monitoring. Preserve the existing 16 ms submission-lead gate.
4. If warm starts are clean and first post-login reproduction remains needed,
   use at most one owner-approved normal reboot with this same candidate.
   Capture the first post-login state before any manual retry.
5. On failure, retain all evidence and restore the exact qualified build.
   On non-reproduction, record that result without claiming a fix; keep the
   qualified baseline as default and decide the next offline work from evidence.

No latency sweep or repeated boot campaign is part of this plan.
A successful read-only query does not qualify playback.

Frozen files are under `build/candidates/startup-evidence-20261019/`.
They include the signed bundle, source identity, all 39 production source
inputs, deployment helper and matching HAL inspector. Validation under
`build/validation/startup-investigation-20261019/` contains hash-pinned
`install.sh` and `rollback.sh`, `hardware-plan.json` and read-only
`collect_startup.py`. The collector saves first-failure/status, timing, worker,
delivery and pump properties; service lifetimes, CPU, narrowly filtered kernel
and AirPlay logs; and the selected FTU USB registry. It starts no audio and
does not retry a failed or timed-out query. Missing OS evidence remains explicit.

## Offline checks and review

- 132 Rust tests passed; three benchmarks remain ignored.
- HAL boundary ASan/UBSan passed, including property size/read-only checks
  and signed/sentinel JSON round trips.
- All 51 loopback Python tests and the C callback sanitizer passed.
- Clippy passed with warnings denied; the signed bundle passed deployment checks.
- Collector tests use simulated property/command results, not hardware evidence.

Claude reviewed the proposed design and supplied FTU source excerpts. Publication
before teardown and explicit unavailable packet data were adopted. Existing poll
order was retained and documented. Additional activation-time bus queries,
persistent driver logging and a multi-boot comparison were deferred. The exact
review and dispositions are retained with validation; Claude did not review the
final implementation. More context may still be necessary if packet evidence
cannot distinguish the startup hypotheses.

A primary-source check found no Ultra-specific startup packet-skipping rule to
copy: Linux's nearby rule targets C400/C600 (0763:2030/2031), whereas Ultra/8R
(0763:2080/2081) receive generic implicit-feedback handling using IN endpoint
0x81 on interface 2. This does not prove that Ultra needs no workaround and
provides no basis for a blind delay or accepting this overrun.
Sources inspected 19 October 2026: Linux
[quirks.c](https://raw.githubusercontent.com/torvalds/linux/master/sound/usb/quirks.c)
and [implicit.c](https://raw.githubusercontent.com/torvalds/linux/master/sound/usb/implicit.c).
No Linux implementation was copied.

## Installed warm-start outcome

The owner installed the hash-pinned bundle at 20:24:06 local time on 19 October.
The loaded identity matched. The first read-only capture took 0.435 seconds;
all five property readbacks were idle at epoch 0, with no retained failure.
CoreAudio PID 55865 initially measured 76.8% CPU at 19 seconds of lifetime,
then 2.7% at 43 seconds. Its logs contain four AirPlay registrations, after
the previous daemon's two, with helper PID 424 still alive. This continues the
separate restart-accumulation observation; it does not identify the USB cause.

Before isolation, macOS's `systemsoundserverd` (PID 716) opened two FTU
sessions at 20:24:31 and 20:24:36. CoreAudio logged successful starts and stops.
The guarded epoch-zero preflight therefore stopped before launching the audio
instrument. That rejected preflight remains preserved. It was not a USB failure
and is not counted as a diagnostic playback pass.

After preserving the client evidence, a separate plan used the now-isolated,
clean epoch-2 state for the two intended **warm** starts, without resetting the
driver or discarding a failure. The existing audio/timing gates were unchanged.

| Warm check | Epoch | Frames / callbacks | Signal result | Minimum OUT lead |
|---|---:|---:|---|---:|
| 8 s silence, 512 frames | 3 | 384,000 / 750 | Input-4 peak 0.0000572 | 23 ms |
| 8 s loopback, 128 frames | 4 | 384,000 / 3,000 | Both chirps accepted; 81.205802–81.205805 ms RTL | 23 ms |

Both returned to Idle with zero pending transfers, zero fault/cleanup status,
no retained completion failure, flat steady audio-loss counters and the restored
512-frame buffer. Startup playback-miss totals were 2,836 and 1,776; they remain
recorded rather than reported as zero. Availability screens retained 183 and
154 frames. Timing, worker, delivery and all-pump structural/correlation checks
passed. Default output and alerts were restored to FTU and verified. No listening
claim is made for these output-3/input-4 checks.

The all-pump recorder retained 4,379 and 4,340 calls. Steady empty timeouts
reached 7.624 and 7.247 ms despite the requested 2 ms. The worst retained output
delivery delays were 0.809 and 1.595 ms; linked intervals partitioned exactly.
These short observations demonstrate usable recorder output, not an OS cause,
a scheduling bound or justification for reducing margins. The planned long
all-pump latency sessions remain unrun.

Evidence is under `build/validation/startup-investigation-20261019/`:
`capture-20261019T182424.779288Z`, the rejected
`warm-check-20261019T182541Z`, the isolated
`warm-isolated-20261019T182733Z` and final
`capture-20261019T182815.163251Z`. `warm-isolation-decision.json` records the
scope change before audio began. `warm-hardware-outcome.json` records the result
separately from the immutable candidate and original plan.

The planned owner-performed reboot was then completed with this exact candidate
and the same connected devices. Its failed outcome is below. These warm passes
remain preserved and are not used to replace the failed first-post-login test.

## First-post-login recurrence and decision

The owner rebooted normally at **21:21:16 Europe/Zagreb on 19 October** with
the same USB path and power supply. FTU was enumerated at 480 Mbps and configured
at 21:21:25. At **21:23:02**, the first I/O start was on behalf of
`systemsoundserverd`, PID 740. CoreAudio returned a start error. This is a
different initiating client from the earlier Discord Helper occurrence.

The read-only capture at 21:24:14 verified the installed and loaded diagnostic
identity. The first HAL query took 0.104 seconds. Every property retained the
same faulted epoch 1; no diagnostic audio, manual retry, device reset or further
CoreAudio restart was performed.

| Evidence | Observed result |
|---|---|
| First request | IN sequence 0, ticket 1, slot 0; matched completion |
| Request status | `0xe00002e8` / `kIOReturnOverrun` |
| Independent kernel log | FTU endpoint 0x81 babble at 21:23:02.351; remaining three requests aborted two milliseconds later |
| Per-packet requests | All 64 requested 312 bytes, the validated endpoint capacity |
| Raw packet statuses | All 64 returned overrun; no untouched sentinel |
| Raw actual counts | 63 entries report 144 bytes; entry 1 reports zero |
| Admission | Start frame 98466, checked frame 98450: 16 ms input lead |
| Delivery | Last packet timestamp to callback 0.072917 ms; callback to consumption 0.005208 ms |
| Between-pump gap | Maximum startup gap 0.021458 ms |
| Native lifecycle | Four IN admissions/completions, no OUT admissions, zero pending; cleanup and close returned zero |
| Rate readbacks | Both endpoints report 48 kHz before prepare/activation and after cleanup; no streaming-alt rate observation |

The complete first-failure record survived teardown and was decoded successfully.
Worker/pump structures validate without a steady-state requirement. The delivery
analyzer correctly rejects `completion_error`; valid diagnostic structure does
not make this an accepted audio run. No audio was captured or prepared.

The packet counts are **error-marked metadata**, not accepted PCM or proof of
the bytes actually transferred on the wire. All statuses being overrun does not
establish 64 independent babbles. Their timestamps group as 2 entries, then 6,
then seven groups of 8; any controller-event interpretation remains speculative.
The native shim copies raw completed fields and does not replace their statuses.
Apple/controller propagation semantics have not been established.

The rate GETs occur outside active streaming; their 48 kHz values do not establish
clock settling during alt 1. Cached native info before teardown confirms opened
interfaces and no callback fault; the post-cleanup bank confirms restoration and
closed interfaces. Successful IN submit-return bus time is unmeasured, so its
zero field is unavailable, not zero lead.

CoreAudio PID 458 and AirPlay helper PID 436 are from this boot. The registration
log count reset to **one**. CoreAudio's measured CPU fell from 58.3% during the
first capture to 0.0% in the subsequent process snapshot, while HAL remained
responsive. The USB failure reproduced with no accumulated AirPlay registrations.
This keeps it separate from the progressive restart-enumeration issue.

**Decision:** stop candidate audio tests, preserve this failure and restore
`dev-b683adc88b54ab87`. The next bounded comparison is **one normal reboot with
that exact qualified build on the same USB topology**, not another observation
build or a simultaneous port/rate/activation change. The baseline has warm
qualification but no established first-post-login result.

- Same baseline signature: it can occur without the newer observers; investigate
  initialization/transport/topology offline before choosing a behavioral test.
- Baseline first-start non-reproduction: prioritize the observer-family startup
  diff, while retaining the possibility of an intermittent shared fault. One
  passing boot does not prove a regression or qualify general reliability.
- No observed first start or missing evidence: inconclusive, never an audio pass.

The baseline has no persistent `ftuF`. The prepared read-only collector saves
the boot boundary and this-boot kernel/client logs alongside status, so an
earlier error is not dismissed just because a later session cleared current
status. Four simulated collector checks passed; this is not a hardware result.
No extra baseline warm run or automatic retry is part of this comparison.

Claude reviewed the new source excerpts and sample-free metadata, and agreed
with the baseline comparison. The review's speculative xHCI/Apple-shim mechanism
is not adopted as fact. Reordering alternates or writing SET_CUR is deferred
until the comparison and a primary-source review justify a specific test.
No production code or driver artifact changed after this failure.

Evidence: `capture-20261019T192414.493028Z` within the existing validation folder,
including `fault-summary.json`, raw properties, kernel/client logs, offline
decoders and review/disposition. `post-login-hardware-outcome.json` and
`baseline-first-post-login-plan.json` record the decision separately from the
immutable original candidate plan. Rollback authentication and the comparison
reboot were completed next; their outcome follows.

## Qualified baseline after reboot: shared failure

The owner restored **`dev-b683adc88b54ab87`** and rebooted at **21:40:33
Europe/Zagreb on 19 October**. Installed SHA-256 and loaded identity match the
frozen qualified build. The same FTU path `20211100` enumerated at 480 Mbps.
The first read-only capture at 21:48:02 found clean Idle, **epoch 0**, no observed
FTU start and no this-boot FTU USB error. That passive comparison was closed as
**inconclusive**, not a startup pass.

A separate, predeclared **single silent first-start attempt** then used the
existing authorization for startup diagnostics. Its plan and scope change were
saved before audio. Default output and alerts were isolated to built-in speakers;
preflight still required clean epoch 0, the exact installed build and buffer 512.
No reboot, device reset, rate write, port change or retry was added. The initiating
client and delay since enumeration differ from the earlier natural system-sound
starts; this is a controlled first FTU start, not the same client scenario.

At **21:52:20.530**, macOS again logged FTU endpoint **0x81 babble**, followed two
milliseconds later by aborting the other three IN requests. The baseline's
existing delivery recorder retained:

| Evidence | Baseline result |
|---|---|
| First request | IN sequence 0, ticket 1, slot 0; matched |
| Completion | `-536870168` / `0xe00002e8` overrun |
| IN admission | Frame 699140 versus checked 699124: 16 ms lead |
| Last packet timestamp → callback | 0.1625 ms |
| Callback → consumption | 0.003667 ms |
| Lifecycle | Four IN admissions/completions; three drained; zero OUT admissions |
| Final state | Fault, epoch 1, fault 6 / reason 23; cleanup 0, pending [0, 0] |
| Audio | Zero captured/prepared frames and zero audio callbacks |
| Buffer | Original 512 frames restored |

This baseline has no persistent `ftuF`, so there is **no raw packet array** or
equivalent full first-failure record for this run. Its retained recent completion,
admission, current status and independent kernel log establish the matching
signature. The analyzer rejects the run; missing steady metrics are unavailable,
not a measured violation of the 16 ms OUT gate. No OUT request was submitted.
The planned eight-second run never reached streaming.

**Decision:** the newer all-pump and first-failure observers are **not necessary**
for this failure. Investigate the shared initialization/transport path and host
topology. This does not identify the exact cause or prove that observation has
no timing effect. Earlier successful warm/long baseline tests keep their original
scope; first-post-login reliability is now explicitly failed on that build too.

At the close of that baseline run, it remained installed, faulted and fully drained. Normal
output and alerts remain **BuiltInSpeakerDevice**, deliberately not restored to
FTU after failure. No second attempt or recovery was performed. CoreAudio remained
responsive, with one AirPlay registration; a later CPU snapshot was 0.0%.

Evidence in the existing validation folder: `baseline-passive-outcome.json`,
`baseline-single-start-20261019T195158Z/` (including the decision saved before
audio), `baseline-capture-20261019T195230.263291Z/` and
`baseline-single-start-outcome.json`. The prior passive collector's readiness
flag was only readiness for a separate preflight, not a hardware pass.

## Next experiment: explicit 48 kHz initialization

Historical design decision from 19 October. The later candidate was implemented,
offline-tested and failed its hardware gate despite successful rate writes;
see the [20 October result](STARTUP-48K.md#first-hardware-result-20-october-2026).

A frozen-source audit verified all 37 baseline and 39 diagnostic inputs.
Fifteen native functions, including configuration, rate reads, prepare,
activation, submission, completion copies, cleanup and thread configuration,
are text-identical. The full diff and function hashes are preserved. Observer
allocation/reset and callback/pump bookkeeping do differ, so textual identity
of those functions alone is not a timing-equivalence proof.

The concrete missing initialization step to investigate is a rate write:

- Linux v6.12 marks both FTU and Ultra 8R data endpoints as supporting sample-rate
  control. See the [fixed profiles](https://github.com/torvalds/linux/blob/v6.12/sound/usb/quirks-table.h#L1670).
- Its UAC1 routine sends a three-byte endpoint `SET_CUR` before attempting a
  readback. A pre-existing GET of 48 kHz does not skip that write.
  See [rate initialization](https://github.com/torvalds/linux/blob/v6.12/sound/usb/clock.c#L414).
- Normal UAC1 preparation selects the streaming alternate before rate setup.
  See [endpoint preparation](https://github.com/torvalds/linux/blob/v6.12/sound/usb/endpoint.c#L1358).
- Fixed-stream creation also initializes `rate_max` after selecting alternate
  zero. Thus Linux performs more initialization than the proposed single-change
  test; do not describe the test as reproducing Linux's full startup.
  See [fixed-stream creation](https://github.com/torvalds/linux/blob/v6.12/sound/usb/quirks.c#L138).

**Selected offline direction, not an implemented fix:** explicitly initialize
both data endpoints to the already-required 48 kHz before any isochronous
submission. Preserve the existing IN→OUT activation order, then perform bounded
IN81 and OUT01 rate writes and verify both readbacks before applying the realtime
policy. Keep the USB path, capture-led output scheduling, queues and safety margins
fixed. Do not add 96 kHz priming, arbitrary sleeps, alternate-order changes,
automatic retries or ignored transfer errors to the same experiment.

Rust should own the fixed-rate policy; the native boundary should only execute
the bounded request and report its status/actual length. Offline tests must cover
payload/address/order, short transfers, each failed stage, no isochronous
admission after failure and complete cleanup. Setup results must survive a failure
that occurs before the existing completion-only `ftuF` latch can operate.

`startup-next-experiment-plan.json` records these requirements. No production
source or driver artifact changed in this comparison. A new candidate still
needs implementation, offline checks and a frozen deployment plan. The current
evidence justifies this experiment; it does not establish that the device clock,
rather than firmware/host/controller state, caused the babble.
