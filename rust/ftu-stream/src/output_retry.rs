//! Observation only: one USB-owner writer, bounded coherent control-thread reads.
//! Counts do not alter readiness, clocks, deadlines or ownership.
use crate::{output_admission::*, timing::STEADY_AFTER};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
pub const HEADER: usize = 12;
pub const COUNTS: usize = 10;
const WORDS: usize = HEADER + 2 * COUNTS;
pub const CHECKS: usize = 0;
pub const ATTEMPTS: usize = 1;
pub const READY: usize = 2;
pub const DEFERRALS: usize = 3;
pub const FALLBACKS: usize = 4;
pub const RETRIES: usize = 5;
pub const OVER_BUDGET: usize = 6;
pub const INVALID_TIME: usize = 7;
pub const MAX_NS: usize = 8;
pub const LAST_NS: usize = 9;
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    // version, valid, epoch, policy, cutoff bus frames, lead gate bus frames,
    // retry budget ns, work budget ns, steady cursor, timebase num/den, waiting
    pub header: [u64; HEADER],
    // all / captured cursor >= STEADY_AFTER; see constants above
    pub counts: [[u64; COUNTS]; 2],
}
impl Snapshot {
    pub fn new(epoch: u64, policy: OutputPolicy, numer: u32, denom: u32) -> Self {
        Self {
            header: [
                1,
                1,
                epoch,
                policy as u64,
                WAIT_CUTOFF_MS,
                MIN_LEAD_MS,
                RETRY_ALLOWANCE_MS * 1_000_000,
                WORK_ALLOWANCE_MS * 1_000_000,
                STEADY_AFTER,
                numer.into(),
                denom.into(),
                0,
            ],
            counts: [[0; COUNTS]; 2],
        }
    }
    fn words(self) -> [u64; WORDS] {
        let mut a = [0; WORDS];
        a[..HEADER].copy_from_slice(&self.header);
        a[HEADER..HEADER + COUNTS].copy_from_slice(&self.counts[0]);
        a[HEADER + COUNTS..].copy_from_slice(&self.counts[1]);
        a
    }
    fn from_words(a: [u64; WORDS]) -> Self {
        Self {
            header: a[..HEADER].try_into().unwrap(),
            counts: [
                a[HEADER..HEADER + COUNTS].try_into().unwrap(),
                a[HEADER + COUNTS..].try_into().unwrap(),
            ],
        }
    }
}
pub struct Tracker {
    pub snapshot: Snapshot,
    deferred_at: Option<u64>,
}
impl Tracker {
    pub fn new(epoch: u64, policy: OutputPolicy, numer: u32, denom: u32) -> Self {
        Self {
            snapshot: Snapshot::new(epoch, policy, numer, denom),
            deferred_at: None,
        }
    }
    fn rows(&mut self, captured: u64) -> &mut [[u64; COUNTS]] {
        &mut self.snapshot.counts[..if captured >= STEADY_AFTER { 2 } else { 1 }]
    }
    pub fn begin(&mut self, captured: u64, ticks: u64) {
        let previous = self.deferred_at.take();
        self.snapshot.header[11] = 0;
        let ns = previous
            .and_then(|p| ticks.checked_sub(p))
            .filter(|_| self.snapshot.header[9] != 0 && self.snapshot.header[10] != 0)
            .and_then(|delta| {
                u64::try_from(
                    u128::from(delta) * u128::from(self.snapshot.header[9])
                        / u128::from(self.snapshot.header[10]),
                )
                .ok()
            });
        for row in self.rows(captured) {
            row[CHECKS] = row[CHECKS].saturating_add(1);
            if previous.is_some() {
                row[RETRIES] = row[RETRIES].saturating_add(1);
                if let Some(ns) = ns {
                    row[LAST_NS] = ns;
                    row[MAX_NS] = row[MAX_NS].max(ns);
                    row[OVER_BUDGET] = row[OVER_BUDGET]
                        .saturating_add(u64::from(ns > RETRY_ALLOWANCE_MS * 1_000_000));
                } else {
                    row[INVALID_TIME] = row[INVALID_TIME].saturating_add(1);
                }
            }
        }
    }
    pub fn decision(&mut self, captured: u64, ready: bool, deferred: bool, ticks: u64) {
        debug_assert!(!ready || !deferred);
        self.deferred_at = deferred.then_some(ticks);
        self.snapshot.header[11] = u64::from(deferred);
        for row in self.rows(captured) {
            row[ATTEMPTS] = row[ATTEMPTS].saturating_add(1);
            let at = if ready {
                READY
            } else if deferred {
                DEFERRALS
            } else {
                FALLBACKS
            };
            row[at] = row[at].saturating_add(1);
        }
    }
}
pub struct Published {
    sequence: AtomicU64,
    words: [AtomicU64; WORDS],
}
impl Published {
    pub fn new() -> Self {
        let a = Snapshot::new(0, LIVE_OUTPUT_POLICY, 0, 0).words();
        Self {
            sequence: AtomicU64::new(0),
            words: std::array::from_fn(|i| AtomicU64::new(a[i])),
        }
    }
    // Lifecycle joins the prior owner before resetting; reset and owner publish
    // never overlap. All payload words are atomic, including failed reads.
    pub fn publish(&self, s: &Snapshot) {
        self.sequence.fetch_add(1, SeqCst);
        for (a, v) in self.words.iter().zip(s.words()) {
            a.store(v, SeqCst);
        }
        self.sequence.fetch_add(1, SeqCst);
    }
    pub fn snapshot(&self) -> Snapshot {
        let begin = self.sequence.load(SeqCst);
        if begin & 1 == 0 {
            let a = std::array::from_fn(|i| self.words[i].load(SeqCst));
            if begin == self.sequence.load(SeqCst) {
                return Snapshot::from_words(a);
            }
        }
        let mut s = Snapshot::new(0, LIVE_OUTPUT_POLICY, 0, 0);
        s.header[1] = 0;
        s
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_budget_counts_elapsed_time_and_invalid_clocks_without_audio_loss() {
        let mut t = Tracker::new(7, OutputPolicy::WhenReady, 125, 3);
        for (i, delta) in [192_000, 192_001, 0].into_iter().enumerate() {
            t.begin(0, 1_000_000);
            t.decision(0, false, true, 1_000_000);
            t.begin(STEADY_AFTER, 1_000_000 + delta);
            t.decision(STEADY_AFTER, i == 0, false, 0);
        }
        let a = t.snapshot.counts[0];
        assert_eq!(
            (a[CHECKS], a[ATTEMPTS], a[DEFERRALS], a[RETRIES]),
            (6, 6, 3, 3)
        );
        assert_eq!(
            (a[READY], a[FALLBACKS], a[OVER_BUDGET], a[MAX_NS]),
            (1, 2, 1, 8_000_041)
        );
        assert_eq!(t.snapshot.counts[1][RETRIES], 3);
        t.decision(0, false, true, 2);
        t.begin(0, 1);
        assert_eq!(t.snapshot.counts[0][INVALID_TIME], 1);
        t.snapshot.header[10] = 0;
        t.decision(0, false, true, 1);
        t.begin(0, 2);
        assert_eq!(t.snapshot.counts[0][INVALID_TIME], 2);
        t.snapshot.counts[0][CHECKS] = u64::MAX;
        t.begin(0, 3);
        assert_eq!(t.snapshot.counts[0][CHECKS], u64::MAX);
    }
    #[test]
    fn bounded_snapshot_rejects_torn_reads_and_resets_epochs() {
        let p = std::sync::Arc::new(Published::new());
        let r = p.clone();
        let reader = std::thread::spawn(move || {
            for _ in 0..30000 {
                let s = r.snapshot();
                if s.header[1] == 1 && s.header[2] > 0 {
                    assert!(s.counts.iter().flatten().all(|v| *v == s.header[2]));
                }
            }
        });
        for epoch in 1..10000 {
            let mut s = Snapshot::new(epoch, OutputPolicy::WhenReady, 125, 3);
            s.counts = [[epoch; COUNTS]; 2];
            p.publish(&s);
        }
        reader.join().unwrap();
        p.publish(&Snapshot::new(10000, OutputPolicy::WhenReady, 125, 3));
        let s = p.snapshot();
        assert_eq!(s.header[2], 10000);
        assert_eq!(s.counts, [[0; COUNTS]; 2]);
        p.sequence.fetch_add(1, SeqCst);
        assert_eq!(p.snapshot().header[1], 0);
        assert_eq!(std::mem::size_of::<Snapshot>(), 256);
    }
}
