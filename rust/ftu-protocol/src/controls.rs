//! Fixed read-only class request plan for the Fast Track Ultra mixer and the
//! rules used to judge each response (`controls.h`). Modelled on Linux
//! sound/usb/mixer_quirks.c snd_ftu_create_*. Only UAC1 GET requests are
//! planned or allowed. `ctl_emit` (JSON) stays in C.
//!
//! [`mixer_request`] separately constructs a crosspoint SET_CUR for the
//! engine; it is never part of the plan and `ctl_allowed` rejects it.

use core::ffi::c_char;
use core::fmt::{self, Write as _};

pub const CTL_PLAN_COUNT: usize = 157;
pub const CTL_MAX_LENGTH: u16 = 3;
pub const CTL_TIMEOUT_MS: u32 = 500;
pub const CTL_MAX_CONSECUTIVE_FAILURES: u32 = 3;
pub const CTL_LABEL_BYTES: usize = 128;

/// `enum ctl_value_kind` values.
pub const CTL_U24LE_HZ: u32 = 0;
pub const CTL_S16LE: u32 = 1;
pub const CTL_U8: u32 = 2;
pub const CTL_LOW_BYTE_OF_U16: u32 = 3;

/// `enum ctl_outcome` values.
pub const CTL_OK: u32 = 0;
pub const CTL_SHORT: u32 = 1;
pub const CTL_BAD_LENGTH: u32 = 2;
pub const CTL_ERROR: u32 = 3;

const RT_IN_CLASS_INTERFACE: u8 = 0xA1;
const RT_IN_CLASS_ENDPOINT: u8 = 0xA2;
const RT_OUT_CLASS_INTERFACE: u8 = 0x21;

const UAC_SET_CUR: u8 = 0x01;
const UAC_GET_CUR: u8 = 0x81;
const UAC_GET_MIN: u8 = 0x82;
const UAC_GET_MAX: u8 = 0x83;
const UAC_GET_RES: u8 = 0x84;

const CONTROL_INTERFACE: u16 = 0;
const UNIT_MIXER: u16 = 5;
const UNIT_EFFECT: u16 = 6;
const UNIT_EFFECT_RETURN: u16 = 7;
const SEL_EFFECT_SEND: u16 = 9;
const SEL_EFFECT_RETURN: u16 = 7;
const SAMPLING_FREQ_VALUE: u16 = 0x0100; // SAMPLING_FREQ_CONTROL << 8

const MATRIX_OUTPUTS: u16 = 8;
const MATRIX_CHANNELS: u16 = 16; // 1-8 hardware inputs, 9-16 host returns
const EFFECT_RETURNS: u16 = 4;

/// `struct ctl_request` (140 bytes, no padding). `kind` is a raw
/// `enum ctl_value_kind`; `label` is NUL-terminated ASCII.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CtlRequest {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
    pub kind: u32,
    pub label: [u8; CTL_LABEL_BYTES],
}

impl CtlRequest {
    /// Label up to its NUL; empty if not UTF-8.
    pub fn label_str(&self) -> &str {
        let end = self
            .label
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(CTL_LABEL_BYTES);
        core::str::from_utf8(&self.label[..end]).unwrap_or("")
    }
}

/// `struct ctl_result`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CtlResult {
    pub ioreturn: u32,
    /// false: the transfer length was not reported.
    pub have_actual: bool,
    pub actual: u32,
    pub data: [u8; CTL_MAX_LENGTH as usize],
    pub elapsed_ms: f64,
}

/// Truncating label writer with vsnprintf semantics (always NUL-terminated).
struct Label([u8; CTL_LABEL_BYTES], usize);

