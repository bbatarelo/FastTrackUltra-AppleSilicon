# ftu-probe

A probe for the M-Audio Fast Track Ultra (USB `0763:2080`) and Fast Track Ultra 8R
(`0763:2081`). It is a standalone diagnostic accompanying the userspace driver. It is **not** a
CoreAudio driver. It supports bounded input capture, IN82 observation and Rust duplex experiments.

**Status, 15 October 2026:** built and tested on a desktop Ultra (`0763:2080`,
release field `0x0151`) on Apple Silicon. Cached descriptor capture and all 157
fixed control reads succeeded. Both rate reads returned 48 kHz; all mixer
crosspoints returned zero, whose effective routing meaning is unverified.
The 8R is a supported selection target but has not been hardware-tested.
See [the evidence and unresolved questions](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/HARDWARE-BRINGUP.md).
Capture also passes four complete trials and a SIGINT stop on the desktop
Ultra, including a successful restart after the stop. See
[capture results](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/CAPTURE-BRINGUP.md) and the
[capture command reference](CAPTURE.md). IN82 observations and stop/restart also
pass; see [feedback findings](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/FEEDBACK-BRINGUP.md) and the
[feedback command reference](FEEDBACK.md).

## Build and test

```sh
make          # builds build/ftu-probe
make test     # builds, then runs the offline tests (no hardware needed)
make sanitize # original portable C reference tests under ASan/UBSan
make clean
```

You need Xcode command line tools, Rust/Cargo (tested with 1.95.0) and Python 3
(standard library only). Rust crates have no external dependencies; builds use
`--offline`. There's
no install target. `make test` runs the Rust suite, five C programs against both implementations,
a native callback-identity test and the Python tests. Rust coverage includes real session-loop simulation,
variable packet sizes, failed submits, out-of-order/mismatched callbacks,
empty feedback, stale scheduling, signals and the FFI/evidence path. Also:

- `build/test_controls`: C tests for the request allowlist, the response acceptance
  rule, value decoding, the stop rule and the JSON line format;
- `test_decode.py`: the descriptor decoder tests;
- `test_control_plan.py`: checks the output of `build/ftu-probe control-plan`;
- `test_hardware_fixture.py`: replays the actual desktop descriptor capture,
  checks the difference from Linux's overridden profile, and mutates 1,000
  copies to exercise bounds and lossless accepted decoding.
- The capture ownership, descriptor-profile and PCM C tests include exhaustive
  packed-24 round trips and 25,000 descriptor mutations. Python tests cover CLI
  rejection and independently replay every capture trial, including interruption.
- Feedback profile/packet C tests and Python replay tests cover three-byte
  bounds, empty/short/error packets, numeric hypotheses and abort rejection.

The [reusable-pool commands](STREAM.md) add `stream-silence` (up to 60 seconds
and three same-process cycles) and `stream-tone` (fixed ten-second repeated
tone). Silent sustained/restart tests pass; the repeated tone awaits listening.
`make sanitize-native` checks the actual native callback adapter with fake
IOKit submissions under ASan/UBSan.

## Commands

```sh
build/ftu-probe list
build/ftu-probe descriptors --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION]
build/ftu-probe controls    --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION]
build/ftu-probe control-plan
build/ftu-probe capture --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION] [--duration-ms N]
build/ftu-probe feedback --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION] [--duration-ms N]
build/ftu-probe duplex-silence --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION] [--duration-ms N]
# Only when a listener is ready, with monitor level low:
build/ftu-probe playback-tone --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION]
python3 analyze_duplex.py DUPLEX_DIRECTORY
python3 decode.py NEW_DIRECTORY/config-0.bin > config-0.json
python3 analyze_capture.py CAPTURE_DIRECTORY
python3 analyze_feedback.py FEEDBACK_DIRECTORY
```

- `list` prints the supported units it finds, using IORegistry properties only.
  Enumeration uses one exact VID/PID matching dictionary per supported product, so
  other USB devices are never examined. It exits nonzero only if enumeration was
  incomplete.
- `descriptors` and `controls` select exactly one supported unit. Use `--product`
  (`2080` or `2081`, optional `0x` prefix) and/or `--location` (the hex `locationID`
  shown by `list`) to narrow the choice. They refuse to run when:
  - no unit matches the filters;
  - more than one unit matches;
  - an option is malformed, unknown or repeated;
  - `--product` names an unsupported PID;
  - registry enumeration was incomplete.

  `NEW_DIRECTORY` must not already exist, so earlier evidence is never overwritten.
  Files inside it are created with `O_EXCL`.
- `control-plan` prints, one JSON object per line, the exact requests `controls`
  would issue. It doesn't enumerate or touch any hardware.

### `descriptors` output

