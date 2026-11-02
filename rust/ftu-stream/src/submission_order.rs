//! Opt-in owner-thread scheduling, with bounded reconsideration and catch-up.
//! Deadlines are integer USB bus frames, not guarantees about OS service time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum SubmissionOrder {
    #[default]
    InputFirst = 0,
    Deadline = 1,
}
pub const RECHECK_ATTEMPTS: usize = 2;
pub const RECHECK_NS: u128 = 2_000_000;
/// At most one queue generation of work before dispatching native callbacks.
pub const CATCH_UP_ATTEMPTS: usize = 10;
pub const LIVE: SubmissionOrder = if cfg!(any(
    feature = "prototype-4ms-deadline",
    feature = "independent-4ms"
)) {
    SubmissionOrder::Deadline
} else {
    SubmissionOrder::InputFirst
};
impl SubmissionOrder {
    /// Inputs must precede start-1; output must return with >=16 frames lead.
    /// OUT wins a tie. Eligibility, sequence and ownership are supplied by pool.
    pub fn choose(self, starts: [Option<u64>; 2]) -> Option<usize> {
        match starts {
            [None, None] => None,
            [Some(_), None] => Some(0),
            [None, Some(_)] => Some(1),
            [Some(input), Some(output)] => Some(
                if self == Self::Deadline && output.saturating_sub(16) <= input.saturating_sub(2) {
                    1
                } else {
                    0
                },
            ),
        }
    }
}
/// Cooperative bound: a synchronous native call cannot be preempted. Recheck
/// after it returns, even if it alone exceeded the time allowance. Invalid or
/// backward time requests a recheck rather than silently extending the batch.
pub fn recheck(attempts: usize, begin: u64, end: u64, numer: u32, denom: u32) -> bool {
    attempts >= RECHECK_ATTEMPTS
        || denom == 0
        || numer == 0
        || end < begin
        || u128::from(end.saturating_sub(begin)) * u128::from(numer)
            >= RECHECK_NS * u128::from(denom)
}
#[no_mangle]
pub extern "C" fn ftu_live_submission_order() -> u32 {
    LIVE as u32
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn order_uses_effective_deadlines_and_only_eligible_heads() {
        let d = SubmissionOrder::Deadline;
        assert_eq!(d.choose([None, None]), None);
        assert_eq!(d.choose([Some(40), None]), Some(0));
        assert_eq!(d.choose([None, Some(40)]), Some(1));
        assert_eq!(d.choose([Some(40), Some(50)]), Some(1));
        assert_eq!(d.choose([Some(40), Some(54)]), Some(1));
        assert_eq!(d.choose([Some(40), Some(55)]), Some(0));
        assert_eq!(d.choose([Some(0), Some(0)]), Some(1));
        assert_eq!(d.choose([Some(u64::MAX), Some(u64::MAX)]), Some(1));
        assert_eq!(
            SubmissionOrder::InputFirst.choose([Some(40), Some(40)]),
            Some(0)
        );
    }
    #[test]
    fn catch_up_rechecks_count_elapsed_and_invalid_time() {
        assert!(!recheck(1, 10, 48009, 125, 3));
        assert!(recheck(1, 10, 48010, 125, 3));
        assert!(recheck(2, 10, 10, 125, 3));
        assert!(recheck(1, 10, 9, 125, 3));
        assert!(recheck(1, 10, 10, 0, 3));
        assert!(recheck(1, 10, 10, 125, 0));
        assert!(recheck(1, 0, u64::MAX, u32::MAX, u32::MAX));
        assert_eq!(CATCH_UP_ATTEMPTS, 10);
    }
}
