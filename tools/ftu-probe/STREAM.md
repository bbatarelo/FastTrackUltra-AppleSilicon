# Reusable-buffer Rust stream experiment

```sh
make -C tools/ftu-probe
tools/ftu-probe/build/ftu-probe stream-silence --output NEW_DIRECTORY --product 2080 --duration-ms 10000
python3 tools/ftu-probe/analyze_stream.py NEW_DIRECTORY/session-001
```

The new command exercises continuous buffer reuse while the standalone diagnostic
bounds the experiment. `stream-silence` accepts multiples of 8 ms in 8–60,000 ms,
default 10,000, and `--cycles 1..3` (default 1). Cycles run in the same process,
fully drain/close the previous context and create a fresh epoch/context. Each
gets `session-001`, `session-002`, etc. The root has device `metadata.json`.
Existing evidence is never overwritten. Selection accepts product 2080/2081
and optional `--location HEX_LOCATION`; only the desktop Ultra is tested.

The device must already report 48 kHz on OUT01/IN81, be high speed and have both
streaming interfaces idle at alternate 0. The adapter prepares/activates IN81
interface 2 followed by OUT01 interface 1. No mixer/rate SET, reset, device
seize, firmware operation or HAL installation is performed. IN82 is not used.

## Ownership and memory

Rust owns four slots per direction and a 32-entry sequence-tagged feedback
store. An OUT packet mirrors the corresponding successful IN packet's actual
positive whole-frame length, within the descriptor's 312-byte capacity. Observed
5/6/7-frame counts are a result, not a hardcoded accepted-count list. There is
no nominal-size fallback. Each request spans 64 microframes/8 USB milliseconds;
IN starts with 16 ms lead, and OUT sequence n starts 64 ms after IN sequence n.

Reuse requires the matching completion ticket. Every native submission also
gets a unique opaque callback token, never dereferenced and never reused within
that thread, including across contexts. Duplicate/stale tokens cannot release
the current slot. Callbacks without an active context latch an orphan count;
a later session reports a callback fault and refuses activation. The standalone
adapter supports one active pool context per thread.

Native audio buffers occupy a fixed 159,744 bytes (eight times 19,968), plus
fixed frame lists/adapter metadata. Transport state has fixed size. The diagnostic
journal is separate: it is allocated for the requested finite duration before
activation and copies status/timestamps/counters, not native buffer references.
It grows with the requested test duration, not during the stream. Tone samples
are precomputed before activation. Simulations instrument Rust heap allocation
and show none in run/drain; native framework realtime behavior remains unproven.

A slot is filled only before submission and inspected only after completion.
CRC32 of the full OUT allocation is retained before and after each valid request.
CRC is a diagnostic content check, not a cryptographic integrity primitive.
The independent Python audit regenerates expected bytes and checks CRC, peak,
nonzero counts and channel windows. Input PCM is not recorded by this mode.

Four pending requests give about 24 ms from a slot's completion until its next
scheduled start, less the scheduling guard and call overhead. The 64 ms feedback
offset is not extra callback-stall tolerance. No late queue rebasing occurs.

## Clock and faults

Feed one observation per USB millisecond, in input sequence order, even if
callbacks arrive out of order. Use the final processing timestamp of its eight
microframes; entries may be equal or nondecreasing. The first listening trial
disproved the original equality assumption despite successful audio packets.
Zero timestamps, backwards values within/across groups and a group span over
5 ms invalidate the clock. Samples are cumulative actual frames before the
group. `SampleClock` fits frame count against relative Mach time using stable
least squares; at least 64 observations and 100 ms span are required. A mature
rate must be within 1% of 48 kHz. Timestamps must be nonzero, increasing, and
0.5–5 ms apart. A foreign epoch is rejected without changing the current fit;
an in-epoch clock fault invalidates it and stops this diagnostic session.

The saved fit measures a slope against USB host timestamps. Its anchor convention
is not a calibrated sample-presentation time. No extrapolation, HAL zero
timestamps, coherent cross-thread publication or analogue latency is implemented.
A different controller timestamp convention can cause an explicit stop. The
all-session fit is not a fast detector for every oscillator change.

A signal or fault stops admission, aborts only pending pipes and drains callbacks
for up to one second. A wrong ticket does not release ownership; the original
reason is retained while other callbacks drain. Unresolved ownership returns
exit 4 and the C standalone wrapper exits without freeing live native resources.
A private pending-ownership status distinguishes this from an ordinary IOKit
teardown error. This is not in-process recovery for a long-lived HAL host.
Normal teardown restores both alternates before journal formatting/filesystem I/O.

## Files and replay

New plans use schema 3 and `clock_group_anchor: last_processing_tick`. The
analyzer also replays schema 2 using its original equal-timestamp convention.
Each session contains `plan.json`, `config.bin`, `events.jsonl`, `trace.jsonl`
and `summary.json`. A flushed `stream_ready` event precedes clock sampling and
submission. All further evidence formatting happens after drain and cleanup.

The trace has one row per attempted request: epoch, direction, sequence, slot,
ticket, USB start/admission frame and Mach admission time, lengths, sample cursor,
submission result, full completion status/length/timestamp arrays, CRC and level
statistics. Unattempted records are omitted. `admission_ticks` must follow the
previous owner's callback time when a slot is reused. Failed/aborted requests
are not accepted as audio or feedback. `output_planned_frames` includes a failed
OUT submit attempt, whereas `accepted_frames` counts only wholly valid blocks.

The summary distinguishes submitted/completed/pending, reused slots, accepted
frames, abort/drain outcome, cleanup/restore faults, rate reads and the last
observed `capture_fit`. A valid saved fit describes past observations, not a
clock still running after shutdown. Both endpoint rate arrays are ordered
OUT01 then IN81, independently of direction 0=IN/1=OUT in the trace.

`analyze_stream.py` handles plain/gzip traces, validates geometry and callback
accounting, slot generations/admission ordering, packet mapping, payload checks,
clock anchors and an independent batch regression. `audit:passed` can describe
an internally consistent interrupted run; check `experiment_status` separately.

Exit 0: complete/restored; 3: incomplete including a drained signal; 1:
setup/evidence/teardown error; 2: invalid usage; 4: unresolved native ownership.
There is no automatic retry after a fault.

## Optional listening test

Only with the listener ready and monitor level low:

```sh
tools/ftu-probe/build/ftu-probe stream-tone --output NEW_DIRECTORY --product 2080
```

Fixed 10 seconds, one session. Repeat the existing two-second waveform: ramped
440 Hz on channel 1 at 0.1–0.7 s, then 660 Hz on channel 2 at 0.9–1.5 s, followed
by silence before repeating. Peak -40 dBFS, 20 ms ramps, channels 3–8 zero.
Existing mixer routing is preserved. This exercises tone continuity across
reused buffers; it is not application/CoreAudio playback.
