//! Last runtime stop of the streaming owner (ftuE v1). Metadata only, no samples.
//! Distinct from the lifetime first failed-completion latch (ftuF): every new
//! runtime fault replaces the previous record (counted in `overwritten`); a later
//! Start, reset or successful recovery never clears it. The primary record is
//! published before teardown; `finish` then adds drain/cleanup/close outcomes.
//! Only the epoch's worker writes, and epochs are serialized by join, so there is
//! exactly one writer at a time. Readers use a bounded seqlock; never blocking.
// Only the independent worker publishes; the baseline exposes an empty record.
#![cfg_attr(not(feature = "independent-4ms"), allow(dead_code))]
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

pub const WORDS: usize = 46;
pub const VERSION: u64 = 1;

// Word indices; layout documented in docs/RUNTIME-RECOVERY.md.
pub const VERSION_W: usize = 0;
pub const VALID: usize = 1;
pub const PRESENT: usize = 2;
pub const EPOCH: usize = 3;
pub const REASON: usize = 4;
pub const STAGE: usize = 5;
pub const FLAGS: usize = 6;
pub const DIRECTION: usize = 7;
pub const SEQUENCE: usize = 8;
pub const SLOT: usize = 9;
pub const SCHEDULED: usize = 10;
pub const CHECKED_FRAME: usize = 11;
pub const CHECKED_TICKS: usize = 12;
pub const RETURNED_FRAME: usize = 13;
pub const RETURNED_TICKS: usize = 14;
pub const REQUIRED_LEAD: usize = 15;
pub const BASE: usize = 16;
pub const PRIOR_STEP_FRAME: usize = 17;
pub const PRIOR_STEP_TICKS: usize = 18;
pub const PRIOR_PROGRESS_FRAME: usize = 19;
pub const PRIOR_PROGRESS_TICKS: usize = 20;
pub const STEPS: usize = 21;
pub const NEXT: usize = 22; // IN, OUT
pub const COMPLETED: usize = 24; // IN, OUT
pub const PENDING: usize = 26; // IN, OUT
pub const CAPTURED: usize = 28;
pub const PREPARED: usize = 29;
pub const TIMEBASE_NUMER: usize = 30;
pub const TIMEBASE_DENOM: usize = 31;
pub const MIN_OUT_LEAD: usize = 32;
pub const SUBMIT_STATUS: usize = 33;
pub const UPTIME_MS: usize = 34;
pub const FINISHED: usize = 35;
pub const DRAIN_FAILED: usize = 36;
pub const CLEANUP: usize = 37;
pub const CLOSE: usize = 38;
pub const TERMINAL_PHASE: usize = 39;
pub const TERMINAL_FAULT: usize = 40;
pub const OVERWRITTEN: usize = 41;
pub const RECORDS: usize = 42;
pub const QUEUE_DEPTH: usize = 43;
pub const BLOCK_MS: usize = 44;
pub const REPLAY_MS: usize = 45;

// Validity flags. An unset flag means "not measured", never zero.
pub const F_DIRECTION: u64 = 1; // direction + sequence
pub const F_SLOT: u64 = 2;
pub const F_SCHEDULED: u64 = 4;
pub const F_CHECKED: u64 = 8; // failed check, or final pre-submit check at stage 6
pub const F_RETURNED: u64 = 16; // frame + host ticks after native submit returned
pub const F_PRIOR_STEP: u64 = 32;
pub const F_PRIOR_PROGRESS: u64 = 64;
pub const F_MIN_OUT_LEAD: u64 = 128;
pub const F_SUBMIT_STATUS: u64 = 256;
pub const F_CLOSE: u64 = 512;
pub const F_TIMEBASE: u64 = 1024;
pub const F_BASE: u64 = 2048;

// Guard stage within Owner::step. Each scheduling guard is separately named.
pub const STAGE_GENERIC: u64 = 1; // callback/control/clock error; no precise check clock
pub const STAGE_WATCHDOG: u64 = 2; // completion watchdog over in-flight requests
pub const STAGE_READINESS: u64 = 3; // OUT lead before PCM readiness handling
pub const STAGE_BEFORE_PACKING: u64 = 4; // IN stale / OUT lead, clock after readiness
pub const STAGE_AFTER_PACKING: u64 = 5; // IN stale / OUT lead, clock after packing
pub const STAGE_AFTER_RETURN: u64 = 6; // IN stale / OUT lead, clock after submit returned
pub const STAGE_SUBMIT: u64 = 7; // native submit returned an error