impl fmt::Write for Label {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            if self.1 < CTL_LABEL_BYTES - 1 {
                self.0[self.1] = b;
                self.1 += 1;
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn request(
    bm_request_type: u8,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    w_length: u16,
    kind: u32,
    label: fmt::Arguments<'_>,
) -> CtlRequest {
    let mut buf = Label([0; CTL_LABEL_BYTES], 0);
    let _ = buf.write_fmt(label);
    CtlRequest {
        bm_request_type,
        b_request,
        w_value,
        w_index,
        w_length,
        kind,
        label: buf.0,
    }
}

const fn unit_index(unit: u16) -> u16 {
    unit << 8 | CONTROL_INTERFACE
}

const fn control_value(selector: u16, channel: u16) -> u16 {
    selector << 8 | channel
}

/// Linux's AIn names cover hardware stream channels 1-8; channels 9-16 are
/// the eight host returns.
fn source(ch: u16) -> (&'static str, u16) {
    if ch <= MATRIX_CHANNELS / 2 {
        ("hardware input", ch)
    } else {
        ("host return", ch - MATRIX_CHANNELS / 2)
    }
}

/// Interface GET_CUR, the most common plan entry.
fn get(value: u16, index: u16, len: u16, kind: u32, label: fmt::Arguments<'_>) -> CtlRequest {
    request(
        RT_IN_CLASS_INTERFACE,
        UAC_GET_CUR,
        value,
        index,
        len,
        kind,
        label,
    )
}

/// Produces the plan in order; the single source for both APIs.
fn for_each_planned(mut add: impl FnMut(CtlRequest)) {
    for out in 1..=MATRIX_OUTPUTS {
        for ch in 1..=MATRIX_CHANNELS {
            let (name, num) = source(ch);
            add(get(
                control_value(out, ch),
                unit_index(UNIT_MIXER),
                2,
                CTL_S16LE,
                format_args!("unit 5 crosspoint: output destination {out} <- {name} {num}"),
            ));
        }
    }
    for ch in 1..=MATRIX_CHANNELS {
        let (name, num) = source(ch);
        add(get(
            control_value(SEL_EFFECT_SEND, ch),
            unit_index(UNIT_MIXER),
            2,
            CTL_S16LE,
            format_args!("unit 5 effect send <- {name} {num}"),
        ));
    }
    for ch in 1..=EFFECT_RETURNS {
        add(get(
            control_value(SEL_EFFECT_RETURN, ch),
            unit_index(UNIT_EFFECT_RETURN),
            2,
            CTL_S16LE,
            format_args!("unit 7 effect return {ch}"),
        ));
    }

    let effect = unit_index(UNIT_EFFECT);
    add(get(
        control_value(1, 0),
        effect,
        2,
        CTL_LOW_BYTE_OF_U16,
        format_args!("unit 6 effect program (two bytes read; Linux uses the low byte)"),
    ));
    add(get(
        control_value(2, 0),
        effect,
        1,
        CTL_U8,
        format_args!("unit 6 effect level"),
    ));
    add(get(
        control_value(3, 0),
        effect,
        2,
        CTL_S16LE,
        format_args!("unit 6 effect duration"),
    ));
    add(get(
        control_value(4, 0),
        effect,
        1,
        CTL_U8,
        format_args!("unit 6 effect feedback"),
    ));

    for (req, name) in [
        (UAC_GET_MIN, "GET_MIN"),
        (UAC_GET_MAX, "GET_MAX"),
        (UAC_GET_RES, "GET_RES"),
    ] {
        add(request(
            RT_IN_CLASS_INTERFACE,
            req,
            control_value(1, 1),
            unit_index(UNIT_MIXER),
            2,
            CTL_S16LE,
            format_args!(
                "unit 5 crosspoint output destination 1 <- hardware input 1: {name} for this crosspoint only"
            ),
        ));
    }

    // Endpoint reads go last: at alternate setting 0 they may fail, and two
    // early failures would otherwise count toward the consecutive-failure
    // stop before any mixer value was read.
    add(request(
        RT_IN_CLASS_ENDPOINT,
        UAC_GET_CUR,
        SAMPLING_FREQ_VALUE,
        0x01,
        3,
        CTL_U24LE_HZ,
        format_args!("endpoint 0x01 (OUT) current sampling frequency"),
    ));
    add(request(
        RT_IN_CLASS_ENDPOINT,
        UAC_GET_CUR,
        SAMPLING_FREQ_VALUE,
        0x81,
        3,
        CTL_U24LE_HZ,
        format_args!("endpoint 0x81 (IN) current sampling frequency"),
    ));
}

/// The full 157-entry plan.
pub fn build_plan() -> Vec<CtlRequest> {
    let mut plan = Vec::with_capacity(CTL_PLAN_COUNT);
    for_each_planned(|r| plan.push(r));
    plan
}

/// True only for the exact device-to-host GET requests the plan may contain.
pub fn allowed(r: &CtlRequest) -> bool {
    let (sel, ch, unit) = (r.w_value >> 8, r.w_value & 0xFF, r.w_index >> 8);

    if r.w_length < 1 || r.w_length > CTL_MAX_LENGTH {
        return false;
    }
    if r.bm_request_type == RT_IN_CLASS_ENDPOINT {
        return r.b_request == UAC_GET_CUR
            && r.w_value == SAMPLING_FREQ_VALUE
            && (r.w_index == 0x01 || r.w_index == 0x81)
            && r.w_length == 3;
    }
    if r.bm_request_type != RT_IN_CLASS_INTERFACE || (r.w_index & 0xFF) != CONTROL_INTERFACE {
        return false;
    }
    if matches!(r.b_request, UAC_GET_MIN | UAC_GET_MAX | UAC_GET_RES) {
        return unit == UNIT_MIXER && r.w_value == control_value(1, 1) && r.w_length == 2;
    }
    if r.b_request != UAC_GET_CUR {
        return false;
    }
    match unit {
        UNIT_MIXER => {
            (1..=SEL_EFFECT_SEND).contains(&sel)
                && (1..=MATRIX_CHANNELS).contains(&ch)
                && r.w_length == 2
        }
        // program and duration are 2 bytes, level and feedback 1 byte
        UNIT_EFFECT => {
            (1..=4).contains(&sel) && ch == 0 && r.w_length == if sel % 2 == 1 { 2 } else { 1 }
        }
        UNIT_EFFECT_RETURN => {
            sel == SEL_EFFECT_RETURN && (1..=EFFECT_RETURNS).contains(&ch) && r.w_length == 2
        }
        _ => false,
    }
}

pub fn classify(r: &CtlRequest, res: &CtlResult) -> u32 {
    let want = u32::from(r.w_length);
    if res.ioreturn != 0 {
        CTL_ERROR
    } else if !res.have_actual || res.actual > want {
        CTL_BAD_LENGTH
    } else if res.actual < want {
        CTL_SHORT
    } else {
        CTL_OK
    }
}

fn outcome_name_nul(o: u32) -> &'static str {
    match o {
        CTL_OK => "ok\0",
        CTL_SHORT => "short\0",
        CTL_BAD_LENGTH => "bad_length\0",
        CTL_ERROR => "error\0",
        _ => "unknown\0",
    }
}

pub fn outcome_name(o: u32) -> &'static str {
    outcome_name_nul(o).trim_end_matches('\0')
}

