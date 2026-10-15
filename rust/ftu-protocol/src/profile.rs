//! Fixed first-experiment descriptor profiles (`capture_profile.h`,
//! `feedback_profile.h`). Validates only the known Fast Track Ultra
//! configuration; no claim to general UAC validation.
//!
//! Capture: interface 2 alt 1, one endpoint 0x81 iso async, 312 bytes.
//! Feedback/playback: interface 1 alt 1, OUT 0x01 iso async 312 bytes plus
//! explicit feedback IN 0x82, 3 bytes. Both require exactly one idle alt 0,
//! one active alt 1 (vendor class 0xff/2/0), one AS_GENERAL with PCM tag and
//! one FORMAT_TYPE I, 8 channels, 3-byte 24-bit, listing 48000 Hz.

const DESC_INTERFACE: u8 = 4;
const DESC_ENDPOINT: u8 = 5;
const DESC_CS_INTERFACE: u8 = 0x24;
/// The header carries a 16-bit total length, so nothing longer can match.
const MAX_CONFIG_BYTES: usize = 0xFFFF;

#[derive(Default)]
struct Counts {
    idle: u32,
    active: u32,
    general: u32,
    format: u32,
    ep_out: u32,
    ep_in: u32,
}

fn bump(n: &mut u32) -> u32 {
    *n += 1;
    *n
}

/// Validates a full configuration descriptor. `feedback` selects the
/// playback+feedback profile instead of the capture profile.
pub fn validate_profile(p: &[u8], feedback: bool) -> bool {
    let n = p.len();
    if n < 9 || p[0] != 9 || p[1] != 2 || (usize::from(p[2]) | usize::from(p[3]) << 8) != n {
        return false;
    }
    let (iface, active_endpoints) = if feedback { (1, 2) } else { (2, 1) };
    let mut k = Counts::default();
    let mut target = false;
    let mut off = 9;
    while off < n {
        if n - off < 2 || p[off] < 2 || usize::from(p[off]) > n - off {
            return false;
        }
        let len = usize::from(p[off]);
        let d = &p[off..off + len];
        match d[1] {
            DESC_INTERFACE => {
                if len < 9 {
                    return false;
                }
                target = d[2] == iface && d[3] == 1;
                if d[2] == iface && d[3] == 0 && (bump(&mut k.idle) != 1 || d[4] != 0) {
                    return false;
                }
                if target
                    && (bump(&mut k.active) != 1
                        || d[4] != active_endpoints
                        || d[5] != 0xff
                        || d[6] != 2
                        || d[7] != 0)
                {
                    return false;
                }
            }
            DESC_CS_INTERFACE if target => {
                if len < 3 {
                    return false;
                }
                if d[2] == 1 {
                    // AS_GENERAL, PCM format tag
                    if bump(&mut k.general) != 1 || len < 7 || d[5] != 1 || d[6] != 0 {
                        return false;
                    }
                } else if d[2] == 2 {
                    if bump(&mut k.format) != 1
                        || len < 8
                        || d[3] != 1
                        || d[4] != 8
                        || d[5] != 3
                        || d[6] != 24
                        || d[7] == 0
                        || len != 8 + 3 * usize::from(d[7])
                    {
                        return false;
                    }
                    let has_48k = d[8..].chunks_exact(3).any(|r| {
                        u32::from(r[0]) | u32::from(r[1]) << 8 | u32::from(r[2]) << 16 == 48_000
                    });
                    if !has_48k {
                        return false;
                    }
                }
            }
            DESC_ENDPOINT if target => {
                let ok = if feedback {
                    feedback_endpoint(d, &mut k)
                } else {
                    bump(&mut k.ep_in) == 1
                        && len >= 7
                        && d[2] == 0x81
                        && d[3] == 5
                        && d[4] == 0x38
                        && d[5] == 1
                        && d[6] == 1
                };
                if !ok {
                    return false;
                }
            }
            _ => {}
        }
        off += len;
    }
    k.idle == 1
        && k.active == 1
        && k.general == 1
        && k.format == 1
        && k.ep_in == 1
        && k.ep_out == u32::from(feedback)
}

fn feedback_endpoint(d: &[u8], k: &mut Counts) -> bool {
    if d.len() < 7 || d[6] != 1 {
        return false;
    }
    match d[2] {
        0x01 => bump(&mut k.ep_out) == 1 && d[3] == 5 && d[4] == 0x38 && d[5] == 1,
        0x82 => bump(&mut k.ep_in) == 1 && d[3] == 1 && d[4] == 3 && d[5] == 0,
        _ => false,
    }
}

/// # Safety
/// `p` is null or readable for `n` bytes, not mutated during the call.
unsafe fn borrow<'a>(p: *const u8, n: usize) -> Option<&'a [u8]> {
    if p.is_null() || n > MAX_CONFIG_BYTES {
        return None;
    }
    // SAFETY: non-null and n readable bytes per caller contract; n is small.
    Some(unsafe { core::slice::from_raw_parts(p, n) })
}