/// The precise failing check, when one exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Site {
    pub stage: u64,
    pub direction: usize,
    pub sequence: u64,
    pub slot: Option<usize>,
    pub scheduled: Option<u64>,
    /// (bus frame, host ticks) of the failed check; at stage 6 this is the
    /// final pre-submit check and `returned` holds the failed return check.
    pub checked: Option<(u64, u64)>,
    pub returned: Option<(u64, u64)>,
    /// Gate in bus frames that the check required; 0 when not a lead guard.
    pub required: u64,
    pub submit_status: Option<i32>,
}

/// Owner state captured when the runtime stop is decided, before teardown.
#[derive(Clone, Copy, Debug, Default)]
pub struct Primary {
    pub epoch: u64,
    pub reason: u32,
    pub site: Option<Site>,
    pub base: Option<u64>,
    pub prior_step: Option<(u64, u64)>,
    pub prior_progress: Option<(u64, u64)>,
    pub steps: u64,
    pub next: [u64; 2],
    pub completed: [u64; 2],
    pub pending: [u64; 2],
    pub captured: u64,
    pub prepared: u64,
    pub timebase: Option<(u32, u32)>,
    pub min_out_lead: Option<u64>,
    pub uptime_ms: u64,
    pub queue_depth: u64,
    pub block_ms: u64,
    pub replay_ms: u64,
}

/// Teardown outcomes added after the primary record.
#[derive(Clone, Copy, Debug)]
pub struct Terminal {
    pub drain_failed: bool,
    pub cleanup: i32,
    pub close: Option<i32>,
    pub phase: u32,
    pub fault: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub words: [u64; WORDS],
}
impl Default for Snapshot {
    fn default() -> Self {
        let mut words = [0; WORDS];
        words[VERSION_W] = VERSION;
        words[VALID] = 1;
        Self { words }
    }
}
impl Snapshot {
    pub fn primary(p: &Primary) -> Self {
        let mut s = Self::default();
        let w = &mut s.words;
        let mut flags = 0;
        w[PRESENT] = 1;
        w[EPOCH] = p.epoch;
        w[REASON] = p.reason.into();
        w[STAGE] = STAGE_GENERIC;
        if let Some(site) = p.site {
            w[STAGE] = site.stage;
            w[DIRECTION] = site.direction as u64;
            w[SEQUENCE] = site.sequence;
            w[REQUIRED_LEAD] = site.required;
            flags |= F_DIRECTION;
            if let Some(slot) = site.slot {
                w[SLOT] = slot as u64;
                flags |= F_SLOT;
            }
            if let Some(start) = site.scheduled {
                w[SCHEDULED] = start;
                flags |= F_SCHEDULED;
            }
            if let Some((frame, ticks)) = site.checked {
                w[CHECKED_FRAME] = frame;
                w[CHECKED_TICKS] = ticks;
                flags |= F_CHECKED;
            }
            if let Some((frame, ticks)) = site.returned {
                w[RETURNED_FRAME] = frame;
                w[RETURNED_TICKS] = ticks;
                flags |= F_RETURNED;
            }
            if let Some(status) = site.submit_status {
                w[SUBMIT_STATUS] = status as i64 as u64;
                flags |= F_SUBMIT_STATUS;
            }
        }
        if let Some(base) = p.base {
            w[BASE] = base;
            flags |= F_BASE;
        }
        if let Some((frame, ticks)) = p.prior_step {
            w[PRIOR_STEP_FRAME] = frame;
            w[PRIOR_STEP_TICKS] = ticks;
            flags |= F_PRIOR_STEP;
        }
        if let Some((frame, ticks)) = p.prior_progress {
            w[PRIOR_PROGRESS_FRAME] = frame;
            w[PRIOR_PROGRESS_TICKS] = ticks;
            flags |= F_PRIOR_PROGRESS;
        }
        if let Some((numer, denom)) = p.timebase {
            w[TIMEBASE_NUMER] = numer.into();
            w[TIMEBASE_DENOM] = denom.into();
            flags |= F_TIMEBASE;
        }
        if let Some(lead) = p.min_out_lead {
            w[MIN_OUT_LEAD] = lead;
            flags |= F_MIN_OUT_LEAD;
        }
        w[STEPS] = p.steps;
        w[NEXT..NEXT + 2].copy_from_slice(&p.next);
        w[COMPLETED..COMPLETED + 2].copy_from_slice(&p.completed);
        w[PENDING..PENDING + 2].copy_from_slice(&p.pending);
        w[CAPTURED] = p.captured;
        w[PREPARED] = p.prepared;
        w[UPTIME_MS] = p.uptime_ms;
        w[QUEUE_DEPTH] = p.queue_depth;
        w[BLOCK_MS] = p.block_ms;
        w[REPLAY_MS] = p.replay_ms;
        w[FLAGS] = flags;
        s
    }
    pub fn present(&self) -> bool {
        self.words[VALID] == 1 && self.words[PRESENT] == 1
    }
    pub fn flag(&self, f: u64) -> bool {
        self.words[FLAGS] & f == f
    }
}

