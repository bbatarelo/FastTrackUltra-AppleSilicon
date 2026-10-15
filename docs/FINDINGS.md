# Findings that guide implementation

These are measured facts and bounded interpretations from the research baseline.
See [provenance](PROVENANCE.md) for the archive revision and source references.

## Device and transport

- Desktop Ultra `0763:2080` was tested with external power. Ultra 8R `0763:2081`
  is a shared profile target, not a hardware-qualified model.
- The real 340-byte configuration descriptor describes eight channels of packed
  24-bit little-endian PCM: three bytes per sample. The descriptor fixture is
  retained and hashed. Do not substitute 32-bit USB samples because HAL uses
  Float32 or infer wire packing from a host-language integer type.
- Playback is OUT01; capture is IN81. Playback packet planning uses observed
  capture packet lengths. IN82 exists and returned a constant `008001` at the
  reported 48 kHz mode, but clock tracking from that endpoint is unproved.
- Descriptor OUT attributes differ from Linux's fixed-format override. Preserve
  the observed descriptor and distinguish protocol references from measured
  hardware. The descriptor lists more rates than the current driver supports.
- USB completion timestamps are processing observations, not automatically
  packet presentation times. The accepted grouping convention uses each
  millisecond group's last processing tick; individual group entries may differ.
  Retain the clock/ownership regression tests when changing this interpretation.

## HAL and applications

The device exposes eight interleaved Float32 channels at native 48 kHz. VLC
playback of 44.1 kHz and GarageBand recording to 44.1 kHz demonstrate application
conversion only. Desktop-Ultra outputs 1/2 were heard cleanly; output 3 → input 4
was measured. No all-channel or Ultra 8R pass follows from those results.

The external HAL reports a 15–192-frame buffer range although plug-in source
declares 64–1024. Actual 64/128/192 callbacks passed short loopback sessions.
1024 requests returned success but read back 192. E-MU's empirical
`min(4096, ZeroTimeStampPeriod * 3/8)` cap predicts 192 from FTU's period of 512;
this is an explanation to verify, not an API guarantee. The local SDK documents
a minimum zero-timestamp period of 10923. Review that period, the published clock
and maximum supported buffer capacity together.

Input/output safety offsets are 1024/4096 frames. Raw loopback was approximately
111–116 ms while timestamp RTL was 1.206 ms. Their paired difference was exactly
`2 * buffer + safety_in + safety_out`. See
[the measurement definitions](../tools/hal-loopback/README.md). These values
are the FTU baseline, not constants to copy to E-MU or other devices.

GarageBand's sampled playback and later transport intervals each had flat loss
counters, but the counter increased by 128 frames between them. Its exact cause
is unclassified; continuous transition tracing is required before a full DAW
stability claim. Final teardown was Idle with no pending work or faults.

## Controls, input routing and permissions

Initial read-only mixer crosspoints all returned zero, but existing playback
worked. Therefore a successful GET and numeric zero are not proof of the
physical routing state. No mixer writes were made in the baseline.

An output-3/input-3 physical loop saturated before any chirp. Disconnecting input
3 restored its noise floor; output-3/input-4 worked. Direct-monitor feedback is
plausible but its route is unproved. Front/Rear selection and source-level gain
must be checked separately from the device's monitor mixer. Keep software
monitoring off during these loopback experiments.

Denied microphone permission produced all-zero CoreAudio input without a Start
error. The new loopback instrument checks authorization read-only before audio.
A caller's parent application may own that permission; do not reset or enable
privacy settings silently. Restore temporary access as agreed with the owner.

## Open boundaries

MIDI, hardware DSP controls, reliable mixer read-back, native rate/clock changes,
automatic recovery, sleep/wake and multiple devices remain planned work.
The current engine's terminal quarantine prevents unsafe resource reuse; it
is not a complete recovery mechanism. Packaging validation is separate from
installation and audio qualification. Follow [ROADMAP.md](ROADMAP.md).
