//! Sample-free completion delivery statistics. One quiescent lifecycle owner and
//! one USB writer; readers retry twice and never block the writer.
//! Word layouts are mirrored in shared/ftu_live.h and documented in DELIVERY-OBSERVATION.md.
use crate::{
    native::{Pump, WorkerSample},
    pool::{Flight, PoolCompletion},
    timing::STEADY_AFTER,
    worker_timing::{self as wt, Event},
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
pub const HEADER: usize = 28;
pub const METRICS: usize = 8;
pub const BUCKETS: usize = 18;
pub const HIST: usize = 29;
pub const RECORD: usize = 18;
pub const RECENT: usize = 8;
pub const LINK: usize = wt::WORDS + 12;
pub const SAMPLE: usize = 12;
pub const LIMITS_NS: [u64; BUCKETS - 1] = [
    1000, 2000, 4000, 8000, 16000, 32000, 64000, 128000, 256000, 512000, 1000000, 2000000, 4000000,
    8000000, 16000000, 32000000, 64000000,
];
pub const EXCEED_NS: [u64; 4] = [1000000, 4000000, 8000000, 16000000];
const M: usize = HEADER;
const W: usize = M + 2 * METRICS * HIST;
const R: usize = W + 2 * RECORD;
const L: usize = R + RECENT * RECORD;
const S: usize = L + 2 * LINK;
const WORDS: usize = S + 2 * SAMPLE;
const DROPPED: usize = 14;
const OVERFLOW: usize = 23;
#[repr(C)]
pub struct Snapshot {
    pub header: [u64; HEADER],
    pub metrics: [[[u64; HIST]; METRICS]; 2],
    pub worst: [[u64; RECORD]; 2],
    pub recent: [[u64; RECORD]; RECENT],
    pub lead: [[u64; LINK]; 2],
    pub samples: [[u64; SAMPLE]; 2],
}
pub struct Delivery {
    busy: AtomicBool,
    generation: AtomicU64,
    words: [AtomicU64; WORDS],
}
impl Delivery {
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
        let old = self.get(i);
        if old == u64::MAX {
            self.set(OVERFLOW, 1);
        } else {
            self.set(i, old + 1);
        }
    }
    fn store(&self, at: usize, v: &[u64]) {
        for (i, &n) in v.iter().enumerate() {
            self.set(at + i, n);
        }
    }
    fn load<const N: usize>(&self, at: usize) -> [u64; N] {
        std::array::from_fn(|i| self.get(at + i))
    }
    fn drop_event(&self) {
        let _ = self.words[DROPPED].fetch_update(SeqCst, SeqCst, |n| Some(n.saturating_add(1)));
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
    fn ns(&self, begin: u64, end: u64) -> Option<u64> {
        if end < begin || self.get(3) == 0 || self.get(4) == 0 {
            return None;
        }
        u64::try_from(u128::from(end - begin) * u128::from(self.get(3)) / u128::from(self.get(4)))
            .ok()
    }
    fn histogram(
        &self,
        d: usize,
        m: usize,
        begin: Option<u64>,
        end: Option<u64>,
        sample: [u64; 2],
    ) {
        let at = M + (d * METRICS + m) * HIST;
        let (Some(begin), Some(end)) = (begin, end) else {
            self.inc(at + 3);
            return;
        };
        if end < begin || self.get(3) == 0 || self.get(4) == 0 {
            self.inc(at + 4);
            return;
        }
        let Some(ns) = self.ns(begin, end) else {
            self.inc(at + 5);
            self.set(OVERFLOW, 1);
            return;
        };
        let n = self.get(at);
        if n == u64::MAX {
            self.inc(at + 5);
            self.set(OVERFLOW, 1);
            return;
        }
        self.set(at, n + 1);
        if sample[0] != 0 && sample[0] <= sample[1] && begin <= sample[1] && end >= sample[0] {
            self.inc(at + 28);
        }
        if n == 0 || ns < self.get(at + 1) {
            self.set(at + 1, ns);
        }
        if n == 0 || ns > self.get(at + 2) {
            self.set(at + 2, ns);
        }
        let b = LIMITS_NS
            .iter()
            .position(|&limit| ns < limit)
            .unwrap_or(BUCKETS - 1);
        self.inc(at + 6 + b);
        for (i, &limit) in EXCEED_NS.iter().enumerate() {
            if ns >= limit {
                self.inc(at + 6 + BUCKETS + i);
            }
        }
    }
    // Identity, phase and cursor are supplied together by the transport owner.
    #[allow(clippy::too_many_arguments)]
    pub fn completion(
        &self,
        d: usize,
        slot: usize,
        flight: Flight,
        c: &PoolCompletion,
        consumed: u64,
        captured: u64,
        draining: bool,
    ) {
        if d > 1 {
            self.drop_event();
            return;
        }
        let Some(g) = self.begin() else { return };
        self.inc(6 + d);
        let ordinal = self.get(6).saturating_add(self.get(7));
        let p = c.pump;
        let stamp = c.result.packets[crate::scheduler::ENTRIES - 1].ticks;
        let callback = c.result.callback_ticks;
        let record = [
            ordinal,
            d as u64,
            flight.seq as u64,
            slot as u64,
            flight.ticket,
            captured,
            stamp,
            callback,
            consumed,
            p.id,
            p.enter_ticks,
            p.exit_ticks,
            p.result,
            c.result.status as i64 as u64,
            u64::from(c.result.matched),
            u64::from(draining),
            p.sample_begin_ticks,
            p.sample_end_ticks,
        ];
        self.store(
            R + (ordinal.saturating_sub(1) as usize % RECENT) * RECORD,
            &record,
        );
        if draining {
            self.inc(24 + d);
        }
        if !draining && captured >= STEADY_AFTER {
            self.inc(8 + d);
            let cb = (callback != 0).then_some(callback);
            let sample = [p.sample_begin_ticks, p.sample_end_ticks];
            self.histogram(d, 0, (stamp != 0).then_some(stamp), cb, sample);
            self.histogram(d, 1, cb, Some(consumed), sample);
            let linked = p.id != 0
                && p.enter_ticks <= callback
                && callback <= p.exit_ticks
                && p.exit_ticks <= consumed
                && (1..=4).contains(&p.result);
            if !linked {
                self.inc(15 + d);
            }
            self.histogram(
                d,
                2,
                linked.then_some(p.enter_ticks),
                linked.then_some(p.exit_ticks),
                sample,
            );
            if stamp != 0 && callback >= stamp {
                let old = self.load::<RECORD>(W + d * RECORD);
                if old[0] == 0 || callback - stamp > old[7].saturating_sub(old[6]) {
                    self.store(W + d * RECORD, &record);
                }
            }
        }
        self.end(g);
    }
    pub fn admission(&self, mut e: Event, p: Pump, feedback: Pump) {
        if e.direction > 1 {
            self.drop_event();
            return;
        }
        let Some(g) = self.begin() else { return };
        let d = e.direction as usize;
        self.inc(10 + d);
        e.ordinal = self.get(10).saturating_add(self.get(11));
        if e.captured >= STEADY_AFTER {
            self.inc(12 + d);
            let sample = [self.get(S + SAMPLE), self.get(S + SAMPLE + 1)];
            let has = |flag| e.flags & flag != 0;
            self.histogram(
                d,
                3,
                has(wt::CHECKED).then_some(e.check_end_ticks),
                has(wt::PREPARED).then_some(e.prepare_end_ticks),
                sample,
            );
            self.histogram(
                d,
                4,
                has(wt::SUBMITTED).then_some(e.submit_begin_ticks),
                has(wt::SUBMITTED).then_some(e.submit_end_ticks),
                sample,
            );
            self.histogram(
                d,
                5,
                has(wt::SUBMITTED).then_some(e.submit_end_ticks),
                has(wt::QUERIED).then_some(e.query_end_ticks),
                sample,
            );
            let end = if has(wt::QUERIED) {
                Some(e.query_end_ticks)
            } else if has(wt::SUBMITTED) {
                Some(e.submit_end_ticks)
            } else {
                None
            };
            self.histogram(
                d,
                6,
                has(wt::PREVIOUS).then_some(e.previous_callback_ticks),
                end,
                sample,
            );
            self.histogram(
                d,
                7,
                has(wt::PREVIOUS).then_some(e.previous_consumed_ticks),
                Some(e.check_begin_ticks),
                sample,
            );
            if let Some(lead) = e.lead() {
                let old = Event::from_words(self.load::<{ wt::WORDS }>(L + d * LINK));
                if old.ordinal == 0 || old.lead().is_none_or(|n| lead < n) {
                    self.store(L + d * LINK, &e.words());
                    self.store(
                        L + d * LINK + wt::WORDS,
                        &[
                            p.id,
                            p.enter_ticks,
                            p.exit_ticks,
                            p.result,
                            p.sample_begin_ticks,
                            p.sample_end_ticks,
                            feedback.id,
                            feedback.enter_ticks,
                            feedback.exit_ticks,
                            feedback.result,
                            feedback.sample_begin_ticks,
                            feedback.sample_end_ticks,
                        ],
                    );
                }
            }
        }
        self.end(g);
    }
    pub fn sample(&self, s: WorkerSample) {
        let Some(g) = self.begin() else { return };
        let w = s.words();
        if self.get(17) > 0 {
            let previous = self.load::<SAMPLE>(S + SAMPLE);
            if previous[4] == 0 && s.cpu_status == 0 && previous[11] != s.basic_policy {
                self.inc(26);
            }
            if previous[5] == 0 && s.policy_status == 0 && previous[6..11] != w[6..11] {
                self.inc(27);
            }
        }
        if self.get(17) == 0 {
            self.store(S, &w);
        }
        self.store(S + SAMPLE, &w);
        self.inc(17);
        if s.cpu_status != 0 {
            self.inc(18);
        }
        if s.policy_status != 0 {
            self.inc(19);
        } else if s.policy_default != 0 {
            self.inc(20);
        }
        if let Some(ns) = (s.begin_ticks != 0 && s.end_ticks != 0)
            .then(|| self.ns(s.begin_ticks, s.end_ticks))
            .flatten()
        {
            self.set(22, self.get(22).max(ns));
        } else {
            self.inc(21);
        }
        self.end(g);
    }
    pub fn snapshot(&self) -> Snapshot {
        for _ in 0..2 {
            let g = self.generation.load(SeqCst);
            if !g.is_multiple_of(2) {
                continue;
            }
            let mut header = self.load::<HEADER>(0);
            header[0] = 1;
            header[1] = 1;
            header[5] = STEADY_AFTER;
            let s = Snapshot {
                header,
                metrics: std::array::from_fn(|d| {
                    std::array::from_fn(|m| self.load(M + (d * METRICS + m) * HIST))
                }),
                worst: std::array::from_fn(|d| self.load(W + d * RECORD)),
                recent: std::array::from_fn(|i| self.load(R + i * RECORD)),
                lead: std::array::from_fn(|d| self.load(L + d * LINK)),
                samples: std::array::from_fn(|i| self.load(S + i * SAMPLE)),
            };
            if self.generation.load(SeqCst) == g {
                return s;
            }
        }
        Snapshot {
            header: {
                let mut h = [0; HEADER];
                h[0] = 1;
                h
            },
            metrics: [[[0; HIST]; METRICS]; 2],
            worst: [[0; RECORD]; 2],
            recent: [[0; RECORD]; RECENT],
            lead: [[0; LINK]; 2],
            samples: [[0; SAMPLE]; 2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn completed(delay: u64, pump_id: u64) -> PoolCompletion {
        let mut c = PoolCompletion {
            ticket: 1,
            ..PoolCompletion::default()
        };
        c.result.matched = 1;
        c.result.callback_ticks = 100 + delay;
        c.result.packets[63].ticks = 100;
        c.pump = Pump {
            id: pump_id,
            enter_ticks: 90,
            exit_ticks: 101 + delay,
            result: 4,
            ..Pump::default()
        };
        c
    }
    fn flight(seq: usize) -> Flight {
        Flight {
            seq,
            ticket: seq as u64 + 1,
            start: 100,
        }
    }
    fn recorder() -> Delivery {
        let t = Delivery::new();
        t.reset(7);
        t.configure(1000, 1);
        t
    }
    #[test]
    fn distributions_cover_every_steady_completion_and_keep_missing_distinct() {
        let t = recorder();
        t.completion(0, 2, flight(2), &completed(1000, 1), 1102, 0, false); // startup only
        for seq in 0..20 {
            t.completion(
                0,
                seq % 4,
                flight(seq),
                &completed(1000 + seq as u64, seq as u64 + 2),
                1200,
                48000,
                false,
            );
        }
        let s = t.snapshot();
        let h = s.metrics[0][0];
        assert_eq!((s.header[6], s.header[8], h[0]), (21, 20, 20));
        assert_eq!(h[1], 1000000);
        assert_eq!(h[6..24].iter().sum::<u64>(), 20);
        assert_eq!(h[24], 20);
        assert_eq!(s.worst[0][2], 19);
        assert!(s.recent.iter().all(|r| r[0] >= 14));
        let mut c = completed(1, 0);
        c.result.packets[63].ticks = 0;
        t.completion(1, 0, flight(0), &c, 1000, 48000, false);
        let s = t.snapshot();
        assert_eq!((s.metrics[1][0][0], s.metrics[1][0][3]), (0, 1));
        assert_eq!(s.header[16], 1);
        assert_eq!(s.metrics[1][2][3], 1);
        c.result.packets[63].ticks = 200;
        t.completion(1, 1, flight(1), &c, 1000, 48000, false);
        assert_eq!(t.snapshot().metrics[1][0][4], 1);
    }
    #[test]
    fn admission_links_the_predecessor_pump_not_the_latest_pump() {
        let t = recorder();
        let e = Event {
            direction: 1,
            sequence: 9,
            slot: 1,
            ticket: 10,
            captured: 48000,
            flags: wt::PREVIOUS
                | wt::CHECKED
                | wt::PREPARED
                | wt::SUBMITTED
                | wt::QUERIED
                | wt::RETURNED
                | wt::PUMP,
            start_frame: 100,
            checked_frame: 80,
            returned_frame: 81,
            previous_sequence: 5,
            previous_callback_ticks: 100,
            previous_consumed_ticks: 102,
            pump_enter_ticks: 180,
            pump_exit_ticks: 190,
            check_begin_ticks: 200,
            check_end_ticks: 202,
            prepare_end_ticks: 210,
            submit_begin_ticks: 212,
            submit_end_ticks: 220,
            query_end_ticks: 224,
            ..Event::default()
        };
        let p = Pump {
            id: 3,
            enter_ticks: 98,
            exit_ticks: 101,
            result: 4,
            ..Pump::default()
        };
        t.admission(e, p, Pump::default());
        let s = t.snapshot();
        assert_eq!(&s.lead[1][32..36], &[3, 98, 101, 4]);
        assert_eq!(s.lead[1][21], 180);
        assert_eq!(s.metrics[1][6][1], 124000);
        assert_eq!(s.metrics[1][3][1], 8000);
        assert_eq!(s.metrics[1][4][1], 8000);
        assert_eq!(s.metrics[1][5][1], 4000);
        t.reset(8);
        let s = t.snapshot();
        assert_eq!(s.header[2], 8);
        assert_eq!(s.header[6], 0);
        assert_eq!(s.metrics[1][6][0], 0);
        assert_eq!(s.lead[1][0], 0);
    }
    #[test]
    fn failed_readbacks_and_histogram_overflow_are_not_zero_measurements() {
        let t = recorder();
        let sample = WorkerSample {
            begin_ticks: 200,
            end_ticks: 210,
            cpu_status: (-3i64) as u64,
            policy_status: 5,
            ..WorkerSample::default()
        };
        t.sample(sample);
        let s = t.snapshot();
        assert_eq!(
            (s.header[17], s.header[18], s.header[19], s.header[22]),
            (1, 1, 1, 10000)
        );
        assert_eq!(s.samples[0], sample.words());
        t.histogram(0, 0, None, Some(5), [0, 0]);
        t.histogram(0, 0, Some(6), Some(5), [0, 0]);
        t.set(3, u64::MAX);
        t.histogram(0, 0, Some(1), Some(u64::MAX), [0, 0]);
        let h = t.snapshot().metrics[0][0];
        assert_eq!(&h[0..6], &[0, 0, 0, 1, 1, 1]);
        assert_eq!(t.snapshot().header[23], 1);
        t.busy.store(true, SeqCst);
        t.sample(sample);
        t.busy.store(false, SeqCst);
        assert_eq!(t.snapshot().header[DROPPED], 1);
        t.generation.store(3, SeqCst);
        assert_eq!(t.snapshot().header[1], 0);
        assert_eq!(std::mem::size_of::<Snapshot>(), 6272);
    }
    #[test]
    fn reader_never_mixes_epochs_records_or_histogram_totals() {
        let t = std::sync::Arc::new(recorder());
        let w = t.clone();
        let writer = std::thread::spawn(move || {
            for epoch in 1..60 {
                w.reset(epoch);
                w.configure(1000, 1);
                for seq in 0..100 {
                    w.completion(
                        0,
                        seq % 4,
                        flight(seq),
                        &completed(epoch, 1),
                        1000,
                        48000,
                        false,
                    );
                }
            }
        });
        while !writer.is_finished() {
            let s = t.snapshot();
            if s.header[1] == 0 {
                continue;
            }
            for r in s.recent.iter().chain(s.worst.iter()).filter(|r| r[0] != 0) {
                assert_eq!(r[7] - r[6], s.header[2]);
                assert!(r[0] <= s.header[6] + s.header[7]);
            }
            assert_eq!(s.metrics[0][0][0], s.header[8]);
            assert_eq!(
                s.metrics[0][0][6..24].iter().sum::<u64>(),
                s.metrics[0][0][0]
            );
        }
        writer.join().unwrap();
    }

    #[test]
    fn drain_is_counted_but_never_changes_steady_tails_and_sampler_overlap_is_explicit() {
        let t = recorder();
        let mut c = completed(1000, 1);
        c.pump.sample_begin_ticks = 800;
        c.pump.sample_end_ticks = 900;
        t.completion(0, 0, flight(0), &c, 1200, 48000, false);
        let before = t.snapshot();
        assert_eq!(before.metrics[0][0][28], 1);
        assert_eq!(before.metrics[0][1][28], 0);
        c.result.status = -536870212;
        c.result.packets[63].ticks = 100000;
        t.completion(0, 1, flight(1), &c, 1201, 48000, true);
        let after = t.snapshot();
        assert_eq!(after.header[6], 2);
        assert_eq!(after.header[8], 1);
        assert_eq!(after.header[24], 1);
        assert_eq!(before.metrics, after.metrics);
        assert_eq!(before.worst, after.worst);
        assert!(after.recent.iter().any(|r| r[15] == 1));
        let mut sample = WorkerSample {
            begin_ticks: 1,
            end_ticks: 2,
            basic_policy: 2,
            period_ticks: 8,
            ..WorkerSample::default()
        };
        t.sample(sample);
        sample.basic_policy = 1;
        t.sample(sample);
        sample.basic_policy = 2;
        sample.period_ticks = 4;
        t.sample(sample);
        let s = t.snapshot();
        assert_eq!(s.header[26], 2);
        assert_eq!(s.header[27], 1);
        assert_eq!(s.samples[0][11], 2);
        assert_eq!(s.samples[1][11], 2);
    }
    #[test]
    #[ignore = "explicit release-only telemetry overhead measurement"]
    fn recorder_cost() {
        let t = recorder();
        let c = completed(1000, 1);
        let mut means = Vec::with_capacity(100);
        for _ in 0..100 {
            let start = std::time::Instant::now();
            for seq in 0..1000 {
                t.completion(1, seq % 4, flight(seq), &c, 1200, 48000, false);
                t.admission(
                    Event {
                        direction: 1,
                        captured: 48000,
                        flags: wt::CHECKED
                            | wt::PREPARED
                            | wt::SUBMITTED
                            | wt::PREVIOUS
                            | wt::QUERIED
                            | wt::RETURNED,
                        previous_callback_ticks: 1100,
                        previous_consumed_ticks: 1200,
                        check_begin_ticks: 1201,
                        check_end_ticks: 1202,
                        prepare_end_ticks: 1203,
                        submit_begin_ticks: 1203,
                        submit_end_ticks: 1204,
                        query_end_ticks: 1205,
                        start_frame: 100,
                        checked_frame: 80,
                        returned_frame: 81,
                        ..Event::default()
                    },
                    c.pump,
                    Pump::default(),
                );
            }
            means.push(start.elapsed().as_nanos() as f64 / 1000.);
        }
        means.sort_by(f64::total_cmp);
        println!("completion_plus_admission_batch_mean_ns median={:.1} p99={:.1} max={:.1}; excludes native pump/clock/sampler costs; snapshot_bytes={}",means[50],means[98],means[99],std::mem::size_of::<Snapshot>());
        assert_eq!(t.snapshot().header[14], 0);
    }
}
