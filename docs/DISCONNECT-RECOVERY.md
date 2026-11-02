# Disconnect retirement candidate — 30 October 2026

This is an offline-tested follow-up to the [selected-but-silent incident](DISCONNECT-QUARANTINE-20261030.md).
The prepared build is **`dev-fdb892cf3bcabfec`**, a separate 4 ms/twelve-slot
candidate. It has not been installed or tested
on hardware. Ordinary `make install` still builds the baseline profile.

## What changes

The independent native route no longer treats `AbortPipe` returning exactly
`kIOReturnNoDevice` or `kIOReturnNotAttached` as an ownership fault by itself.
It still requires every actual callback to return, every slot to be released
and acknowledged, the dispatcher to detach its sources and join, and no
poison/identity error. Rust must separately retire its matching slot generations
before native destruction. A missing callback, a timeout that has not drained,
wrong identity, callback panic or unrelated abort failure still retains the
context and prevents rebind. No quarantine-reset escape hatch was added.

Only after this retirement can the native adapter tolerate removal-related
alternate/cleanup/close results. It records each attempted operation and its
raw result. `NoDevice`/`NotAttached` establish removal for that endpoint;
`NotOpen` or `MACH_SEND_INVALID_DEST` alone do not. Those latter statuses are
accepted only once the retired endpoint is known gone. Generic errors and a
successful alternate readback of the wrong setting still fail cleanup. Each
buffer is handed to IOUSBLib for destruction once, then its interface is closed
and released, as in the previous adapter; the driver never manually frees or
retries a returned buffer pointer.