| File | Contents |
|------|----------|
| `metadata.json` | `status` (`ok`/`error`) and `errors`, plus these separate sections: `registry` (IORegistry properties, including the raw `Device Speed`, `USBSpeed` and `UsbLinkSpeed` values), `device_interface` (cached values from the IOUSBDeviceInterface500 getters, including speed) and `configuration_descriptors` (header fields and the file written for each cached configuration). `device_descriptor` is always `null`: raw device descriptor bytes aren't captured, and the tool doesn't rebuild them from registry fields. A missing value is `null`, never 0. |
| `config-N.bin` | Byte copy of cached configuration N. The length comes from its little-endian `wTotalLength`. |

The same JSON is printed to stdout. If something fails after the directory has been
created, the tool still writes `metadata.json` with `"status": "error"` alongside any
configurations that were saved, then exits nonzero.

### `controls` output

| File | Contents |
|------|----------|
| `metadata.json` | Same registry and device-interface sections as above, written **before** any request is issued. `configuration_descriptors` is `null`. |
| `observations.jsonl` | One line per attempted request, flushed as soon as the response arrives, so a partial capture survives an interrupted run. |
| `summary.json` | Written after the run (also printed to stdout): counts, elapsed time, `abort_reason`, status. |

Each observation keeps `bmRequestType`, `bRequest`, `wValue`, `wIndex`, `wLength`,
the label, the target (interface/entity/selector/channel, or endpoint), `ioreturn` as
hex, `outcome`, `accepted`, `actual_length`, `raw_hex` and `elapsed_ms`.

- A response is **accepted** only if the IOReturn is success and exactly `wLength`
  bytes came back. Only then is `decoded` set.
- Otherwise `decoded` is `null`. The returned bytes are still kept in `raw_hex`.
- If the transfer length wasn't reported, `actual_length` and `raw_hex` are `null`.

So a genuine all-zero mixer value (`"accepted": true, "decoded": 0`) can always be
told apart from an error.

Exit status:

| Code | Meaning |
|------|---------|
| 0 | All 157 requests were accepted. |
| 3 | The run completed or stopped early, and at least one request wasn't accepted. |
| 1 | Setup or file error. |
| 2 | Usage error. |

### The `controls` request plan

`controls` uses `DeviceRequestTO` on IOUSBDeviceInterface500. It doesn't open the
device first: Apple's IOUSBLib.h documents that this call doesn't need it. If macOS
refuses access (for example `kIOReturnNotOpen` or `kIOReturnExclusiveAccess`), the run
stops and reports it. The tool never opens or seizes the device to work around that.

All requests are UAC1 class `GET`s (device-to-host). They follow Linux
`mixer_quirks.c` `snd_ftu_create_*` and `mixer.c` `get_ctl_value_v1` at commit
`4056b3c9d3c9`. `wIndex` is `(unit << 8) | 0` (control interface 0), and `wValue` is
`(selector << 8) | channel` with one-based channels:

| Requests | Target | Count |
|----------|--------|------:|
| `GET_CUR`, 2 bytes | unit 5, selectors 1–8 (output destinations) × channels 1–16 (1–8 hardware inputs, 9–16 host returns, per Linux naming) | 128 |
| `GET_CUR`, 2 bytes | unit 5, selector 9, channels 1–16 (effect sends) | 16 |
| `GET_CUR`, 2 bytes | unit 7, selector 7, channels 1–4 (effect returns) | 4 |
| `GET_CUR` | unit 6, channel 0: selector 1 program (2 bytes; Linux uses the low byte, both are kept), selector 2 level (1 byte), selector 3 duration (2 bytes), selector 4 feedback (1 byte) | 4 |
| `GET_MIN`/`GET_MAX`/`GET_RES`, 2 bytes | unit 5, selector 1, channel 1 only | 3 |
| `GET_CUR` sampling frequency, 3 bytes, `bmRequestType 0xa2` | endpoints `0x01` and `0x81` | 2 |

Total: 157.

- The endpoint reads go last. At alternate setting 0 they may fail. Such a failure is
  recorded as unknown and isn't proof that a rate is unsupported.
- The range values apply to that one crosspoint only and must not be extrapolated.
- Signed 16-bit values are recorded raw. Their scale isn't established as dB.

Every planned request is checked against an exact allowlist (`ctl_allowed` in
`rust/ftu-protocol/src/controls.rs`; `controls.c` remains an offline reference) before the run starts. Both timeouts are 500 ms, and nothing is retried.

The run stops early, keeping the results captured so far, when:

- three consecutive responses are failed, short or of bad length;
- the device is removed (`kIOReturnNoDevice` / `kIOReturnNotAttached`);
- access is refused;
- SIGINT or SIGTERM arrives.

## What the tool does not do

The original `list`, `descriptors`, `controls` and `control-plan` commands never:

- opens the device or any interface;
- sends a SET or any host-to-device request;
- sets a configuration or alternate setting;
- resets the device;
- does pipe I/O;
- touches firmware or DFU;
- installs anything.

`capture` is the explicit exception for per-interface opening, alternate-setting
selection and IN pipe reads. It is bounded to 8–2048 ms at the existing 48 kHz
rate, restores its original alternate, and never opens the playback interface
or sends output audio. It adds no mixer/rate writes, reset, seize or installer.
See [CAPTURE.md](CAPTURE.md) for its evidence format and stop behavior.

