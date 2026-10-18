# Longer sessions and GarageBand transitions — 18 October 2026

## Driver and scope

The installed and loaded driver remained `dev-8387395854108015`, SHA-256
`87a3378b39beceedca5c223b906d7d7c90cffc703fe67b51194507851d8c2780`.
No driver source, installed bundle, safety margin or USB geometry changed during
these tests. Test tools were extended separately. This is one desktop Ultra,
one Apple Silicon Mac, macOS 26.7.1, native 48 kHz, output safety 2560,
input safety 1024 and four 8 ms USB slots per direction.

Rear output 3 was cabled to rear input 4, with channel 4 set to Rear.
Only input 4 was retained by the physical diagnostic; quiet chirps used only
output 3. Application input monitoring remained off. The owner was away:
these results are measured transport/recording evidence, not a listening pass.

## Two ten-minute physical duplex runs

An eight-second silence preflight preceded two independent ten-minute runs.
Each long run continuously submitted output and captured input, with a quiet
chirp every five seconds. This is continuous duplex transport with periodic
stimuli, not ten minutes of continuous audible music.

| Actual buffer | Captured frames | Callbacks | Chirps | Raw round trip | Minimum OUT submit lead |
|---:|---:|---:|---:|---:|---:|
| 64 | 28,800,000 | 450,000 | 120 | 78.539 ms | 18 ms |
| 512 | 28,800,000 | 56,250 | 120 | 97.206 ms | 17 ms |

Both passed the predeclared gates: all callbacks had the requested size,
timestamps stayed continuous, every chirp correlated, latency stayed within
two frames of the preceding short qualification, steady loss counters stayed
flat, and teardown returned to Idle with no pending transfers, faults or cleanup
errors. Both restored the original 512-frame buffer.

The two runs retained 300,064 worker observations with zero drops or attention
events below the 16 ms submission-lead gate. The 17 ms worst case leaves only
1 ms above that experimental gate; it is evidence against reducing margins
without further scheduling work. The additional availability screen retained
at least 156 frames at 64 and 150 frames at 512 after reserving one client buffer
and a maximum USB block. This screen is a heuristic, not a proof of contiguous
sample ownership or an operating-system scheduling bound.

Startup playback-miss totals were 1944 and 2772 respectively and then stayed
flat. Capture-miss totals stayed zero. These are zero **steady** losses, not
zero lifetime playback misses. The two runs cover 20 minutes, 57,600,000 captured
frames, 506,250 callbacks and 240 chirps.

## GarageBand

GarageBand 10.4.14 used Fast Track Ultra for both input and output. A separate
copy of the qualification project preserved the original audio files. Input
monitoring, count-in, metronome and noise gate were off; compressor and EQ were
bypassed. The recording track remained muted.

The following actions completed under a continuous observer:

- Three observed play/stop pairs and a verified backward seek while playing.
- At least **315.277 seconds of cycle playback**, with the first four bars
  repeating over eight seconds at 120 BPM. This is a duration/range observation,
  not a separately counted number of loop boundaries.
- A **180-second shared-client stimulus**, while GarageBand recorded mono input 4.
  The external client delivered 8,640,000 frames in 16,875 actual 512-frame
  callbacks; all 36 chirps correlated. No buffer change was requested.
- Recording stop, save, project close, reopen with the new region present,
  and application quit.

The saved take is a **216-second, mono 24-bit, 44.1 kHz WAV**, including time
before and after the stimulus. Offline conversion to 48 kHz found exactly
**36 returns**, with minimum normalized correlation **0.997926**. Their spacing
was 239999.999774–240000.000250 frames, within the predeclared three-frame
allowance around five seconds. Original and converted hashes and the conversion
command are retained. This exercises application sample-rate conversion; the
driver still runs only at native 48 kHz. Recording-file positions do not establish
physical round-trip latency.

The observer retained **17,340 status samples** spanning startup through final
Idle. During established epoch 22, playback misses stayed at the startup total
of 1895 and capture misses stayed zero. No faults, cleanup failures or counter
regressions occurred. GarageBand kept the device running while its transport was
stopped and across project close/reopen; quitting finally returned it to Idle
with no pending transfers. The device buffer read back as 512 afterward, so no
restoration write was needed. The installed binary hash was unchanged.

HAL writes were 128 frames throughout the app epoch; this is the observed HAL
boundary, not a direct measurement of GarageBand's internal processing size.
The shared auxiliary client's callbacks were 512 frames. Steady timing snapshots
reported no input/output payload loss and a minimum OUT submission lead of
18 ms. The observer's maximum status-sampling interval was 137.952 ms: cumulative
counters retain intervening losses, but this does not provide exact causal
attribution to individual UI actions or clients.

The auxiliary run's ordinary isolated-run analyzer correctly retained
`unclean_or_shared_stop`, because GarageBand continued using the device when the
auxiliary client stopped. That result remains rejected as an **isolated latency
trial**. The separate, predeclared application assessment accepts the recorded
signal, shared lifecycle and eventual app teardown; it does not remove or
reinterpret the original rejection.

This sequence did not reproduce the earlier research build's unclassified
128-frame transition loss. It does not explain or erase that older event.

## Reproducibility and evidence

Local ignored evidence is retained under:

- `build/validation/long-session-20261018/`: predeclared plans, exact tool hashes,
  runner versions, results, UI markers, observer trace and a separate GarageBand
  project copied from the research qualification project.
- `build/trials/2026-10-18-long-session/`: original stimuli, captures, callback
  records, run logs and assessments for every physical attempt.

The first silence preflight was incorrectly rejected by the orchestration
wrapper because it compared integer callback-size keys against strings.
The original physical capture, false assessment and wrapper were preserved.
A separately hashed correction normalized those keys and re-evaluated that same
capture offline; it passed. No hardware rerun replaced the attempt. Both long
runs used `run_extended.v2.py` and have separate `results.v2.json` evidence.

The ordinary `hal-loopback` limit remains 30 seconds. `hal-soak` explicitly opts
into a 600-second bound and preallocated callback storage. `hal-observe` reads
status at approximately 20 Hz and timing at approximately 1 Hz without starting
audio or opening USB. Its timestamps bracket property reads; cumulative counters
can bound changes between observations but cannot identify every HAL call or
assign a change to a particular app. See the
[diagnostic guide](../tools/hal-loopback/README.md) for commands and analysis.

Offline checks passed the callback ASan/UBSan checks, the separate 600-second
capacity build's callback checks, 26 Python analysis regressions and a read-only
observer smoke check. Exact diagnostic sources and binary identities are retained
in `diagnostic-source.tar.gz` and its manifest. Test-only Makefile changes affect
future source-derived build identities; they do not relabel the installed build.

The earlier short qualification and its frozen candidate were not rewritten.
The exact four-slot/output-3072 rollback remains `dev-517459af36927152`.
The rejected three-slot experiment remains rejected. Wider host/channel/rate
coverage, sleep/wake, hotplug, recovery and long-session scheduling guarantees
remain outside this bounded test.
