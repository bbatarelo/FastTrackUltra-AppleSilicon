//! Every native pump, including no-callback returns. One quiescent-reset owner,
//! fixed storage, bounded readers, and no allocation or clock/policy calls.
use crate::{native::PumpObservation, timing::STEADY_AFTER, worker_delivery::LIMITS_NS};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
pub const HEADER: usize = 13;
pub const PHASES: usize = 3;
pub const COUNTS: usize = 12;
pub const METRICS: usize = 6;
pub const HIST: usize = 21;
pub const RECORD: usize = 13;
pub const PAIR: usize = 2 * RECORD;
pub const RECENT: usize = 32;
pub const LINK: usize = 4 + PAIR;
const C: usize = HEADER;
const M: usize = C + PHASES * COUNTS;
const W: usize = M + PHASES * METRICS * HIST;
const R: usize = W + PHASES * METRICS * PAIR;
const L: usize = R + RECENT * RECORD;
const WORDS: usize = L + 4 * LINK;
#[repr(C)]
pub struct Snapshot {
    pub header: [u64; HEADER],
    pub counts: [[u64; COUNTS]; PHASES],
    pub metrics: [[[u64; HIST]; METRICS]; PHASES],
    pub worst: [[[u64; PAIR]; METRICS]; PHASES],
    pub recent: [[u64; RECORD]; RECENT],
    pub links: [[u64; LINK]; 4],
}
pub struct Pumps {
    busy: AtomicBool,
    generation: AtomicU64,
    words: [AtomicU64; WORDS],
}
impl Pumps {
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn get(&self, i: usize) -> u64 {
        self.words[i].load(SeqCst)
    }
    fn set(&self, i: usize, v: u64) {
        self.words[i].store(v, SeqCst);
    }
    fn inc(&self, i: usize) {
        if self.get(i) == u64::MAX {
            self.set(10, 1);
        } else {
            self.set(i, self.get(i) + 1);
        }
    }
    fn load<const N: usize>(&self, at: usize) -> [u64; N] {
        std::array::from_fn(|i| self.get(at + i))
    }
    fn store(&self, at: usize, v: &[u64]) {
        for (i, &n) in v.iter().enumerate() {
            self.set(at + i, n);
        }
    }
    fn drop_event(&self) {
        let _ = self.words[9].fetch_update(SeqCst, SeqCst, |v| Some(v.saturating_add(1)));
    }
    fn begin(&self) -> Option<u64> {
        if self
            .busy
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_err()
        {
            self.drop_event();
            return None;
        }
        let g = self.generation.load(SeqCst);
        if g > u64::MAX - 2 {
            self.drop_event();
            self.busy.store(false, SeqCst);
            return None;
        }
        self.generation.store(g + 1, SeqCst);
        Some(g + 2)
    }
    fn end(&self, g: u64) {
        self.generation.store(g, SeqCst);
        self.busy.store(false, SeqCst);
    }
    pub fn reset(&self, epoch: u64) {
        let Some(g) = self.begin() else { return };
        for a in &self.words {
            a.store(0, SeqCst);
        }
        self.set(2, epoch);
        self.set(5, STEADY_AFTER);
        self.end(g);
    }
    pub fn configure(&self, numer: u32, denom: u32) {
        let Some(g) = self.begin() else { return };
        self.set(3, numer.into());
        self.set(4, denom.into());
        self.end(g);
    }
    fn ns(&self, a: u64, b: u64) -> Option<u64> {
        if b < a || self.get(3) == 0 || self.get(4) == 0 {
            return None;
        }
        let n = u128::from(b - a) * u128::from(self.get(3)) / u128::from(self.get(4));
        match u64::try_from(n) {
            Ok(v) => Some(v),
            Err(_) => {
                self.set(10, 1);
                None
            }
        }
    }
    fn histogram(&self, phase: usize, metric: usize, ns: u64, pair: &[u64; PAIR]) {
        let at = M + (phase * METRICS + metric) * HIST;
        let n = self.get(at);
        if n == 0 || ns < self.get(at + 1) {
            self.set(at + 1, ns);
        }
        if n == 0 || ns > self.get(at + 2) {
            self.set(at + 2, ns);
            self.store(W + (phase * METRICS + metric) * PAIR, pair);
        }
        self.inc(at);
        let b = LIMITS_NS.iter().position(|&limit| ns < limit).unwrap_or(17);
        self.inc(at + 3 + b);
    }
    pub fn observe(&self, p: PumpObservation, captured: u64, draining: bool) {
        let Some(g) = self.begin() else { return };
        let phase = if draining {
            2
        } else {
            usize::from(captured >= STEADY_AFTER)
        };
        let c = C + phase * COUNTS;
        let ordinal = self.get(6);
        let prev = if ordinal > 0 {
            self.load::<RECORD>(R + ((ordinal - 1) % RECENT as u64) as usize * RECORD)
        } else {
            [0; RECORD]
        };
        self.inc(6);
        self.inc(c);
        let record = [
            self.get(6),
            phase as u64,
            captured,
            p.pump.id,
            p.pump.enter_ticks,
            p.pump.exit_ticks,
            p.pump.result,
            p.pump.sample_begin_ticks,
            p.pump.sample_end_ticks,
            p.callbacks,
            p.first_callback_ticks,
            p.last_callback_ticks,
            p.invalid_callbacks,
        ];
        self.store(R + (ordinal % RECENT as u64) as usize * RECORD, &record);
        let mut pair = [0; PAIR];
        pair[..RECORD].copy_from_slice(&record);
        pair[RECORD..].copy_from_slice(&prev);
        if p.pump.id == 0 {
            self.inc(7);
            self.inc(c + 2);
            self.end(g);
            return;
        }
        if (ordinal == 0 && p.pump.id != 1)
            || (ordinal > 0 && (prev[3].checked_add(1) != Some(p.pump.id) || prev[0] != ordinal))
        {
            self.inc(11);
        }
        let duration = self.ns(p.pump.enter_ticks, p.pump.exit_ticks);
        let callback_times = if p.callbacks == 0 {
            p.first_callback_ticks == 0 && p.last_callback_ticks == 0
        } else {
            p.first_callback_ticks >= p.pump.enter_ticks
                && p.last_callback_ticks >= p.first_callback_ticks
                && p.last_callback_ticks <= p.pump.exit_ticks
                && (p.callbacks != 1 || p.first_callback_ticks == p.last_callback_ticks)
        };
        if p.pump.enter_ticks == 0
            || !(1..=4).contains(&p.pump.result)
            || duration.is_none()
            || !callback_times
            || p.invalid_callbacks > 0
            || p.callbacks == u64::MAX
            || p.pump.sample_begin_ticks > p.pump.sample_end_ticks
            || p.pump.sample_end_ticks > p.pump.enter_ticks
        {
            self.inc(8);
            self.inc(c + 3);
            self.end(g);
            return;
        }
        self.inc(c + 1);
        self.inc(c + 3 + p.pump.result as usize);
        match self.get(c + 8).checked_add(p.callbacks) {
            Some(n) => self.set(c + 8, n),
            None => self.set(10, 1),
        }
        if p.callbacks == 0 {
            self.inc(c + 9);
        }
        if p.callbacks > 1 {
            self.inc(c + 10);
        }
        self.histogram(phase, 0, duration.unwrap(), &pair);
        if p.pump.result == 3 && p.callbacks == 0 {
            self.histogram(phase, 1, duration.unwrap(), &pair);
            self.histogram(phase, 2, duration.unwrap().saturating_sub(2_000_000), &pair);
        }
        if p.callbacks > 0 {
            if let Some(n) = self.ns(p.pump.enter_ticks, p.first_callback_ticks) {
                self.histogram(phase, 3, n, &pair);
                if n >= 2_100_000 {
                    self.inc(c + 11);
                }
            }
            if let Some(n) = self.ns(p.last_callback_ticks, p.pump.exit_ticks) {
                self.histogram(phase, 4, n, &pair);
            }
        }
        // Cross-phase gaps are deliberately excluded; no synthetic zero first gap.
        if prev[0] == ordinal
            && prev[0] > 0
            && prev[1] == phase as u64
            && prev[3] > 0
            && prev[3].checked_add(1) == Some(p.pump.id)
        {
            if let Some(n) = self.ns(prev[5], p.pump.enter_ticks) {
                self.histogram(phase, 5, n, &pair);
            } else {
                self.inc(8);
            }
        }
        self.end(g);
    }
    /// Pin current+previous pump from the recent ring to the *actual* worst
    /// steady delivery / minimum lead event. Missing context is explicit.
    pub fn link(&self, kind: usize, sequence: u64, ticket: u64, value: i64, id: u64) {
        if kind >= 4 {
            return;
        }
        let Some(g) = self.begin() else { return };
        let at = L + kind * LINK;
        let replace = self.get(at) == 0
            || if kind < 2 {
                value > self.get(at + 3) as i64
            } else {
                value < self.get(at + 3) as i64
            };
        if replace {
            self.store(at, &[1, sequence, ticket, value as u64]);
            let mut pair = [0; PAIR];
            if id > 0 {
                for i in 0..RECENT {
                    let r = self.load::<RECORD>(R + i * RECORD);
                    if r[0] > 0 && r[3] == id {
                        pair[..RECORD].copy_from_slice(&r);
                        if r[0] > 1 {
                            let prev = self
                                .load::<RECORD>(R + ((r[0] - 2) % RECENT as u64) as usize * RECORD);
                            if prev[0] == r[0] - 1 {
                                pair[RECORD..].copy_from_slice(&prev);
                            }
                        }
                        break;
                    }
                }
            }
            if pair[0] == 0 || (pair[0] > 1 && pair[RECORD] == 0) {
                self.inc(12);
            }
            self.store(at + 4, &pair);
        }
        self.end(g);
    }
    #[allow(clippy::too_many_arguments)] // Mirrors the already-owned completion context.
    pub fn delivery(
        &self,
        direction: usize,
        sequence: u64,
        ticket: u64,
        p: &crate::pool::PoolCompletion,
        captured: u64,
        draining: bool,
    ) {
        if direction > 1 || draining || captured < STEADY_AFTER {
            return;
        }
        let last = p.last_packet_ticks();
        if last == 0 || p.result.matched != 1 {
            return;
        }
        // A failed conversion is counted by observe; do not mutate from here
        // outside the recorder's writer window.
        if self.get(3) == 0 || self.get(4) == 0 || p.result.callback_ticks < last {
            return;
        }
        // Select by exact tick delta, matching ftuD even across ns rounding ties.
        if let Ok(value) = i64::try_from(p.result.callback_ticks - last) {
            self.link(direction, sequence, ticket, value, p.pump.id);
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut out = Snapshot {
            header: [0; HEADER],
            counts: [[0; COUNTS]; PHASES],
            metrics: [[[0; HIST]; METRICS]; PHASES],
            worst: [[[0; PAIR]; METRICS]; PHASES],
            recent: [[0; RECORD]; RECENT],
            links: [[0; LINK]; 4],
        };
        for _ in 0..2 {
            let g = self.generation.load(SeqCst);
            if g & 1 != 0 {
                continue;
            }
            out.header = self.load(0);
            for p in 0..PHASES {
                out.counts[p] = self.load(C + p * COUNTS);
                for m in 0..METRICS {
                    out.metrics[p][m] = self.load(M + (p * METRICS + m) * HIST);
                    out.worst[p][m] = self.load(W + (p * METRICS + m) * PAIR);
                }
            }
            for i in 0..RECENT {
                out.recent[i] = self.load(R + i * RECORD);
            }
            for i in 0..4 {
                out.links[i] = self.load(L + i * LINK);
            }
            if g == self.generation.load(SeqCst) {
                out.header[1] = 1;
                break;
            }
        }
        out.header[0] = 1;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::Pump;
    fn recorder() -> Pumps {
        let p = Pumps::new();
        p.reset(17);
        p.configure(1, 1);
        p
    }
    fn empty(id: u64, start: u64, duration: u64) -> PumpObservation {
        PumpObservation {
            pump: Pump {
                id,
                enter_ticks: start,
                exit_ticks: start + duration,
                result: 3,
                ..Pump::default()
            },
            ..PumpObservation::default()
        }
    }
    fn handled(id: u64, start: u64, delay: u64, tail: u64) -> PumpObservation {
        PumpObservation {
            pump: Pump {
                id,
                enter_ticks: start,
                exit_ticks: start + delay + tail,
                result: 4,
                ..Pump::default()
            },
            callbacks: 1,
            first_callback_ticks: start + delay,
            last_callback_ticks: start + delay,
            invalid_callbacks: 0,
        }
    }
    #[test]
    fn no_callback_timeout_and_late_callback_keep_distinct_causes() {
        let p = recorder();
        p.observe(empty(1, 1_000, 9_000_000), 48_000, false);
        p.observe(handled(2, 9_002_000, 3_000_000, 100_000), 48_384, false);
        let s = p.snapshot();
        assert_eq!(s.header[6], 2);
        assert_eq!(s.counts[1], [2, 2, 0, 0, 0, 0, 1, 1, 1, 1, 0, 1]);
        assert_eq!(&s.metrics[1][1][..3], &[1, 9_000_000, 9_000_000]);
        assert_eq!(&s.metrics[1][2][..3], &[1, 7_000_000, 7_000_000]);
        assert_eq!(s.metrics[1][3][2], 3_000_000);
        assert_eq!(s.metrics[1][4][2], 100_000);
        assert_eq!(s.metrics[1][5][2], 1_000);
        assert_eq!(s.worst[1][3][3], 2);
        assert_eq!(s.worst[1][3][RECORD + 3], 1);
        for h in s.metrics[1] {
            assert_eq!(h[0], h[3..].iter().sum());
        }
    }
    #[test]
    fn phases_and_multiple_callbacks_do_not_blend() {
        let p = recorder();
        p.observe(empty(1, 1, 2_000_000), 0, false);
        let mut multi = handled(2, 2_001_000, 500_000, 1_000_000);
        multi.callbacks = 2;
        multi.last_callback_ticks += 750_000;
        p.observe(multi, 48_000, false);
        p.observe(empty(3, 4_000_000, 2_000_000), 48_384, true);
        let s = p.snapshot();
        assert_eq!(s.counts.map(|c| c[0]), [1, 1, 1]);
        assert_eq!(s.counts[1][8], 2);
        assert_eq!(s.counts[1][10], 1);
        assert_eq!(s.metrics[1][4][2], 250_000);
        assert_eq!(s.metrics.iter().map(|m| m[5][0]).sum::<u64>(), 0);
    }
    #[test]
    fn absent_exhausted_invalid_and_nonconsecutive_ids_are_visible() {
        let p = recorder();
        p.observe(PumpObservation::default(), 0, false);
        let mut bad = handled(2, 10, 4, 1);
        bad.invalid_callbacks = 1;
        p.observe(bad, 0, false);
        p.observe(empty(4, 100, 10), 0, false);
        let s = p.snapshot();
        assert_eq!(s.header[7], 1);
        assert_eq!(s.header[8], 1);
        assert_eq!(s.header[11], 2);
        assert_eq!(&s.counts[0][..4], &[3, 1, 1, 1]);
        assert_eq!(s.metrics[0][0][0], 1);
    }
    #[test]
    fn callbacks_outside_pump_and_invalid_conversion_are_rejected() {
        let p = recorder();
        let mut bad = handled(1, 100, 10, 10);
        bad.last_callback_ticks = 121;
        p.observe(bad, 0, false);
        p.configure(0, 1);
        p.observe(empty(2, 200, 10), 0, false);
        p.configure(u32::MAX, 1);
        p.observe(empty(3, 1, u64::MAX - 1), 0, false);
        let s = p.snapshot();
        assert_eq!(s.header[8], 3);
        assert_eq!(s.header[10], 1);
    }
    #[test]
    fn delayed_consumption_pins_actual_callback_pump_not_latest_and_expires_explicitly() {
        let p = recorder();
        for id in 1..=6 {
            p.observe(empty(id, id * 10_000_000, 3_000_000), 48_000, false);
        }
        p.link(0, 12, 19, 8_000_000, 2);
        p.link(2, 14, 23, 16, 5);
        let s = p.snapshot();
        assert_eq!(&s.links[0][..4], &[1, 12, 19, 8_000_000]);
        assert_eq!(s.links[0][4 + 3], 2);
        assert_eq!(s.links[0][4 + RECORD + 3], 1);
        assert_eq!(s.links[2][4 + 3], 5);
        for id in 7..=40 {
            p.observe(empty(id, id * 10_000_000, 3_000_000), 48_000, false);
        }
        // Existing pinned context survives rolling overwrite.
        assert_eq!(p.snapshot().links[0], s.links[0]);
        p.link(0, 99, 99, 9_000_000, 2);
        assert_eq!(p.snapshot().header[12], 1);
        assert_eq!(p.snapshot().links[0][4], 0);
        p.reset(18);
        p.configure(1, 1);
        let s = p.snapshot();
        assert_eq!(s.header[2], 18);
        assert_eq!(s.header[6], 0);
        assert_eq!(s.links, [[0; LINK]; 4]);
    }
    #[test]
    fn generation_exhaustion_contention_and_counter_overflow_fail_as_telemetry() {
        let p = recorder();
        p.busy.store(true, SeqCst);
        p.observe(empty(1, 1, 1), 0, false);
        p.busy.store(false, SeqCst);
        assert_eq!(p.snapshot().header[9], 1);
        p.generation.store(u64::MAX - 1, SeqCst);
        p.observe(empty(1, 1, 1), 0, false);
        assert_eq!(p.snapshot().header[9], 2);
        p.generation.store(1, SeqCst);
        assert_eq!(p.snapshot().header[1], 0);
        p.generation.store(0, SeqCst);
        p.set(6, u64::MAX);
        p.observe(empty(1, 1, 1), 0, false);
        assert_eq!(p.snapshot().header[10], 1);
        assert_eq!(std::mem::size_of::<Snapshot>(), 11448);
        assert_eq!(std::mem::size_of::<PumpObservation>(), 80);
        assert_eq!(std::mem::size_of::<crate::pool::PoolOps>(), 152);
    }
    #[test]
    fn concurrent_readers_never_accept_partial_metrics() {
        let p = std::sync::Arc::new(recorder());
        let writer = p.clone();
        let t = std::thread::spawn(move || {
            for id in 1..=10_000 {
                writer.observe(empty(id, id * 3_000_000, 2_000_000), 48_000, false);
            }
        });
        for _ in 0..1000 {
            let s = p.snapshot();
            if s.header[1] == 0 {
                continue;
            }
            assert_eq!(s.header[6], s.counts[1][0]);
            assert_eq!(s.metrics[1][0][0], s.counts[1][1]);
            assert_eq!(s.metrics[1][0][0], s.metrics[1][0][3..].iter().sum());
        }
        t.join().unwrap();
        assert_eq!(p.snapshot().header[6], 10_000);
    }
    #[test]
    fn observe_links_and_bounded_read_do_not_allocate() {
        let p = recorder();
        crate::pool_simulation::assert_no_alloc(|| {
            for id in 1..=100 {
                p.observe(handled(id, id * 3_000_000, 1_000_000, 1000), 48_000, false);
                p.link(0, id, id, id as i64, id);
                std::hint::black_box(p.snapshot());
            }
        });
    }
    #[test]
    #[ignore = "measurement, not a real-time bound"]
    fn recorder_microbenchmark() {
        let p = recorder();
        let mut means = [0.; 100];
        let mut id = 0;
        for mean in &mut means {
            let now = std::time::Instant::now();
            for _ in 0..2000 {
                id += 1;
                p.observe(handled(id, id * 3_000_000, 1_000_000, 1000), 48_000, false);
                // Normal completion and admission calls, mostly not new extrema.
                p.link(0, id, id, 1_000_000, id);
                p.link(2, id, id, 20, id);
            }
            *mean = now.elapsed().as_nanos() as f64 / 2000.;
        }
        means.sort_by(f64::total_cmp);
        println!("all-pump + delivery/admission link batch mean ns: median={} p99={} max={}; excludes native bookkeeping and property reads",means[50],means[98],means[99]);
    }
}