`feedback` uses the same bounded lifecycle on interface 1 and reads only IN82.
It checks the companion OUT pipe properties but never submits that pipe.
See [FEEDBACK.md](FEEDBACK.md); its nominal-rate payload is not yet a validated
playback pacing source.

`duplex-silence` and `playback-tone` use the Rust stream engine with a thin
IOKit adapter, opening interfaces 2 and 1. They read IN81 and write OUT01; IN82
is not submitted. Rate and mixer controls are never written. Digital silence,
interruptions and restarts pass on the desktop Ultra. The user also confirmed
clean left/right tones on outputs 1/2. See [DUPLEX.md](DUPLEX.md),
[silent results](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/RUST-DUPLEX-BRINGUP.md) and
[first playback](https://github.com/bbatarelo/M-Audio-FastTrackUltra-Apple-Silicon/blob/9c87ae0fc82fbe995641436e4841a720c519fee0/docs/PLAYBACK-BRINGUP.md).

`list` and `descriptors` issue no USB requests at all. They read only:

- IORegistry properties;
- values macOS has already cached, through IOUSBDeviceInterface500 getters that
  are documented as not needing the device open;
- the cached configuration descriptors returned by `GetConfigurationDescriptorPtr`.

`controls` adds only the fixed reads listed above. There is no command for arbitrary
USB requests. Clock-source selection isn't queried; it's a separate, unresolved task.

The tool doesn't report which driver currently claims each interface. Capture that
separately with `ioreg`.

On the tested host, the normal-user command worked outside Codex's execution
sandbox. Inside that sandbox, user-client creation returned `0xe00002be`.
No root privileges, driver installation or security-setting changes were used.

## decode.py

`decode.py` is an offline, lossless walker that turns a configuration blob into JSON:

- Every descriptor keeps its offset, length, type and raw hex. Joining the
  `raw_hex` values in order reproduces the input.
- It checks the configuration header, that `wTotalLength` equals the input size, and
  that every descriptor has `bLength >= 2` and fits in the input.
- It decodes standard configuration, interface and endpoint descriptors. Both 7-byte
  and 9-byte (audio) endpoints are supported.
- It groups descriptors by interface and alternate setting.
- It calculates isochronous capacity:
  - base packet size;
  - transaction multiplier;
  - service interval from `bInterval`.

  These figures assume a high-speed link. Check the reported speed in `metadata.json`.
- It decodes UAC1 `AS_GENERAL`, Type I `FORMAT_TYPE` (discrete or continuous rates)
  and `EP_GENERAL` descriptors inside streaming interfaces (subclass 2). That
  includes vendor class `0xff`, where the UAC1 layout is an assumption, and the
  output says so.
- UAC1 class-specific layouts are used only when the interface's
  `bInterfaceProtocol` is `0x00`. For any other protocol (e.g. `0x20`, UAC2), the
  class-specific descriptors stay raw with a note, and the interface gets a
  `protocol_note`.
- It names the subtypes of Audio Control and MIDI (subclass 3) class-specific
  descriptors but leaves their contents raw. They are never read as streaming
  format descriptors.
- Unknown descriptors stay raw.
- A malformed recognized descriptor or rate list exits with status 1. The message
  gives the byte offset, e.g. `offset 18 (0x0012): ...`. The input file is only
  ever read. Values that parse but look odd, such as a reserved multiplier or a
  mismatched endpoint count, are listed under `warnings`.

## Hypotheses to check against real bytes

The Linux snd-usb-audio quirk table describes this layout:

- interface 0: vendor-specific mixer;
- interface 1, alt 1: OUT endpoint `0x01`;
- interface 2, alt 1: IN endpoint `0x81`;
- 8 channels of S24_3LE at 44.1/48/88.2/96 kHz.

The desktop capture confirms the format/rates, but actual OUT attributes are
`0x05` and playback also exposes a three-byte IN endpoint `0x82`. Linux's `0x09`
OUT profile is a workaround, not the raw descriptor. Capture also has alt 2.
Keep these observations distinct from the transport strategy and from 8R
validation. MIDI is a separate interface and remains untouched.

## Known limitations / uncertainties

- Registry key names: `USB Product Name`/`kUSBProductString`,
  `USB Serial Number`/`kUSBSerialNumberString`, `kUSBCurrentConfiguration`,
  `Device Speed`, `USBSpeed` and `UsbLinkSpeed` differ between macOS versions. When
  a key is missing, its value is `null`. The speed properties are recorded raw and
  not interpreted.
- `config-N.bin` is macOS's cached copy. The probe trusts that the cache holds at
  least `wTotalLength` bytes, which is how IOKit stores it.
- `controls` records `actual_length` from `wLenDone` as macOS reports it, including
  on failures. It doesn't infer a length that wasn't reported.
- Accepted control responses are observations, not proof of effective mixer
  state. Uniform zero GET_CUR responses must not automatically become a restore
  image for later routing writes; readback behavior needs separate validation.
- `metadata.json` includes the unit's serial number. Keep that in mind before you
  share captures.