This follows the ownership behavior in Apple's historical
[IOUSBLib implementation](https://github.com/apple-oss-distributions/IOUSBFamily/blob/IOUSBFamily-560.4.2/IOUSBLib/Classes/IOUSBInterfaceClass.cpp):
buffer destruction releases local allocation after a failed kernel release,
and interface close handles remaining buffers. That old implementation is
supporting evidence, not a verification of the current macOS binary. Physical
disconnect qualification on the current system remains necessary.

Removal does not manufacture successful alternate or rate readbacks. The
removed mask is separate from `restored_mask`; an endpoint without removal
evidence must still have its original alternate and a valid 48 kHz rate.
Native directions are IN/OUT, whereas rate replies are OUT/IN. Successful
retirement of a removed service ends in **Fault 16 (device removed)** and cannot
trigger same-binding automatic deadline recovery. The existing monitor must
observe removal and a fresh USB enumeration before rebind and a new Start.
If removal status occurs while the same enumeration remains present, automatic
reopen stays refused. After all retained clients Stop, an explicit fresh Start
may retry a new, fully closed, pinned factory context on that enumeration.

Old active client IDs stay retired until their Stop/Remove acknowledgement.
Their delayed Stops cannot stop the new session; a stale binding cannot open
the new enumeration. Whether Spotify or GarageBand automatically issues the
required new Start after a physical reconnect is a hardware/application test,
not established by the offline transport tests.

The monitor now observes terminal quarantine even when detached with no active
clients, replacing a stale `healthy` recovery report. It announces a returning
device before explicitly notifying its running/alive properties, including a
returning quarantined device whose local `alive=false` value never changed.
Such a device remains diagnostically visible but is unavailable for Start.
Actual CoreAudio caching behavior still needs validation.

## Unchanged acceptance and scope

48 kHz, four-millisecond transfers, twelve slots, 64 ms count replay, PCM offset
3072, input/output safety 1024/2560, OUT lead 16 ms, IN checked/returned lead
2/1 ms, watchdog 64 ms, readiness window 26 ms and bounded catch-up work remain
unchanged. No audio-loss, identity, timing, latency-comparison or ownership
gate was relaxed. Normal qualification continues to reject a changed epoch;
automatic recovery never converts an interrupted run into a pass.

This addresses a demonstrated conservative shutdown path. The installed build
does not expose enough native stop detail to prove that path caused the real
incident. Other abort codes still quarantine; inspect the new record before
considering any broader treatment. The earlier repeated deadline faults,
first-start reliability, latency reduction and physical reboot qualification
remain separate open work.

## Shutdown diagnostic: `ftuQ` version 1

`build/hal-check --shutdown` reads the last native shutdown without starting
audio or opening USB. Its 44-word fixed-width snapshot is coherent, has one
worker writer and bounded lock-free readers, and survives new starts. Unknown
or unattempted fields are JSON `null`, not successful zero. No audio samples
are retained.

| Words | Meaning |
|---|---|
| 0–3 | Version, valid, present, session epoch |
| 4–11 | Stop finished, native stop succeeded, Rust slots retired, destroy attempted/succeeded, callback panic, cleanup report present, record count |
| 12–17 | Native record version, callbacks, faults, releases, acknowledgements, synchronous rejections |
| 18–25 | Actual native pending/held/returned slots, stop requested, joined, quarantined, poisoned, historical timeout |
| 26–27 | Signed abort status, IN then OUT |
| 28–33 | Cleanup version, attempted-operation mask, removed mask, restored mask, open mask, fatal cleanup status |
| 34–38 | IN: set alternate, read alternate, destroy data, destroy frame list, close interface |
| 39–43 | OUT: the same five operations |

Attempt bit `5 * direction + operation` guards each of the ten operation
statuses. Removed/restored/open masks use bit 0 for IN and bit 1 for OUT.
The route portion is captured before destruction, or again after failed
destruction while the retained route is still valid. Successful destruction
does not leave a pointer that is read for diagnostics.

`stop_finished` means the stop call returned, not that it succeeded. `ftuQ`
covers native route disposal and interface cleanup; it excludes the subsequent
Rust postconditions and factory-context close. Read `ftuS` and the finished
`ftuE` record for the terminal phase/fault and factory-close result. A normal
requested stop may have no new `ftuE` runtime-fault record. In particular,
`destroy_ok=1` does not by itself assert an entirely successful session.

`ftuF`'s legacy PoolSession pending array remains unrelated to actual independent
native ownership. Use `ftuQ.native_pending/native_held/native_returned` for this
shutdown. Do not use any diagnostic to override quarantine manually.

## Offline evidence

The frozen validation directory contains per-case logs, sanitizer scopes,
source hashes, the exact candidate/rollback identities, source archive and
Claude's review with dispositions. The reproduction and new tests cover:

- Real native callback delivery, acknowledgement, detach and join, including
  NoDevice/NotAttached aborts, missing callbacks, poison and late completion.
- The actual `duplex.c` attach/dispose adapter with injected interfaces,
  per-operation removal/cleanup failures, partial removal and contradictory
  alternate readback. A stand-alone runner registers every case:
  `python3 tools/ftu-probe/test_live_route.py`.
- The production Rust worker/control path linked to the actual native router:
  sixteen running disconnect/rebind cycles, fresh clock epochs, exact pinned
  enumeration, old-client retirement, continuous simulated HAL reads/writes
  and zero steady input/output loss after the existing warm-up window.
- A removal callback that self-stops before detach; a deadline that fails
  before removal is noticed; and no automatic reopen of either old binding.
- Missing abort callbacks with no active clients: retained context, terminal
  quarantine, truthful recovery state and no reopen/rebind.
- Partial removal in each direction checks the opposite, still-connected
  endpoint's rate reply. A swapped IN/OUT mapping fails the positive and negative
  tests. A removal status without enumeration change is also tested: retained
  clients cannot trigger a reopen; all must Stop before a fresh manual Start.
- Missing removal evidence: failed alternate/rate postconditions remain a fault.
  The linked fake derives its removal mask from real native abort statistics;
  the actual native inference/disposal is tested separately by the adapter suite.
- Existing timing/stall/startup, HAL boundary, callback-epoch qualification,
  baseline transport and analyzer regressions.

Final validation passed:

| Suite | Result |
|---|---|
| Integrated native/Rust | 44 cases normally, with Rust+C TSan, and with Rust ASan+C UBSan |
| Native router | 36 cases with C ASan/UBSan and C TSan |
| Actual native attach/dispose adapter | 14 cases with C ASan/UBSan and C TSan |
| Baseline regressions | 215 Rust tests passed, 3 existing ignored; 120 Python tests passed |
| Baseline lifetime/recovery TSan | 15 targeted cases passed |
| HAL boundary | Baseline and independent profiles passed C ASan/UBSan checks |
| Qualification tools | Callback/epoch guards and 60 analyzer tests passed |
| Builds/style | Both profiles built; warning-denied Clippy and format checks passed |

The last two review cases changed only test fixtures/tests. The final-source
executables ran all 44 cases in each configuration (15 recovery cases, then
29 remaining cases on that same executable). The earlier full 42-case results
and development failures remain in the evidence archive.

These are injected offline tests, not a new latency measurement or physical
sleep/wake pass. The installed binary remains unchanged during preparation.

## Hardware sequence

1. Install the frozen candidate using its validation installer; verify the
   loaded build identity before any audio test. Preserve the exact rollback.
2. Run silence preflight, stop and inspect `ftuQ`, `ftuS`, `ftuE`, `ftuH` and
   `ftuA`. Require full retirement, clean connected-device cleanup and no fault.
3. Repeat short loopback at actual 128 and 64 frames with the existing signal,
   steady-loss, ownership and timing criteria. Keep the prior latency baselines
   and failed historical 64-frame comparison unchanged.
4. With the user available, test a stopped reconnect, then an active USB
   disconnect/reconnect and application resumption. Inspect the old epoch's
   shutdown and new binding/epoch; never count the intentionally interrupted
   run as uninterrupted audio.
5. Only after those pass, test the reported laptop/dock departure and return.
   Verify that CoreAudio's public alive state agrees with the driver. Keep
   missing-callback or unexpected native errors as rejected evidence.

Installing a new bundle restarts the host and clears the old process's terminal
quarantine; this candidate cannot repair that already-retained live session
in place. No restart or physical manipulation was performed during this work.
