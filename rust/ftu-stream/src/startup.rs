//! Latest startup attempt, coherent and retained through teardown. No PCM or
//! steady-state work. Read with ftuS and matching epoch; not an audio-health flag.
use crate::{first_failure, geometry::RATE, live::Factory, native::Rates, pool::PoolOps};
use std::{
    ffi::c_void,
    sync::atomic::{AtomicU64, Ordering::SeqCst},
};
pub const WORDS: usize = 12 + 8 + 24;
#[repr(C)]
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    // version, valid, epoch, stage, outcome, config status, prepare status,
    // activate status, before available, verify available, prepare call state, activate call state
    pub header: [u64; 12],
    // Direction order IN81, OUT01: attempted, returned, signed status, actual bytes
    pub writes: [[u64; 4]; 2],
    // Endpoint order OUT01, IN81 in each bank, as in ftuF: before, verify
    pub rates: [[u64; 12]; 2],
}
impl Snapshot {
    pub fn new(epoch: u64) -> Self {
        let mut s = Self {
            header: [0; 12],
            writes: [[0; 4]; 2],
            rates: [[0; 12]; 2],
        };
        s.header[0] = 1;
        s.header[1] = 1;
        s.header[2] = epoch;
        s
    }
    pub fn before(&mut self, r: Rates) {
        self.header[8] = 1;
        self.rates[0] = first_failure::rates(r);
    }
    fn words(&self) -> [u64; WORDS] {
        let mut a = [0; WORDS];
        a[..12].copy_from_slice(&self.header);
        for i in 0..2 {
            a[12 + i * 4..16 + i * 4].copy_from_slice(&self.writes[i]);
            a[20 + i * 12..32 + i * 12].copy_from_slice(&self.rates[i]);
        }
        a
    }
    fn from_words(a: [u64; WORDS]) -> Self {
        let mut s = Self::new(0);
        s.header.copy_from_slice(&a[..12]);
        for i in 0..2 {
            s.writes[i].copy_from_slice(&a[12 + i * 4..16 + i * 4]);
            s.rates[i].copy_from_slice(&a[20 + i * 12..32 + i * 12]);
        }
        s
    }
}
pub struct Startup {
    sequence: AtomicU64,
    words: [AtomicU64; WORDS],
}
impl Startup {
    pub fn new() -> Self {
        let a = Snapshot::new(0).words();
        Self {
            sequence: AtomicU64::new(0),
            words: std::array::from_fn(|i| AtomicU64::new(a[i])),
        }
    }
    // Lifecycle serializes writers. Sequentially consistent atomic words avoid
    // both a Rust data race and a reader accepting a mixture of publications.
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
        let mut s = Snapshot::new(0);
        s.header[1] = 0;
        s
    }
}
/// Only the live worker calls this, after both alternates have been validated.
/// No write is repeated; cancellation between calls prevents further control IO.
pub unsafe fn initialize(
    ctx: *mut c_void,
    ops: &PoolOps,
    factory: Factory,
    report: &Startup,
    s: &mut Snapshot,
    cancelled: impl Fn() -> bool,
) -> Result<(), &'static str> {
    for d in 0..2 {
        if cancelled() {
            s.header[4] = 3;
            report.publish(s);
            return Err("signal");
        }
        s.header[3] = 4 + d as u64;
        s.writes[d][0] = 1;
        report.publish(s);
        let mut actual = u32::MAX;
        let status = (factory.set_rate)(ctx, d as u32, RATE, &mut actual);
        s.writes[d] = [1, 1, status as i64 as u64, actual.into()];
        if status != 0 || actual != 3 {
            s.header[4] = 2;
            report.publish(s);
            return Err("rate_write_failed");
        }
        report.publish(s);
    }
    if cancelled() {
        s.header[4] = 3;
        report.publish(s);
        return Err("signal");
    }
    s.header[3] = 6;
    report.publish(s);
    let mut verified = Rates::default();
    (ops.base.rates)(ctx, &mut verified);
    s.header[9] = 1;
    s.rates[1] = first_failure::rates(verified);
    if !verified.valid() {
        s.header[4] = 2;
        report.publish(s);
        return Err("rate_readback_failed");
    }
    if cancelled() {
        s.header[4] = 3;
        report.publish(s);
        return Err("signal");
    }
    s.header[3] = 7;
    s.header[4] = 1;
    report.publish(s);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn concurrent_publication_never_returns_mixed_epochs() {
        let p = Arc::new(Startup::new());
        let reader = p.clone();
        let t = std::thread::spawn(move || {
            for _ in 0..30000 {
                let s = reader.snapshot();
                if s.header[1] == 1 && s.header[2] != 0 {
                    assert!(s
                        .writes
                        .iter()
                        .flatten()
                        .chain(s.rates.iter().flatten())
                        .all(|&v| v == s.header[2]));
                }
            }
        });
        for epoch in 1..3000 {
            let mut s = Snapshot::new(epoch);
            s.writes = [[epoch; 4]; 2];
            s.rates = [[epoch; 12]; 2];
            p.publish(&s);
        }
        t.join().unwrap();
        assert_eq!(std::mem::size_of::<Snapshot>(), WORDS * 8);
        p.sequence.store(1, SeqCst);
        assert_eq!(p.snapshot().header[1], 0);
    }
}
