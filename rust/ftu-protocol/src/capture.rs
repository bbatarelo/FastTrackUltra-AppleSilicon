//! Capture block ownership model and per-packet length gates
//! (`capture_model.h`, plus `feedback_packet_check` from `feedback_profile.h`).

use core::ffi::c_char;

pub const CAP_MAX_BLOCKS: usize = 256;
pub const CAP_QUEUE_DEPTH: usize = 4;
pub const CAP_PACKET_BYTES: usize = 312;
pub const CAP_AUDIO_FRAME_BYTES: usize = 24;
/// Largest feedback payload accepted as bounded evidence.
pub const FEEDBACK_PACKET_BYTES: usize = 3;

/// `enum cap_block_state` values.
pub const CAP_NEW: u32 = 0;
pub const CAP_PENDING: u32 = 1;
pub const CAP_COMPLETE: u32 = 2;
pub const CAP_SUBMIT_FAILED: u32 = 3;

/// `enum cap_packet_kind` values.
pub const CAP_PACKET_OK: u32 = 0;
pub const CAP_PACKET_EMPTY: u32 = 1;
pub const CAP_PACKET_PENDING: u32 = 2;
pub const CAP_PACKET_ERROR: u32 = 3;
pub const CAP_PACKET_OVERSIZE: u32 = 4;
pub const CAP_PACKET_UNALIGNED: u32 = 5;

/// `struct cap_model`. Blocks are submitted strictly in order, at most
/// `CAP_QUEUE_DEPTH` pending; completions may arrive in any order. A failed
/// submit consumes its index, owes no completion, and stops the model.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapModel {
    pub state: [u32; CAP_MAX_BLOCKS],
    pub count: usize,
    pub next: usize,
    pub pending: usize,
    pub submitted: usize,
    pub completed: usize,
    pub failed: usize,
    pub stopped: bool,
}

impl CapModel {
    /// `None` unless `1 <= count <= CAP_MAX_BLOCKS`.
    pub fn new(count: usize) -> Option<Self> {
        if count == 0 || count > CAP_MAX_BLOCKS {
            return None;
        }
        Some(Self {
            state: [CAP_NEW; CAP_MAX_BLOCKS],
            count,
            next: 0,
            pending: 0,
            submitted: 0,
            completed: 0,
            failed: 0,
            stopped: false,
        })
    }

    pub fn can_submit(&self, index: usize) -> bool {
        !self.stopped
            && index == self.next
            && self.next < self.count
            && self.pending < CAP_QUEUE_DEPTH
    }

    /// True iff the attempt was recorded; a recorded failure (`success ==
    /// false`) also returns true. False means invalid and `self` is unchanged.
    pub fn submit(&mut self, index: usize, success: bool) -> bool {
        // The array bound also guards a caller-corrupted `count`.
        if !self.can_submit(index) || index >= CAP_MAX_BLOCKS {
            return false;
        }
        self.next = index + 1;
        if success {
            self.state[index] = CAP_PENDING;
            self.pending += 1;
            self.submitted = self.submitted.wrapping_add(1);
        } else {
            self.state[index] = CAP_SUBMIT_FAILED;
            self.failed = self.failed.wrapping_add(1);
            self.stopped = true;
        }
        true
    }

    pub fn complete(&mut self, index: usize) -> bool {
        if index >= self.count || self.state.get(index) != Some(&CAP_PENDING) {
            return false;
        }
        self.state[index] = CAP_COMPLETE;
        self.pending = self.pending.wrapping_sub(1);
        self.completed = self.completed.wrapping_add(1);
        true
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn can_release(&self) -> bool {
        self.pending == 0
    }
}

/// `struct cap_packet`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapPacket {
    pub kind: u32,
    pub audio_frames: usize,
    pub accepted: bool,
}

const fn packet(kind: u32, audio_frames: usize, accepted: bool) -> CapPacket {
    CapPacket {
        kind,
        audio_frames,
        accepted,
    }
}

/// Capture packet gate. Precedence: pending, oversize, error status,
/// unaligned, empty (accepted, zero frames), ok.
pub fn packet_check(completed: bool, status_ok: bool, actual: usize) -> CapPacket {
    if !completed {
        packet(CAP_PACKET_PENDING, 0, false)
    } else if actual > CAP_PACKET_BYTES {
        packet(CAP_PACKET_OVERSIZE, 0, false)
    } else if !status_ok {
        packet(CAP_PACKET_ERROR, 0, false)
    } else if !actual.is_multiple_of(CAP_AUDIO_FRAME_BYTES) {
        packet(CAP_PACKET_UNALIGNED, 0, false)
    } else if actual == 0 {
        packet(CAP_PACKET_EMPTY, 0, true)
    } else {
        packet(CAP_PACKET_OK, actual / CAP_AUDIO_FRAME_BYTES, true)
    }
}

