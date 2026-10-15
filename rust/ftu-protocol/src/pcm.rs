//! Signed 24-bit little-endian PCM conversion (`pcm.h`).
//!
//! Float layouts are strided: element `frame * frame_stride + channel *
//! channel_stride`. Packed bytes are always interleaved. Every extent is
//! validated before any read or write; a rejected call touches nothing.

use core::mem::size_of;

pub const PCM_MAX: i32 = 8_388_607;
pub const PCM_MIN: i32 = -8_388_608;
const PCM_SCALE: f64 = 8_388_608.0;

pub fn decode_s24(bytes: [u8; 3]) -> i32 {
    let u = u32::from(bytes[0]) | u32::from(bytes[1]) << 8 | u32::from(bytes[2]) << 16;
    (u & 0x7F_FFFF) as i32 - (u & 0x80_0000) as i32
}

/// Exact: value / 2^23 in [-1, 1).
pub fn s24_to_f32(bytes: [u8; 3]) -> f32 {
    decode_s24(bytes) as f32 / 8_388_608.0f32
}

/// NaN -> 0, saturates (incl. infinities) to [PCM_MIN, PCM_MAX], half-LSB
/// ties round away from zero.
fn quantize(sample: f32) -> i32 {
    if sample.is_nan() {
        return 0;
    }
    let d = f64::from(sample) * PCM_SCALE;
    if d >= f64::from(PCM_MAX) {
        return PCM_MAX;
    }
    if d <= f64::from(PCM_MIN) {
        return PCM_MIN;
    }
    // |d| < 2^23 here, so truncation and the fraction are exact.
    let a = d.abs();
    let mut t = a as i32;
    if a - f64::from(t) >= 0.5 {
        t += 1;
    }
    if d < 0.0 {
        -t
    } else {
        t
    }
}

pub fn encode_s24(sample: f32) -> [u8; 3] {
    let u = quantize(sample) as u32;
    [u as u8, (u >> 8) as u8, (u >> 16) as u8]
}

fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Validates both extents for one transfer (`pcm_check` in C). `*_present`
/// is false for a null pointer, which is only allowed with zero capacity.
#[allow(clippy::too_many_arguments)]
fn check_layout(
    bytes_present: bool,
    byte_cap: usize,
    floats_present: bool,
    float_cap: usize,
    frames: usize,
    channels: usize,
    fs: usize,
    cs: usize,
) -> bool {
    if channels == 0 || channels > usize::MAX / 3 {
        return false;
    }
    if (!bytes_present && byte_cap != 0) || (!floats_present && float_cap != 0) {
        return false;
    }
    let frame_bytes = channels * 3;
    if !byte_cap.is_multiple_of(frame_bytes) {
        return false;
    }
    if frames == 0 {
        return true;
    }
    if byte_cap / frame_bytes < frames {
        return false;
    }

    // Addresses f*fs + c*cs collide iff some 0<df<frames, 0<dc<channels
    // satisfy df*fs == dc*cs; the smallest such pair is (cs/g, fs/g).
    if frames > 1 && fs == 0 {
        return false;
    }
    if channels > 1 && cs == 0 {
        return false;
    }
    if frames > 1 && channels > 1 {
        let g = gcd(fs, cs);
        if cs / g < frames && fs / g < channels {
            return false;
        }
    }

    let last = match ((frames - 1).checked_mul(fs), (channels - 1).checked_mul(cs)) {
        (Some(a), Some(b)) => match a.checked_add(b) {
            Some(last) => last,
            None => return false,
        },
        _ => return false,
    };
    // Element offsets must also fit when scaled to byte addresses.
    last < float_cap && last < usize::MAX / size_of::<f32>()
}