/// # Safety
/// `p` is null (returns false) or readable for `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn cap_profile_validate(p: *const u8, n: usize) -> bool {
    // SAFETY: caller contract above.
    unsafe { borrow(p, n) }.is_some_and(|b| validate_profile(b, false))
}

/// # Safety
/// `p` is null (returns false) or readable for `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn feedback_profile_validate(p: *const u8, n: usize) -> bool {
    // SAFETY: caller contract above.
    unsafe { borrow(p, n) }.is_some_and(|b| validate_profile(b, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(descs: &[&[u8]]) -> Vec<u8> {
        let mut v = vec![9, 2, 0, 0, 2, 1, 0, 0x80, 50];
        for d in descs {
            v.extend_from_slice(d);
        }
        let n = v.len() as u16;
        v[2..4].copy_from_slice(&n.to_le_bytes());
        v
    }

    const GENERAL: &[u8] = &[7, 0x24, 1, 1, 1, 1, 0];
    const FORMAT: &[u8] = &[14, 0x24, 2, 1, 8, 3, 24, 2, 0x44, 0xac, 0, 0x80, 0xbb, 0];
    const CAP_IDLE: &[u8] = &[9, 4, 2, 0, 0, 0xff, 2, 0, 0];
    const CAP_ALT1: &[u8] = &[9, 4, 2, 1, 1, 0xff, 2, 0, 0];
    const CAP_EP: &[u8] = &[9, 5, 0x81, 5, 0x38, 1, 1, 0, 0];
    const PB_IDLE: &[u8] = &[9, 4, 1, 0, 0, 0xff, 2, 0, 0];
    const PB_ALT1: &[u8] = &[9, 4, 1, 1, 2, 0xff, 2, 0, 0];
    const PB_OUT: &[u8] = &[9, 5, 0x01, 5, 0x38, 1, 1, 0, 0];
    const PB_FB: &[u8] = &[9, 5, 0x82, 1, 3, 0, 1, 0, 0];
    const CS_EP: &[u8] = &[7, 0x25, 1, 0, 0, 0, 0];

    fn both() -> Vec<u8> {
        config(&[
            PB_IDLE, PB_ALT1, GENERAL, FORMAT, PB_OUT, CS_EP, PB_FB, CAP_IDLE, CAP_ALT1, GENERAL,
            FORMAT, CAP_EP, CS_EP,
        ])
    }

    #[test]
    fn synthetic_profiles() {
        let all = both();
        assert!(validate_profile(&all, false));
        assert!(validate_profile(&all, true));
        let cap = config(&[CAP_IDLE, CAP_ALT1, GENERAL, FORMAT, CAP_EP]);
        assert!(validate_profile(&cap, false) && !validate_profile(&cap, true));
        let pb = config(&[PB_IDLE, PB_ALT1, GENERAL, FORMAT, PB_OUT, PB_FB]);
        assert!(validate_profile(&pb, true) && !validate_profile(&pb, false));
        // Missing feedback endpoint, or a duplicate one.
        assert!(!validate_profile(
            &config(&[PB_IDLE, PB_ALT1, GENERAL, FORMAT, PB_OUT]),
            true
        ));
        assert!(!validate_profile(
            &config(&[PB_IDLE, PB_ALT1, GENERAL, FORMAT, PB_OUT, PB_FB, PB_FB]),
            true
        ));
        // No 48 kHz rate.
        let f44: &[u8] = &[11, 0x24, 2, 1, 8, 3, 24, 1, 0x44, 0xac, 0];
        assert!(!validate_profile(
            &config(&[CAP_IDLE, CAP_ALT1, GENERAL, f44, CAP_EP]),
            false
        ));
        // Class descriptors after a non-target interface are ignored.
        let other: &[u8] = &[9, 4, 3, 1, 1, 0xff, 2, 0, 0];
        let cap2 = config(&[
            CAP_IDLE, CAP_ALT1, GENERAL, FORMAT, CAP_EP, other, f44, PB_FB,
        ]);
        assert!(validate_profile(&cap2, false));
    }

    #[test]
    fn truncation_and_header_length() {
        let all = both();
        for i in 0..all.len() {
            assert!(!validate_profile(&all[..i], false));
            assert!(!validate_profile(&all[..i], true));
            // Consistent header over a truncated walk must not panic.
            let mut cut = all[..i].to_vec();
            if i >= 9 {
                cut[2..4].copy_from_slice(&(i as u16).to_le_bytes());
                let _ = validate_profile(&cut, false);
                let _ = validate_profile(&cut, true);
            }
        }
        assert!(!unsafe { cap_profile_validate(core::ptr::null(), 0) });
        assert!(!unsafe { feedback_profile_validate(core::ptr::null(), 9) });
        assert!(unsafe { cap_profile_validate(all.as_ptr(), all.len()) });
    }

    #[test]
    fn zero_length_descriptor_rejected() {
        let mut v = both();
        v[9] = 0;
        assert!(!validate_profile(&v, false));
        v[9] = 1;
        assert!(!validate_profile(&v, true));
    }
}