/// Feedback packet gate. Accepted means bounded observed bytes (0..=3), not a
/// validated feedback encoding; short payloads are never padded.
pub fn feedback_check(completed: bool, status_ok: bool, actual: usize) -> CapPacket {
    if !completed {
        packet(CAP_PACKET_PENDING, 0, false)
    } else if actual > FEEDBACK_PACKET_BYTES {
        packet(CAP_PACKET_OVERSIZE, 0, false)
    } else if !status_ok {
        packet(CAP_PACKET_ERROR, 0, false)
    } else if actual == 0 {
        packet(CAP_PACKET_EMPTY, 0, true)
    } else {
        packet(CAP_PACKET_OK, 0, true)
    }
}

fn packet_name_nul(kind: u32) -> &'static str {
    match kind {
        CAP_PACKET_OK => "OK\0",
        CAP_PACKET_EMPTY => "EMPTY\0",
        CAP_PACKET_PENDING => "PENDING\0",
        CAP_PACKET_ERROR => "ERROR\0",
        CAP_PACKET_OVERSIZE => "OVERSIZE\0",
        CAP_PACKET_UNALIGNED => "UNALIGNED\0",
        _ => "UNKNOWN\0",
    }
}

pub fn packet_name(kind: u32) -> &'static str {
    packet_name_nul(kind).trim_end_matches('\0')
}

// ---- C ABI ----------------------------------------------------------------

/// # Safety
/// `m` is null or valid for writing one `struct cap_model`. Prior contents
/// are never read, so uninitialised storage is fine. On failure `*m` is
/// untouched.
#[no_mangle]
pub unsafe extern "C" fn cap_model_init(m: *mut CapModel, count: usize) -> bool {
    if m.is_null() {
        return false;
    }
    match CapModel::new(count) {
        Some(model) => {
            // SAFETY: non-null, caller guarantees writable storage; write()
            // does not read or drop the old value.
            unsafe { m.write(model) };
            true
        }
        None => false,
    }
}

/// # Safety
/// `m` is null or points to an initialised model not being mutated
/// concurrently.
#[no_mangle]
pub unsafe extern "C" fn cap_model_can_submit(m: *const CapModel, index: usize) -> bool {
    // SAFETY: caller contract above.
    unsafe { m.as_ref() }.is_some_and(|m| m.can_submit(index))
}

/// # Safety
/// `m` is null or points to an initialised model with no other live access.
#[no_mangle]
pub unsafe extern "C" fn cap_model_submit(m: *mut CapModel, index: usize, success: bool) -> bool {
    // SAFETY: caller contract above.
    unsafe { m.as_mut() }.is_some_and(|m| m.submit(index, success))
}

/// # Safety
/// `m` is null or points to an initialised model with no other live access.
#[no_mangle]
pub unsafe extern "C" fn cap_model_complete(m: *mut CapModel, index: usize) -> bool {
    // SAFETY: caller contract above.
    unsafe { m.as_mut() }.is_some_and(|m| m.complete(index))
}

/// # Safety
/// `m` is null or points to an initialised model with no other live access.
#[no_mangle]
pub unsafe extern "C" fn cap_model_stop(m: *mut CapModel) {
    // SAFETY: caller contract above.
    if let Some(m) = unsafe { m.as_mut() } {
        m.stop();
    }
}

/// # Safety
/// `m` is null or points to an initialised model not being mutated
/// concurrently.
#[no_mangle]
pub unsafe extern "C" fn cap_model_can_release(m: *const CapModel) -> bool {
    // SAFETY: caller contract above.
    unsafe { m.as_ref() }.is_some_and(CapModel::can_release)
}

#[no_mangle]
pub extern "C" fn cap_packet_check(completed: bool, status_ok: bool, actual: usize) -> CapPacket {
    packet_check(completed, status_ok, actual)
}

#[no_mangle]
pub extern "C" fn feedback_packet_check(
    completed: bool,
    status_ok: bool,
    actual: usize,
) -> CapPacket {
    feedback_check(completed, status_ok, actual)
}

