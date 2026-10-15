# Bounded output-3/input-3-or-4 loopback instrument

This is an external CoreAudio test client, not part of the driver. The driver
continues to use Rust with a thin C boundary. The instrument uses C for CoreAudio,
one Objective-C read-only permission query, and offline Python/NumPy analysis.
It opens no USB interface and does not install or change the driver.

The measured setup is **rear line output 3 to rear line input 4**, with
**Rear** selected for input 4. Disable software monitoring and avoid a hardware
route feeding the returning input back into output 3. The original 3→3 setup
saturated before any stimulus; its hardware monitor route remains unresolved.
Other clients should be closed for isolated measurements. Monitors can remain
on outputs 1/2: this instrument sends zero on every output except 3.

Build and validate offline:

```sh
make build/hal-loopback test-loopback
```

Python analysis requires NumPy. The normal driver build does not require it.
The tests cover known delays, polarity, noise/gain, duplicate returns, silence,
clipping, missing samples, timestamp gaps and the two timing reference planes.
The C callback tests run under ASan/UBSan and verify channel routing, gap silence,
level cutoff and finite stopping.

With the physical setup ready, prepare a **new** directory, then run explicitly:

```sh
python3 tools/hal-loopback/analyze.py prepare build/trials/MY-NEW-RUN --input 4
python3 scripts/run_hal_loopback.py build/trials/MY-NEW-RUN
python3 tools/hal-loopback/analyze.py analyze build/trials/MY-NEW-RUN
```

Preparation does not open audio. The run is eight seconds, with two 2048-frame
Hann-windowed 200–8000 Hz chirps starting at seconds 2 and 5. Peak output is at
most 0.01 (-40 dBFS). Only the selected input (3 by default, explicitly 4 above)
is retained, and it is never routed to output.
The callback stops emitting if it encounters nonfinite data, input magnitude
above 0.25, invalid/gapped timestamps, or unexpected channel/buffer geometry.
No property reads, allocation or file writes occur in the audio callback.

`prepare-silence` creates an entirely zero output plan. In that mode finite
input above 0.25 is retained for diagnosis, but the analyzer rejects it as an
unexpected level. This allows inspection of clipping/startup transients while
no stimulus can be emitted. The nonfinite-data guard still applies.

Microphone access must be authorized for the app launching the CLI. The tool
only reads that permission; it does not request, enable or reset it. An initial
experiment demonstrated that denied permission can produce all-zero input
without an AudioDeviceStart error. The preflight now refuses to start in that
state. The launching app in the current Codex desktop environment is attributed
to **ChatGPT** by macOS. Temporary access must be restored after testing if that
is what the owner authorized.

An optional `--buffer N` on the runner requests 64–1024 frames, logs status and
immediate/delayed read-back before and after IOProc creation, records actual
callback spans, and restores the original size after stop. A request outside
the HAL-reported range is a coercion/rejection experiment, not a supported-size
qualification. The driver source declares 64–1024 but this Mac's HAL currently
reports **15–192**. Do not label a 192-frame run as a 1024-frame success.

`run.log`, callback CSV, mono Float32 capture, configuration, executable/source
hashes and analysis are retained. Do not reuse/overwrite a run directory. The
runner has a finite timeout; forced termination is a failed trial.

## Measurement definitions and limits

Raw RTL is the return's delivered-frame index minus the emission's delivered-
frame index, with input and output indexed from the same duplex callback.
Timestamp RTL maps those exact events through their own recorded input/output
sample timestamps and subtracts them. Their difference is scheduling distance.
The declaration residual subtracts device and stream latencies in both
directions from timestamp RTL; unread properties remain unknown. Safety offsets
are reported separately. No equal ADC/DAC split is inferred.

Cross-correlation rejects weak, ambiguous and edge-of-window peaks. Whole-run
timestamp gaps, changing steady-state loss counters, driver faults, shared or
unclean teardown and failed restoration invalidate trials. Post-start loss
counters are compared from approximately the first second, before the first
chirp. These are short signal-path tests; they do not establish long-session
glitch performance or an independently calibrated hardware clock.

Methods were informed by [ASFW](../../docs/ASFW-REUSE.md) and the E-MU project's
measurement findings. No ASFW or E-MU source code was copied into this tool.

Preparation accepts `--input 3` or `--input 4`. The runner
reads the channel from the saved configuration and passes it to the native
client. Output remains fixed at 3. Confirm input-4 wiring and Rear selection
before running; this alternative does not itself prove hardware monitoring is off.
See the [measured baseline](../../docs/LOOPBACK-QUALIFICATION.md) for the six
accepted sessions and the limits on their interpretation.