/// Safe strided decode; see module docs. Returns false and writes nothing on
/// any invalid layout.
pub fn decode_frames(
    src: &[u8],
    dst: &mut [f32],
    frames: usize,
    channels: usize,
    frame_stride: usize,
    channel_stride: usize,
) -> bool {
    // SAFETY: slices are valid for their lengths and cannot overlap.
    unsafe {
        pcm_decode_frames(
            src.as_ptr(),
            src.len(),
            dst.as_mut_ptr(),
            dst.len(),
            frames,
            channels,
            frame_stride,
            channel_stride,
        )
    }
}

/// Safe strided encode; see module docs. Returns false and writes nothing on
/// any invalid layout.
pub fn encode_frames(
    src: &[f32],
    dst: &mut [u8],
    frames: usize,
    channels: usize,
    frame_stride: usize,
    channel_stride: usize,
) -> bool {
    // SAFETY: slices are valid for their lengths and cannot overlap.
    unsafe {
        pcm_encode_frames(
            src.as_ptr(),
            src.len(),
            dst.as_mut_ptr(),
            dst.len(),
            frames,
            channels,
            frame_stride,
            channel_stride,
        )
    }
}

// ---- C ABI ----------------------------------------------------------------

/// Null returns 0 (C would fault).
///
/// # Safety
/// `p` is null or readable for 3 bytes (any alignment).
#[no_mangle]
pub unsafe extern "C" fn pcm_s24_decode(p: *const u8) -> i32 {
    if p.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees 3 readable bytes; [u8; 3] has alignment 1.
    decode_s24(unsafe { p.cast::<[u8; 3]>().read() })
}

/// Null returns 0.0.
///
/// # Safety
/// As [`pcm_s24_decode`].
#[no_mangle]
pub unsafe extern "C" fn pcm_s24_float(p: *const u8) -> f32 {
    if p.is_null() {
        return 0.0;
    }
    // SAFETY: as above.
    s24_to_f32(unsafe { p.cast::<[u8; 3]>().read() })
}

/// Null is a no-op.
///
/// # Safety
/// `p` is null or writable for 3 bytes (any alignment).
#[no_mangle]
pub unsafe extern "C" fn pcm_s24_encode(sample: f32, p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: caller guarantees 3 writable bytes; alignment 1.
    unsafe { p.cast::<[u8; 3]>().write(encode_s24(sample)) };
}

/// # Safety
/// When non-null, `src` is readable for `src_bytes` bytes and `dst` is
/// writable for `dst_count` floats (suitably aligned). The two buffers must
/// not overlap (not detected). Only raw pointer accesses are made, no Rust
/// references are formed over the caller's memory.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn pcm_decode_frames(
    src: *const u8,
    src_bytes: usize,
    dst: *mut f32,
    dst_count: usize,
    frames: usize,
    channels: usize,
    frame_stride: usize,
    channel_stride: usize,
) -> bool {
    if !check_layout(
        !src.is_null(),
        src_bytes,
        !dst.is_null(),
        dst_count,
        frames,
        channels,
        frame_stride,
        channel_stride,
    ) {
        return false;
    }
    for f in 0..frames {
        for c in 0..channels {
            // Bounded by check_layout: packed + 3 <= src_bytes, at < dst_count.
            let packed = (f * channels + c) * 3;
            let at = f * frame_stride + c * channel_stride;
            // SAFETY: both offsets are in the caller-guaranteed extents.
            unsafe {
                let v = s24_to_f32(src.add(packed).cast::<[u8; 3]>().read());
                dst.add(at).write(v);
            }
        }
    }
    true
}

