//! First failed completed USB request, metadata only. Published after transport
//! stops admission, immutable for this Live lifetime, including across later starts.
//! The primary record precedes teardown; a second publication adds its result.
use crate::{
    native::{Info, Rates},
    pool::{CompletionFailure, PoolSession},
};
use std::sync::atomic::{
    AtomicU64, AtomicU8,
    Ordering::{Acquire, Relaxed, Release},
};
pub const HEADER: usize = 35;
pub const RATE_WORDS: usize = 12;
pub const INFO_WORDS: usize = 17;
pub const WORDS: usize = HEADER + 2 * RATE_WORDS + 2 * INFO_WORDS + 64 + 64 * 4 + 6;
#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub header: [u64; HEADER],
    pub rates: [[u64; RATE_WORDS]; 2],
    pub info: [[u64; INFO_WORDS]; 2],
    pub requested: [u64; 64],
    pub packets: [[u64; 4]; 64],
    pub pump: [u64; 6],
}
impl Default for Snapshot {
    fn default() -> Self {
        let mut s = Self {
            header: [0; HEADER],
            rates: [[0; RATE_WORDS]; 2],
            info: [[0; INFO_WORDS]; 2],
            requested: [0; 64],
            packets: [[0; 4]; 64],
            pump: [0; 6],
        };
        s.header[0] = 2;
        s.header[1] = 1;
        s
    }
}
pub struct Finish {
    pub epoch: u64,
    pub activated: bool,
    pub before_rates: Rates,
    pub after_rates: Option<Rates>,
    pub before_info: Info,
    pub after_info: Option<Info>,
    pub cleanup: i32,
    pub close: Option<i32>,
}
pub(crate) fn rates(r: Rates) -> [u64; RATE_WORDS] {
    [
        r.status[0] as i64 as u64,
        r.status[1] as i64 as u64,
        r.hz[0].into(),
        r.hz[1].into(),
        r.actual[0].into(),
        r.actual[1].into(),
        r.raw[0].into(),
        r.raw[1].into(),
        r.raw[2].into(),
        r.raw[3].into(),
        r.raw[4].into(),
        r.raw[5].into(),
    ]
}
fn info(i: Info) -> [u64; INFO_WORDS] {
    [
        i.timebase_numer.into(),
        i.timebase_denom.into(),
        i.location.into(),
        i.vid.into(),
        i.pid.into(),
        i.stage.into(),
        i.original[0].into(),
        i.original[1].into(),
        i.pipe[0].into(),
        i.pipe[1].into(),
        i.restored_mask.into(),
        i.open_mask.into(),
        i.callback_fault.into(),
        i.restore_status[0] as i64 as u64,
        i.restore_status[1] as i64 as u64,
        i.cleanup_status[0] as i64 as u64,
        i.cleanup_status[1] as i64 as u64,
    ]
}
impl Snapshot {
    fn record(f: &CompletionFailure, session: &PoolSession, end: &Finish) -> Self {
        let mut s = Self::default();
        let t = &f.trace;
        s.header[2] = 1;
        s.header[31] = session.geometry().entries() as u64;
        s.header[32] = session.queue_depth() as u64;
        s.header[33] = session.geometry().block_ms();
        s.header[34] = session.geometry().history() as u64;
        s.header[3] = end.epoch;
        s.header[4] = f.reason.into();
        s.header[5] = f.direction as u64;
        s.header[8] = t.slot as u64;
        s.header[10] = t.submitted_at;
        s.header[11] = t.submit_return_frame;
        s.header[12] = t.admission_ticks;
        s.header[13] = t.submit.unwrap_or(0) as i64 as u64;
        s.header[17] = u64::from(end.activated);
        s.header[18] = u64::from(session.drain_failed);
        s.header[19] = end.cleanup as i64 as u64;
        s.header[20] = u64::from(end.after_rates.is_some() && end.after_info.is_some());
        s.header[21] = u64::from(end.close.is_some());
        s.header[22] = end.close.unwrap_or(0) as i64 as u64;
        for d in 0..2 {
            s.header[24 + d] = session.next[d] as u64;
            s.header[26 + d] = session.completed[d] as u64;
            s.header[28 + d] = session.pending()[d] as u64;
        }
        s.rates[0] = rates(end.before_rates);
        if let Some(r) = end.after_rates {
            s.rates[1] = rates(r);
        }
        s.info[0] = info(end.before_info);
        if let Some(i) = end.after_info {
            s.info[1] = info(i);
        }
        s.requested = t.lengths.map(u64::from);
        if let Some(flight) = t.flight {
            s.header[6] = flight.seq as u64;
            s.header[7] = flight.ticket;
            s.header[9] = flight.start;
            if let Some(c) = t.completion {
                s.header[23] = u64::from(
                    f.direction < 2
                        && t.slot < session.queue_depth()
                        && c.ticket == flight.ticket
                        && t.submit == Some(0),
                );
                s.header[14] = c.result.status as i64 as u64;
                s.header[15] = c.result.matched.into();
                s.header[16] = c.result.callback_ticks;
                s.packets = c.result.packets.map(|p| {
                    [
                        p.status as i64 as u64,
                        p.requested.into(),
                        p.actual.into(),
                        p.ticks,
                    ]
                });
                let p = c.pump;
                s.pump = [
                    p.id,
                    p.enter_ticks,
                    p.exit_ticks,
                    p.result,
                    p.sample_begin_ticks,
                    p.sample_end_ticks,
                ];
            }
        }
        s
    }
    fn words(&self) -> [u64; WORDS] {
        let mut a = [0; WORDS];
        let mut at = 0;
        for slice in std::iter::once(self.header.as_slice())
            .chain(self.rates.iter().map(|x| x.as_slice()))
            .chain(self.info.iter().map(|x| x.as_slice()))
            .chain(std::iter::once(self.requested.as_slice()))
            .chain(self.packets.iter().map(|x| x.as_slice()))
            .chain(std::iter::once(self.pump.as_slice()))
        {
            a[at..at + slice.len()].copy_from_slice(slice);
            at += slice.len();
        }
        a
    }
    fn from_words(a: &[u64; WORDS]) -> Self {
        let mut s = Self::default();
        let mut at = 0;
        for slice in std::iter::once(s.header.as_mut_slice())
            .chain(s.rates.iter_mut().map(|x| x.as_mut_slice()))
            .chain(s.info.iter_mut().map(|x| x.as_mut_slice()))
            .chain(std::iter::once(s.requested.as_mut_slice()))
            .chain(s.packets.iter_mut().map(|x| x.as_mut_slice()))
            .chain(std::iter::once(s.pump.as_mut_slice()))
        {
            slice.copy_from_slice(&a[at..at + slice.len()]);
            at += slice.len();
        }
        s
    }
}
struct Published {
    // 0 empty, 1 publishing, 2 immutable. Never reset/reused within a Live.
    state: AtomicU8,
    words: [AtomicU64; WORDS],
}
impl Published {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn publish(&self, s: &Snapshot) -> bool {
        if self.state.compare_exchange(0, 1, Acquire, Relaxed).is_err() {
            return false;
        }
        for (a, v) in self.words.iter().zip(s.words()) {
            a.store(v, Relaxed);
        }
        self.state.store(2, Release);
        true
    }
    pub fn snapshot(&self) -> Snapshot {
        match self.state.load(Acquire) {
            2 => Snapshot::from_words(&std::array::from_fn(|i| self.words[i].load(Relaxed))),
            0 => Snapshot::default(),
            _ => {
                let mut s = Snapshot::default();
                s.header[1] = 0;
                s
            }
        }
    }
}
/// Two immutable publications: primary error before any teardown, then terminal
/// evidence if teardown returns. Neither slot is reused by a later session.
pub struct FirstFailure {
    primary: Published,
    terminal: Published,
}
impl FirstFailure {
    pub fn new() -> Self {
        Self {
            primary: Published::new(),
            terminal: Published::new(),
        }
    }
    pub fn latch(&self, session: &PoolSession, end: &Finish) -> bool {
        session
            .completion_failure
            .as_ref()
            .is_some_and(|f| self.primary.publish(&Snapshot::record(f, session, end)))
    }
    pub fn finish(&self, session: &PoolSession, end: &Finish, owns_record: bool) {
        if owns_record {
            if let Some(f) = &session.completion_failure {
                let mut s = Snapshot::record(f, session, end);
                s.header[30] = 1;
                self.terminal.publish(&s);
            }
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        if self.terminal.state.load(Acquire) == 2 {
            self.terminal.snapshot()
        } else {
            self.primary.snapshot()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    #[test]
    fn publication_is_immutable_across_later_faults_and_readers() {
        let r = Arc::new(Published::new());
        let barrier = Arc::new(Barrier::new(2));
        let mut wanted = Snapshot::from_words(&std::array::from_fn(|i| i as u64 + 20));
        wanted.header[0] = 1;
        wanted.header[1] = 1;
        wanted.header[2] = 1;
        let expected = wanted.clone();
        let child = r.clone();
        let gate = barrier.clone();
        let reader = std::thread::spawn(move || {
            gate.wait();
            for _ in 0..10000 {
                let s = child.snapshot();
                if s.header[2] == 1 {
                    assert_eq!(s, expected);
                } else {
                    assert_eq!(s.header[2], 0);
                }
            }
        });
        barrier.wait();
        r.publish(&wanted);
        reader.join().unwrap();
        let mut later = wanted.clone();
        later.header[3] = 999;
        r.publish(&later);
        assert_eq!(r.snapshot(), wanted);
        assert_eq!(std::mem::size_of::<Snapshot>(), WORDS * 8);
    }
    #[test]
    fn in_progress_and_empty_are_explicit() {
        let f = Published::new();
        assert_eq!(f.snapshot().header[..3], [2, 1, 0]);
        f.state.store(1, Relaxed);
        assert_eq!(f.snapshot().header[..3], [2, 0, 0]);
    }
}