/// Bytes `ctl_decode` reads for a kind; unknown kinds read nothing.
fn kind_bytes(kind: u32) -> usize {
    match kind {
        CTL_U24LE_HZ => 3,
        CTL_S16LE => 2,
        CTL_U8 | CTL_LOW_BYTE_OF_U16 => 1,
        _ => 0,
    }
}

/// Decodes per kind; only meaningful for a `CTL_OK` response. Unknown kinds
/// decode to 0. Bytes beyond the kind's width are ignored.
pub fn decode_value(kind: u32, d: [u8; 3]) -> i64 {
    match kind {
        CTL_U24LE_HZ => i64::from(u32::from(d[0]) | u32::from(d[1]) << 8 | u32::from(d[2]) << 16),
        CTL_S16LE => i64::from(i16::from_le_bytes([d[0], d[1]])),
        CTL_U8 | CTL_LOW_BYTE_OF_U16 => i64::from(d[0]),
        _ => 0,
    }
}

/// Resets on `CTL_OK`, otherwise counts (saturating); true means stop.
pub fn note_outcome(consecutive: &mut u32, o: u32) -> bool {
    *consecutive = if o == CTL_OK {
        0
    } else {
        consecutive.saturating_add(1)
    };
    *consecutive >= CTL_MAX_CONSECUTIVE_FAILURES
}