/// Returns a static NUL-terminated name; unknown kinds give "UNKNOWN".
#[no_mangle]
pub extern "C" fn cap_packet_name(kind: u32) -> *const c_char {
    packet_name_nul(kind).as_ptr().cast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of, MaybeUninit};

    fn invariants(m: &CapModel) {
        let mut n = [0usize; 4];
        for &s in &m.state[..m.count] {
            n[s as usize] += 1;
        }
        assert!(m.next <= m.count);
        assert_eq!(m.next, m.submitted + m.failed);
        assert_eq!(m.submitted, m.completed + m.pending);
        assert_eq!(n[CAP_NEW as usize], m.count - m.next);
        assert_eq!(n[CAP_PENDING as usize], m.pending);
        assert_eq!(n[CAP_COMPLETE as usize], m.completed);
        assert_eq!(n[CAP_SUBMIT_FAILED as usize], m.failed);
        assert!(m.pending <= CAP_QUEUE_DEPTH);
        assert!(m.failed <= 1 && (m.failed == 0 || m.stopped));
    }

    fn submit_ok(m: &mut CapModel, i: usize) {
        assert!(m.submit(i, true));
        invariants(m);
    }

    fn complete_ok(m: &mut CapModel, i: usize) {
        assert!(m.complete(i));
        invariants(m);
    }

    fn rejected_unchanged(m: &mut CapModel, f: impl FnOnce(&mut CapModel) -> bool) {
        let before = *m;
        assert!(!f(m));
        assert_eq!(before, *m);
    }

    #[test]
    fn layout_matches_c() {
        let w = size_of::<usize>();
        assert_eq!(offset_of!(CapModel, count), 1024);
        assert_eq!(offset_of!(CapModel, stopped), 1024 + 6 * w);
        assert_eq!(offset_of!(CapPacket, audio_frames), w.max(4));
        assert_eq!(offset_of!(CapPacket, accepted), w.max(4) + w);
    }

    #[test]
    fn init_bounds_and_uninit_storage() {
        assert!(CapModel::new(0).is_none());
        assert!(CapModel::new(CAP_MAX_BLOCKS + 1).is_none());
        assert!(CapModel::new(usize::MAX).is_none());
        assert!(!unsafe { cap_model_init(core::ptr::null_mut(), 1) });

        let mut slot = MaybeUninit::<CapModel>::uninit();
        let p = slot.as_mut_ptr();
        unsafe { core::ptr::write_bytes(p.cast::<u8>(), 0x5A, size_of::<CapModel>()) };
        assert!(!unsafe { cap_model_init(p, 0) });
        assert!(unsafe { cap_model_init(p, CAP_MAX_BLOCKS) });
        let m = unsafe { slot.assume_init() };
        assert_eq!(m, CapModel::new(CAP_MAX_BLOCKS).unwrap());
        assert!(m.can_release());
        invariants(&m);
    }

    #[test]
    fn out_of_order_completion() {
        let mut m = CapModel::new(8).unwrap();
        for i in 0..4 {
            submit_ok(&mut m, i);
        }
        rejected_unchanged(&mut m, |m| m.submit(4, true)); // queue full
        rejected_unchanged(&mut m, |m| m.submit(4, false));
        complete_ok(&mut m, 2);
        complete_ok(&mut m, 0);
        rejected_unchanged(&mut m, |m| m.submit(5, true)); // not sequential
        rejected_unchanged(&mut m, |m| m.submit(3, true)); // already used
        submit_ok(&mut m, 4);
        submit_ok(&mut m, 5);
        for i in [5, 3, 1, 4] {
            complete_ok(&mut m, i);
        }
        submit_ok(&mut m, 6);
        submit_ok(&mut m, 7);
        assert!(!m.can_release());
        complete_ok(&mut m, 7);
        complete_ok(&mut m, 6);
        assert!(m.can_release() && !m.stopped);
        assert_eq!((m.submitted, m.completed, m.failed), (8, 8, 0));
        rejected_unchanged(&mut m, |m| m.submit(8, true)); // exhausted
    }

    #[test]
    fn failed_submit_keeps_debt_until_drained() {
        let mut m = CapModel::new(8).unwrap();
        for i in 0..3 {
            submit_ok(&mut m, i);
        }
        assert!(m.submit(3, false));
        invariants(&m);
        assert_eq!(m.state[3], CAP_SUBMIT_FAILED);
        assert!(m.stopped && m.failed == 1 && m.next == 4 && m.pending == 3);
        rejected_unchanged(&mut m, |m| m.submit(4, true));
        rejected_unchanged(&mut m, |m| m.complete(3)); // failed submit owes nothing
        complete_ok(&mut m, 1);
        complete_ok(&mut m, 2);
        assert!(!m.can_release());
        complete_ok(&mut m, 0);
        assert!(m.can_release());

        let mut first = CapModel::new(2).unwrap();
        assert!(first.submit(0, false));
        assert!(first.stopped && first.can_release());
        rejected_unchanged(&mut first, |m| m.complete(0));
    }

    #[test]
    fn stop_drains_and_rejects_bad_completions() {
        let mut m = CapModel::new(8).unwrap();
        submit_ok(&mut m, 0);
        submit_ok(&mut m, 1);
        m.stop();
        invariants(&m);
        rejected_unchanged(&mut m, |m| m.submit(2, true));
        complete_ok(&mut m, 1);
        assert!(!m.can_release());
        complete_ok(&mut m, 0);
        assert!(m.can_release());
        for i in [0, 2, 8, CAP_MAX_BLOCKS, usize::MAX] {
            rejected_unchanged(&mut m, |m| m.complete(i));
        }
        unsafe { cap_model_stop(core::ptr::null_mut()) };
        assert!(!unsafe { cap_model_complete(core::ptr::null_mut(), 0) });
        assert!(!unsafe { cap_model_can_submit(core::ptr::null(), 0) });
        assert!(!unsafe { cap_model_can_release(core::ptr::null()) });
    }

    #[test]
    fn corrupted_count_never_indexes_out_of_bounds() {
        let mut m = CapModel::new(4).unwrap();
        m.count = usize::MAX;
        m.next = CAP_MAX_BLOCKS;
        rejected_unchanged(&mut m, |m| m.submit(CAP_MAX_BLOCKS, true));
        rejected_unchanged(&mut m, |m| m.complete(CAP_MAX_BLOCKS));
    }

    #[test]
    fn packet_gates() {
        let cases = [
            (false, true, 24, CAP_PACKET_PENDING, 0, false),
            (false, false, 400, CAP_PACKET_PENDING, 0, false),
            (true, true, 313, CAP_PACKET_OVERSIZE, 0, false),
            (true, false, 313, CAP_PACKET_OVERSIZE, 0, false),
            (true, true, usize::MAX, CAP_PACKET_OVERSIZE, 0, false),
            (true, false, 24, CAP_PACKET_ERROR, 0, false),
            (true, false, 0, CAP_PACKET_ERROR, 0, false),
            (true, false, 25, CAP_PACKET_ERROR, 0, false),
            (true, true, 1, CAP_PACKET_UNALIGNED, 0, false),
            (true, true, 311, CAP_PACKET_UNALIGNED, 0, false),
            (true, true, 0, CAP_PACKET_EMPTY, 0, true),
            (true, true, 24, CAP_PACKET_OK, 1, true),
            (true, true, 312, CAP_PACKET_OK, 13, true),
        ];
        for (c, ok, n, kind, frames, accepted) in cases {
            assert_eq!(
                cap_packet_check(c, ok, n),
                packet(kind, frames, accepted),
                "{n}"
            );
        }
        for len in 0..=4usize {
            let a = feedback_packet_check(true, true, len);
            assert_eq!(a.accepted, len <= 3);
            assert_eq!(a.audio_frames, 0);
            let want = match len {
                0 => CAP_PACKET_EMPTY,
                1..=3 => CAP_PACKET_OK,
                _ => CAP_PACKET_OVERSIZE,
            };
            assert_eq!(a.kind, want);
            assert!(!feedback_packet_check(true, false, len).accepted);
            assert_eq!(
                feedback_packet_check(false, true, len).kind,
                CAP_PACKET_PENDING
            );
        }
    }

    #[test]
    fn packet_names_distinct_and_terminated() {
        let names: Vec<&str> = (0..6).map(packet_name).collect();
        for (i, a) in names.iter().enumerate() {
            assert!(names[..i].iter().all(|b| b != a));
        }
        assert_eq!(packet_name(99), "UNKNOWN");
        let p = cap_packet_name(CAP_PACKET_UNALIGNED);
        let s = unsafe { core::ffi::CStr::from_ptr(p) };
        assert_eq!(s.to_str(), Ok("UNALIGNED"));
    }
}
