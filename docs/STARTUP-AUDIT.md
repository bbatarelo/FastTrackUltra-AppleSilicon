# Startup sequence audit

20 October 2026. The offline audit found no demonstrated startup fix. Its initial
plan adds a **full interface power reset before a normal Mac reboot**,
using the installed binary and direct USB path unchanged. This tests a device
power history that neither failed comparison controlled. It does not separate
all host and device causes. No hardware operation was performed during this audit.

## What the successful archive establishes

The first audible Rust duplex probe followed a 256 ms capture-only run, a
separate endpoint-0x82 feedback run and silent duplex runs. Its successful
2.048-second tone is valid evidence of that sequence; there is no recorded
full interface power reset or independent Mac boot immediately before it.
The initial capture-only run also passed: 32 eight-millisecond blocks,
2,048 accepted packets and 12,289 frames. Neither record proves that capture
priming was necessary, or that the combined startup worked from an untouched
device state.

The archived first-tone binary hashes to
`d5602c5cc3b796d50f71cebf1ad9ebd1e5bdff3d67af79cc36e9da36aa9b0a05`.
All 49 source inputs in its archived duplex source snapshot were verified.
The native functions `config`, `rates`, `select_interface`, `prepare`,
`activate`, `bus_clock`, `buffer`, `completion` and `abort_pipe` are
text-identical to the current implementation. In particular, IN-before-OUT
activation is not a newly introduced change.

The differing submission function retains the same packet count, capacity,
buffer-offset and scheduled-frame construction; it now clears entries at
submission and routes callbacks through pool tokens. Completion and pump
functions add observations; cleanup changes a pending-ownership error code.
The finite prototype and reusable live pool still differ in allocation,
ownership and host lifecycle. Identical functions do not prove equivalent
timing or exclude a shared defect.

## Linux reference comparison

This is a protocol/lifecycle reading of pinned Linux **v6.12**, not a hardware
result or code imported into this project.