/// Host-to-device class SET_CUR for one unit 5 crosspoint.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixerRequest {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
    /// Signed little-endian gain.
    pub data: [u8; 2],
}

/// Builds (does not send) the SET_CUR for output 1..=8 <- channel 1..=16
/// (9..=16 are host returns). Positive gain is refused: `gain <= 0` only,
/// where 0 is unity and `i16::MIN` (0x8000) is the UAC1 -inf/mute value.
pub fn mixer_request(output: u8, channel: u8, gain: i16) -> Option<MixerRequest> {
    let (out, ch) = (u16::from(output), u16::from(channel));
    if !(1..=MATRIX_OUTPUTS).contains(&out) || !(1..=MATRIX_CHANNELS).contains(&ch) || gain > 0 {
        return None;
    }
    Some(MixerRequest {
        request_type: RT_OUT_CLASS_INTERFACE,
        request: UAC_SET_CUR,
        value: control_value(out, ch),
        index: unit_index(UNIT_MIXER),
        length: 2,
        data: gain.to_le_bytes(),
    })
}

// ---- C ABI ----------------------------------------------------------------

/// Fills up to `cap` entries and returns the full plan length (157). A null
/// `plan` writes nothing. Entries are written whole via raw pointers.
///
/// # Safety
/// `plan` is null or writable for `cap` `struct ctl_request` entries.
#[no_mangle]
pub unsafe extern "C" fn ctl_build_plan(plan: *mut CtlRequest, cap: usize) -> usize {
    let mut n = 0usize;
    for_each_planned(|r| {
        if !plan.is_null() && n < cap {
            // SAFETY: n < cap entries are writable per caller contract.
            unsafe { plan.add(n).write(r) };
        }
        n += 1;
    });
    n
}

/// Null is never allowed.
///
/// # Safety
/// `r` is null or points to a readable `struct ctl_request`.
#[no_mangle]
pub unsafe extern "C" fn ctl_allowed(r: *const CtlRequest) -> bool {
    // SAFETY: caller contract above; every bit pattern is a valid CtlRequest.
    unsafe { r.as_ref() }.is_some_and(allowed)
}

/// Null `r` or `res` classifies as `CTL_ERROR` (C would fault).
///
/// # Safety
/// Each pointer is null or points to a readable, initialised struct.
#[no_mangle]
pub unsafe extern "C" fn ctl_classify(r: *const CtlRequest, res: *const CtlResult) -> u32 {
    // SAFETY: caller contract above.
    match unsafe { (r.as_ref(), res.as_ref()) } {
        (Some(r), Some(res)) => classify(r, res),
        _ => CTL_ERROR,
    }
}

/// Returns a static NUL-terminated name; unknown values give "unknown".
#[no_mangle]
pub extern "C" fn ctl_outcome_name(o: u32) -> *const c_char {
    outcome_name_nul(o).as_ptr().cast()
}

/// Reads only the bytes `r->kind` needs (3, 2 or 1). Null pointers or an
/// unknown kind decode to 0.
///
/// # Safety
/// `r` is null or readable; `data` is null or readable for the kind's width.
#[no_mangle]
pub unsafe extern "C" fn ctl_decode(r: *const CtlRequest, data: *const u8) -> i64 {
    // SAFETY: caller contract above.
    let Some(r) = (unsafe { r.as_ref() }) else {
        return 0;
    };
    if data.is_null() {
        return 0;
    }
    let mut d = [0u8; 3];
    for (i, b) in d.iter_mut().enumerate().take(kind_bytes(r.kind)) {
        // SAFETY: i is below the kind's width, readable per caller contract.
        *b = unsafe { data.add(i).read() };
    }
    decode_value(r.kind, d)
}

