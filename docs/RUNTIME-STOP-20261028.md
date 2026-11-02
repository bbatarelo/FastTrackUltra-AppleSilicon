# Runtime output deadline stop — 28 October 2026

## Finding

The user reported an unattended loss of Fast Track audio on 26 or 27 October,
then selected another output. Read-only inspection on 28 October preserved a
stopped streaming engine: **phase 4 (Fault), fault 6, reason 55
(`output_lead_deadline`), epoch 48**. The fault stayed identical across all
snapshots. USB and CoreAudio discovery still worked. This is a runtime output
scheduling failure, distinct from the earlier fixed-location reconnect defect.
No restart, reset, StartIO, recording, install or audio-selection change was made.
The physical hardware was not exercised, so this does not establish that a new
stream would succeed or that the interface's internal state is healthy.

## Observed state

- Installed and loaded: `dev-0d800cbf6bdead51`, the reconnect candidate;
  SHA-256 `a70672c39983242e1446c1dbe0dd2728a91a7535f27aa39453eea9212397f8e5`.
  Installation is now verified, superseding the 26 October offline-only status.
- CoreAudio PID 84908 and FTU host PID 84920 both started 26 October at
  16:37:27 CET and were still responsive. The captured host log confirms loading
  this plug-in at that time. There is no observed host crash/restart.
- USB: desktop Ultra `0763:2080`, high-speed 480 Mbps, location `0x20211100`,
  registry entry `0x10100d550`; same entry before/after inspection.
- HAL: device 128, UID `net.batarelo.FastTrackUltra:2080:single`, published and
  alive, 48 kHz, current buffer 512. The retained epoch's writes were 480 frames;
  the present buffer property is not proof of its historical callback size.
- Hotplug observation: bound/coherent; zero removals, rebinds or rejection.
  This supplies no physical reconnect qualification: none was detected.
- Session 48 captured 8,446,896 frames (about 176 seconds of frame data), prepared
  8,444,592, and ended with pending `[0,0]`, cleanup 0. Phase is Fault, not terminal
  Quarantine. These frame counts are not a wall-clock failure timestamp.
- Startup observation belongs to epoch 48 and reports successful configuration,
  preparation, activation and 48 kHz rate verification.
- Retained timing: 43,734 steady OUT submissions, zero steady output missing
  frames/fallbacks/retry deferrals, 26 input reads with zero missing frames.
  Lifetime playback missing is 2,367; do not describe this as zero lifetime loss.
  Successful OUT return lead: minimum 25 ms overall / 32 ms steady; last 43 ms.
- Current defaults: LG UltraFine Display Audio for output/system sounds;
  MacBook Pro Microphone for input. Inspection left these selections untouched.

## What the stop reason proves, and what is missing

The installed source matches the frozen reconnect build source. In
`rust/ftu-stream/src/independent_live.rs`, `Owner::step` checks the same 16 ms
OUT lead before readiness handling, before/after packing, and after native
submission returns. Crossing any of these guards stops the session with reason
55. Cleanup then completed successfully and the worker published Fault.

The successful-submission timing record never falls below the gate, so the
retained evidence points to a **pre-submission guard** rather than a recorded
late successful return. It does not identify which guard, the late request's
bus frames, the duration of the delay, or whether OS scheduling, USB-call cost,
callback delivery, another stall, or a clock anomaly caused it. A successful
last OUT return with 43 ms lead is not a measurement of the next failed check.
No low-level USB submission error is recorded for this stop (`submit_status=0`);
the zero submit-detail fields mean absent diagnostics, not bus frame zero.

The first-failure property is intentionally a lifetime latch for failed USB
completions. It still contains **epoch 1**, reason 23, IN sequence 0,
`kIOReturnOverrun` (`0xe00002e8`), with clean teardown. It is an earlier startup
failure and must not be mistaken for this epoch-48 scheduling stop. The current
independent path also leaves the legacy worker/delivery/pump observation arrays
empty; those empty records do not prove that no callback or scheduling delay
occurred.

The completed FTU-host log query (26 October 16:30 through 28 October 12:45 CET)
contains startup records but no timestamped runtime-fault event. Power history
shows FTU assertion activity ending 27 October at 11:07:19 and later releases
at 20:57:13. These are power/client lifecycle clues, **not** the failure time.
A targeted CoreAudio query at 10:55–11:15 returned no entries. The retained power
window contains no system Sleep/Wake/DarkWake transition entries; display and
idle-sleep assertions must not be confused with system sleep. None of these
observations establish the initiating delay.

## Why it stays stopped

The code deliberately stops instead of submitting with insufficient safety
margin. A recoverable Fault stays published/alive, while DeviceIsRunning becomes
false; there is no automatic worker restart. A new stream can be admitted only
after old clients have stopped/been removed, the old worker has joined, and
ownership is safe. A client left registered after a fault can block another
Start. Client membership is not exposed in the current snapshot, so that is a
code-level recovery risk, not a proven explanation of a failed retry here.
No retry was attempted during this investigation.

## Next development decision

Reliability takes precedence over further latency reduction:

1. Add a bounded last-runtime-fault record covering all scheduling guards:
   epoch, guard stage, direction/sequence, scheduled/checked/returned bus frames,
   host ticks, prior owner progress and queue state. Preserve it through teardown
   and keep it separate from the first failed USB-completion latch.
2. Reproduce a delayed output owner offline against the actual independent
   path; verify the 16 ms refusal, clean drain, fault notification and client
   lifecycle. Keep all current timing and ownership gates.
3. Design/test explicit recovery from a cleanly drained fault, including clients
   still attached and delayed Stop calls. Never reopen a quarantined context or
   hide an underrun by silently discarding/retiming samples.
4. Only then use a controlled hardware retry/soak and resume reconnect and
   fresh 64-frame latency qualification. A lower-margin change would obscure
   this reliability failure and is not justified by these observations.

No source fix, commit or replacement candidate was produced in this inspection.

## Evidence

Local evidence: `build/trials/2026-10-28-runtime-stop/`.
It includes original HAL property snapshots, USB state before/after, installed
identity, relevant process start times, completed log queries and result codes,
FTU power assertions, read-only audio-device inventory and matching source files.
`MANIFEST.sha256` covers the retained files. Full unrelated process/power history
was excluded from the project evidence. The initial broad log query timed out
and was superseded by the completed host-only query; it is not treated as a
complete negative search.