/// Fixed-size seqlock of atomic words. Writers must be externally serialized.
pub struct Seq<const N: usize> {
    sequence: AtomicU64,
    words: [AtomicU64; N],
}
impl<const N: usize> Seq<N> {
    pub fn new(initial: [u64; N]) -> Self {
        Self {
            sequence: AtomicU64::new(0),
            words: std::array::from_fn(|i| AtomicU64::new(initial[i])),
        }
    }
    pub fn publish(&self, w: &[u64; N]) {
        self.sequence.fetch_add(1, SeqCst);
        for (a, v) in self.words.iter().zip(w) {
            a.store(*v, SeqCst);
        }
        self.sequence.fetch_add(1, SeqCst);
    }
    /// The serialized writer's own last publication (no concurrent writer).
    pub fn current(&self) -> [u64; N] {
        std::array::from_fn(|i| self.words[i].load(SeqCst))
    }
    /// Bounded: None after four torn attempts. Never waits on the writer.
    pub fn read(&self) -> Option<[u64; N]> {
        for _ in 0..4 {
            let begin = self.sequence.load(SeqCst);
            if begin & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let w = self.current();
            if self.sequence.load(SeqCst) == begin {
                return Some(w);
            }
        }
        None
    }
}

pub struct RuntimeFault {
    seq: Seq<WORDS>,
}
impl RuntimeFault {
    pub fn new() -> Self {
        Self {
            seq: Seq::new(Snapshot::default().words),
        }
    }
    /// Primary record, before teardown. Replaces (and counts) any earlier record.
    pub fn publish(&self, s: &Snapshot) {
        let old = self.seq.current();
        let mut w = s.words;
        w[OVERWRITTEN] = old[OVERWRITTEN].saturating_add(u64::from(old[PRESENT] == 1));
        w[RECORDS] = old[RECORDS].saturating_add(1);
        w[FINISHED] = 0;
        self.seq.publish(&w);
    }
    /// Teardown outcomes for the epoch that published the current record.
    /// A record from another epoch or one already finished is never changed.
    pub fn finish(&self, epoch: u64, t: Terminal) -> bool {
        let mut w = self.seq.current();
        if w[PRESENT] != 1 || w[EPOCH] != epoch || w[FINISHED] != 0 {
            return false;
        }
        w[FINISHED] = 1;
        w[DRAIN_FAILED] = u64::from(t.drain_failed);
        w[CLEANUP] = t.cleanup as i64 as u64;
        if let Some(close) = t.close {
            w[CLOSE] = close as i64 as u64;
            w[FLAGS] |= F_CLOSE;
        }
        w[TERMINAL_PHASE] = t.phase.into();
        w[TERMINAL_FAULT] = t.fault as i64 as u64;
        self.seq.publish(&w);
        true
    }
    pub fn snapshot(&self) -> Snapshot {
        match self.seq.read() {
            Some(words) => Snapshot { words },
            None => {
                let mut s = Snapshot::default();
                s.words[VALID] = 0;
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn record(epoch: u64) -> Snapshot {
        Snapshot::primary(&Primary {
            epoch,
            reason: 55,
            site: Some(Site {
                stage: STAGE_AFTER_PACKING,
                direction: 1,
                sequence: epoch,
                slot: Some(3),
                scheduled: Some(epoch),
                checked: Some((epoch, epoch)),
                returned: None,
                required: 16,
                submit_status: None,
            }),
            steps: epoch,
            next: [epoch; 2],
            completed: [epoch; 2],
            pending: [epoch; 2],
            captured: epoch,
            prepared: epoch,
            uptime_ms: epoch,
            ..Primary::default()
        })
    }

    #[test]
    fn layout_and_absent_fields_are_flagged_not_zeroed() {
        assert_eq!(std::mem::size_of::<Snapshot>(), WORDS * 8);
        let s = Snapshot::primary(&Primary {
            epoch: 48,
            reason: 24,
            ..Primary::default()
        });
        assert_eq!(s.words[STAGE], STAGE_GENERIC);
        assert_eq!(s.words[FLAGS], 0);
        assert!(!s.flag(F_CHECKED) && !s.flag(F_DIRECTION) && !s.flag(F_RETURNED));
        let s = record(48);
        assert!(s.flag(F_DIRECTION | F_SLOT | F_SCHEDULED | F_CHECKED));
        assert!(!s.flag(F_RETURNED) && !s.flag(F_PRIOR_STEP) && !s.flag(F_CLOSE));
        assert_eq!(s.words[REQUIRED_LEAD], 16);
    }

    #[test]
    fn records_persist_count_overwrites_and_finish_only_their_epoch() {
        let r = RuntimeFault::new();
        assert!(!r.snapshot().present());
        r.publish(&record(5));
        let t = Terminal {
            drain_failed: false,
            cleanup: 0,
            close: Some(0),
            phase: 4,
            fault: 6,
        };
        assert!(!r.finish(4, t), "another epoch never finishes this record");
        assert!(r.finish(5, t));
        assert!(!r.finish(5, t), "finish is single-shot");
        let s = r.snapshot();
        assert_eq!(
            (s.words[FINISHED], s.words[OVERWRITTEN], s.words[RECORDS]),
            (1, 0, 1)
        );
        assert!(s.flag(F_CLOSE));
        r.publish(&record(6));
        let s = r.snapshot();
        assert_eq!((s.words[EPOCH], s.words[FINISHED]), (6, 0));
        assert_eq!((s.words[OVERWRITTEN], s.words[RECORDS]), (1, 2));
        r.finish(
            6,
            Terminal {
                drain_failed: true,
                cleanup: crate::PENDING_OWNERSHIP,
                close: None,
                phase: 5,
                fault: 7,
            },
        );
        let s = r.snapshot();
        assert!(!s.flag(F_CLOSE));
        assert_eq!(s.words[CLEANUP] as i64, crate::PENDING_OWNERSHIP as i64);
        assert_eq!(s.words[TERMINAL_FAULT], 7);
    }

    #[test]
    fn concurrent_publication_never_returns_a_mixed_record() {
        let r = Arc::new(RuntimeFault::new());
        let reader = r.clone();
        let t = std::thread::spawn(move || {
            let mut coherent = 0;
            for _ in 0..30000 {
                let s = reader.snapshot();
                if s.present() {
                    let e = s.words[EPOCH];
                    for i in [
                        SEQUENCE,
                        SCHEDULED,
                        CHECKED_FRAME,
                        STEPS,
                        NEXT,
                        PENDING + 1,
                        UPTIME_MS,
                    ] {
                        assert_eq!(s.words[i], e);
                    }
                    if s.words[FINISHED] == 1 {
                        assert_eq!(s.words[TERMINAL_FAULT], e);
                    }
                    coherent += 1;
                } else if s.words[VALID] == 0 {
                    assert_eq!(s.words[PRESENT], 0, "torn reads carry no record");
                }
            }
            coherent
        });
        for epoch in 1..3000 {
            r.publish(&record(epoch));
            r.finish(
                epoch,
                Terminal {
                    drain_failed: false,
                    cleanup: 0,
                    close: Some(0),
                    phase: 4,
                    fault: epoch as i32,
                },
            );
        }
        t.join().unwrap();
        let s = r.snapshot();
        assert_eq!((s.words[OVERWRITTEN], s.words[RECORDS]), (2998, 2999));
        r.seq.sequence.store(1, SeqCst);
        assert_eq!(r.snapshot().words[VALID], 0);
    }
}
