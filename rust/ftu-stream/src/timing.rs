//! Bounded observation only: no timing policy, samples, allocation or OS calls.
//! Each metric uses one fail-fast writer gate and a two-attempt snapshot. Lost
//! observations are explicit. Metrics are individually coherent, not one global
//! instant; reset has its own generation so readers cannot mix stream epochs.
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::SeqCst};

pub const LIMITS: [i64; 16] = [
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384,
];
pub const BUCKETS: usize = LIMITS.len() + 1;
pub const METRICS: usize = 12;
pub const STEADY_AFTER: u64 = 48_000;
#[repr(usize)]
#[derive(Clone, Copy)]
pub enum Metric {
    WriteFrames,
    WriteCaptureLead,
    WritePreparationLead,
    OutputCaptureLead,
    OutputHighWaterMargin,
    OutputMissing,
    ReadFrames,
    ReadCaptureMargin,
    ReadMissing,
    SubmitCheckLeadMs,
    SubmitReturnLeadMs,
    SubmitClockError,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct MetricSnapshot {
    pub valid: u64,
    pub count: u64,
    pub min: i64,
    pub max: i64,
    pub last: i64,
    pub dropped: u64,
    pub buckets: [u64; BUCKETS],
}
#[repr(C)]
#[derive(Default)]
pub struct Snapshot {
    pub version: u32,
    pub metric_count: u32,
    pub epoch: u64,
    pub steady_after: u64,
    pub valid: u64,
    pub limits: [i64; 16],
    pub metrics: [[MetricSnapshot; METRICS]; 2],
}
struct Counter {
    busy: AtomicBool,
    sequence: AtomicU64,
    count: AtomicU64,
    min: AtomicI64,
    max: AtomicI64,
    last: AtomicI64,
    dropped: AtomicU64,
    buckets: [AtomicU64; BUCKETS],
}
impl Counter {
    fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            sequence: AtomicU64::new(0),
            count: AtomicU64::new(0),
            min: AtomicI64::new(0),
            max: AtomicI64::new(0),
            last: AtomicI64::new(0),
            dropped: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn record(&self, value: i64) {
        if self
            .busy
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_err()
        {
            self.dropped.fetch_add(1, SeqCst);
            return;
        }
        let sequence = self.sequence.load(SeqCst);
        // Exhaustion cannot wrap a version into a previous reader's identity.
        let Some(next) = sequence.checked_add(2) else {
            self.dropped.fetch_add(1, SeqCst);
            self.busy.store(false, SeqCst);
            return;
        };
        self.sequence.store(sequence + 1, SeqCst);
        let count = self.count.load(SeqCst);
        self.min.store(
            if count == 0 {
                value
            } else {
                self.min.load(SeqCst).min(value)
            },
            SeqCst,
        );
        self.max.store(
            if count == 0 {
                value
            } else {
                self.max.load(SeqCst).max(value)
            },
            SeqCst,
        );
        self.last.store(value, SeqCst);
        let bucket = LIMITS
            .iter()
            .position(|&limit| value < limit)
            .unwrap_or(LIMITS.len());
        self.buckets[bucket].fetch_add(1, SeqCst);
        self.count.store(count + 1, SeqCst);
        self.sequence.store(next, SeqCst);
        self.busy.store(false, SeqCst);
    }
    // The session owner calls reset only after all realtime calls/worker exit.
    // Preserve the monotonically increasing version for concurrent inspectors.
    fn reset(&self) {
        let sequence = self.sequence.load(SeqCst);
        let Some(next) = sequence.checked_add(2) else {
            return;
        };
        self.sequence.store(sequence + 1, SeqCst);
        self.count.store(0, SeqCst);
        self.min.store(0, SeqCst);
        self.max.store(0, SeqCst);
        self.last.store(0, SeqCst);
        self.dropped.store(0, SeqCst);
        for b in &self.buckets {
            b.store(0, SeqCst);
        }
        self.sequence.store(next, SeqCst);
    }
    fn snapshot(&self) -> MetricSnapshot {
        for _ in 0..2 {
            let before = self.sequence.load(SeqCst);
            if before % 2 == 1 {
                continue;
            }
            let value = MetricSnapshot {
                valid: 1,
                count: self.count.load(SeqCst),
                min: self.min.load(SeqCst),
                max: self.max.load(SeqCst),
                last: self.last.load(SeqCst),
                dropped: self.dropped.load(SeqCst),
                buckets: self.buckets.each_ref().map(|b| b.load(SeqCst)),
            };
            if self.sequence.load(SeqCst) == before {
                return value;
            }
        }
        MetricSnapshot {
            dropped: self.dropped.load(SeqCst),
            ..MetricSnapshot::default()
        }
    }
}
pub struct Timing {
    generation: AtomicU64,
    epoch: AtomicU64,
    metrics: [[Counter; METRICS]; 2],
}
impl Timing {
    pub fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            metrics: std::array::from_fn(|_| std::array::from_fn(|_| Counter::new())),
        }
    }
    pub fn reset(&self, epoch: u64) {
        self.generation.fetch_add(1, SeqCst);
        for bank in &self.metrics {
            for c in bank {
                c.reset();
            }
        }
        self.epoch.store(epoch, SeqCst);
        self.generation.fetch_add(1, SeqCst);
    }
    pub fn observe(&self, metric: Metric, value: i64, captured: u64) {
        self.metrics[0][metric as usize].record(value);
        if captured >= STEADY_AFTER {
            self.metrics[1][metric as usize].record(value);
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let before = self.generation.load(SeqCst);
        let mut s = Snapshot {
            version: 1,
            metric_count: METRICS as u32,
            epoch: self.epoch.load(SeqCst),
            steady_after: STEADY_AFTER,
            valid: 0,
            limits: LIMITS,
            metrics: self
                .metrics
                .each_ref()
                .map(|bank| bank.each_ref().map(Counter::snapshot)),
        };
        s.valid = u64::from(before.is_multiple_of(2) && self.generation.load(SeqCst) == before);
        s
    }
}
/// Signed differences never wrap into a misleading positive margin.
pub fn distance(a: u64, b: u64) -> i64 {
    (i128::from(a) - i128::from(b)).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn histogram_boundaries_negative_margins_and_empty_values() {
        let c = Counter::new();
        assert_eq!(c.snapshot().count, 0);
        let mut values = vec![-2, 0, 16384, i64::MIN, i64::MAX];
        for v in LIMITS {
            values.extend([v - 1, v, v + 1]);
        }
        for &v in &values {
            c.record(v);
        }
        let s = c.snapshot();
        assert_eq!(s.valid, 1);
        assert_eq!(s.count, values.len() as u64);
        assert_eq!(s.min, i64::MIN);
        assert_eq!(s.max, i64::MAX);
        for b in 0..BUCKETS {
            let expected = values
                .iter()
                .filter(|&&v| LIMITS.iter().position(|&limit| v < limit).unwrap_or(16) == b)
                .count();
            assert_eq!(s.buckets[b], expected as u64);
        }
        assert_eq!(distance(0, u64::MAX), i64::MIN);
        assert_eq!(distance(u64::MAX, 0), i64::MAX);
        assert_eq!(distance(10, 20), -10);
    }
    #[test]
    fn drops_are_explicit_and_a_busy_reader_never_spins() {
        let c = Counter::new();
        c.busy.store(true, SeqCst);
        c.record(42);
        assert_eq!(c.snapshot().dropped, 1);
        assert_eq!(c.snapshot().count, 0);
        c.busy.store(false, SeqCst);
        c.sequence.store(1, SeqCst);
        assert_eq!(c.snapshot().valid, 0);
        c.sequence.store(u64::MAX - 1, SeqCst);
        c.record(2);
        assert_eq!(c.snapshot().dropped, 2);
        assert_eq!(c.snapshot().count, 0);
    }
    #[test]
    fn startup_is_separate_and_reset_cannot_mix_epochs() {
        let t = Timing::new();
        t.reset(1);
        t.observe(Metric::OutputMissing, 100, STEADY_AFTER - 1);
        t.observe(Metric::OutputMissing, 0, STEADY_AFTER);
        let s = t.snapshot();
        assert_eq!(s.epoch, 1);
        assert_eq!(s.valid, 1);
        assert_eq!(s.metrics[0][Metric::OutputMissing as usize].count, 2);
        assert_eq!(s.metrics[1][Metric::OutputMissing as usize].max, 0);
        t.reset(2);
        let s = t.snapshot();
        assert_eq!(s.epoch, 2);
        assert!(s
            .metrics
            .iter()
            .flatten()
            .all(|m| m.count == 0 && m.dropped == 0));
        t.generation.fetch_add(1, SeqCst);
        assert_eq!(t.snapshot().valid, 0);
    }
    #[test]
    fn concurrent_inspection_only_returns_coherent_metrics() {
        let c = std::sync::Arc::new(Counter::new());
        let writer = c.clone();
        let thread = std::thread::spawn(move || {
            for _ in 0..10000 {
                writer.record(17);
            }
        });
        while !thread.is_finished() {
            let s = c.snapshot();
            if s.valid == 1 {
                assert_eq!(s.buckets.iter().sum::<u64>(), s.count);
                if s.count > 0 {
                    assert_eq!((s.min, s.max, s.last), (17, 17, 17));
                }
            }
        }
        thread.join().unwrap();
        let s = c.snapshot();
        assert_eq!(s.count, 10000);
        assert_eq!(s.dropped, 0);
    }
    #[test]
    fn stable_c_layout() {
        assert_eq!(std::mem::size_of::<MetricSnapshot>(), 184);
        assert_eq!(std::mem::size_of::<Snapshot>(), 4576);
    }
}
