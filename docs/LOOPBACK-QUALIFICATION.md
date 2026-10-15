# GarageBand and physical loopback qualification

Completed the short baseline on 15–16 October 2026. Installed driver:
`dev-57287986d44956d9`, executable SHA-256
`1be5a06cc93e4c8fdfc950eaa3f688171fd7d769a3558d160e324290387c8af5`.
No driver update, hardware mixer write or native-rate change was made.

GarageBand playback was confirmed clean and correctly routed. It also recorded
the physical input-4 return. Six independent loopback sessions accepted all
twelve chirps, with actual callback sizes of 64, 128 and 192 frames. The measured
round trip is **110.54–115.87 ms**, too high for comfortable software monitoring.
This establishes a reproducible development baseline, not release qualification.

## Physical loopback and buffer results

The owner connected **rear output 3 → rear input 4**, with channel 4 set to
**Rear**. Monitors remained on outputs 1/2. GarageBand was closed during these
independent runs. The diagnostic never routed input to output; all outputs
except 3 were zero. Its two original chirps peaked at 0.01 (-40 dBFS).

| Requested and delivered frames | Independent starts / accepted chirps | Raw round trip (mean) | Timestamp round trip (mean) |
|---|---|---|---|
| 64 | 2 / 4 | 110.539 ms | 1.206 ms |
| 128 | 2 / 4 | 113.206 ms | 1.206 ms |
| 192 | 2 / 4 | 115.872 ms | 1.206 ms |

Every callback in each run had the stated size. All six runs had continuous
sample timestamps, flat post-start miss counters, zero capture misses/faults,
clean isolated teardown and the original 192-frame setting restored. Correlation
quality was approximately 0.99735, positive polarity, with no ambiguous return.
Return peak was approximately 0.0114. Trials and unrounded results are retained
in the [evidence index](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/captures/hal/2026-10-15-ultra-qualification/README.md).

Raw RTL counts delivered frames between output submission and input return.
Timestamp RTL maps those same events through their saved input/output sample
timestamps. Their difference was exactly `2 * buffer + 1024 + 4096` frames in
every trial. The latter values are the declared input/output safety offsets.
At 48 kHz those safety offsets alone total 106.67 ms. Device and stream latency
declarations were all zero; the remaining timestamp-relative delay was about
57.88 frames (1.206 ms).

**1.206 ms is not the usable software-monitoring round trip.** It excludes the
scheduling distance above. Nor does it split ADC/DAC delays or independently
calibrate the hardware clock. The very small variation across these short,
same-device correlations should not be interpreted as absolute timing accuracy.
The next engine work must explain queue lead, buffer ownership and timestamp
geometry before reducing safety offsets or assigning hardware latency values.

Trial 004 also passed a 20-second two-client silent scenario at requested/read-
back 64 frames, restored to 192. A ran for five seconds, B joined for five, then
continued for ten after A stopped. Both clients had zero invalid timestamps or
gaps and clean stop/destruction. Frames/callbacks average exactly 64; that older
diagnostic did not retain a span histogram. Twenty sampled running states held
output misses at 3712 and input misses at zero.

## GarageBand playback, recording and lifecycle

GarageBand 10.4.14 used a separate saved project,
`build/qualification/FTU Qualification.band`. An existing music project was
not edited. Both app devices changed from LG UltraFine Display Audio to Fast
Track Ultra; system default devices were unchanged. Software input monitoring,
count-in and metronome were off; compressor, EQ, echo and reverb were bypassed.

- A quiet original 30-second stereo fixture played left first, right next, then
  both. The owner reported **“Yes, clean and correctly routed”**. Thirty-one
  health snapshots stayed in epoch 23 with output misses flat at 4736, input
  misses zero and no faults.
- A dry mono track recorded input 4 while the external instrument sent two
  output-3 chirps. The saved file is 40 seconds of 24-bit mono PCM at **44.1 kHz**,
  while the device remained at **48 kHz**. Both chirps were detected with
  correlation above 0.991, positive polarity and three-second spacing. This
  verifies application conversion and recording, not native 44.1 kHz operation.
  Region position is not an independent RTL measurement.
- Play/stop, Cycle playback and playback after seeking to bar 10 were exercised.
  Twenty-six transport health snapshots and the post-seek snapshot kept output
  misses flat at 4864, capture misses zero and no faults. The exact number of
  cycle-boundary crossings was not retained.
