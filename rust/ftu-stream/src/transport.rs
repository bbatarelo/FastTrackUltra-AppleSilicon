//! Immutable live-pool geometry. Legacy one-shot probes keep scheduler.rs.
//! This module does not select a lower latency or grant access to pending USB.
use crate::scheduler::{ENTRIES, FEEDBACK_DELAY_MS};

pub const POOL_ABI_VERSION: u32 = 4;
pub const MAX_SLOTS: usize = 32;
pub const MAX_HISTORY: usize = 256;

/// Private native request v1, rebuilt with pool ABI v4. Native validates every
/// word before opening interfaces or allocating transfer storage.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transport {
    version: u32,
    entries: u32,
    slots: u32,
    history: u32,
    replay_ms: u32,
    pcm_offset: u32,
}

impl Transport {
    pub const fn new(block_ms: u32, slots: usize) -> Option<Self> {
        if !matches!(block_ms, 1 | 2 | 4 | 8) || slots < 2 || slots > MAX_SLOTS {
            return None;
        }
        // Retain a whole replay interval plus two queue generations and one
        // guard entry. This deliberately exceeds the nominal backlog; an
        // arbitrary stall still faults, rather than overwriting live history.
        let needed = FEEDBACK_DELAY_MS as usize / block_ms as usize + 2 * slots + 1;
        let mut history = 1;
        while history < needed {
            history *= 2;
        }
        if history > MAX_HISTORY {
            return None;
        }
        Some(Self {
            version: 1,
            entries: block_ms * 8,
            slots: slots as u32,
            history: history as u32,
            replay_ms: FEEDBACK_DELAY_MS as u32,
            pcm_offset: FEEDBACK_DELAY_MS as u32 * 48,
        })
    }
    pub const fn entries(self) -> usize {
        self.entries as usize
    }
    pub const fn block_ms(self) -> u64 {
        self.entries as u64 / 8
    }
    pub const fn slots(self) -> usize {
        self.slots as usize
    }
    pub const fn history(self) -> usize {
        self.history as usize
    }
    pub const fn bytes(self) -> usize {
        self.entries() * 312
    }
    pub const fn max_frames(self) -> usize {
        self.entries() * 13
    }
    pub const fn replay_ms(self) -> u64 {
        self.replay_ms as u64
    }
    pub const fn pcm_offset(self) -> u64 {
        self.pcm_offset as u64
    }
    pub fn start_frame(self, base: u64, seq: usize, output: bool) -> Option<u64> {
        base.checked_add((seq as u64).checked_mul(self.block_ms())?)?
            .checked_add(if output { self.replay_ms() } else { 0 })
    }
    pub fn valid(self) -> bool {
        self.entries <= ENTRIES as u32
            && self.entries.is_multiple_of(8)
            && Self::new(self.entries / 8, self.slots()) == Some(self)
    }
}

pub const BASELINE: Transport = match Transport::new(8, 4) {
    Some(g) => g,
    None => panic!("invalid baseline"),
};
pub const SHORTER: Transport = match Transport::new(4, 8) {
    Some(g) => g,
    None => panic!("invalid shorter profile"),
};
pub const SHORTER_TEN: Transport = match Transport::new(4, 10) {
    Some(g) => g,
    None => panic!("invalid ten-slot profile"),
};
#[cfg(any(
    all(
        feature = "independent-4ms",
        any(
            feature = "prototype-4ms",
            feature = "prototype-4ms-ten-slot",
            feature = "prototype-4ms-ready",
            feature = "prototype-4ms-deadline"
        )
    ),
    all(
        feature = "prototype-4ms",
        any(
            feature = "prototype-4ms-ten-slot",
            feature = "prototype-4ms-ready",
            feature = "prototype-4ms-deadline"
        )
    ),
    all(
        feature = "prototype-4ms-ten-slot",
        any(feature = "prototype-4ms-ready", feature = "prototype-4ms-deadline")
    ),
    all(feature = "prototype-4ms-ready", feature = "prototype-4ms-deadline")
))]
compile_error!("select exactly one experimental transport profile");
pub const LIVE: Transport = if cfg!(feature = "independent-4ms") {
    match Transport::new(4, 12) {
        Some(g) => g,
        None => panic!("invalid independent geometry"),
    }
} else if cfg!(any(
    feature = "prototype-4ms-ten-slot",
    feature = "prototype-4ms-ready",
    feature = "prototype-4ms-deadline"
)) {
    SHORTER_TEN
} else if cfg!(feature = "prototype-4ms") {
    SHORTER
} else {
    BASELINE
};

/// Read the linked library's profile without constructing a session or touching USB.
/// # Safety
/// A non-null output must point to writable storage for one Transport value.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_transport_geometry(out: *mut Transport) -> i32 {
    let Some(out) = out.as_mut() else {
        return -1;
    };
    *out = LIVE;
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolved_profiles_cover_replay_and_preserve_sample_coordinates() {
        for (ms, slots, history) in [
            (8, 4, 32),
            (4, 8, 64),
            (4, 10, 64),
            (2, 16, 128),
            (1, 32, 256),
        ] {
            let g = Transport::new(ms, slots).unwrap();
            assert!(g.valid());
            assert_eq!(g.history(), history);
            assert_eq!(
                g.block_ms() * g.slots() as u64,
                if slots == 10 { 40 } else { 32 }
            );
            assert_eq!(g.entries(), ms as usize * 8);
            assert_eq!(g.pcm_offset(), 3072);
            assert!(g.history() > 64 / ms as usize + 2 * slots);
            for seq in [0, 1, 31, 100_000] {
                assert_eq!(
                    g.start_frame(100, seq, true).unwrap()
                        - g.start_frame(100, seq, false).unwrap(),
                    64
                );
            }
            assert!(g.start_frame(u64::MAX, 1, false).is_none());
        }
        assert_eq!(std::mem::size_of::<Transport>(), 24);
    }
    #[test]
    fn malformed_or_unbounded_geometry_is_rejected() {
        for ms in [0, 3, 5, 7, 9, u32::MAX] {
            assert!(Transport::new(ms, 4).is_none());
        }
        for slots in [0, 1, 33, usize::MAX] {
            assert!(Transport::new(4, slots).is_none());
        }
        let mut g = SHORTER;
        g.history = 32;
        assert!(!g.valid());
        g = SHORTER;
        g.entries = 31;
        assert!(!g.valid());
        g = SHORTER;
        g.version = 2;
        assert!(!g.valid());
        g = SHORTER;
        g.pcm_offset = 0;
        assert!(!g.valid());
    }
}
