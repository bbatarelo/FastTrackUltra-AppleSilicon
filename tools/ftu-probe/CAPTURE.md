# Bounded capture experiment

```sh
make
build/ftu-probe capture --output NEW_DIRECTORY --product 2080 --duration-ms 256
```

`--location HEX_LOCATION` selects a specific unit when necessary. The same code
recognizes the Ultra 8R (`2081`), with separate hardware validation. Duration is
a decimal multiple of 8 ms, from 8 through 2048; the default is 256 ms. Existing
evidence directories are refused before accessing hardware.

This command records only the eight-channel input stream. It requires the
existing rate reads on endpoints `01` and `81` to both report 48,000 Hz, a
high-speed connection, and the validated interface-2 alternate-1 profile. It
does not change rates, write mixer controls, send output audio or install a HAL.

Only capture interface 2 is opened, without seize. It must initially be idle
at alternate 0. The command allocates its full bounded memory, installs its USB
run-loop source, then selects alternate 1. After capture it restores alternate
0 on that same interface, releases buffers and closes its own connection.
Interfaces 0, 1, 3 and 4 are not opened. The fixed rate GETs use endpoint 0
without opening the device.

## Files and meaning

| File | Contents |
|---|---|
| `metadata.json` | Device identity and cached getters, saved before requests or interface opens. Its status describes this metadata collection, not the final capture. |
| `plan.json` | Duration, required rate, fixed geometry, scheduling lead, queue depth, drain timeout and Mach timebase, saved before USB requests. |
| `capture-config.bin` | Cached current configuration used by the fixed-profile validator. |
| `events.jsonl` | Setup/teardown operations, exact IOReturn values and host ticks, flushed during those phases. |
| `observations.jsonl` | Every planned service interval, in scheduled order, with submission/completion state, raw packet status, requested/actual length, timing and payload offsets. Written after drain. |
| `packets.bin` | 312-byte reserved slots for completed requests whose callback identity matched. Padding is retained; only the observed actual length describes returned data. Missing requests occupy no file space and have null offsets. |
| `audio.s24le` | Accepted samples concatenated as eight interleaved signed 24-bit little-endian channels. This file removes rejected intervals; use the observations and segment IDs to locate gaps. It is not by itself a continuous sample timeline. |
| `summary.json` | Final accounting, errors, signal, before/after rates, cleanup outcome, clock anchors and per-channel peak/RMS/nonzero counts. Also printed to stdout. |

The hardware trial record adds source/build hashes and registry snapshots; the
probe does not invent a source revision for an uncommitted executable.

Each request holds 64 service intervals (8 ms at high speed). Its start is a
1 ms USB bus-frame coordinate, so successive starts differ by 8. Up to four
requests are outstanding, with a 16 ms initial lead. Each request owns a
different region of the preallocated buffers and is never recycled. New
submissions stop if the schedule becomes stale; the probe does not silently
rebase a missing section.

`requested_length` is 312, the endpoint capacity, not the expected number of
audio bytes. At nominal 48 kHz, six audio frames occupy 144 bytes. Packet status
success or `kIOReturnUnderrun` can be accepted when the actual count is bounded
and divisible by 24. The latter status is retained and counted separately;
the SDK describes it as less data than the maximum packet size. Other errors
are not treated as audio. Zero-length packets remain explicit observations.
The enclosing request must also complete successfully or with the short-packet
underrun status. Data from an aborted/failed request is retained in raw slots
but conservatively excluded from accepted audio, even if some entries look
valid. Missing/unsubmitted observations have their own `unobserved_packets`
counter, separate from observed but rejected packets.

`usb_timestamp_ticks` comes from `frTimeStamp`; `callback_ticks` is the later
delivery time to this process. Both retain raw Mach ticks with the timebase in
the plan and summary. Neither is an analogue input-latency measurement. The
optional bus anchor records whether `GetBusFrameNumberWithTime` succeeded.

## Stop and failure behavior

SIGINT/SIGTERM handlers only set a flag. The command stops admissions, aborts
pending input transfers, and continues dispatching callbacks for up to one
second. All successful submissions must complete before buffers are released.
If callbacks do not drain, it records the completed portion and exits with
code 4, retaining USB resources until process termination. Pending frame-list
memory is not inspected or freed. This exceptional path is a diagnostic escape
route, not validated recovery for a future long-lived HAL process.

| Exit | Meaning |
|---:|---|
| 0 | All planned transfers/packets accepted, rates still 48 kHz, normal teardown. Silence can still produce this result. |
| 1 | Selection/initial metadata access, evidence-file, ownership or cleanup error. Inspect the available metadata, events and summary. |
| 2 | Invalid command-line arguments. |
| 3 | Capture precondition/setup failed or capture was incomplete/interrupted. |
| 4 | Callback drain deadline; resources retained until process exit. |

Source/cached-getter failure before `capture_run` may leave only `metadata.json`.
Synchronous IOKit calls use their platform behavior; the deadline bounds the
asynchronous capture loop and drain, not an OS call that itself stops returning.
Rate GETs each have 500 ms timeouts and no retries.
If writing or closing the summary itself fails, the final stdout summary and
process exit status report that error; the saved summary may be incomplete or
may predate the close failure.

## Offline validation

`make test` includes portable ownership/conservation tests, real-descriptor
profile checks, invalid-CLI checks, and an exhaustive round trip across all
2^24 packed sample values. Conversion tests also cover nonfinite Float32
values, clipping, rounding, strides, overflow and exact buffer bounds.
`make sanitize` runs the new portable C tests under AddressSanitizer and
UndefinedBehaviorSanitizer. These targets do not access hardware.

The [API review](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/USB-CAPTURE-CONTRACT.md) distinguishes allocation,
callback ownership, bus scheduling and hardware timestamps. Actual capture,
repeat-start and stop results belong in the hardware evidence report.
