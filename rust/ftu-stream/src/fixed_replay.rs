//! Offline-only wall-time absence and capture-service counterfactuals.
//! No scheduling policy, ABI, or native dispatch is changed by this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    pub start: u64,
    pub end: u64,
}
impl Window {
    pub const fn new(start: u64, end: u64) -> Self {
        assert!(start < end);
        Self { start, end }
    }
    pub fn resume(self, at: u64) -> u64 {
        if (self.start..self.end).contains(&at) {
            self.end
        } else {
            at
        }
    }
    /// CPU work is suspended; the window wins a tie at its starting boundary.
    pub fn work(self, at: u64, cost: u64) -> u64 {
        let at = self.resume(at);
        let end = at.checked_add(cost).unwrap();
        if at < self.start && end >= self.start {
            end.checked_add(self.end - self.start).unwrap()
        } else {
            end
        }
    }
    /// A wall-clock timeout keeps expiring while this context is unavailable.
    pub fn wait(self, target: u64) -> u64 {
        self.resume(target)
    }
}
pub(crate) fn work(at: u64, cost: u64, absent: Option<Window>) -> u64 {
    absent.map_or_else(|| at.checked_add(cost).unwrap(), |w| w.work(at, cost))
}
pub(crate) fn resume(at: u64, absent: Option<Window>) -> u64 {
    absent.map_or(at, |w| w.resume(at))
}
#[test]
fn fixed_replay_work_wait_and_overlap_are_distinct() {
    let w = Window::new(100, 120);
    assert_eq!(w.work(90, 5), 95);
    assert_eq!(w.work(90, 10), 120);
    assert_eq!(w.work(90, 15), 125);
    assert_eq!(w.work(105, 5), 125);
    assert_eq!(w.work(120, 5), 125);
    assert_eq!(w.wait(115), 120);
    assert_eq!(w.wait(125), 125);
    // A nominal wake at 105 plus lateness 10 overlaps the same absence.
    assert_eq!(w.wait(105 + 10), 120);
    // A separately imposed delay beyond that absence legitimately extends it.
    assert_eq!(w.wait(105 + 20), 125);
    assert_eq!(work(90, 15, None), 105);
}
