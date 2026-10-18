//! Bounded, sample-free USB-worker observations. One worker writes; lifecycle
//! reset/configure only run while it is quiescent. Property readers never block.
use crate::timing::{distance, STEADY_AFTER};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::SeqCst};
pub const WORDS: usize = 32;
pub const RECENT: usize = 16;
pub const LATE: usize = 8;
pub const PREVIOUS: u64 = 1;
pub const FEEDBACK: u64 = 2;
pub const CHECKED: u64 = 4;
pub const PREPARED: u64 = 8;
pub const SUBMITTED: u64 = 16;
pub const RETURNED: u64 = 32;
pub const PUMP: u64 = 64;
pub const QUERIED: u64 = 128;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Event {
    pub ordinal: u64,
    pub direction: u64,
    pub sequence: u64,
    pub slot: u64,
    pub ticket: u64,
    pub captured: u64,
    pub flags: u64,
    pub start_frame: u64,
    pub checked_frame: u64,
    pub returned_frame: u64,
    pub previous_sequence: u64,
    pub previous_start_frame: u64,
    pub previous_last_packet_ticks: u64,
    pub previous_callback_ticks: u64,
    pub previous_consumed_ticks: u64,
    pub feedback_sequence: u64,
    pub feedback_last_packet_ticks: u64,
    pub feedback_callback_ticks: u64,
    pub feedback_consumed_ticks: u64,
    pub loop_enter_ticks: u64,
    pub poll_end_ticks: u64,
    pub pump_enter_ticks: u64,
    pub pump_exit_ticks: u64,
    pub check_begin_ticks: u64,
    pub check_end_ticks: u64,
    pub prepare_end_ticks: u64,
    pub submit_begin_ticks: u64,
    pub submit_end_ticks: u64,
    pub query_end_ticks: u64,
    pub check_status: u64,
    pub submit_status: u64,
    pub return_clock_status: u64,
}
impl Event {
    fn words(self) -> [u64; WORDS] {
        [
            self.ordinal,
            self.direction,
            self.sequence,
            self.slot,
            self.ticket,
            self.captured,
            self.flags,
            self.start_frame,
            self.checked_frame,
            self.returned_frame,
            self.previous_sequence,
            self.previous_start_frame,
            self.previous_last_packet_ticks,
            self.previous_callback_ticks,
            self.previous_consumed_ticks,
            self.feedback_sequence,
            self.feedback_last_packet_ticks,
            self.feedback_callback_ticks,
            self.feedback_consumed_ticks,
            self.loop_enter_ticks,
            self.poll_end_ticks,
            self.pump_enter_ticks,
            self.pump_exit_ticks,
            self.check_begin_ticks,
            self.check_end_ticks,
            self.prepare_end_ticks,
            self.submit_begin_ticks,
            self.submit_end_ticks,
            self.query_end_ticks,
            self.check_status,
            self.submit_status,
            self.return_clock_status,
        ]
    }
    fn from_words(w: [u64; WORDS]) -> Self {
        Self {
            ordinal: w[0],
            direction: w[1],
            sequence: w[2],
            slot: w[3],
            ticket: w[4],
            captured: w[5],
            flags: w[6],
            start_frame: w[7],
            checked_frame: w[8],
            returned_frame: w[9],
            previous_sequence: w[10],
            previous_start_frame: w[11],
            previous_last_packet_ticks: w[12],
            previous_callback_ticks: w[13],
            previous_consumed_ticks: w[14],
            feedback_sequence: w[15],
            feedback_last_packet_ticks: w[16],
            feedback_callback_ticks: w[17],
            feedback_consumed_ticks: w[18],
            loop_enter_ticks: w[19],
            poll_end_ticks: w[20],
            pump_enter_ticks: w[21],
            pump_exit_ticks: w[22],
            check_begin_ticks: w[23],
            check_end_ticks: w[24],
            prepare_end_ticks: w[25],
            submit_begin_ticks: w[26],
            submit_end_ticks: w[27],
            query_end_ticks: w[28],
            check_status: w[29],
            submit_status: w[30],
            return_clock_status: w[31],
        }
    }
    pub fn lead(&self) -> Option<i64> {
        if self.flags & RETURNED != 0 {
            Some(distance(self.start_frame, self.returned_frame))
        } else if self.flags & CHECKED != 0 {
            Some(distance(self.start_frame, self.checked_frame))
        } else {
            None
        }
    }
}
struct AtomicEvent([AtomicU64; WORDS]);
impl AtomicEvent {
    fn new() -> Self {
        Self(std::array::from_fn(|_| AtomicU64::new(0)))
    }
    fn store(&self, e: Event) {
        for (a, w) in self.0.iter().zip(e.words()) {
            a.store(w, SeqCst);
        }
    }
    fn load(&self) -> Event {
        Event::from_words(self.0.each_ref().map(|a| a.load(SeqCst)))
    }
}
#[repr(C)]
#[derive(Default)]
pub struct Snapshot {
    pub version: u64,
    pub valid: u64,
    pub epoch: u64,
    pub timebase_numer: u64,
    pub timebase_denom: u64,
    pub queue_depth: u64,
    pub threshold_ms: u64,
    pub observed: u64,
    pub dropped: u64,
    pub recent_overwritten: u64,
    pub late_seen: u64,
    pub late_retained: u64,
    pub late_overflow: u64,
    pub worst: [Event; 2],
    pub recent: [Event; RECENT],
    pub late: [Event; LATE],
}
pub struct WorkerTiming {
    busy: AtomicBool,
    generation: AtomicU64,
    epoch: AtomicU64,
    numer: AtomicU64,
    denom: AtomicU64,
    depth: AtomicU64,
    observed: AtomicU64,
    dropped: AtomicU64,
    late_seen: AtomicU64,
    worst_lead: [AtomicI64; 2],
    worst: [AtomicEvent; 2],
    recent: [AtomicEvent; RECENT],
    late: [AtomicEvent; LATE],
}
impl WorkerTiming {
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            numer: AtomicU64::new(0),
            denom: AtomicU64::new(0),
            depth: AtomicU64::new(0),
            observed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            late_seen: AtomicU64::new(0),
            worst_lead: std::array::from_fn(|_| AtomicI64::new(i64::MAX)),
            worst: std::array::from_fn(|_| AtomicEvent::new()),
            recent: std::array::from_fn(|_| AtomicEvent::new()),
            late: std::array::from_fn(|_| AtomicEvent::new()),
        }
    }
    fn begin(&self) -> Option<u64> {
        if self
            .busy
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_err()
        {
            self.dropped.fetch_add(1, SeqCst);
            return None;
        }
        let g = self.generation.load(SeqCst);
        if g > u64::MAX - 2 {
            self.dropped.fetch_add(1, SeqCst);
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
        self.epoch.store(epoch, SeqCst);
        for a in [
            &self.numer,
            &self.denom,
            &self.depth,
            &self.observed,
            &self.dropped,
            &self.late_seen,
        ] {
            a.store(0, SeqCst);
        }
        for a in &self.worst_lead {
            a.store(i64::MAX, SeqCst);
        }
        for e in self
            .worst
            .iter()
            .chain(self.recent.iter())
            .chain(self.late.iter())
        {
            e.store(Event::default());
        }
        self.end(g);
    }
    pub fn configure(&self, numer: u32, denom: u32, depth: usize) {
        let Some(g) = self.begin() else { return };
        self.numer.store(u64::from(numer), SeqCst);
        self.denom.store(u64::from(denom), SeqCst);
        self.depth.store(depth as u64, SeqCst);
        self.end(g);
    }
    pub fn observe(&self, mut e: Event) {
        if e.direction > 1 {
            self.dropped.fetch_add(1, SeqCst);
            return;
        }
        let Some(g) = self.begin() else { return };
        let n = self.observed.load(SeqCst);
        let Some(ordinal) = n.checked_add(1) else {
            self.dropped.fetch_add(1, SeqCst);
            self.end(g);
            return;
        };
        e.ordinal = ordinal;
        self.recent[n as usize % RECENT].store(e);
        self.observed.store(ordinal, SeqCst);
        let steady = e.captured >= STEADY_AFTER;
        if steady {
            if let Some(lead) = e.lead() {
                let d = e.direction as usize;
                if lead < self.worst_lead[d].load(SeqCst) {
                    self.worst_lead[d].store(lead, SeqCst);
                    self.worst[d].store(e);
                }
            }
        }
        // Trigger is observation only: nominal queue lead minus one 8 ms block.
        // Keep faults even during startup; IN has pre-check lead, OUT post-query.
        let threshold = self.depth.load(SeqCst).saturating_sub(2) * 8;
        let fault = e.check_status != 0 || e.submit_status != 0 || e.return_clock_status != 0;
        if fault || (steady && e.lead().is_some_and(|lead| lead < threshold as i64)) {
            let n = self.late_seen.load(SeqCst);
            if n < LATE as u64 {
                self.late[n as usize].store(e);
            }
            self.late_seen.store(n.saturating_add(1), SeqCst);
        }
        self.end(g);
    }
    pub fn snapshot(&self) -> Snapshot {
        for _ in 0..2 {
            let g = self.generation.load(SeqCst);
            if !g.is_multiple_of(2) {
                continue;
            }
            let n = self.observed.load(SeqCst);
            let late = self.late_seen.load(SeqCst);
            let depth = self.depth.load(SeqCst);
            let s = Snapshot {
                version: 1,
                valid: 1,
                epoch: self.epoch.load(SeqCst),
                timebase_numer: self.numer.load(SeqCst),
                timebase_denom: self.denom.load(SeqCst),
                queue_depth: depth,
                threshold_ms: depth.saturating_sub(2) * 8,
                observed: n,
                dropped: self.dropped.load(SeqCst),
                recent_overwritten: n.saturating_sub(RECENT as u64),
                late_seen: late,
                late_retained: late.min(LATE as u64),
                late_overflow: late.saturating_sub(LATE as u64),
                worst: self.worst.each_ref().map(AtomicEvent::load),
                recent: self.recent.each_ref().map(AtomicEvent::load),
                late: self.late.each_ref().map(AtomicEvent::load),
            };
            if self.generation.load(SeqCst) == g {
                return s;
            }
        }
        Snapshot {
            version: 1,
            ..Snapshot::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(i: u64, lead: u64) -> Event {
        Event {
            direction: i % 2,
            sequence: i,
            ticket: i + 1,
            captured: STEADY_AFTER,
            flags: CHECKED | SUBMITTED | RETURNED | QUERIED,
            start_frame: 100,
            checked_frame: 90,
            returned_frame: 100 - lead,
            ..Event::default()
        }
    }
    #[test]
    fn layout_retention_overflow_and_startup_are_explicit() {
        assert_eq!(std::mem::size_of::<Event>(), 256);
        assert_eq!(std::mem::size_of::<Snapshot>(), 6760);
        let t = WorkerTiming::new();
        t.reset(3);
        t.configure(125, 3, 4);
        let mut startup = event(0, 1);
        startup.captured = 0;
        t.observe(startup);
        assert_eq!(t.snapshot().late_seen, 0);
        assert_eq!(t.snapshot().worst[0].ordinal, 0);
        for i in 1..41 {
            t.observe(event(i, 6));
        }
        let s = t.snapshot();
        assert_eq!(
            (
                s.valid,
                s.epoch,
                s.observed,
                s.late_seen,
                s.late_retained,
                s.late_overflow
            ),
            (1, 3, 41, 40, 8, 32)
        );
        assert_eq!(s.recent_overwritten, 25);
        assert_eq!(s.late[0].sequence, 1);
        assert_eq!(s.late[7].sequence, 8);
        assert!(s.recent.iter().all(|e| e.ordinal >= 26));
        assert_eq!(s.worst[0].sequence, 2);
        assert_eq!(s.worst[1].sequence, 1);
        t.observe(event(42, 3));
        assert_eq!(t.snapshot().worst[0].sequence, 42);
        t.reset(4);
        let s = t.snapshot();
        assert_eq!((s.epoch, s.observed, s.late_seen, s.dropped), (4, 0, 0, 0));
        assert!(s
            .recent
            .iter()
            .chain(s.late.iter())
            .chain(s.worst.iter())
            .all(|e| e.ordinal == 0));
    }
    #[test]
    fn contention_invalid_reads_and_faults_remain_distinguishable() {
        let t = WorkerTiming::new();
        t.reset(9);
        t.configure(125, 3, 4);
        t.busy.store(true, SeqCst);
        t.observe(event(0, 6));
        t.busy.store(false, SeqCst);
        assert_eq!(t.snapshot().dropped, 1);
        t.generation.store(3, SeqCst);
        assert_eq!(t.snapshot().valid, 0);
        t.generation.store(4, SeqCst);
        let e = Event {
            check_status: (-42i64) as u64,
            ..Event::default()
        };
        assert_eq!(e.lead(), None);
        t.observe(e);
        let s = t.snapshot();
        assert_eq!(s.late_seen, 1);
        assert_eq!(s.late[0].flags, 0);
        assert_eq!(s.late[0].check_status as i64, -42);
        t.generation.store(u64::MAX - 1, SeqCst);
        t.observe(event(1, 2));
        assert_eq!(t.snapshot().dropped, 2);
    }
    #[test]
    fn concurrent_readers_never_mix_records_or_epochs() {
        let t = std::sync::Arc::new(WorkerTiming::new());
        let w = t.clone();
        let thread = std::thread::spawn(move || {
            for epoch in 1..100 {
                w.reset(epoch);
                w.configure(125, 3, 4);
                for _ in 0..100 {
                    w.observe(event(epoch, 6));
                }
            }
        });
        while !thread.is_finished() {
            let s = t.snapshot();
            if s.valid == 0 {
                continue;
            }
            for e in s
                .recent
                .iter()
                .chain(s.worst.iter())
                .chain(s.late.iter())
                .filter(|e| e.ordinal != 0)
            {
                assert_eq!(e.sequence, s.epoch);
                assert_eq!(e.ticket, e.sequence + 1);
                assert_eq!(e.lead(), Some(6));
                assert!(e.ordinal <= s.observed);
            }
            assert_eq!(s.late_retained + s.late_overflow, s.late_seen);
        }
        thread.join().unwrap();
        assert_eq!(t.snapshot().epoch, 99);
    }
}

#[cfg(test)]
mod benchmark {
    use super::*;
    #[test]
    #[ignore = "explicit release-only observation overhead measurement"]
    fn recorder_cost() {
        let t = WorkerTiming::new();
        t.reset(1);
        t.configure(125, 3, 4);
        let mut means = Vec::with_capacity(100);
        for _ in 0..100 {
            let start = std::time::Instant::now();
            for i in 0..1000 {
                t.observe(std::hint::black_box(Event {
                    direction: i % 2,
                    sequence: i,
                    captured: 48000,
                    flags: CHECKED | RETURNED,
                    start_frame: 100,
                    checked_frame: 76,
                    returned_frame: 76,
                    ..Event::default()
                }));
            }
            means.push(start.elapsed().as_nanos() as f64 / 1000.);
        }
        means.sort_by(f64::total_cmp);
        println!("recorder_batch_mean_ns median={:.1} p99={:.1} max={:.1}; state_bytes={} snapshot_bytes={}; excludes clock calls, native API and scheduling",means[50],means[98],means[99],std::mem::size_of::<WorkerTiming>(),std::mem::size_of::<Snapshot>());
        assert_eq!(t.snapshot().observed, 100000);
        assert_eq!(t.snapshot().dropped, 0);
    }
}
