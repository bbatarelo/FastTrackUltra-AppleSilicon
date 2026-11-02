# CoreAudio restart degradation investigation

19 October 2026, one Mac on macOS 26.7.1 (25G241).
The all-pump candidate `dev-27ce3330fc59d37e` was rejected after a first-start
USB overrun. The qualified `dev-b683adc88b54ab87` was restored and passed two controlled
starts. The later [first-failure diagnostic](STARTUP-EVIDENCE.md),
`dev-c46bc0d543e8d11f` passed two short warm checks but **reproduced the USB
failure after the next owner reboot**. The restored qualified baseline then
reproduced the same failure in a single controlled first start after reboot.
The initial CoreAudio enumeration stall and the later USB failure remain
separate findings. Planned all-pump latency sessions have **not started**.

## Observed evidence

CoreAudio PID 45246 started at 08:45:01 local time. Its launch service reported
13 launches. Process measurements showed roughly 99–104% CPU while device
enumeration was unresponsive. Apple’s AirPlay helper, PID 189, had been running
for over six days without restarting.

The last five retained CoreAudio startups logged these registration counts
for the same bundle ID, `com.apple.AirPlayXPCHelper`:

| CoreAudio PID | Local startup date/time | Logged AirPlay registrations |
|---:|---|---:|
| 31352 | 18 Oct 13:29:59 | 256 |
| 44211 | 18 Oct 14:02:06 | 512 |
| 59159 | 18 Oct 14:44:33 | 1,024 |
| 95435 | 18 Oct 23:07:20 | 2,048 |
| 45246 | 19 Oct 08:45:02 | 4,096 |

These are counts from retained logs, not a direct inspection of Apple's internal
registration table. The exact doubling is strong evidence of accumulation or
repeated registration across CoreAudio restarts.

The owner supplied a three-second stack sample at 08:52:25, while the problem
was still present. The same CoreAudio PID existed before and after sampling.
The active path repeatedly enumerates plug-ins/device managers through
`HALS_System::GetNumberPlugIns`, `GetPlugInList`,
`_CopyDeviceManagerList` and `HALS_ObjectMap::ReleaseObject`.
Another thread is in `HALC_ShellSimpleProxyList::Reconcile` and property queries.
This differs from the previous post-recovery sample, which mostly showed waiting.

Both E-MU 0404 USB and Tracker Pre remained connected, alongside FTU and MOTU M4.
No driver was removed and no audio stream or loopback test was started by this
investigation. The FTU and E-MU drivers run in separate remote host processes;
the sampled busy path is in Apple's CoreAudio process. This does not exclude an
interaction, but the evidence currently points more strongly to AirPlay
registration handling than either driver's audio worker.

## Interpretation and limits

A long-lived AirPlay helper appears to retain state that causes registration
work to grow across CoreAudio restarts. This plausibly explains why early
restarts seemed normal and later ones became progressively slower. The exact
internal defect, ownership of the duplicated state and the original trigger
are not established by logs and sampling alone. No general claim about other
macOS versions or machines is made.

## Controlled recovery attempt

We prepared a one-attempt, PID-guarded script to restart only
`system/com.apple.AirPlayXPCHelper`, preserving CoreAudio PID 45246 and leaving
both E-MU devices connected. The owner ran it. macOS rejected
`launchctl kickstart -k` with:

> Operation not permitted while System Integrity Protection is engaged

The helper-only comparison therefore did not happen. No SIP change, service
disablement, driver change or additional CoreAudio restart was performed.

## Reboot outcome and separate FTU startup failure

The owner restarted the Mac normally on 19 October at 17:42 local time,
leaving both E-MU devices, FTU and MOTU M4 connected. CoreAudio PID 440 and
AirPlay helper PID 424 each report one launch. The read-only HAL query returned
in **0.128 seconds**, with loaded identity `dev-27ce3330fc59d37e` and the same
installed binary hash. CoreAudio measured 3.3% CPU. Retained registration logs
for the new CoreAudio PID contain **one** AirPlay registration, down from 4,096.

