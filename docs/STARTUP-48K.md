# Explicit 48 kHz startup candidate

**Hardware gates failed on both USB paths on 20 October 2026.** Both 48 kHz
writes and readbacks succeeded, but the first IN request still returned overrun
with kernel-confirmed endpoint-0x81 babble, first through the dock/hubs and then
directly on the Mac's USB controller. Neither this initialization sequence nor
bypassing the dock resolved startup. The candidate remains installed and faulted,
with input, output and alerts on the Mac's built-in devices. Each test had one
attempt after its own normal reboot; neither was retried.

The offline tests and original deployment plan below remain evidence of what
was tested, not a recommendation to install this candidate for normal use.

## Why this experiment

The GET-only `dev-b683adc88b54ab87` baseline and the two newer diagnostic
builds all reproduced a first-IN overrun with kernel-confirmed endpoint-0x81
babble after reboot. The newer observers are not required for that signature.
Both endpoints reported 48000 before streaming. That readback does not establish
that the device's streaming clock has been initialized correctly.

The [source comparison](STARTUP-EVIDENCE.md#next-experiment-explicit-48-khz-initialization)
found explicit UAC1 rate writes in Linux's setup. This candidate tests that
specific missing operation. Linux also has a fixed-stream rate-max operation at
alternate zero; this candidate does not reproduce the entire Linux sequence.
No Linux implementation code was copied.

## Behavior and boundaries

Rust owns the policy; native C executes the bounded IOUSBLib request.

1. Keep the existing descriptor and GET_CUR preconditions, including 48000 on
   both endpoints. Do not change a device found at another rate.
2. Prepare four 8 ms slots per direction, then activate and validate both
   streaming interfaces in the existing IN → OUT order.
3. SET_CUR once to IN endpoint 0x81, then once to OUT endpoint 0x01.
   Each uses request type 0x22, request 0x01, value 0x0100, length 3 and payload
   `80 bb 00`. The existing 500 ms control-request timeout is used.
4. Require success and exactly three bytes transferred for each write.
   Read back both endpoints using GET_CUR; require success, three bytes and
   48000 on both.
5. Only then request realtime scheduling and admit isochronous transfers.

A failed/short write or invalid readback stops startup. There is no retry,
96 kHz priming, added sleep, mixer operation or alternate-order change.
Cancellation is checked between initialization operations. The existing
two-second StartIO deadline is unchanged; control-request timeouts are not a
promise that the full startup will fit that deadline. If it expires, existing
cancellation/quarantine behavior applies.

Native code refuses rate writes unless both interfaces are open and activated,
and refuses them after any transfer slot has been used. Cleanup still restores
original alternate settings, releases buffers/interfaces and retains uncertain
callback ownership when necessary. Scheduling is restored only if applying it
was attempted.

Native 48 kHz, capture-led scheduling, queue depth four, 8 ms transfers,
input/output safety 1024/2560, PCM offset 3072 and the 16 ms OUT lead gate remain
unchanged. The previous all-pump and first-failure observers remain present;
their earlier failed candidates and evidence are preserved.

## Read-only startup report

`hal-check --startup` reads the new `ftuI` property; it does not start audio.
The 352-byte C/Rust snapshot uses atomic words and a bounded coherent read.
It contains no audio samples and adds no steady-state callback instrumentation.

The report is the **latest startup epoch**, retained through teardown and
replaced at the next start. Capture it before a manual retry. Pair it with
`ftuS` status and `ftuF` first transport failure; those reports have different
retention semantics. A read racing publication has `valid:0`, not zero-cost
success. Empty epoch zero is not an attempted initialization.

| Field | Meaning |
|---|---|
| `version` / `valid` / `epoch` | Schema 1, coherent-read flag, live epoch |
| `stage` | 0 empty, 1 preconditions, 2 prepare, 3 activate, 4 SET IN, 5 SET OUT, 6 verify GET, 7 initializer ready |
| `outcome` | 0 pending, 1 initializer ready, 2 failed, 3 cancelled |
| `config_status` | Signed result; null until precondition results are published |
| `prepare_state`, `activate_state` | 0 not called, 1 in progress, 2 returned |
| `prepare_status`, `activate_status` | Signed results, null unless the call returned |
| `writes` | IN81 then OUT01: endpoint, attempted, returned, signed status, actual byte count |
| `rates` | Before and verification banks; null when unavailable |

Each rate bank has 12 words: status[2], hz[2], actual[2], raw[6], with endpoint
order OUT01 then IN81. Signed rate statuses are sign-extended into uint64,
matching `ftuF`. Unreturned writes have null status/actual. An actual count
of 4294967295 means the native API left the sentinel unchanged; it is rejected.

Initializer-ready does **not** mean audio started, passed timing gates or
survived cleanup. For example, scheduling can fail after successful
initialization. New terminal reason codes are 38 (rate write) and 39 (readback),
with fault 14 for initializer failure; a later cleanup/quarantine fault remains
authoritative in `ftuS`. Failures before opening the native context may leave
`ftuI` empty; use `ftuS` for those failures.

## Offline validation

The tests exercise real worker ordering across 16 cases: failed precondition,
prepare and activation, failure/short result/untouched byte count at either
write, bad status/length/rate at either verification endpoint, and successful
initialization followed by injected scheduling failure. They verify zero
isochronous admission on failure, cleanup, policy-restore ordering and retained
reports. Existing live-session tests exercise successful streaming, multiple
clients, stop/restart and quarantine with the initializer enabled.

Additional tests cover cancellation at each control boundary, concurrent
snapshot reads, exact USB request/payload/address/timeouts, refusal after slot
use, actual native alternate restoration/close after a failed write, and
read-only HAL discovery and JSON serialization of signed errors and absent data.
Native and HAL boundaries run under AddressSanitizer/UndefinedBehaviorSanitizer.

The prepared build is `dev-673cc272caa79014`, binary SHA-256
`e8b420efc0156e334137caa6870fc45e4c47e58abc6d4ffcaf7d09037831594c`.
All final checks passed: 135 Rust tests (three measurement tests intentionally
ignored), 120 probe Python tests, 51 loopback-analysis tests, ten deployment
tests, native/HAL/callback sanitizers, Clippy with warnings denied, and formatting.
An initial Clippy style warning was corrected before the final pass.

After explicit approval of the nine-file payload, Claude reviewed the frozen
candidate and found **no blocking code defect**. Review safeguards were adopted
in the external test tools and plan; the driver binary and installation scripts
remain unchanged. The review, response and supplemental tools are preserved in
`build/validation/startup-48k-20261019/claude-authorized-review/`.

The instrument now records the AudioDeviceStart return code and monotonic
begin/end ticks with the host timebase, plus startup/failure reports immediately
after start and after stop. This distinguishes a failed or slow start from a
later clean idle state. Its callback sanitizer and 51 analysis tests passed.
The supplemental passive collector preserves incoherent snapshots and makes
at most three read-only attempts; seven mock collector tests passed. No hardware
or audio start was used for these checks.

Both captured alt-1 data endpoints independently advertise sampling-frequency
control: class-specific descriptor bmAttributes is 1 at offsets 131 and 197.
That is desktop-Ultra evidence, not Ultra8R hardware qualification. The hardware run below confirmed successful device-level SET completion and
readback on this unit. For future units, a failing control request would leave
the initialization hypothesis untested; preserve its status and context rather
than infer a unique host/device cause.

A passing experiment supports the complete post-activation SET+GET sequence
and its timing. It does not isolate the SET from the extra GET or elapsed delay.
The original frozen source archive retains its preparation-time documentation;
this review supplement records the later decisions.

The final build identity, hashes, test logs and review disposition are preserved
under `build/validation/startup-48k-20261019/`. Frozen artifacts are under
`build/candidates/startup-48k-20261019/`. Build artifacts are local evidence,
not committed binary releases.

## Original installation and first-start plan

Use the frozen installation script, not a newly rebuilt working tree:

```sh
/bin/bash /Users/bruno/development/projects/bruno/AudioInterfaces/FastTrackUltra-AppleSilicon/build/validation/startup-48k-20261019/install.sh
```

It validates the frozen bundle, checks hashes, backs up the installed bundle and
leaves CoreAudio running using the existing deployment helper. The normal reboot
below loads the new bundle; this avoids another daemon-only restart. Enter the
administrator password when requested. Set default input to the built-in microphone and normal output and alerts to
built-in speakers, and do not select FTU or open an audio client on it yet. Reboot the Mac
normally after installation; keep the interface powered and the USB connection
unchanged. Then collect passive evidence before any controlled start using the prepared
`claude-authorized-review/collect_startup.py` in the validation directory. It reads properties and logs;
it never starts, restarts or retries audio. Missing logs are unavailable evidence,
not a clean result. A clean epoch zero still needs boot/client-log inspection.
The installation and rollback scripts also support `--check` for validation
without installing.

The planned decision is one first-start attempt, not a latency sweep:

- Verify the installed and loaded candidate identity, boot boundary, clean
  epoch-zero state, unchanged USB topology, default input/output isolation and
  absence of an earlier FTU start. Close explicit FTU clients, including restored
  DAW sessions.
  An uncontrolled earlier start or unavailable evidence makes the first-start
  comparison inconclusive; preserve it.
- Run one bounded silent duplex start at the existing 512-frame buffer, recording
  `ftuI`, `ftuF`, status and kernel/client logs before and after. Use the supplemental
  `claude-authorized-review/tools/hal-soak` instrument to retain the start return
  code and duration. Require epoch 1 in both status and initializer reports; a
  later epoch contaminates the first-start comparison. Capture immediately.
- Require both writes and readbacks valid; zero startup/transport/cleanup faults,
  drained pending transfers, expected audio progress and existing timing gates.
  Evaluate reason and initializer evidence alongside fault: a later cleanup fault
  can overwrite the primary fault number. An incoherent snapshot must never pass.
  A ready initializer followed by babble is a failed hypothesis test.
- On any failure, stop and preserve evidence. Do not retry, reset USB, prime a
  different rate or change topology within that attempt.
- A pass permits a second independent boot with the same gate. It does not by
  itself qualify the driver. Warm starts, loopback, longer sessions, GarageBand
  transitions and owner listening follow only after startup reliability passes.

If the same first-IN signature remains after proven successful initialization,
do not keep tuning this candidate. The next separate experiment should isolate
USB topology/controller or activation behavior. If initialization itself fails,
the retained stage/status/length determines the next protocol investigation.

`rollback.sh` in the same validation directory restores the preserved
`dev-b683adc88b54ab87` binary. It has earlier warm/long-session results **and a
known first-start failure**; rollback is a comparison/recovery checkpoint, not
a promise of cold-start reliability.

## First hardware result: 20 October 2026

The owner installed the frozen binary and normally rebooted at 10:08:38
Europe/Zagreb. Installed hash and loaded identity matched the prepared candidate.
The FTU remained at location 0x20211100, 480 Mbps, behind the Thunderbolt xHCI
controller and the same USB 2 hub chain. CoreAudio PID 447 was unchanged since
login. Passive properties showed clean epoch zero; captured boot logs contained
no earlier FTU audio start or FTU babble. Input, output and alerts were all on
built-in devices before and after the test.

Exactly one bounded silent duplex attempt used the 512-frame buffer. At
10:16:30 it failed before CoreAudio delivered any audio callbacks. The eight-second
streaming portion never began.

| Evidence | Result |
|---|---|
| Initializer | Epoch 1, stage 7 / outcome 1: ready |
| IN81 and OUT01 SET | Both status 0, actual length 3 |
| Verification GET | Both status 0, length 3, 48000 Hz |
| AudioDeviceStart | 2003329396 / 0x77686174, returned after 69.542 ms |
| First failed request | IN sequence 0, ticket 1, slot 0; matched and complete record |
| Completion | -536870168 / 0xe00002e8 overrun |
| Kernel | Endpoint 0x81 babble at 10:16:30.296; three remaining IN requests aborted |
| Admission | Requested frame 459011, checked frame 458995: 16 ms lead |
| Lifecycle | Four IN submitted/completed, zero OUT; zero pending |
| Final status | Fault phase 4, epoch 1, fault 6 / reason 23 |
| Cleanup | Cleanup and close 0; original alternates restored, interfaces closed |
| Audio | Zero captured/prepared frames; zero CoreAudio callbacks |

All 64 retained packet statuses report overrun: 63 have actual length 144 and
one has length zero, against requested length 312. These failed lengths are raw
metadata, not valid PCM. Successful IN submission does not take a return-frame
clock sample: its zero returned_frame is unavailable evidence, not a real frame
from which to compute lead. The checked lead above is the valid measurement.

The first-failure analyzer accepts the evidence and confirms the same epoch.
The four worker admission events are valid. The delivery/all-pump clean-run
gates reject the Fault state as not_cleanly_stopped; those labels do not mean
native cleanup failed. There is no steady audio, latency, audible or endurance
result, and no OUT submission with which to evaluate an OUT lead margin.

**Decision:** reject this candidate as a demonstrated startup fix. The explicit
post-activation SET+GET sequence succeeded but did not prevent the shared
first-IN failure. It was neither a rejected rate write nor a two-second StartIO
timeout. Other initialization differences, controller/topology effects and
activation behavior remain open; this does not establish which component caused
the babble.

The next separate experiment was planned to keep this exact binary and change the USB path
to a direct Mac connection, bypassing the dock and hubs. The owner confirmed a
suitable connection is available. After reconnection and a normal reboot, verify
the new registry path, the same binary, built-in defaults, clean epoch zero and
absence of earlier FTU starts before one silent 512-frame attempt. Do not run
latency sweeps or add rate/activation/queue changes to this comparison. A failure
stops that attempt; a pass requires another independent boot before broader
qualification. A direct-path pass would implicate the path/state combination,
not identify a particular hub or prove the rate writes were necessary.

Evidence: build/validation/startup-48k-20261019/hardware-20261020T081341Z/.
The original candidate, source archive, review and pre-test manifests are
unchanged. At the close of that dock-path run, the candidate was still installed; no rollback,
daemon restart, USB reset, topology change or second audio attempt was performed.
The separately rebooted direct-path result follows.

## Direct USB result: same failure

The owner moved the powered interface to a direct Mac USB connection, then
normally rebooted at **10:38:06 Europe/Zagreb on 20 October**. The registry showed
Root → AppleT6000USBXHCI@02000000 → Fast Track Ultra@02100000 at 480 Mbps,
with no intervening hubs. The dock and other USB devices remained attached on
their separate path. The installed binary and all test-tool hashes were unchanged.

Preflight found clean epoch zero, empty initializer/failure reports, buffer 512
and built-in input/output/alerts. The all-boot CoreAudio log had no audio starts.
The pre-login daemon did not load the FTU plugin; post-login CoreAudio PID 449
activated FTU once and remained unchanged throughout this test. A boot USB
transport-restriction message is retained alongside successful enumeration,
configuration and subsequent HAL availability; it is not counted as a streaming
failure. CoreAudio's initial 67.1% CPU snapshot later fell to 0.0% before the test.

Exactly one silent attempt began streaming setup at **10:44:52** and failed
before any audio callback. The same initializer completed both SETs and GETs.
AudioDeviceStart returned 2003329396 / 0x77686174 after **71.736 ms**, well before
the two-second deadline. The eight-second streaming phase never began.

| Evidence | Dock/hubs | Direct Mac controller |
|---|---:|---:|
| Build | dev-673cc272caa79014 | Same |
| Initializer | Both writes/readbacks valid, 48000 Hz | Same |
| First failure | IN seq 0, ticket 1, slot 0 | Same |
| Completion | 0xe00002e8 overrun | Same |
| Kernel | Endpoint 0x81 babble | Same, at 10:44:52.071 |
| Checked admission lead | 16 ms | 16 ms |
| Packet errors | All 64; 63 actual 144, one actual 0 | Same |
| IN submitted/completed | 4 / 4 | 4 / 4 |
| OUT submissions | 0 | 0 |
| Audio callbacks/captured frames | 0 / 0 | 0 / 0 |
| Final status | Epoch 1, fault 6 / reason 23 | Same |
| Cleanup/close; pending | 0 / 0; [0, 0] | Same |

The direct request's scheduled frame was 405990 versus checked 405974.
As before, returned_frame is unmeasured for successful IN submission and the
failed packet lengths are not valid PCM. The three remaining IN requests were
aborted at 10:44:52.074. Both alternates were restored and interfaces closed.
Native cleanup succeeded even though the driver retains Fault and the clean-run
delivery/pump analyzers reject that state. No latency or endurance pass exists.

**Decision:** the dock/hub chain is not required to reproduce the failure in the
current device state, and connecting directly is insufficient to fix it. The
comparison does not prove the dock never contributes or identify host versus
device causation. Both tests share the Mac, driver, device and powered-device
history; no physical interface power cycle was requested or verified, so retained
device state remains a possibility.

Stop repeating this rate-write/path comparison. The subsequent
[offline sequence audit](STARTUP-AUDIT.md) is complete. Its next experiment is
full USB and DC power removal followed by a normal Mac reboot, with the same
binary and direct path. This remains prepared and unrun; owner intervention is
required. Keep queues, safety margins and rejected evidence fixed; do not add
retries or ignore overruns.

The failed candidate remains installed and fully drained on the direct path;
all audio defaults remain built-in. No second attempt, rollback, reset or further
restart was performed. Evidence and the reproducible offline verification are
under build/validation/startup-direct-usb-20261020/hardware-20261020T084340Z/.
The previous evidence manifests were reverified unchanged, including the 369-file
baseline record and 127-file dock-path result.

## Warm recovery after full power removal: passed

At the owner's request, the next test **omitted the Mac reboot**. The owner
reported disconnecting USB and DC, waiting, and reconnecting. The off duration
and LED state were not independently measured. This amended the original
fresh-boot plan; it is a separate warm recovery experiment.

Preflight verified the same Mac boot (10:38:06), CoreAudio PID 449, candidate
binary and direct 480 Mbps path. Kernel logs show USB disconnection at 11:44:37
and re-enumeration at 11:44:57. The registry object changed from 0x100000bce to
0x10000244e. The HAL retained epoch 1, its fault and its immutable first-failure
record; all prior requests were drained. Built-in input/output/alerts and buffer
512 were unchanged.

Exactly one eight-second digital-silence attempt followed at approximately
11:48:33 on 20 October, using diagnostic client PID 6633 and retaining only
input 4. It started successfully in **160.349 ms** as epoch 2. No CoreAudio
restart, new driver installation or code change was made.

| Check | Result |
|---|---|
| Requested capture | 384,000 frames; 750 callbacks, all 512 frames |
| Initialization | Both 48 kHz writes and readbacks succeeded |
| Audio/timing analysis | Accepted; no run rejections |
| Steady input/output missing samples | 0 / 0 |
| Lifetime playback misses | 2,564 during startup; retained, not counted as zero lifetime loss |
| Minimum returned OUT submission lead | 23 ms, against unchanged 16 ms gate |
| Worker/delivery/all-pump analysis | All accepted; 2,048 observed admissions/completions and 4,259 pumps |
| Teardown | Idle epoch 2, no fault/reason/cleanup error, pending [0,0] |
| Test-window kernel log | No babble; four IN and four OUT requests aborted during normal stop/drain |
| Audio defaults and buffer | Unchanged; built-in selections and 512 frames |

The epoch-1 first-failure record remained byte-for-byte equivalent as parsed
before and after the run. Its analyzer explicitly reports current_epoch=2 and
same_epoch=false. It is historical evidence, not a new error or proof of epoch-2
packet behavior. Current status, startup observations, timing and worker
observations all refer to epoch 2.

**Decision:** the exact candidate can stream and recover in this existing host
session after the reported power reset. This does not isolate power removal as
the cause: there was no same-host retry without power removal for comparison.
It also does not establish first-start reliability after a Mac reboot, audible
quality, loopback latency or endurance. Both earlier first-boot failures remain.

Leave the binary and initialization sequence unchanged. When the owner can
reboot, the next useful comparison is one normal Mac reboot with the interface
left powered and on the same direct port, followed by passive checks and one
silent first start. Do not require another power cycle for that comparison.
The initially planned full-reset-plus-reboot test remains unrun; this warm pass
does not satisfy it. Defer the per-endpoint initialization change until the next
boot result. No additional warm starts or audible tests were performed.

Evidence:
`build/validation/startup-power-reset-warm-20261020/hardware-20261020T094626Z/`.
The predeclared warm-test plan, immediate gates, immutable old failure record,
all raw observations and single-attempt invocation are preserved together.