/// # Safety
/// When non-null, `src` is readable for `src_count` floats (suitably aligned)
/// and `dst` is writable for `dst_bytes` bytes. The buffers must not overlap
/// (not detected). Only raw pointer accesses are made.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn pcm_encode_frames(
    src: *const f32,
    src_count: usize,
    dst: *mut u8,
    dst_bytes: usize,
    frames: usize,
    channels: usize,
    frame_stride: usize,
    channel_stride: usize,
) -> bool {
    if !check_layout(
        !dst.is_null(),
        dst_bytes,
        !src.is_null(),
        src_count,
        frames,
        channels,
        frame_stride,
        channel_stride,
    ) {
        return false;
    }
    for f in 0..frames {
        for c in 0..channels {
            let packed = (f * channels + c) * 3;
            let at = f * frame_stride + c * channel_stride;
            // SAFETY: both offsets are in the caller-guaranteed extents.
            unsafe {
                let bytes = encode_s24(src.add(at).read());
                dst.add(packed).cast::<[u8; 3]>().write(bytes);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSB: f32 = 1.0 / 8_388_608.0;
    const FR: usize = 5;
    const CH: usize = 8;
    const SENTINEL: f32 = -7.0;

    fn le3(u: u32) -> [u8; 3] {
        [u as u8, (u >> 8) as u8, (u >> 16) as u8]
    }

    fn sext24(u: u32) -> i32 {
        ((u << 8) as i32) >> 8
    }

    #[test]
    fn scalar_decode_boundaries() {
        for (u, v) in [
            (0x000000, 0),
            (0x000001, 1),
            (0xFFFFFF, -1),
            (0x7FFFFF, PCM_MAX),
            (0x800000, PCM_MIN),
            (0x123456, 0x123456),
            (0xFF0000, -65536),
        ] {
            assert_eq!(decode_s24(le3(u)), v);
            assert_eq!(s24_to_f32(le3(u)), v as f32 / 8_388_608.0);
        }
        assert_eq!(s24_to_f32(le3(0x800000)), -1.0);
        assert_eq!(s24_to_f32(le3(0xFFFFFF)), -LSB);
        let p = [0xEE, 0x56, 0x34, 0x92];
        assert_eq!(
            unsafe { pcm_s24_decode(p.as_ptr().add(1)) },
            sext24(0x923456)
        );
        assert_eq!(unsafe { pcm_s24_decode(core::ptr::null()) }, 0);
    }

    #[test]
    fn scalar_encode_ties_nan_saturation() {
        let cases: &[(f32, i32)] = &[
            (0.0, 0),
            (-0.0, 0),
            (LSB, 1),
            (-LSB, -1),
            (8_388_607.0 * LSB, PCM_MAX),
            (-1.0, PCM_MIN),
            (1.0, PCM_MAX),
            (0.25 * LSB, 0),
            (-0.25 * LSB, 0),
            (0.5 * LSB, 1),
            (-0.5 * LSB, -1),
            (0.75 * LSB, 1),
            (1.5 * LSB, 2),
            (-1.5 * LSB, -2),
            (2.5 * LSB, 3),
            (-2.5 * LSB, -3),
            (8_388_606.5 * LSB, PCM_MAX),
            (8_388_607.5 * LSB, PCM_MAX),
            (-8_388_607.5 * LSB, PCM_MIN),
            (f32::MIN_POSITIVE, 0),
            (-f32::MIN_POSITIVE, 0),
            (f32::NAN, 0),
            (-f32::NAN, 0),
            (f32::INFINITY, PCM_MAX),
            (f32::NEG_INFINITY, PCM_MIN),
            (f32::MAX, PCM_MAX),
            (f32::MIN, PCM_MIN),
            (2.0, PCM_MAX),
            (-8_388_609.0 * LSB, PCM_MIN),
        ];
        for &(x, v) in cases {
            assert_eq!(encode_s24(x), le3(v as u32), "encode({x:e})");
        }
        let mut buf = [0xA5u8; 5];
        unsafe { pcm_s24_encode(-LSB, buf.as_mut_ptr().add(1)) };
        assert_eq!(buf, [0xA5, 0xFF, 0xFF, 0xFF, 0xA5]);
        unsafe { pcm_s24_encode(1.0, core::ptr::null_mut()) };
    }

    /// 16.7M iterations: run with `cargo test --release`.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "exhaustive; run in release")]
    fn roundtrip_exhaustive() {
        for u in 0..=0xFF_FFFFu32 {
            let b = le3(u);
            let v = decode_s24(b);
            let f = s24_to_f32(b);
            assert_eq!(v, sext24(u));
            assert_eq!(f, v as f32 / 8_388_608.0);
            assert_eq!(encode_s24(f), b, "0x{u:06x}");
        }
    }

    fn bits(f: usize, c: usize) -> u32 {
        ((f * CH + c + 1) as u32).wrapping_mul(0x2A5B3D) & 0xFF_FFFF
    }

    fn packed() -> Vec<u8> {
        (0..FR * CH)
            .flat_map(|i| le3(bits(i / CH, i % CH)))
            .collect()
    }

    fn roundtrip_layout(fs: usize, cs: usize, count: usize) {
        let raw = packed();
        let mut floats = vec![SENTINEL; count + 4];
        assert!(decode_frames(&raw, &mut floats[..count], FR, CH, fs, cs));
        for f in 0..FR {
            for c in 0..CH {
                assert_eq!(
                    floats[f * fs + c * cs],
                    sext24(bits(f, c)) as f32 / 8_388_608.0
                );
            }
        }
        assert_eq!(floats.iter().filter(|&&x| x != SENTINEL).count(), FR * CH);
        assert!(floats[count..].iter().all(|&x| x == SENTINEL));
        let mut out = vec![0xA5u8; raw.len() + 4];
        assert!(encode_frames(
            &floats[..count],
            &mut out[..raw.len()],
            FR,
            CH,
            fs,
            cs
        ));
        assert_eq!(&out[..raw.len()], &raw[..]);
        assert!(out[raw.len()..].iter().all(|&b| b == 0xA5));
    }

    #[test]
    fn strided_layouts() {
        roundtrip_layout(CH, 1, FR * CH); // interleaved
        roundtrip_layout(1, FR, FR * CH); // planar
        roundtrip_layout(CH + 2, 1, (FR - 1) * (CH + 2) + CH);
        roundtrip_layout(1, FR + 2, (CH - 1) * (FR + 2) + FR);
    }

    #[test]
    fn zero_frames_one_frame_and_interleaved_strides() {
        assert!(unsafe {
            pcm_decode_frames(core::ptr::null(), 0, core::ptr::null_mut(), 0, 0, 2, 0, 0)
        });
        assert!(unsafe {
            pcm_encode_frames(core::ptr::null(), 0, core::ptr::null_mut(), 0, 0, 2, 0, 0)
        });
        let mut floats = [SENTINEL; CH];
        let mut one = [0xA5u8; CH * 3];
        assert!(decode_frames(&one, &mut floats, 0, CH, 0, 0));
        assert!(encode_frames(&[SENTINEL; CH], &mut one, 0, CH, 0, 0));
        assert!(floats.iter().all(|&x| x == SENTINEL) && one.iter().all(|&b| b == 0xA5));
        // One frame: frame stride irrelevant.
        assert!(decode_frames(&one, &mut floats, 1, CH, 0, 1));
        // Non-nested, non-overlapping strides: addresses 0,3,2,5.
        let src: Vec<u8> = [1, 2, 3, 4].into_iter().flat_map(le3).collect();
        let mut f6 = [SENTINEL; 6];
        assert!(decode_frames(&src, &mut f6, 2, 2, 2, 3));
        assert_eq!(
            f6,
            [LSB, SENTINEL, 3.0 * LSB, 2.0 * LSB, SENTINEL, 4.0 * LSB]
        );
    }

    #[test]
    fn rejects_overflow_alias_and_short_extents() {
        let half = usize::MAX / 2 + 1;
        let need = FR * CH * 3;
        #[rustfmt::skip]
        let cases: &[(usize, usize, usize, usize, usize, usize)] = &[
            (need, FR * CH, FR, 0, CH, 1),             // zero channels
            (need + 1, FR * CH, FR, CH, CH, 1),        // partial frame
            (need - 1, FR * CH, FR, CH, CH, 1),        // partial + short
            (need - CH * 3, FR * CH, FR, CH, CH, 1),   // short bytes
            (need, FR * CH - 1, FR, CH, CH, 1),        // short floats
            (need, FR * CH - 1, FR, CH, 1, FR),        // short planar
            (need, FR * CH, FR, CH, 1, 1),             // overlap
            (need, FR * CH, FR, CH, 0, 1),             // zero fstride
            (need, FR * CH, FR, CH, CH, 0),            // zero cstride
            (need, FR * CH, FR, CH, CH / 2, 2),        // overlap (gcd)
            (need, FR * CH, FR, CH, CH - 1, 1),        // overlap
            (5, FR * CH, 0, 2, 0, 0),                  // zero frames, partial
            (need, FR * CH, usize::MAX, 1, 1, 1),      // frames overflow
            (need, FR * CH, 1, usize::MAX / 3 + 1, 0, 1), // frame bytes ovf
            (need, FR * CH, 3, 1, half, 1),            // 2*fs wraps
            (need, FR * CH, 1, 4, 0, usize::MAX / 3 + 1), // 3*cs wraps
            (need, FR * CH, 2, 2, half, half + 1),     // sum wraps
        ];
        // Buffers have slack so a buggy capacity check stays in bounds.
        let mut src = packed();
        src.resize(need + 8, 0);
        let fsrc = [0.25f32; FR * CH];
        for (i, &(bytes, count, frames, ch, fs, cs)) in cases.iter().enumerate() {
            let mut fdst = [SENTINEL; FR * CH];
            let mut bdst = [0xA5u8; FR * CH * 3 + 8];
            // count never exceeds the real buffers here.
            let ok_d = unsafe {
                pcm_decode_frames(
                    src.as_ptr(),
                    bytes,
                    fdst.as_mut_ptr(),
                    count,
                    frames,
                    ch,
                    fs,
                    cs,
                )
            };
            let ok_e = unsafe {
                pcm_encode_frames(
                    fsrc.as_ptr(),
                    count,
                    bdst.as_mut_ptr(),
                    bytes,
                    frames,
                    ch,
                    fs,
                    cs,
                )
            };
            assert!(!ok_d && !ok_e, "case {i}");
            assert!(fdst.iter().all(|&x| x == SENTINEL), "case {i}");
            assert!(bdst.iter().all(|&b| b == 0xA5), "case {i}");
        }
        let null_u8 = core::ptr::null::<u8>();
        let mut fdst = [SENTINEL; FR * CH];
        unsafe {
            assert!(!pcm_decode_frames(
                null_u8,
                need,
                fdst.as_mut_ptr(),
                FR * CH,
                FR,
                CH,
                CH,
                1
            ));
            assert!(!pcm_decode_frames(
                src.as_ptr(),
                need,
                core::ptr::null_mut(),
                FR * CH,
                FR,
                CH,
                CH,
                1
            ));
            assert!(!pcm_decode_frames(
                null_u8,
                3,
                core::ptr::null_mut(),
                0,
                0,
                1,
                0,
                0
            ));
            assert!(!pcm_encode_frames(
                core::ptr::null(),
                1,
                core::ptr::null_mut(),
                0,
                0,
                1,
                0,
                0
            ));
        }
    }

    #[test]
    fn element_offset_fits_but_byte_address_overflows() {
        let mut small = [0u8; 6];
        let mut guarded = [0.25f32, -0.25];
        let fs = usize::MAX / size_of::<f32>();
        // Capacity is a lie; validation must reject before any access.
        unsafe {
            assert!(!pcm_decode_frames(
                small.as_ptr(),
                6,
                guarded.as_mut_ptr(),
                usize::MAX,
                2,
                1,
                fs,
                1
            ));
            assert!(!pcm_encode_frames(
                guarded.as_ptr(),
                usize::MAX,
                small.as_mut_ptr(),
                6,
                2,
                1,
                fs,
                1
            ));
        }
        assert_eq!(guarded, [0.25, -0.25]);
        assert_eq!(small, [0; 6]);
    }
}
