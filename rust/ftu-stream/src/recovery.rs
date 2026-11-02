//! Automatic control-side recovery policy (ftuA v1). Pure decisions; the Live
//! control mutex owns the only instance and performs every reopen itself.
//! Recoverable: a cleanly drained OUT lead-deadline stop (reason 55) of the
//! independent profile, while admitted clients remain on the same USB binding.
//! Everything else is refused and stays visible. Retries are bounded by a
//! consecutive-attempt budget that resets only after a healthy duration.
use crate::runtime_fault::{self as rf, Snapshot as Record};

pub const WORDS: usize = 30;
pub const VERSION: u64 = 1;
/// Only the independent profile qualifies; the baseline never auto-recovers.
pub const ENABLED: bool = cfg!(feature = "independent-4ms");
pub const RECOVERABLE_REASON: u64 = 55;
const FAULT_PHASE: u64 = 4;
const RUNTIME_FAULT: u64 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Consecutive attempts without an intervening healthy run.
    pub budget: u32,
    /// Delay before attempt n (n = consecutive attempts already made).
    pub backoff_ms: [u64; 3],
    /// A recovered/started run must stay up this long to reset the budget.
    pub healthy_ms: u64,
}
pub const POLICY: Policy = Policy {
    budget: 3,
    backoff_ms: [0, 1000, 4000],
    healthy_ms: 30_000,
};

// State word.
pub const IDLE: u64 = 0;
pub const PENDING: u64 = 1;
pub const RECOVERED: u64 = 2;
pub const REFUSED: u64 = 3;
pub const EXHAUSTED: u64 = 4;
pub const CANCELLED: u64 = 5;
pub const HEALTHY: u64 = 6;

// Refusal word: why automatic recovery will not run for this fault.
pub const R_NONE: u64 = 0;
pub const R_QUARANTINE: u64 = 1;
pub const R_REASON: u64 = 2;
pub const R_CLEANUP: u64 = 3;
pub const R_CLOSE: u64 = 4;
pub const R_FAULT_CODE: u64 = 5; // policy restore, postcondition, panic, ...
pub const R_NO_RECORD: u64 = 6; // startup/setup fault or record of another epoch
pub const R_UNFINISHED: u64 = 7;
pub const R_ATTEMPT_FAILED: u64 = 8; // a failed recovery start without retry eligibility
pub const R_PROFILE: u64 = 9;
pub const R_STAGE: u64 = 10;

// Word indices.
pub const STATE: usize = 3;
pub const REFUSAL: usize = 4;
pub const CONSECUTIVE: usize = 5;
pub const ATTEMPTS: usize = 7;
pub const RECOVERED_COUNT: usize = 8;
pub const FAILED: usize = 9;
pub const REFUSED_COUNT: usize = 10;
pub const CANCELLED_COUNT: usize = 11;
pub const EXHAUSTED_COUNT: usize = 12;
pub const HEALTHY_RESETS: usize = 13;
pub const FAULT_EPOCH: usize = 14;
pub const RECOVERED_EPOCH: usize = 15;
pub const LAST_STATUS: usize = 16;
pub const PENDING_SINCE: usize = 17;
pub const DUE: usize = 18;
pub const RUNNING_SINCE: usize = 19;
pub const CLIENTS: usize = 27;
pub const RETIRED: usize = 28;
pub const TIME_FLAGS: usize = 29;
pub const T_PENDING: u64 = 1; // pending_since and due valid
pub const T_RUNNING: u64 = 2; // running_since valid

/// A fault the policy may recover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub epoch: u64,
    pub reason: u64,
    pub published_ms: u64,
}

