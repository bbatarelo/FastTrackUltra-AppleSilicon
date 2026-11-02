# E-MU reconnect audit — 26 October 2026

The clean `emu-tracker-pre` project does **not** have the Fast Track Ultra's
permanent first-port binding. Its registry retires a departed engine and creates
one at a newly discovered location. CoreAudio UIDs use the unit's USB serial, so
an ordinary port move preserves identity. This is a source audit with a small
offline registry fixture, not a new hardware qualification of E-MU hotplug.
No E-MU files, installed drivers or running devices were changed.

Audited checkout: `/Users/bruno/development/projects/bruno/emu-tracker-pre`,
commit `5921d33` (full commit and source hash retained in the evidence).
The original `/Users/bruno/development/projects/bruno/emu` is the earlier
research implementation and is not the contributor driver's current reference.

## Evidence for normal port moves

- `driver/usb_engine.c`: first-match and termination notifications are registered
  per supported product on a serial queue. Attached-count changes notify the HAL.
- `driver/plugin.c`, `reconcile_devices()` (around line 644): slots whose locations
  disappear are retired; newly seen locations are adopted. Engines are destroyed
  outside the state lock.
- `device_adopt()` (around line 598): the new engine receives the current location;
  the published UID is derived from the serial rather than the location.
- `shared/usb_util.c`, `emu_enumerate_units()` (around line 38): enumerates product,
  location and serial for attached units.

A source-extracted fixture uses the actual registry functions with stub engine
creation/destruction. It passes an ordinary port move with stable UID and an
observed absence/arrival with a fresh engine. It runs under C AddressSanitizer and
UndefinedBehaviorSanitizer. The fixture does not exercise IOKit notifications,
CoreAudio or real USB teardown and does not establish hardware reliability.

## Related issues to track separately

1. **Same-location replacement is not distinguished.** Both `still_there` and
   `slot_for_location()` compare location only. If discovery sees a replacement
   at the same location without first seeing an empty slot, the old engine and
   UID remain even when the serial has changed. A same-unit re-enumeration is
   also indistinguishable: `EmuUnit` contains no USB service generation. The
   fixture reproduces the different-serial case. Whether actual notifications
   expose the empty interval is hardware/timing dependent.
2. **Old failure callbacks can affect a reused slot.** `device_adopt()` reuses a
   `Device` slot before `reconcile_devices()` destroys the retired engine. Its
   failure-handler context is the slot pointer, and `engine_failed()` has no
   generation check. A callback already selected by the old worker can mark the
   replacement device not alive. The fixture delivers that stale callback after
   reuse and reproduces the effect; it does not measure the real-world race rate.
3. **Published slot/engine lifetime is not synchronized with every HAL reader.**
   `device_for_object()` reads `present` without the registry mutex, and
   `DoIOOperation()` dereferences `dev->engine` while reconciliation may replace
   it and free the retired engine. The architecture document already records the
   `present` race. Worker shutdown alone is not proof that all external HAL
   readers have relinquished an engine pointer. This is a source-level lifetime
   risk; no live crash or full-engine sanitizer reproduction is claimed here.

A future E-MU fix should add a USB generation/identity comparison, prevent stale
callbacks from changing a new session, and establish safe retirement relative to
HAL readers. Do not copy its immediate slot reuse/freeing into FTU. Keep the
single-unit FTU reconnect fix separate from E-MU's multi-device registry work.

## Retained local evidence

`build/validation/reconnect-20261026/emu-audit/` contains the source-extracted C
fixture, build/run results, full audited commit and source SHA-256. Its assertions
for the two issues intentionally confirm the current defective behavior; a
successful fixture exit is not a claim that those issues are fixed.
