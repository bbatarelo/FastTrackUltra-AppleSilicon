# Bounded feedback-endpoint experiment

```sh
make
build/ftu-probe feedback --output NEW_DIRECTORY --product 2080 --duration-ms 256
python3 analyze_feedback.py NEW_DIRECTORY
```

Selection, duration bounds (8–2048 ms, multiples of 8), output-file exclusivity,
SIGINT/SIGTERM handling and exit codes match [capture](CAPTURE.md). The same
command recognizes product 2081, but the Ultra 8R has not been hardware-tested.

Only interface 1 is opened. It must start at alternate 0 and match the exact
cached descriptor profile. The command prepares its buffers and event source,
selects alternate 1, validates both pipes, then submits **only IN82 reads**.
No OUT transfer, mixer/rate write, reset, seize or HAL installation is performed.
Both endpoint rate GETs must report the existing 48 kHz. The original alternate
is restored after callbacks drain. Check the summary and external registry
snapshots for the actual cleanup result, including failures.

The shared engine preserves `capture` as an interface 2-only IN81 experiment.
No arbitrary interface/endpoint/alternate/rate selection is exposed.

Files follow the capture schema, with these differences:

- `plan.json` and `summary.json` have mode `feedback`, interface 1, endpoint 130
  (`0x82` in the summary), capacity 3. The summary records the resolved pipe ref.
- `feedback-config.bin` retains the cached configuration.
- `packets.bin` holds three-byte reserved slots for completed requests whose
  callback identity matched. Only the actual length describes returned bytes.
- No `audio.s24le` is created. Audio counts are 0, `channels` is empty and audio
  offsets are null; feedback is never passed through PCM conversion.
- `frame_requested_length` preserves the returned frame-list request count;
  `requested_length` records the intended capacity. `callback_pointer_matches`
  records whether the completion returned the expected frame-list address.
  These two diagnostic fields are also added to capture observations.
- Some shared event names retain `capture` for compatibility; the plan, summary
  mode and `feedback_pipe_ready` identify the selected experiment.

Accepted means an observed, bounded payload under successful/underrun packet
**and request** statuses with correct request length. Actual lengths 0 through 3
are retained and counted; empty or short bytes are not silently padded into
numeric values. Aborted packets stay in the raw evidence but cannot enter the
accepted payload histogram. Pending buffers are never inspected.

Exit 0 and summary `status:ok` mean the bounded transport and cleanup completed
successfully. They do not require a complete three-byte clock value: a run of
empty or short packets can still be a successful observation experiment. Use
`full_payload_packets` and `numeric_hypotheses` in the offline result to see
whether any full values were available.

The offline analyzer independently checks slot offsets, byte bounds, acceptance,
block accounting and summary totals. It compares byte-order, fractional-bit and
time-unit hypotheses only for accepted length 3 payloads. Passing the audit or
finding a plausible nominal value does not prove feedback freshness or playback
fitness. `audit:passed` covers stored accounting; the separate
`experiment_outcome` retains errors, cleanup/restoration and before/after rates.
The analyzer also checks summary geometry and the resolved pipe reference.
`distinct_full_payloads` and `constant_full_payload` expose whether accepted
three-byte payloads varied; the latter is null when none were observed. A
constant nominal value cannot show live clock tracking. Both plain and
lossless-gzip evidence files are supported.

See [the hardware results](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/FEEDBACK-BRINGUP.md) for observed payloads,
timestamp granularity, stop/restart and the remaining clock questions.
