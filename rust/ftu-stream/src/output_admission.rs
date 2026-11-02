//! Slot capacity does not prove PCM readiness. Opt-in owner-thread admission;
//! no permissions to touch pending USB data and no waits on the HAL thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum OutputPolicy {
    Eager = 0,
    /// Retry incomplete PCM only while both retry and work reserves remain.
    WhenReady = 1,
}
pub const MIN_LEAD_MS: u64 = 16;
pub const RETRY_ALLOWANCE_MS: u64 = 8;
pub const WORK_ALLOWANCE_MS: u64 = 2;
pub const WAIT_CUTOFF_MS: u64 = MIN_LEAD_MS + RETRY_ALLOWANCE_MS + WORK_ALLOWANCE_MS;
impl OutputPolicy {
    pub fn defer(self, ready: bool, lead_ms: u64) -> bool {
        self == Self::WhenReady && !ready && lead_ms > WAIT_CUTOFF_MS
    }
}
pub const LIVE_OUTPUT_POLICY: OutputPolicy = if cfg!(any(
    feature = "prototype-4ms-ready",
    feature = "independent-4ms",
    feature = "prototype-4ms-deadline"
)) {
    OutputPolicy::WhenReady
} else {
    OutputPolicy::Eager
};
/// Read-only linked build discriminator; never constructs a session.
#[no_mangle]
pub extern "C" fn ftu_live_output_policy() -> u32 {
    if LIVE_OUTPUT_POLICY == OutputPolicy::WhenReady {
        WAIT_CUTOFF_MS as u32
    } else {
        0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wait_is_bounded_and_never_uses_high_water_as_readiness() {
        assert!(OutputPolicy::WhenReady.defer(false, 27));
        for lead in [0, 15, 16, 18, 26] {
            assert!(!OutputPolicy::WhenReady.defer(false, lead));
        }
        assert!(!OutputPolicy::WhenReady.defer(true, 40));
        assert!(!OutputPolicy::Eager.defer(false, 40));
        assert_eq!(MIN_LEAD_MS, 16);
    }
}