- The saved project closed and reopened with both recorded and imported regions
  present. GarageBand then quit. Final status was Idle, epoch 23, buffer 192,
  zero pending callbacks, cleanup errors, faults and capture misses.

Between the listening interval and recording/transport interval, the output-
miss counter rose by **128 frames**, from 4736 to 4864 in the same epoch. The
exact UI/client transition was not continuously sampled, so its cause remains
unclassified. No listener defect was reported in the listening interval. Do
not call the whole session zero-loss or dismiss that increase as proven startup.
Add continuous lifecycle instrumentation before broader DAW qualification.

The diagnostic's own stop during recording deliberately left GarageBand running;
that shared run is application evidence, not an isolated latency acceptance.
The six RTL sessions above all ended Idle with no pending work.

## Rejected trials and input-3 investigation

| Trial | Observation | Interpretation |
|---|---|---|
| 001 | Eight seconds, 2,000 callbacks of 192 frames; input 3 exactly zero; flat post-start misses and clean stop | Rejected: no return. A subsequent permission query returned denied; routing cannot be inferred |
| 002 | Permission authorized; first callback triggered the input-level guard before either chirp | Rejected: no latency measurement |
| 003 | Digital silence output; first input sample `0.999999881` triggered the guard | Positive full scale before stimulus; physical loop/routing needed investigation |
| 005 | Owner disconnected the input-3 end; eight seconds at 128 frames had normal noise, peak `3.27825546e-05` | Clean passive capture; no chirps and no RTL claim |

Changing the physical loop to output 3 → input 4 then produced the successful
measurements. The full-scale input-3 value was associated with the connected
3→3 loop, not a permanently saturated capture channel. Hardware direct-monitor
feedback is plausible but **no mixer route has been proved**. Do not repeat
that same-channel loop until its route is understood. The rear line input's
source level and the device's internal monitor mix are separate controls.

macOS attributed shell diagnostics to ChatGPT. Its microphone switch was
initially off; GarageBand's was already on. The owner authorized temporary
access for this test. **ChatGPT's switch was restored to off**, and a fresh
read-only query reported denied. macOS warned the already-running parent app
may retain access until it quits; “Later” preserved the chat. No diagnostic
remains running. GarageBand's existing permission was unchanged.

## The apparent 192-frame limit

The external HAL reports a buffer range of **15–192**, despite the plug-in's
source declaring 64–1024. Only 64/128/192 were qualified here; neither the
15-frame lower bound nor a 1024-frame request is a streaming qualification.

The E-MU clean repository documented empirical CoreAudio behavior:
`maxBufferFrames = min(4096, ZeroTimeStampPeriod * 3/8)`. FTU publishes 512;
`512 * 3/8 = 192` exactly matches the observed cap. This is a strong explanation,
not an API guarantee or an FTU period-change experiment. Earlier 1024-frame
requests succeeded but read back 192.

The local macOS SDK's `AudioServerPlugIn.h` documents a minimum zero-timestamp
period of 10923 frames. FTU's 512 needs a deliberate timing review. Raising it
blindly could expose buffers larger than current 1024-frame guards and change
clock/scheduling behavior. Record this compatibility issue for implementation.

ASFW's `hal_geometry` method emphasizes reading host-visible values and actual
callback spans. The E-MU references are `docs/FINDINGS.md`, “The HAL's cap on a
client's IO buffer”, and `driver/usb_engine.h`. No E-MU files were changed.

## Reproducibility and next work

The [external instrument](../tools/hal-loopback/README.md) has six offline
analysis tests and ASan/UBSan callback checks. Evidence retains rejected runs,
source/binary hashes, raw audio, callback timestamps, application observations
and final cleanup. [ASFW lessons](ASFW-REUSE.md) distinguish adopted measurement
methods from future engine work and later E-MU candidates.

The short baseline was used for the clean-repository handoff
with explicit limitations: high monitoring latency, host buffer geometry, the
unclassified 128-frame transition, unresolved input-3 monitor routing, untested
channels/models/rates and long-session recovery. It is not a production release
or an all-channel/all-DAW pass. Continue with [the development roadmap](ROADMAP.md) and retain this baseline
when changing timing or routing. [Provenance](PROVENANCE.md) records the extraction.
