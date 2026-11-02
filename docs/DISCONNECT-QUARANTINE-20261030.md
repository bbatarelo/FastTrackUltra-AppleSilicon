# Disconnect, failed teardown and refused rebind — 30 October 2026

## Finding

Read-only inspection found the installed recovery candidate **quarantined at
epoch 14**, after an output USB completion error and unsuccessful teardown.
The physical Ultra is present on USB again, but the resident session is unbound
and rejects rebind because quarantine is terminal. macOS still selects and
publishes that logical audio device. This explains the silent output.

The observed chain is consistent with active laptop/dock disconnection:
USB output completion error → stop/drain not certified safe → quarantine →
removal detected → reconnected device refused. The exact native stop condition
is not exposed by this build, so an AbortPipe error versus incomplete callbacks,
a callback invariant failure, or native disposal failure is not distinguishable.
No stream start, recording, USB reset, installation, audio restart or selection
change was performed. Final queries returned the same stopped state.

## Captured state

- Installed **and loaded** `dev-369d1059e326aa6d`, independent 4 ms/twelve-slot
  profile. Binary SHA-256
  `0c66982319bac1b9d26a9a5458454f20aa3d4c91f75976fa6dcec75fecc13675`.
  All build-identity source hashes match the checkout.
- CoreAudio PID 78408 and FTU host PID 78418 started **28 October 15:56:14 CET**.
  Property queries remain responsive; no process restart is observed.
- `ftuS`: phase **5 Quarantined**, fault **7**, reason **23
  request_completion_error**, epoch **14**, cleanup **-1000001
  FTU_PENDING_OWNERSHIP**. Captured 7,103,760 frames (~148 seconds of frame data),
  prepared 7,102,991. These counts are not a wall-clock failure timestamp.
- `ftuE`: finished runtime record for epoch 14, generic callback-error stage;
  **drain_failed=1**, close not attempted. It retains 14 fault records counted,
  with the previous 13 overwritten. The reported `[12,12]` pending values are
  retained owner accounting, not a current measurement of native outstanding IO.
- `ftuF`: first failed completion is this same epoch, OUT sequence **36980**,
  slot **3**, scheduled USB frame **539574925**, checked **539574881**
  (44 ms lead at that earlier check), callback ticks **18802474035594**.
  Completion **0xe000400e / kIOUSBNotSent1Err** means transaction not sent.
  Of 32 packet results, 6 carry that code and 26 carry **0xe00002be /
  kIOReturnNoResources**. SDK definitions and source are retained with evidence;
  these codes do not by themselves identify the physical disconnect instant.
- `ftuF`'s `[0,0]` pending values come from the diagnostic PoolSession, which does
  not own this independent runtime's native slots. They do **not** establish
  that native callbacks drained. Neither pending array permits freeing memory.
- Epoch-14 startup succeeded, including 48 kHz writes/readbacks. Prior successful
  OUT submissions had a minimum return lead of **20 ms**, above the 16 ms gate;
  steady output-missing maximum was zero. Startup playback missing was 1,703;
  no input-read samples exist. This is not a zero-loss uninterrupted-session pass.
- `ftuH`: **bound=0**, **removals=1**, **rebinds=0**, **detach_status=-3**,
  **reject=3 / quarantine**. No current binding location/registry is published.
- USB sees one desktop Ultra, **0763:2080**, 480 Mbps, location **0x20211100**,
  current registry entry **0x10190704f**. Presence does not prove audio operation.
- CoreAudio device **130**, UID `net.batarelo.FastTrackUltra:2080:single`, is still
  default output/system output; built-in microphone remains default input.
  The public CoreAudio query reports **alive=1** although the runtime is
  quarantined. Current nominal rate is 48 kHz and buffer property is 512.

## What the previous recovery change did

`ftuA` records **13 attempts, 13 recoveries, 0 failed attempts, 0 exhausted
budgets, 13 healthy-window resets**. The last successful recovery opened epoch
14 from an eligible reason-55 fault in epoch 13. These are actual control-path
returns to RUNNING with a new clock epoch. They are not a listening check or
proof that every application resumed without an audible interruption.

The subsequent epoch-14 USB/teardown failure is ineligible for that clean-deadline
recovery. Currently there are zero active and retired client IDs. Keeping the
old unsafe session is intentional; waiting longer or starting another app does
not make it eligible for rebind. The native factory retains its **thread-local** `pool_context` until safe
close. That blocks a second open on that worker thread, not every thread in the
process. The resident Rust Live handle's terminal quarantine is what prevents
launching a replacement worker or rebinding this session.