/// Null `consecutive` returns true (stop).
///
/// # Safety
/// `consecutive` is null or points to a writable `unsigned`.
#[no_mangle]
pub unsafe extern "C" fn ctl_note_outcome(consecutive: *mut u32, o: u32) -> bool {
    // SAFETY: caller contract above.
    match unsafe { consecutive.as_mut() } {
        Some(c) => note_outcome(c, o),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    fn req(t: u8, b: u8, value: u16, index: u16, len: u16, kind: u32) -> CtlRequest {
        request(t, b, value, index, len, kind, format_args!("test"))
    }

    fn res(ioreturn: u32, have_actual: bool, actual: u32) -> CtlResult {
        CtlResult {
            ioreturn,
            have_actual,
            actual,
            data: [0; 3],
            elapsed_ms: 0.0,
        }
    }

    #[test]
    fn layout_matches_c() {
        assert_eq!(size_of::<CtlRequest>(), 140);
        assert_eq!(offset_of!(CtlRequest, kind), 8);
        assert_eq!(offset_of!(CtlRequest, label), 12);
        assert_eq!(size_of::<CtlResult>(), 24);
        assert_eq!(offset_of!(CtlResult, actual), 4 + 4);
        assert_eq!(offset_of!(CtlResult, data), 12);
        assert_eq!(offset_of!(CtlResult, elapsed_ms), 16);
        assert_eq!(size_of::<MixerRequest>(), 10);
    }

    #[test]
    fn plan_order_values_and_labels() {
        let plan = build_plan();
        assert_eq!(plan.len(), CTL_PLAN_COUNT);
        let mut seen = std::collections::HashSet::new();
        for r in &plan {
            assert!(allowed(r), "{}", r.label_str());
            assert!(r.bm_request_type & 0x80 != 0);
            assert!(r.label.is_ascii() && r.label[CTL_LABEL_BYTES - 1] == 0);
            assert!(seen.insert((r.b_request, r.w_value, r.w_index)));
        }
        let check = |i: usize, b: u8, value: u16, index: u16, len: u16, kind: u32, label: &str| {
            let r = &plan[i];
            assert_eq!(
                (
                    r.b_request,
                    r.w_value,
                    r.w_index,
                    r.w_length,
                    r.kind,
                    r.label_str()
                ),
                (b, value, index, len, kind, label),
                "entry {i}"
            );
        };
        check(
            0,
            0x81,
            0x0101,
            0x0500,
            2,
            CTL_S16LE,
            "unit 5 crosspoint: output destination 1 <- hardware input 1",
        );
        check(
            8,
            0x81,
            0x0109,
            0x0500,
            2,
            CTL_S16LE,
            "unit 5 crosspoint: output destination 1 <- host return 1",
        );
        check(
            25,
            0x81,
            0x020a,
            0x0500,
            2,
            CTL_S16LE,
            "unit 5 crosspoint: output destination 2 <- host return 2",
        );
        check(
            127,
            0x81,
            0x0810,
            0x0500,
            2,
            CTL_S16LE,
            "unit 5 crosspoint: output destination 8 <- host return 8",
        );
        check(
            128,
            0x81,
            0x0901,
            0x0500,
            2,
            CTL_S16LE,
            "unit 5 effect send <- hardware input 1",
        );
        check(
            143,
            0x81,
            0x0910,
            0x0500,
            2,
            CTL_S16LE,
            "unit 5 effect send <- host return 8",
        );
        check(
            144,
            0x81,
            0x0701,
            0x0700,
            2,
            CTL_S16LE,
            "unit 7 effect return 1",
        );
        check(
            147,
            0x81,
            0x0704,
            0x0700,
            2,
            CTL_S16LE,
            "unit 7 effect return 4",
        );
        check(
            148,
            0x81,
            0x0100,
            0x0600,
            2,
            CTL_LOW_BYTE_OF_U16,
            "unit 6 effect program (two bytes read; Linux uses the low byte)",
        );
        check(149, 0x81, 0x0200, 0x0600, 1, CTL_U8, "unit 6 effect level");
        check(
            150,
            0x81,
            0x0300,
            0x0600,
            2,
            CTL_S16LE,
            "unit 6 effect duration",
        );
        check(
            151,
            0x81,
            0x0400,
            0x0600,
            1,
            CTL_U8,
            "unit 6 effect feedback",
        );
        check(152, 0x82, 0x0101, 0x0500, 2, CTL_S16LE,
              "unit 5 crosspoint output destination 1 <- hardware input 1: GET_MIN for this crosspoint only");
        check(154, 0x84, 0x0101, 0x0500, 2, CTL_S16LE,
              "unit 5 crosspoint output destination 1 <- hardware input 1: GET_RES for this crosspoint only");
        check(
            155,
            0x81,
            0x0100,
            0x0001,
            3,
            CTL_U24LE_HZ,
            "endpoint 0x01 (OUT) current sampling frequency",
        );
        check(
            156,
            0x81,
            0x0100,
            0x0081,
            3,
            CTL_U24LE_HZ,
            "endpoint 0x81 (IN) current sampling frequency",
        );
        assert_eq!(plan[155].bm_request_type, 0xA2);
        assert_eq!(plan[154].bm_request_type, 0xA1);
    }

    #[test]
    fn ffi_plan_respects_cap_and_null() {
        assert_eq!(
            unsafe { ctl_build_plan(core::ptr::null_mut(), 0) },
            CTL_PLAN_COUNT
        );
        assert_eq!(
            unsafe { ctl_build_plan(core::ptr::null_mut(), 5) },
            CTL_PLAN_COUNT
        );
        let sentinel = req(0, 0, 0xBEEF, 0, 0, 99);
        let mut buf = [sentinel; 4];
        assert_eq!(
            unsafe { ctl_build_plan(buf.as_mut_ptr(), 3) },
            CTL_PLAN_COUNT
        );
        assert_eq!(&buf[..3], &build_plan()[..3]);
        assert_eq!(buf[3], sentinel);
    }

    #[test]
    fn allowlist_rejects() {
        assert!(allowed(&req(0xA1, 0x81, 0x0101, 0x0500, 2, CTL_S16LE)));
        let bad = [
            req(0x21, 0x01, 0x0101, 0x0500, 2, CTL_S16LE), // SET_CUR
            req(0x21, 0x81, 0x0101, 0x0500, 2, CTL_S16LE), // direction bit clear
            req(0xA1, 0x01, 0x0101, 0x0500, 2, CTL_S16LE), // SET_CUR code, IN type
            req(0xA1, 0x85, 0x0101, 0x0500, 2, CTL_S16LE), // GET_MEM
            req(0xA1, 0x81, 0x0101, 0x0501, 2, CTL_S16LE), // interface 1
            req(0xA1, 0x81, 0x0101, 0x0400, 2, CTL_S16LE), // unit 4
            req(0xA1, 0x81, 0x0100, 0x0500, 2, CTL_S16LE), // master channel 0
            req(0xA1, 0x81, 0x0111, 0x0500, 2, CTL_S16LE), // channel 17
            req(0xA1, 0x81, 0x0a01, 0x0500, 2, CTL_S16LE), // selector 10
            req(0xA1, 0x81, 0x0101, 0x0500, 0, CTL_S16LE), // zero length
            req(0xA1, 0x81, 0x0101, 0x0500, 4, CTL_S16LE), // too long
            req(0xA1, 0x82, 0x0201, 0x0500, 2, CTL_S16LE), // range of another crosspoint
            req(0xA1, 0x81, 0x0501, 0x0600, 2, CTL_S16LE), // unit 6 selector 5
            req(0xA1, 0x81, 0x0201, 0x0600, 1, CTL_U8),    // unit 6 with a channel
            req(0xA1, 0x81, 0x0200, 0x0600, 2, CTL_U8),    // effect level is 1 byte
            req(0xA1, 0x81, 0x0705, 0x0700, 2, CTL_S16LE), // effect return 5
            req(0xA2, 0x81, 0x0100, 0x0002, 3, CTL_U24LE_HZ), // endpoint 0x02
            req(0xA2, 0x01, 0x0100, 0x0081, 3, CTL_U24LE_HZ), // SET_CUR to endpoint
            req(0x22, 0x81, 0x0100, 0x0081, 3, CTL_U24LE_HZ), // OUT to endpoint
            req(0xA2, 0x81, 0x0200, 0x0081, 3, CTL_U24LE_HZ), // pitch control
            req(0xA2, 0x82, 0x0100, 0x0081, 3, CTL_U24LE_HZ), // endpoint GET_MIN
            req(0x80, 0x06, 0x0100, 0x0000, 3, CTL_U8),    // GET_DESCRIPTOR
        ];
        for (i, r) in bad.iter().enumerate() {
            assert!(!allowed(r), "bad request {i}");
        }
        assert!(!unsafe { ctl_allowed(core::ptr::null()) });
    }

    #[test]
    fn classify_decode_and_stop_rule() {
        let r = req(0xA1, 0x81, 0x0101, 0x0500, 2, CTL_S16LE);
        assert_eq!(classify(&r, &res(0, true, 2)), CTL_OK);
        assert_eq!(classify(&r, &res(0, true, 1)), CTL_SHORT);
        assert_eq!(classify(&r, &res(0, true, 0)), CTL_SHORT);
        assert_eq!(classify(&r, &res(0xe000404f, true, 2)), CTL_ERROR);
        assert_eq!(classify(&r, &res(0, false, 0)), CTL_BAD_LENGTH);
        assert_eq!(classify(&r, &res(0, true, 3)), CTL_BAD_LENGTH);
        assert_eq!(unsafe { ctl_classify(&r, core::ptr::null()) }, CTL_ERROR);

        assert_eq!(decode_value(CTL_S16LE, [0xff, 0xff, 0]), -1);
        assert_eq!(decode_value(CTL_S16LE, [0x00, 0x80, 0]), -32768);
        assert_eq!(decode_value(CTL_S16LE, [0xff, 0x7f, 0]), 32767);
        assert_eq!(decode_value(CTL_U24LE_HZ, [0x80, 0xbb, 0x00]), 48000);
        assert_eq!(decode_value(CTL_U24LE_HZ, [0x00, 0x77, 0x01]), 96000);
        assert_eq!(decode_value(99, [1, 2, 3]), 0);
        // FFI reads only the kind's width: one byte for U8.
        let u8r = req(0xA1, 0x81, 0x0200, 0x0600, 1, CTL_U8);
        let one = [0xc8u8];
        assert_eq!(unsafe { ctl_decode(&u8r, one.as_ptr()) }, 200);
        let prog = req(0xA1, 0x81, 0x0100, 0x0600, 2, CTL_LOW_BYTE_OF_U16);
        assert_eq!(unsafe { ctl_decode(&prog, [0x05u8, 0x01].as_ptr()) }, 5);

        let mut c = 0u32;
        for o in [CTL_ERROR, CTL_SHORT, CTL_OK, CTL_ERROR, CTL_BAD_LENGTH] {
            assert!(!note_outcome(&mut c, o));
        }
        assert_eq!(c, 2);
        assert!(note_outcome(&mut c, 42)); // unknown counts as failure
        assert_eq!(c, 3);
        let mut max = u32::MAX;
        assert!(note_outcome(&mut max, CTL_ERROR));
        assert!(unsafe { ctl_note_outcome(core::ptr::null_mut(), CTL_OK) });
        assert_eq!(outcome_name(CTL_BAD_LENGTH), "bad_length");
        assert_eq!(outcome_name(7), "unknown");
    }

    #[test]
    fn mixer_requests() {
        let unity = mixer_request(1, 9, 0).unwrap();
        assert_eq!(
            unity,
            MixerRequest {
                request_type: 0x21,
                request: 0x01,
                value: 0x0109,
                index: 0x0500,
                length: 2,
                data: [0, 0]
            }
        );
        let mute = mixer_request(2, 10, i16::MIN).unwrap();
        assert_eq!(
            (mute.value, mute.index, mute.data),
            (0x020a, 0x0500, [0x00, 0x80])
        );
        assert_eq!(mixer_request(8, 16, -256).unwrap().data, [0x00, 0xff]);
        assert_eq!(mixer_request(8, 16, -1).unwrap().value, 0x0810);
        for (o, c, g) in [
            (0, 1, 0),
            (9, 1, 0),
            (1, 0, 0),
            (1, 17, 0),
            (1, 1, 1),
            (1, 1, i16::MAX),
        ] {
            assert!(mixer_request(o, c, g).is_none(), "{o} {c} {g}");
        }
    }
}