| Reference behavior | Implication for this investigation |
|---|---|
| Ultra and Ultra 8R fixed profiles list playback interface 1 and capture interface 2, eight packed 24-bit channels, alternate 1 and maximum rate 96 kHz. | A common transport profile is supported by the reference. [Profiles](https://raw.githubusercontent.com/torvalds/linux/v6.12/sound/usb/quirks-table.h) |
| Fixed-format discovery selects alternate 0 and initializes the maximum rate. | Our prototypes and current driver lack this discovery-time 96 kHz step; their earlier passes do not establish that it is necessary. [Quirk setup](https://raw.githubusercontent.com/torvalds/linux/v6.12/sound/usb/quirks.c) |
| Playback maps to capture endpoint 0x81; PCM preparation prepares the synchronization endpoint first. | Capture-before-playback preparation agrees with our order. Reversing it has weak support. [Mapping](https://raw.githubusercontent.com/torvalds/linux/v6.12/sound/usb/implicit.c), [PCM preparation](https://raw.githubusercontent.com/torvalds/linux/v6.12/sound/usb/pcm.c) |
| UAC1 endpoint preparation activates an endpoint and initializes its rate before preparing the next endpoint. | Our candidate activates both interfaces, writes both rates, then reads both. Per-endpoint interleaving at 48 kHz is a real, untested difference. [Endpoint preparation](https://raw.githubusercontent.com/torvalds/linux/v6.12/sound/usb/endpoint.c), [Rate control](https://raw.githubusercontent.com/torvalds/linux/v6.12/sound/usb/clock.c) |

The endpoint code can skip a redundant alternate-0 selection; it does not prove
that an unconditional reset is required. Pitch initialization is conditional,
and these fixed profiles do not advertise that control. The PCM retry path is
not evidence that our overrun should be ignored or retried. These references
do not justify new 96 kHz priming, pitch writes or error suppression.

## External review and disposition

Claude reviewed only a startup-findings summary, under the existing permission
for this investigation. No additional source files or recordings were sent.
Its useful caveats are retained: the exact installed candidate has no streaming
pass yet; first-boot and first-stream conditions remain coupled; a power-reset
failure cannot identify a host-side or device-side cause.

Claude suggested reconnecting after removing power without rebooting the Mac,
with a fresh live-driver instance and epoch zero. Source inspection shows that
this proposed gate would not hold:

- `driver/plugin.c` retains `g_live` for the host-process lifetime. Its presence
  monitor detects disconnection/reconnection but does not replace an adopted handle.
- `ftu_live_start` can join a completed faulted worker and start another epoch.
  Native open re-enumerates the device, so a warm retry is possible; it is not
  a fresh live instance.
- `Shared::reset` resets ordinary counters and startup observations but retains
  the first-failure record. A second failure could still expose the first epoch's
  packet evidence.

The proposed hot-reconnection experiment therefore needs a different evidence
contract. We defer it and retain a normal reboot for a fresh instance and
comparable first-failure evidence. Do not restart CoreAudio repeatedly to obtain
that instance, given the separately observed restart problem.

The review's stronger claims are not adopted: the history does not prove
“first start always fails,” error-marked 144-byte lengths do not establish valid
48 kHz PCM on the wire, and re-enumeration does not establish which internal
device state was reset. This is a userspace HAL driver, not a DriverKit DEXT.

## Original prepared power-reset comparison

The installed candidate remains `dev-673cc272caa79014`, binary SHA-256
`e8b420efc0156e334137caa6870fc45e4c47e58abc6d4ffcaf7d09037831594c`.
No new build or installation is required.

1. Keep audio clients closed and default input/output/alerts on built-in devices.
   Turn the monitors down before removing interface power.
2. Disconnect both USB and the DC power supply from the interface. Confirm its
   LEDs are off, then leave both disconnected for at least ten seconds.
   This is a reproducible off interval, not proof of complete internal discharge.
3. Reconnect DC, then USB to the **same direct Mac port/adapter**. Keep the
   analog wiring and other devices unchanged.
4. Restart the Mac normally. After login, do not select FTU or open audio
   applications on it. Report completion for passive preflight.

Preflight must verify the installed/loaded identity, new boot and CoreAudio
instance, expected direct 480 Mbps path, built-in defaults, 512-frame buffer,
coherent epoch zero and no earlier FTU start in that boot. Missing or contaminated
evidence stops the comparison before audio. Reconfirm the physical power-removal
steps; a reboot alone is not this experiment.

Make **one** eight-second silent attempt at 512 frames, retaining only input 4
with no monitoring. Preserve the initializer, first-failure record, worker/pump
evidence, boot/kernel logs and cleanup outcome. Keep native 48 kHz, four
8 ms slots per direction, safety 1024/2560, PCM offset 3072 and the 16 ms OUT
submission-lead gate. No listening test or latency sweep is part of this attempt.

A pass establishes one successful first start under the full-reset procedure,
not a fix; repeat on an independent boot before broader qualification. The same
failure makes full power removal insufficient under these conditions. Stop
reset/path repetitions and prepare the bounded **per-endpoint 48 kHz
activation → SET/GET** experiment offline, preserving direction order and all
transport margins. Do not combine it with 96 kHz priming or automatic retries.
A different failure is analyzed on its own evidence before choosing that change.

## Reproducibility

The audit inputs, function diffs, six pinned reference files, Claude request,
response and disposition are retained under
`build/validation/startup-offline-audit-20261020/`.
All 41 production build inputs still match the installed candidate's frozen
source archive. The prior baseline, dock and direct-path evidence remain sealed.
Only documentation and audit artifacts changed; the research archive was read
without modification. See [current status](STATUS.md) and the
[two failed hardware comparisons](STARTUP-48K.md).

## Subsequent owner-directed warm test

The owner removed USB and DC power and reconnected, but could not reboot the
Mac. We amended the evidence contract for one explicit warm epoch-2 attempt,
preserving the epoch-1 failure record and requiring clean prior teardown.
The [eight-second silent run passed](STARTUP-48K.md#warm-recovery-after-full-power-removal-passed)
with the same binary and CoreAudio process. The original fresh-boot experiment
remains unrun. A later normal Mac reboot with the interface left powered is the
next comparison; no additional power reset or sequence change is planned now.
Power-removal causality is not established without a no-reset warm control.
