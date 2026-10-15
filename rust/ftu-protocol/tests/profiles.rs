//! Real desktop configuration descriptor (config-0.bin, 340 bytes); mirrors
//! test_capture_profile.c and test_feedback_profile.c.

use ftu_protocol::profile::{cap_profile_validate, feedback_profile_validate};
use ftu_protocol::validate_profile;
use std::path::PathBuf;

fn fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config-0.bin");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (copy the staged config-0.bin here)",
            path.display()
        )
    });
    assert_eq!(bytes.len(), 340);
    bytes
}

fn check(feedback: bool, fields: &[usize], duplicate_alt: usize) {
    let original = fixture();
    let n = original.len();
    assert!(validate_profile(&original, feedback));
    for i in 0..n {
        assert!(!validate_profile(&original[..i], feedback), "prefix {i}");
    }
    for &f in fields {
        let mut changed = original.clone();
        changed[f] ^= 1;
        assert!(!validate_profile(&changed, feedback), "field {f}");
    }
    let mut changed = original.clone();
    changed[duplicate_alt] = 1;
    assert!(!validate_profile(&changed, feedback));
    // Advertised total adjusted so the walker sees truncated interiors.
    for i in 9..n {
        let mut changed = original[..i].to_vec();
        changed[2..4].copy_from_slice(&(i as u16).to_le_bytes());
        let _ = validate_profile(&changed, feedback);
    }
    let mut rng: u32 = 2080;
    for _ in 0..25_000 {
        let mut changed = original.clone();
        for _ in 0..4 {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let off = rng as usize % n;
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            changed[off] = (rng >> 24) as u8;
        }
        let _ = validate_profile(&changed, feedback);
    }
}

#[test]
fn capture_profile_fixture() {
    check(
        false,
        &[
            149, 156, 157, 158, 159, 160, 161, 168, 169, 173, 174, 175, 176, 177, 181, 192, 193,
            194, 195, 196,
        ],
        207,
    );
}

#[test]
fn feedback_profile_fixture() {
    check(
        true,
        &[
            83, 90, 91, 92, 93, 94, 95, 102, 103, 107, 108, 109, 110, 111, 115, 126, 127, 128, 129,
            130, 140, 141, 142, 143, 144,
        ],
        156,
    );
}

#[test]
fn c_abi_entry_points() {
    let original = fixture();
    unsafe {
        assert!(cap_profile_validate(original.as_ptr(), original.len()));
        assert!(feedback_profile_validate(original.as_ptr(), original.len()));
        assert!(!cap_profile_validate(std::ptr::null(), 0));
        assert!(!feedback_profile_validate(std::ptr::null(), 340));
        assert!(!cap_profile_validate(original.as_ptr(), original.len() - 1));
    }
}