This supports the accumulation hypothesis. It does not prove that reboot was
necessary for recovery: the later log collection also shows the old CoreAudio
PID 45246 successfully starting FTU I/O at 17:31, before shutdown. The exact
time at which it became responsive remains unknown. Logged I/O starts are not
an independent listening or audio-quality check.

The new boot has a separate failure, preserved before any diagnostic audio:
at 17:44:06, CoreAudio attempted an FTU start on behalf of PID 1330, identified
by the process snapshot as **Discord Helper**, launched at 17:44:04.
The driver remained in Fault (phase 4, epoch 1, fault 6, reason 23).

- The first IN completion (sequence 0, ticket 1) returned
  **`kIOReturnOverrun`, `0xe00002e8` / -536870168**. Its callback token matched.
- The other three admitted IN requests returned `kIOReturnAborted` during
  cleanup. There were no OUT admissions and no captured/prepared audio frames.
- Cleanup returned zero; pending debt is `[0, 0]`. No callback invalidity,
  dropped pump observations or telemetry overflow was recorded.
- The SDK's `IOReturn.h` confirms both status names; a decoding excerpt is saved.
- Timing, worker, delivery and all-pump snapshots retain the same faulted epoch.

The initiating application is context, not evidence that Discord caused the
USB error. One failed start does not establish whether the observation changes,
existing startup behavior or USB/device state caused it. No automatic retry,
CoreAudio restart, mixer write or loopback stimulus was performed.

The predeclared startup gate rejected this candidate for continued latency
testing. No failed audio start was retried automatically. The owner installed
the exact qualified rollback before the controlled comparison below.

## Kernel evidence and restored baseline

A targeted USB log query adds a more specific observation. At **17:44:06.244**,
Apple's IOUSBHostFamily recorded FTU at location `20211100`, endpoint `0x81`,
status `0xe00002e8` as a **babble error**. Two milliseconds later it logged the
abort of the remaining three requests. This corroborates the returned USB
status independently of our Rust error mapping. Nearby webcam-interface open
failures name UVCAssistant; they do not establish an FTU interface conflict.

The failed IN request's last-packet-to-callback interval was **0.167458 ms**,
and callback-to-consumption was **0.006041 ms**. Startup gaps between pumps
reached only **0.007125 ms**. The all-pump records validate structurally when
the steady-operation requirement is explicitly disabled for offline review.
This is not an audio or lifecycle pass: no steady state was reached, and these
timestamps do not explain what caused the USB babble.

A source comparison found the same **312-byte IN request per microframe** and
endpoint maximum in the qualified and observation versions. The candidate did
not reduce the IN request capacity. The existing code also reads both endpoint
rates as 48 kHz before starting. No evidence justifies masking the overrun,
adding an arbitrary startup delay or changing the latency margins.

After the owner installed `dev-b683adc88b54ab87`, the installed SHA-256 was
`da9106f061e19e37362e334fcaf5f038676b71cc6adf8b13af92b16efcb6d4da`.
The loaded build matched, and a read-only query returned in **0.113 seconds**,
Idle at epoch 0 with zero faults and pending debt. CoreAudio now has PID 4991;
AirPlay helper PID 424 remained alive, and the new daemon logged **two**
AirPlay registrations. This reproduces the 1-to-2 growth after one restart;
it still does not identify the internal owner of accumulated registrations.

Two isolated starts then passed, without retries:

| Controlled run | Frames / callbacks | Signal result | Minimum OUT lead |
|---|---:|---|---:|
| 8-second silence, 512-frame buffer | 384,000 / 750 | Input-4 peak 0.0000806 | 23 ms |
| 8-second loopback, 128-frame buffer | 384,000 / 3,000 | Both chirps accepted, 81.205804–81.205808 ms RTL | 23 ms |

Steady loss counters were flat; coherent timing/worker/delivery checks passed.
Startup playback-miss totals (2,792 and 1,808) remain preserved, not reported
as zero. Each run returned to Idle with zero pending debt and restored the
512-frame buffer. Normal output and alerts selections were restored to FTU.
These checks establish current usability of the rollback, not cold-boot
reliability or a controlled attribution to the all-pump changes: the candidate's
failure occurred on its first post-login start, under different conditions.

