# USB reconnect candidate — 26 October 2026

**Reviewed offline on 26 October; installation verified on 28 October.**
The installed candidate has a [runtime output-deadline fault](RUNTIME-STOP-20261028.md).
No physical reconnect qualification has been performed; address that reliability
incident before the next hardware tests. Candidate
`dev-0d800cbf6bdead51` keeps the installed candidate's twelve 4 ms slots and all timing
margins. Ordinary `make install` still builds the
8 ms/four-slot profile, now with the same reconnect code. For the controlled
comparison use the frozen candidate below, not ordinary `make install`.

## Problem and fix

Moving the Ultra from direct USB `0x02100000` to hub `0x20211100` left it visible
on USB but absent from CoreAudio. The HAL had adopted its location once, for the
host process's lifetime. The fix retains one resident Rust handle and changes
its binding only after the old session is stopped and quiescent. The 500 ms
monitor detects changes in product, location and IORegistry entry ID, including
re-enumeration at the same port between polls. It publishes removal and then
arrival on separate polls. USB open is pinned to the discovered entry, checked
before creating a user client; it cannot accidentally open a replacement at the
same location between discovery and startup.

Rebind and Start/Stop share the existing Rust control lock. Real-time callbacks
still use the permanent handle, atomics and bounded timelines; no new USB work,
locking or allocation enters the audio callback. New starts use a new timestamp
epoch. An old queued Start is rejected if the binding has changed. Active
client IDs are retired at detach: an ID cannot restart until its old Stop or
RemoveDeviceClient acknowledges it. Active plus retired IDs are bounded at 64;
unknown stops do not interrupt a running different client.

The port-independent UID is now `net.batarelo.FastTrackUltra:2080:single`
(or `2081` for 8R). The desktop Ultra fixture has no USB serial, so this is one
logical device per adopted model, not a proven physical-unit identity. More
than one supported unit is refused; changing Ultra/8R model within the same
host lifetime is also refused. The UID change requires users/apps to select
the device once after installation. The read-only `hal-check --hotplug` option
prints the binding/counters/refusal property when a device is published.

## Review and offline validation

Claude implemented the initial patch and the requested corrections. Codex
rejected the first patch's same-client-ID delayed-stop race and its unpinned
USB-open window. A regression reproduces the first issue against that initial
patch. The revised tests reject reused IDs until acknowledged and reject stale
binding/open generations. Codex added tests of the actual native registry scan
and open guard, fixed one missed fake-open call after the signature change,
formatted Rust, and exposed the read-only diagnostic flag.

- `make test`: 208 Rust tests passed, with 3 existing ignored tests; native C
  regression/mutation checks and 120 Python tests passed.
- `make test-hal test-hal-independent`: both real HAL boundaries passed with
  C AddressSanitizer/UBSan; actual native discovery/open tests also passed.
- Linked independent suite: 21/21 cases passed normally, with Rust+C
  ThreadSanitizer, and with Rust AddressSanitizer+C UBSan.
- Targeted baseline ThreadSanitizer: 8/8 groups passed, including a running
  two-client detach/rebind, bounded retired IDs and quarantine negative control.
- Both Clippy profiles passed with warnings denied; Rust formatting passed.
- Diagnostic tools and loopback callback checks built/passed; all 60 analyzer
  tests passed. Both locally signed driver profiles built successfully.

Initial compile failures from the one missed test call and the first
ThreadSanitizer sandbox/cache failure are retained alongside successful reruns.
No safety or timing gate was relaxed to obtain a pass. Reconnect does not
change the historical failed 64-frame latency comparison, and does not establish
a below-10-ms round trip. No hardware/audio stream was started by this work.

## Limits and next physical check

The target is **idle or clean-drain reconnection**. Active unplug can cause
AbortPipe, alternate-setting, buffer-destruction or close failures; the existing
terminal quarantine is preserved. It must never reopen over uncertain callback
ownership. The independent completion router's existing stop/removal cases pass,
but there is not yet an end-to-end running independent-worker reconnect fixture
through its factory initializer. Physical active-unplug recovery and sleep/wake
remain unqualified. This is not a claim that every unplug automatically recovers.

1. Install the frozen candidate once, with playback stopped. Confirm its loaded
   build identity; select it once because the UID changed.
2. Check idle removal/replug at the same port, direct-to-hub movement, and a few
   repeated reconnects without restarting CoreAudio. Confirm stable UID, changed
   USB entry ID, one device, and no duplicate/stale session.
3. Run silence preflight then the existing short loopback with actual buffer
   size stated and the unchanged loss, teardown and latency-comparison gates.
   Loopback wiring must be confirmed after the cable/port changes.
4. Only then qualify removal during streaming; preserve any quarantine/error
   evidence and use rollback when appropriate. Apps may need to stop/reopen.

Install (administrator password; backs up the driver and restarts audio):

```bash
/bin/bash /Users/bruno/development/projects/bruno/AudioInterfaces/FastTrackUltra-AppleSilicon/build/validation/reconnect-20261026/install.sh
```

Exact rollback to the preserved pre-reconnect candidate:

```bash
/bin/bash /Users/bruno/development/projects/bruno/AudioInterfaces/FastTrackUltra-AppleSilicon/build/validation/reconnect-20261026/rollback.sh
```

Both scripts support `--check` for validation without installation. Candidate
binary SHA-256: `a70672c39983242e1446c1dbe0dd2728a91a7535f27aa39453eea9212397f8e5`. Rollback SHA-256: `48358f0eee994ff8d958935dd38a23e854e02300673a57c2d5bbd8ffec8f99ad`.
Evidence: `build/validation/reconnect-20261026/`; candidate, rollback and diagnostic:
`build/candidates/hotplug-reconnect-20261026/`. No commit or installation was made during the 26 October offline work.
The 28 October inspection subsequently verified this candidate installed and loaded.

The separate [E-MU source audit](EMU-HOTPLUG-AUDIT.md) confirms normal port moves
are handled there, while identifying same-port replacement and stale-callback
risks. No E-MU source or installed driver was changed.