/// Decide from the finished runtime record of the current epoch alone.
pub fn classify(r: &Record, epoch: u64, shared_fault: i32) -> Result<Fault, u64> {
    let w = &r.words;
    if !r.present() || w[rf::EPOCH] != epoch {
        return Err(R_NO_RECORD);
    }
    if w[rf::FINISHED] != 1 {
        return Err(R_UNFINISHED);
    }
    if w[rf::TERMINAL_PHASE] != FAULT_PHASE {
        return Err(R_QUARANTINE);
    }
    if w[rf::DRAIN_FAILED] != 0 || w[rf::CLEANUP] != 0 {
        return Err(R_CLEANUP);
    }
    if !r.flag(rf::F_CLOSE) || w[rf::CLOSE] != 0 {
        return Err(R_CLOSE);
    }
    if shared_fault as i64 as u64 != RUNTIME_FAULT || w[rf::TERMINAL_FAULT] != RUNTIME_FAULT {
        return Err(R_FAULT_CODE);
    }
    if w[rf::REASON] != RECOVERABLE_REASON {
        return Err(R_REASON);
    }
    if !(rf::STAGE_READINESS..=rf::STAGE_AFTER_RETURN).contains(&w[rf::STAGE])
        || !r.flag(rf::F_DIRECTION)
        || w[rf::DIRECTION] != 1
    {
        return Err(R_STAGE);
    }
    Ok(Fault {
        epoch,
        reason: w[rf::REASON],
        published_ms: w[rf::UPTIME_MS],
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Backoff has not elapsed; ask again later.
    Wait,
    /// Reopen now (the caller has joined the worker and drained IO).
    Attempt,
    /// Refused or exhausted; the code is the control API return.
    Refuse(i32),
}
pub const REFUSED_STATUS: i32 = -10;
pub const EXHAUSTED_STATUS: i32 = -11;
pub const QUARANTINE_STATUS: i32 = -3;

#[derive(Clone, Debug)]
pub struct Recovery {
    pub policy: Policy,
    state: u64,
    refusal: u64,
    consecutive: u32,
    attempts: u64,
    recovered: u64,
    failed: u64,
    refused: u64,
    cancelled: u64,
    exhausted: u64,
    healthy_resets: u64,
    fault_epoch: u64,
    recovered_epoch: u64,
    last_status: i32,
    last_reason: u64,
    pending_since: Option<u64>,
    due: Option<u64>,
    running_since: Option<u64>,
    // Session epoch whose fault is settled as refused/exhausted.
    settled: Option<(u64, i32)>,
    binding: u64,
}
impl Recovery {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            state: IDLE,
            refusal: R_NONE,
            consecutive: 0,
            attempts: 0,
            recovered: 0,
            failed: 0,
            refused: 0,
            cancelled: 0,
            exhausted: 0,
            healthy_resets: 0,
            fault_epoch: 0,
            recovered_epoch: 0,
            last_status: 0,
            last_reason: 0,
            pending_since: None,
            due: None,
            running_since: None,
            settled: None,
            binding: 0,
        }
    }
    #[cfg(test)]
    pub fn state(&self) -> u64 {
        self.state
    }
    #[cfg(test)]
    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }
    pub fn note_binding(&mut self, binding: u64) {
        self.binding = binding;
    }
    /// A client-initiated new session: a fresh consecutive budget. Cumulative
    /// counters and the last runtime record are retained.
    pub fn fresh_start(&mut self, running: bool, now: u64) {
        self.consecutive = 0;
        self.state = IDLE;
        self.refusal = R_NONE;
        self.pending_since = None;
        self.due = None;
        self.settled = None;
        self.running_since = running.then_some(now);
    }
    /// Last client stopped or the unit was detached: nothing may be revived.
    pub fn cancel(&mut self) {
        if self.state == PENDING {
            self.cancelled += 1;
            self.state = CANCELLED;
        } else if self.state == HEALTHY || self.state == RECOVERED {
            self.state = IDLE;
        }
        self.pending_since = None;
        self.due = None;
        self.running_since = None;
    }
    /// Terminal ownership failure is diagnostic even without active clients.
    /// Preserve cumulative history, but do not report the earlier epoch healthy.
    pub fn quarantined(&mut self, epoch: u64, reason: u64) {
        self.fault_epoch = epoch;
        self.last_reason = reason;
        self.last_status = QUARANTINE_STATUS;
        self.running_since = None;
        self.refuse(R_QUARANTINE, epoch, QUARANTINE_STATUS);
    }
    /// The session is running. Reset the budget only after a healthy duration.
    pub fn running(&mut self, now: u64) {
        if self.consecutive > 0
            && self
                .running_since
                .is_some_and(|t| now.saturating_sub(t) >= self.policy.healthy_ms)
        {
            self.consecutive = 0;
            self.healthy_resets += 1;
            self.state = HEALTHY;
        }
    }
    /// A decision already made for this session epoch (refused/exhausted).
    pub fn settled(&self, epoch: u64) -> Option<i32> {
        self.settled.filter(|s| s.0 == epoch).map(|s| s.1)
    }
    pub fn refuse(&mut self, refusal: u64, epoch: u64, code: i32) -> Decision {
        if self.settled(epoch).is_none() {
            self.refused += 1;
        }
        self.state = REFUSED;
        self.fault_epoch = epoch;
        self.running_since = None;
        self.refusal = refusal;
        self.pending_since = None;
        self.due = None;
        self.settled = Some((epoch, code));
        Decision::Refuse(code)
    }
    pub fn fault(&mut self, f: Result<Fault, u64>, epoch: u64, now: u64) -> Decision {
        let f = match f {
            Ok(f) => f,
            Err(r) => return self.refuse(r, epoch, REFUSED_STATUS),
        };
        if f.epoch != self.fault_epoch || self.state != PENDING {
            self.fault_epoch = f.epoch;
            self.last_reason = f.reason;
            // Healthy duration is measured to the fault, not to this poll.
            if self.consecutive > 0
                && self
                    .running_since
                    .is_some_and(|t| f.published_ms.saturating_sub(t) >= self.policy.healthy_ms)
            {
                self.consecutive = 0;
                self.healthy_resets += 1;
            }
            self.running_since = None;
            if self.consecutive >= self.policy.budget {
                self.exhausted += 1;
                self.state = EXHAUSTED;
                self.refusal = R_NONE;
                self.pending_since = None;
                self.due = None;
                self.settled = Some((epoch, EXHAUSTED_STATUS));
                return Decision::Refuse(EXHAUSTED_STATUS);
            }
            let delay = self.policy.backoff_ms[(self.consecutive as usize).min(2)];
            self.state = PENDING;
            self.refusal = R_NONE;
            self.pending_since = Some(now);
            self.due = Some(now.saturating_add(delay));
        }
        if self.due.is_some_and(|due| now < due) {
            Decision::Wait
        } else {
            Decision::Attempt
        }
    }
    /// Outcome of a reopen. A failed launch may consume the remaining budget
    /// only with a newly classified, safely closed deadline fault from that
    /// launch. It remains a failed attempt; scheduling a retry is not recovery.
    pub fn attempted(&mut self, status: i32, epoch: u64, now: u64, retry: Option<Fault>) {
        self.attempts += 1;
        self.consecutive = self.consecutive.saturating_add(1);
        self.last_status = status;
        self.pending_since = None;
        self.due = None;
        if status == 0 {
            self.recovered += 1;
            self.recovered_epoch = epoch;
            self.running_since = Some(now);
            self.state = RECOVERED;
            self.refusal = R_NONE;
            self.settled = None;
        } else {
            self.failed += 1;
            self.running_since = None;
            if let Some(f) = retry.filter(|f| {
                status == -5
                    && f.epoch == epoch
                    && epoch > self.fault_epoch
                    && f.reason == RECOVERABLE_REASON
            }) {
                // The failed reopen counts against the same budget. Use the
                // next backoff, and let the next control poll recheck binding,
                // clients, admission, terminal state and fault evidence.
                self.state = IDLE;
                self.settled = None;
                self.fault(Ok(f), epoch, now);
            } else {
                self.fault_epoch = epoch;
                self.state = REFUSED;
                self.refusal = R_ATTEMPT_FAILED;
                self.refused += 1;
                self.settled = Some((epoch, REFUSED_STATUS));
            }
        }
    }
    pub fn words(&self, now: u64, clients: usize, retired: usize) -> [u64; WORDS] {
        let mut w = [0; WORDS];
        w[0] = VERSION;
        w[1] = 1;
        w[2] = u64::from(ENABLED);
        w[STATE] = self.state;
        w[REFUSAL] = self.refusal;
        w[CONSECUTIVE] = self.consecutive.into();
        w[6] = self.policy.budget.into();
        w[ATTEMPTS] = self.attempts;
        w[RECOVERED_COUNT] = self.recovered;
        w[FAILED] = self.failed;
        w[REFUSED_COUNT] = self.refused;
        w[CANCELLED_COUNT] = self.cancelled;
        w[EXHAUSTED_COUNT] = self.exhausted;
        w[HEALTHY_RESETS] = self.healthy_resets;
        w[FAULT_EPOCH] = self.fault_epoch;
        w[RECOVERED_EPOCH] = self.recovered_epoch;
        w[LAST_STATUS] = self.last_status as i64 as u64;
        let mut flags = 0;
        if let (Some(since), Some(due)) = (self.pending_since, self.due) {
            w[PENDING_SINCE] = since;
            w[DUE] = due;
            flags |= T_PENDING;
        }
        if let Some(t) = self.running_since {
            w[RUNNING_SINCE] = t;
            flags |= T_RUNNING;
        }
        w[20] = self.policy.healthy_ms;
        w[21..24].copy_from_slice(&self.policy.backoff_ms);
        w[24] = self.last_reason;
        w[25] = self.binding;
        w[26] = now;
        w[CLIENTS] = clients as u64;
        w[RETIRED] = retired as u64;
        w[TIME_FLAGS] = flags;
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_fault::{Primary, RuntimeFault, Site, Terminal};

    fn finished(epoch: u64, reason: u32, stage: u64, t: Terminal, at: u64) -> Record {
        let r = RuntimeFault::new();
        r.publish(&Record::primary(&Primary {
            epoch,
            reason,
            site: Some(Site {
                stage,
                direction: 1,
                sequence: 9,
                scheduled: Some(180),
                checked: Some((170, 170 * 24000)),
                required: 16,
                ..Site::default()
            }),
            uptime_ms: at,
            ..Primary::default()
        }));
        r.finish(epoch, t);
        r.snapshot()
    }
    const CLEAN: Terminal = Terminal {
        drain_failed: false,
        cleanup: 0,
        close: Some(0),
        phase: 4,
        fault: 6,
    };

    #[test]
    fn only_a_cleanly_closed_out_deadline_of_this_epoch_is_recoverable() {
        for stage in 3..=6 {
            assert!(classify(&finished(7, 55, stage, CLEAN, 0), 7, 6).is_ok());
        }
        let r = finished(7, 55, 5, CLEAN, 0);
        assert_eq!(classify(&r, 8, 6), Err(R_NO_RECORD));
        assert_eq!(classify(&r, 7, 13), Err(R_FAULT_CODE));
        assert_eq!(classify(&finished(7, 20, 4, CLEAN, 0), 7, 6), Err(R_REASON));
        assert_eq!(classify(&finished(7, 21, 2, CLEAN, 0), 7, 6), Err(R_REASON));
        assert_eq!(classify(&finished(7, 23, 1, CLEAN, 0), 7, 6), Err(R_REASON));
        assert_eq!(classify(&finished(7, 55, 1, CLEAN, 0), 7, 6), Err(R_STAGE));
        let q = Terminal { phase: 5, ..CLEAN };
        assert_eq!(classify(&finished(7, 55, 5, q, 0), 7, 6), Err(R_QUARANTINE));
        let c = Terminal {
            cleanup: -1,
            ..CLEAN
        };
        assert_eq!(classify(&finished(7, 55, 5, c, 0), 7, 6), Err(R_CLEANUP));
        let d = Terminal {
            drain_failed: true,
            ..CLEAN
        };
        assert_eq!(classify(&finished(7, 55, 5, d, 0), 7, 6), Err(R_CLEANUP));
        let x = Terminal {
            close: Some(-5),
            ..CLEAN
        };
        assert_eq!(classify(&finished(7, 55, 5, x, 0), 7, 6), Err(R_CLOSE));
        let n = Terminal {
            close: None,
            ..CLEAN
        };
        assert_eq!(classify(&finished(7, 55, 5, n, 0), 7, 6), Err(R_CLOSE));
        let p = Terminal { fault: 10, ..CLEAN };
        assert_eq!(
            classify(&finished(7, 55, 5, p, 0), 7, 10),
            Err(R_FAULT_CODE)
        );
        // Unfinished: published before teardown completed.
        let r = RuntimeFault::new();
        r.publish(&finished(7, 55, 5, CLEAN, 0));
        assert_eq!(classify(&r.snapshot(), 7, 6), Err(R_UNFINISHED));
    }

    #[test]
    fn backoff_budget_and_healthy_reset() {
        let policy = Policy {
            budget: 3,
            backoff_ms: [0, 100, 400],
            healthy_ms: 1000,
        };
        let mut r = Recovery::new(policy);
        r.fresh_start(true, 0);
        let fault = |epoch, at| {
            Ok(Fault {
                epoch,
                reason: 55,
                published_ms: at,
            })
        };
        // First fault: immediate attempt.
        assert_eq!(r.fault(fault(1, 10), 1, 10), Decision::Attempt);
        r.attempted(0, 2, 20, None);
        assert_eq!((r.state(), r.consecutive()), (RECOVERED, 1));
        // Second fault before the healthy duration: backoff 100 ms.
        assert_eq!(r.fault(fault(2, 50), 2, 60), Decision::Wait);
        assert_eq!(r.state(), PENDING);
        assert_eq!(r.fault(fault(2, 50), 2, 159), Decision::Wait);
        assert_eq!(r.fault(fault(2, 50), 2, 160), Decision::Attempt);
        r.attempted(0, 3, 170, None);
        // RUNNING transitions alone never reset the budget.
        r.running(171);
        assert_eq!(r.consecutive(), 2);
        assert_eq!(r.fault(fault(3, 200), 3, 200), Decision::Wait);
        assert_eq!(r.fault(fault(3, 200), 3, 600), Decision::Attempt);
        r.attempted(0, 4, 610, None);
        // Budget exhausted: refused, settled for this epoch, never retried.
        assert_eq!(
            r.fault(fault(4, 700), 4, 700),
            Decision::Refuse(EXHAUSTED_STATUS)
        );
        assert_eq!(r.state(), EXHAUSTED);
        assert_eq!(r.settled(4), Some(EXHAUSTED_STATUS));
        let w = r.words(800, 1, 0);
        assert_eq!(
            (w[ATTEMPTS], w[RECOVERED_COUNT], w[EXHAUSTED_COUNT]),
            (3, 3, 1)
        );
        // A new client session restores the budget; counters persist.
        r.fresh_start(true, 1000);
        assert_eq!(r.settled(4), None);
        assert_eq!(r.fault(fault(5, 1100), 5, 1100), Decision::Attempt);
        r.attempted(0, 6, 1110, None);
        // A healthy run measured to the fault resets the budget.
        r.attempted(0, 7, 1200, None);
        r.attempted(0, 8, 1300, None);
        assert_eq!(r.consecutive(), 3);
        assert_eq!(r.fault(fault(8, 2400), 8, 2500), Decision::Attempt);
        assert_eq!(r.words(0, 0, 0)[HEALTHY_RESETS], 1);
        r.attempted(0, 9, 2510, None);
        r.running(3600);
        assert_eq!((r.consecutive(), r.state()), (0, HEALTHY));
        let w = r.words(3600, 1, 0);
        assert_eq!((w[ATTEMPTS], w[HEALTHY_RESETS]), (7, 2));
    }

    #[test]
    fn refusal_failed_attempt_and_cancel_are_visible_and_terminal() {
        let mut r = Recovery::new(POLICY);
        r.fresh_start(true, 0);
        assert_eq!(
            r.fault(Err(R_CLEANUP), 3, 5),
            Decision::Refuse(REFUSED_STATUS)
        );
        assert_eq!(
            r.refuse(R_CLEANUP, 3, REFUSED_STATUS),
            Decision::Refuse(REFUSED_STATUS)
        );
        let w = r.words(5, 1, 0);
        assert_eq!(
            (w[STATE], w[REFUSAL], w[REFUSED_COUNT]),
            (REFUSED, R_CLEANUP, 1)
        );
        r.fresh_start(true, 10);
        assert_eq!(
            r.fault(
                Ok(Fault {
                    epoch: 4,
                    reason: 55,
                    published_ms: 20
                }),
                4,
                20
            ),
            Decision::Attempt
        );
        r.attempted(-5, 5, 30, None);
        assert_eq!(r.settled(5), Some(REFUSED_STATUS));
        let w = r.words(30, 1, 0);
        assert_eq!(
            (w[STATE], w[REFUSAL], w[FAILED]),
            (REFUSED, R_ATTEMPT_FAILED, 1)
        );
        assert_eq!(w[LAST_STATUS] as i64, -5);
        // Pending -> cancelled by last Stop/detach.
        let mut r = Recovery::new(Policy {
            backoff_ms: [50, 50, 50],
            ..POLICY
        });
        assert_eq!(
            r.fault(
                Ok(Fault {
                    epoch: 1,
                    reason: 55,
                    published_ms: 0
                }),
                1,
                0
            ),
            Decision::Wait
        );
        assert_eq!(r.words(0, 1, 0)[TIME_FLAGS], T_PENDING);
        r.cancel();
        let w = r.words(1, 0, 0);
        assert_eq!(
            (w[STATE], w[CANCELLED_COUNT], w[TIME_FLAGS]),
            (CANCELLED, 1, 0)
        );
    }
    #[test]
    fn clean_failed_reopens_share_budget_backoff_and_never_count_as_recovered() {
        let mut r = Recovery::new(POLICY);
        r.fresh_start(true, 0);
        let f = |epoch| Fault {
            epoch,
            reason: 55,
            published_ms: epoch * 10,
        };
        assert_eq!(r.fault(Ok(f(1)), 1, 10), Decision::Attempt);
        r.attempted(-5, 2, 20, Some(f(2)));
        assert_eq!(r.settled(2), None);
        let w = r.words(20, 2, 0);
        assert_eq!(
            (w[STATE], w[FAILED], w[REFUSED_COUNT], w[FAULT_EPOCH]),
            (PENDING, 1, 0, 2)
        );
        assert_eq!(
            (w[PENDING_SINCE], w[DUE], w[TIME_FLAGS]),
            (20, 1020, T_PENDING)
        );
        assert_eq!(r.fault(Ok(f(2)), 2, 1019), Decision::Wait);
        assert_eq!(r.fault(Ok(f(2)), 2, 1020), Decision::Attempt);
        r.attempted(-5, 3, 1030, Some(f(3)));
        assert_eq!(r.words(1030, 2, 0)[DUE], 5030);
        r.running(100_000); // no RUNNING transition occurred, so no healthy reset
        assert_eq!(r.consecutive(), 2);
        assert_eq!(r.fault(Ok(f(3)), 3, 5029), Decision::Wait);
        assert_eq!(r.fault(Ok(f(3)), 3, 5030), Decision::Attempt);
        r.attempted(-5, 4, 5040, Some(f(4)));
        assert_eq!(r.settled(4), Some(EXHAUSTED_STATUS));
        let w = r.words(5040, 2, 0);
        assert_eq!(
            (
                w[STATE],
                w[ATTEMPTS],
                w[FAILED],
                w[RECOVERED_COUNT],
                w[EXHAUSTED_COUNT]
            ),
            (EXHAUSTED, 3, 3, 0, 1)
        );
        assert_eq!(w[TIME_FLAGS], 0);
        assert_eq!(w[REFUSED_COUNT], 0);
    }

    #[test]
    fn failed_reopen_requires_fresh_evidence_and_expected_launch_status() {
        for (status, retry) in [
            (-5, None),
            (
                -5,
                Some(Fault {
                    epoch: 1,
                    reason: 55,
                    published_ms: 10,
                }),
            ),
            (
                -3,
                Some(Fault {
                    epoch: 2,
                    reason: 55,
                    published_ms: 20,
                }),
            ),
            (
                -5,
                Some(Fault {
                    epoch: 2,
                    reason: 23,
                    published_ms: 20,
                }),
            ),
        ] {
            let mut r = Recovery::new(POLICY);
            assert_eq!(
                r.fault(
                    Ok(Fault {
                        epoch: 1,
                        reason: 55,
                        published_ms: 10
                    }),
                    1,
                    10
                ),
                Decision::Attempt
            );
            r.attempted(status, 2, 20, retry);
            assert_eq!(r.settled(2), Some(REFUSED_STATUS));
            let w = r.words(20, 1, 0);
            assert_eq!(
                (w[STATE], w[REFUSAL], w[FAILED], w[REFUSED_COUNT]),
                (REFUSED, R_ATTEMPT_FAILED, 1, 1)
            );
        }
    }
    #[test]
    fn failed_then_successful_restart_keeps_budget_until_actual_healthy_run() {
        let mut r = Recovery::new(POLICY);
        let f = |epoch, published_ms| Fault {
            epoch,
            reason: 55,
            published_ms,
        };
        assert_eq!(r.fault(Ok(f(1, 10)), 1, 10), Decision::Attempt);
        r.attempted(-5, 2, 20, Some(f(2, 20)));
        assert_eq!(r.fault(Ok(f(2, 20)), 2, 1020), Decision::Attempt);
        r.attempted(0, 3, 1030, None);
        assert_eq!(r.fault(Ok(f(3, 1040)), 3, 1040), Decision::Wait);
        assert_eq!(r.words(1040, 1, 0)[DUE], 5040);
        r.attempted(0, 4, 5050, None);
        r.running(35049);
        assert_eq!(r.consecutive(), 3);
        r.running(35050);
        assert_eq!(r.consecutive(), 0);
        assert_eq!(r.words(35050, 1, 0)[HEALTHY_RESETS], 1);
        assert_eq!(r.fault(Ok(f(4, 35060)), 4, 35060), Decision::Attempt);
        // Even a matching current record cannot reuse an epoch from before
        // this attempted launch (no newly opened session).
        r.attempted(-5, 4, 35070, Some(f(4, 35060)));
        assert_eq!(r.settled(4), Some(REFUSED_STATUS));
    }
}