**Decision:** leave the qualified build installed with unchanged geometry and
margins. Investigate USB startup before another latency candidate installation.
The original one/two-session latency decision remains unmeasured. The next
failure record should preserve completed per-packet status/requested/actual
counts, admission clock/lead and endpoint state from existing owned data.
Do not access pending buffers, discard the failed start or retry silently.
The follow-up [first-failure diagnostic](STARTUP-EVIDENCE.md) now preserves this
completed metadata before teardown. It preserved the repeated first-post-login overrun after two passing warm checks;
rollback and a same-topology qualified-build comparison are selected. No startup
workaround has been established.

## Evidence and pending work

Evidence is preserved under
`build/validation/all-pump-20261019/startup-20261019T064651Z/`:
the timed-out query, launch-service state, process measurements, audio USB
topology, startup logs, three-day registration history, busy stack sample and
the rejected helper-reset attempt. The stack sample is
`coreaudiod-45246-20261019T065223Z.sample.txt`.

Post-reboot evidence is under
`build/validation/all-pump-20261019/reboot-20261019T154522Z/`, including service
lifetimes, selected USB topology, registration logs, loaded identity, all four
diagnostic properties, the initiating-client snapshot and rollback validation.
`hardware-outcome.json` records the startup rejection separately from the
immutable candidate plan. The planned 128/64-frame latency sessions have not
started; a later successful run cannot erase this failure.

The rollback comparison and targeted USB log are preserved under
`build/validation/all-pump-20261019/rollback-check-20261019T155042Z/`.
The exact runner, input hashes, plans, both raw runs, analyses and guarded audio
selection restoration are retained. `rollback-confirmation.json` summarizes the
verified restoration separately from the failed candidate evidence.

Any eventual workaround should be documented for both FTU and E-MU development,
since both workflows restart CoreAudio. Do not add privileged helper resets to
normal installation based on this single-machine finding.

The diagnostic installation at 20:24:06 added a third post-reboot CoreAudio
daemon observation: PID 55865 logged **four** AirPlay registrations with helper
PID 424 unchanged. Its initial CPU spike settled and HAL readbacks were prompt.
The separate USB diagnostic's successful warm starts do not resolve either
cause. The [diagnostic outcome](STARTUP-EVIDENCE.md#installed-warm-start-outcome)
retains the system-sound client contamination, isolated tests and next boot check.

A second normal owner reboot at 21:21:16 reset logged AirPlay registrations to
one (helper PID 436, CoreAudio PID 458). HAL queries remained responsive. The
first FTU start at 21:23:02, now initiated by systemsoundserverd PID 740,
nevertheless returned the same first-IN overrun with kernel ep81 babble.
The new failure-only recorder preserved all packet metadata through clean
teardown. The [recurrence and next comparison](STARTUP-EVIDENCE.md#first-post-login-recurrence-and-decision)
keep this USB startup defect separate from the AirPlay accumulation evidence.

The subsequent baseline reboot at 21:40:33 loaded CoreAudio PID 521 and helper
PID 502, with **one** AirPlay registration. Initial FTU status was Idle/epoch 0.
The separate controlled baseline start at 21:52:20 returned the same first-IN
overrun and independently logged FTU endpoint-0x81 babble. The baseline's existing
delivery recorder retained matched sequence 0, while the other three requests
drained and no OUT request was admitted. Newer observers are not required.
See the [baseline outcome](STARTUP-EVIDENCE.md#qualified-baseline-after-reboot-shared-failure).

CoreAudio CPU snapshots were 48.5% at 1:17 lifetime, 40.8% at 5:45 and 0.0% at
6:55; HAL queries remained responsive. The eventual low reading does not diagnose
the preceding CPU activity. A broad StartIO log also contains a 21:49:03 failure
for context 601. A narrow follow-up identifies **ASFW-000A3500C28A39D6**, not FTU.
It is preserved as separate host evidence, not attributed to the FTU driver or
claimed to cause its USB error. No ASFW or E-MU settings/source were changed.

The qualified FTU build remains installed, faulted and drained. Default output
and alerts remain on built-in speakers after the failed test. No further audio
start, device reset, recovery or CoreAudio restart was performed.