The frequent earlier output-deadline recoveries deserve separate investigation.
Their individual guard/timing records have been overwritten, so this snapshot
cannot diagnose their initiating delays. Recovery is not a substitute for fixing
the underlying scheduling failure.

## Power history and timing limits

The first system-sleep entry in the captured 28–30 October window is **29 October
17:50:36 CET**, clamshell sleep on battery. FTU's power assertion was turned off
at the same second. Many maintenance dark wakes followed. At **21:48:30**, power
history records USB-C-plug dark wake, followed by full wake at **21:48:31**.
This aligns with the reported departure and return.

The fault record's 93,244,764 ms since handle creation is consistent with roughly
17:50 on 29 October when anchored to this host's initialization. It is monotonic
uptime, not a wall-clock timestamp; exact fault time and event order should not
be inferred more precisely from that conversion. The host log contains launch
records, not a timestamped runtime-fault/teardown event. The targeted kernel log
query returned no matching retained entries, which is not proof no USB event
occurred. Stack sampling required administrator privileges and was unavailable;
no restart or password prompt was used to obtain it.

## Where the implementation stops

1. `Completer::receive` records the nonzero OUT completion and reason 23.
2. `Owner::stop` asks the native route to stop, abort/drain and join. It can also
   reject Rust slot retirement or native destruction. A false result is
   collapsed into `session.drain_failed`.
3. The worker retains the context, stores `FTU_PENDING_OWNERSHIP`, fault 7 and
   terminal Quarantined. It does not attempt native close.
4. `poll_presence` detects removal; detach returns -3. On rediscovery,
   `FTU_REJECT_QUARANTINE` prevents rebind. The plug-in keeps the logical device
   published when the same model is present.

One concrete conservative path was reproduced **offline**, using the unchanged
native router and a test-only fake AbortPipe returning `kIOReturnNoDevice`:

| Scenario | Callbacks/released/ACKed | Native pending | Joined | Quarantined |
|---|---:|---:|---:|---:|
| Normal abort | 24 / 24 / 24 | 0 | yes | no |
| Both aborts return NoDevice | 24 / 24 / 24 | 0 | yes | yes |
| Lost abort callback | 0 / 0 / 0 | 1 | no | yes |

All three expectations passed with C AddressSanitizer/UBSan. Any nonzero
AbortPipe return currently increments the fatal route counter, even if all slots
later return and the dispatcher joins. This demonstrates a plausible path to
an unnecessarily permanent stop after removal. It does **not** prove the real
incident took that branch: the live driver did not retain abort statuses,
joined/timed-out flags, or the final native slot counts.

## Status-reporting defects

- Recovery still says **healthy**, reason 55, from the preceding healthy run.
  The monitor calls recovery only in Running/Fault, not Quarantined; detaching
  clears timers/clients without replacing a non-pending recovery state. Thus
  `ftuA` is stale about terminal quarantine. `ftuS`/`ftuE` are authoritative here.
- The plug-in intends `DeviceIsAlive=false` in quarantine, but public CoreAudio
  reports 1 after republishing the device. The direct C boundary tests assert
  false and do not exercise CoreAudio's caching/notification behavior. Reannounce
  and availability notifications need an actual-host regression; the exact cache
  mechanism is not proven by these snapshots.

## Next work

Prioritize disconnect retirement/reconnect ahead of further latency tuning:

1. Preserve native stop details before returning: both abort statuses, callback
   counts, pending/held/returned slots, dispatcher exit/join/timeout, route faults,
   Rust retirement and native-disposal outcome. Keep these distinct from
   diagnostic-session counters.
2. Model real removal through the integrated factory/worker path. Distinguish
   expected device-gone errors with proven callback retirement from genuinely
   outstanding ownership. Rebind only after a proved-safe retirement; retain
   terminal quarantine for the lost-callback/invariant cases. Merely clearing
   the quarantine bit or leaking a new context on each reconnect is not a fix.
3. Publish the actual terminal recovery/availability state, and verify removal,
   return and default-output behavior through the real HAL.
4. Recheck normal playback, loopback and disconnect/reconnect with the same
   timing, loss and ownership criteria. Investigate the 13 earlier deadline
   faults separately, retaining useful history instead of only the last event.

This inspection produced evidence and a test-only reproduction, not a replacement
driver. Restoring the current session requires replacing the quarantined host
session (normally an audio-service restart); it has deliberately not been done
before preserving this evidence, and a subsequent hardware start is unverified.

Evidence: `build/trials/2026-10-30-disconnect-quarantine/`, with a SHA-256 manifest.
