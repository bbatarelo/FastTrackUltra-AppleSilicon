// Closed-loop virtual USB/HAL fixture. Both contexts use the same atomics and
// bounded channel as the real-thread test, never trace-derived resubmit times.
use crate::clock::SampleClock;
use crate::fixed_replay::{self, Window};
use crate::independent_completion::{self as bridge, Slot, State, Ticket};
use crate::output_admission::OutputPolicy;
use crate::submission_order::SubmissionOrder;
const BLOCK: u64 = 4_000;
const COUNT: usize = 600;
const MAX_DEPTH: usize = 12;
const HISTORY: usize = 64;
const BYTES: usize = 32 * 312;
const NEVER: u64 = u64::MAX;
const ABSENCE: Window = Window::new(1_622_800, 1_643_300);
#[derive(Clone, Copy)]
struct Payload {
    c: Completion,
    bytes: [u8; BYTES],
    crc: u32,
}
impl Default for Payload {
    fn default() -> Self {
        Self {
            c: Completion::default(),
            bytes: [0; BYTES],
            crc: 0,
        }
    }
}
#[derive(Clone, Copy, Debug)]
struct Feedback {
    seq: usize,
    first: u64,
    c: Completion,
}
#[derive(Clone, Copy)]
struct NativeFlight {
    seq: usize,
    slot: usize,
    start: u64,
    ready: u64,
    ticket: Ticket,
    delivered: bool,
}
#[derive(Clone, Copy)]
struct CopyJob {
    direction: usize,
    flight: NativeFlight,
    data: Payload,
    end: u64,
}
#[derive(Clone, Copy)]
struct OwnerJob {
    prepared: Option<Payload>,
    direction: usize,
    flight: NativeFlight,
    end: u64,
}
#[derive(Clone, Copy)]
struct Held {
    flight: NativeFlight,
    c: Completion,
    published: bool,
}
#[derive(Clone, Copy)]
struct Config {
    owner_absence: Option<Window>,
    correlated: bool,
    submit: u64,
    copy: u64,
    owner_work: u64,
    handoff: u64,
    wake: u64,
    owner_wake: u64,
    callback_delay: u64,
    reordered: bool,
    hal_pause: bool,
    depth: usize,
    stop: Option<u64>,
    lost_abort: bool,
    limit: usize,
    owner_first: bool,
    hal_late: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            owner_absence: Some(ABSENCE),
            correlated: false,
            submit: 1000,
            copy: 100,
            owner_work: 25,
            handoff: 25,
            wake: 0,
            owner_wake: 0,
            callback_delay: 0,
            reordered: false,
            hal_pause: false,
            depth: 10,
            stop: None,
            lost_abort: false,
            limit: COUNT,
            owner_first: false,
            hal_late: false,
        }
    }
}
#[derive(Debug)]
struct Outcome {
    reason: Option<&'static str>,
    planned: usize,
    missing: u64,
    output_missing: u64,
    lead: u64,
    completed: [usize; 2],
    frames: [u64; 2],
    published: usize,
    during_absence: usize,
    ack_waits: usize,
    deferrals: usize,
    max_burst: usize,
    quarantine: bool,
    coverage: Option<u64>,
    first_out_check: Option<u64>,
    first_out_return: Option<u64>,
    first_out_start: Option<u64>,
    bound_requires_failure: Option<bool>,
    earliest_out_return: Option<u64>,
    failure_us: Option<u64>,
    early_callbacks: usize,
}
impl Outcome {
    fn accepted(&self) -> bool {
        self.reason.is_none()
            && self.missing == 0
            && self.output_missing == 0
            && self.lead >= 16
            && self.completed == [self.planned; 2]
            && self.frames[0] == self.frames[1]
            && self.published == self.planned
            && !self.quarantine
    }
}
struct Submitter {
    next: [usize; 2],
    previous: [[u64; MAX_DEPTH]; 2],
    feedback: [Option<Feedback>; HISTORY],
    counts: bridge::Consumer<Feedback, HISTORY>,
    at: u64,
    job: Option<OwnerJob>,
    burst: usize,
    last_end: [u64; 2],
    count_seq: usize,
    wait_work_until: u64,
}
struct Completer {
    counts: bridge::Producer<Feedback, HISTORY>,
    held: [Option<Held>; HISTORY],
    next: usize,
    frames: u64,
    at: u64,
    job: Option<CopyJob>,
    ack: [[u64; MAX_DEPTH]; 2],
    ack_ticket: [[Option<Ticket>; MAX_DEPTH]; 2],
    clock: SampleClock,
    io: AudioIo,
}
struct Fixture {
    cfg: Config,
    slots: [bridge::Arena<Payload>; 2],
    native: [[Option<NativeFlight>; MAX_DEPTH]; 2],
    owner: Submitter,
    completion: Completer,
    hal: PcmHal,
    now: u64,
    result: Outcome,
    sampled_absence: bool,
    retry_publication: u64,
}
fn ticket(seq: usize, direction: usize, slot: usize) -> Ticket {
    Ticket {
        key: (direction * MAX_DEPTH + slot) as u32,
        epoch: 1,
        generation: seq as u64 + 1,
    }
}
impl Fixture {
    fn new(buffer: usize, phase: u64, cfg: Config) -> Self {
        assert!((2..=MAX_DEPTH).contains(&cfg.depth));
        let transport = Transport::new(4, cfg.depth).unwrap();
        assert_eq!(
            (
                transport.entries(),
                transport.history(),
                transport.replay_ms(),
                transport.pcm_offset()
            ),
            (32, 64, 64, 3072)
        );
        assert_eq!((INPUT_SAFETY, OUTPUT_SAFETY), (1024, 2560));
        let (live, io) = placement_fixture_for(transport);
        let capture = AudioIo::with_geometry(
            live.shared.clone(),
            1,
            Info {
                timebase_numer: 125,
                timebase_denom: 3,
                ..Info::default()
            },
            transport,
        );
        let (producer, consumer) = bridge::channel();
        let wall = Arc::new(AtomicU64::new(100_000));
        let hal = PcmHal {
            live,
            io,
            wall,
            buffer,
            next: phase,
            write: vec![0.; buffer * 8],
            read: vec![0.; buffer * 8],
            steady_callbacks: 0,
            pause_start: phase + (72000_u64 - phase).div_ceil(buffer as u64) * buffer as u64,
            pause_frames: if cfg.hal_pause {
                buffer as u64 + 832
            } else {
                0
            },
            steady_input_missing: 0,
            steady_output_missing: 0,
            min_lead_ms: u64::MAX,
            min_steady_lead_ms: u64::MAX,
            work: crate::pool_simulation::PcmWorkload {
                steady_end_frames: cfg.limit as u64 * 192,
                hal_service_delay_us: u64::from(cfg.hal_late),
                ..Default::default()
            },
            stage_paid: false,
        };
        Self {
            cfg,
            slots: std::array::from_fn(|d| {
                bridge::Arena::new(
                    (0..cfg.depth)
                        .map(|slot| Slot::new(1, (d * MAX_DEPTH + slot) as u32, Payload::default()))
                        .collect(),
                )
            }),
            native: [[None; MAX_DEPTH]; 2],
            owner: Submitter {
                next: [0; 2],
                previous: [[0; MAX_DEPTH]; 2],
                feedback: [None; HISTORY],
                counts: consumer,
                at: 100_000,
                job: None,
                burst: 0,
                last_end: [116_000, 180_000],
                count_seq: 0,
                wait_work_until: 100_000,
            },
            completion: Completer {
                counts: producer,
                held: [None; HISTORY],
                next: 0,
                frames: 0,
                at: 100_000,
                job: None,
                ack: [[NEVER; MAX_DEPTH]; 2],
                ack_ticket: [[None; MAX_DEPTH]; 2],
                clock: SampleClock::new(1, 125, 3),
                io: capture,
            },
            hal,
            now: 100_000,
            result: Outcome {
                reason: None,
                planned: cfg.limit,
                missing: 0,
                output_missing: 0,
                lead: u64::MAX,
                completed: [0; 2],
                frames: [0; 2],
                published: 0,
                during_absence: 0,
                ack_waits: 0,
                deferrals: 0,
                max_burst: 0,
                quarantine: false,
                coverage: None,
                first_out_check: None,
                first_out_return: None,
                first_out_start: None,
                bound_requires_failure: None,
                earliest_out_return: None,
                failure_us: None,
                early_callbacks: 0,
            },
            sampled_absence: false,
            retry_publication: NEVER,
        }
    }
    fn fail(&mut self, reason: &'static str) {
        if self.result.reason.is_none() {
            self.result.reason = Some(reason);
            self.result.failure_us = Some(self.now);
        }
    }
    fn completion_window(&self) -> Option<Window> {
        if self.cfg.correlated {
            self.cfg.owner_absence
        } else {
            None
        }
    }
    fn notify_owner(&mut self) {
        if self.owner.job.is_none() {
            let at = fixed_replay::resume(
                (self.now + self.cfg.owner_wake).max(self.owner.wait_work_until),
                self.cfg.owner_absence,
            );
            self.owner.at = self.owner.at.min(at);
        }
    }
    fn observe_time(&mut self, next: u64) {
        assert!(next >= self.now);
        assert!(
            next >= self.hal.wall.load(SeqCst),
            "HAL cost escaped event clock"
        );
        if let Some(w) = self.cfg.owner_absence {
            if !self.sampled_absence && next >= w.start {
                let boundary = self.owner.last_end[1];
                self.result.coverage = Some(
                    boundary
                        .checked_sub(w.start)
                        .expect("queued OUT coverage expired before absence"),
                );
                // Necessary bound, frozen BEFORE observing post-stall execution.
                // Give OUT the earliest possible next call: remaining in-flight
                // work, then (only if needed) one OUT prepare + native submit.
                // No predicted success: eligibility, wake and earlier IN heads
                // can make the real return later than this optimistic lower bound.
                let earliest = match self.owner.job {
                    Some(j) => {
                        j.end.max(w.end)
                            + if j.prepared.is_some() {
                                self.cfg.submit
                            } else {
                                0
                            }
                            + if j.direction == 0 {
                                self.cfg.owner_work + self.cfg.submit
                            } else {
                                0
                            }
                    }
                    None => w.end + self.cfg.owner_work + self.cfg.submit,
                };
                self.result.earliest_out_return = Some(earliest);
                self.result.bound_requires_failure = Some(boundary / 1000 < earliest / 1000 + 16);
                self.sampled_absence = true;
            }
        }
        self.now = next;
        self.hal.wall.store(next, SeqCst);
        self.hal.service(); // HAL wins equality
    }
    fn counts_and_ack(&mut self) {
        while self.owner.feedback[self.owner.count_seq % HISTORY].is_none() {
            let Some(f) = self.owner.counts.pop() else {
                break;
            };
            if f.seq != self.owner.count_seq {
                self.fail("count_sequence_mismatch");
                return;
            }
            let at = f.seq % HISTORY;
            self.owner.feedback[at] = Some(f);
            self.owner.count_seq += 1;
            self.retry_publication = fixed_replay::resume(
                self.now
                    .max(self.completion.at)
                    .max(self.completion.job.map_or(0, |j| j.end)),
                self.completion_window(),
            );
        }
        for d in 0..2 {
            for s in 0..self.cfg.depth {
                let (g, state) = self.slots[d][s].state();
                if state == State::Released {
                    let t = self.native[d][s]
                        .expect("ACK without submitted ticket")
                        .ticket;
                    assert_eq!(g, t.generation);
                    assert!(self.slots[d][s].acknowledge(t));
                    self.native[d][s] = None;
                }
            }
        }
    }
    fn submit_turn(&mut self) {
        assert_eq!(
            fixed_replay::resume(self.now, self.cfg.owner_absence),
            self.now
        );
        self.counts_and_ack();
        if self.owner.burst >= crate::submission_order::CATCH_UP_ATTEMPTS {
            self.result.max_burst = self.result.max_burst.max(self.owner.burst);
            self.owner.burst = 0;
            self.owner.at = fixed_replay::resume(self.now + 1, self.cfg.owner_absence);
            return;
        }
        let mut starts = [None; 2];
        let mut free_slots = [None; 2];
        for (d, start) in starts.iter_mut().enumerate() {
            let n = self.owner.next[d];
            if n >= self.cfg.limit {
                continue;
            }
            free_slots[d] = self.slots[d]
                .iter()
                .position(|s| s.state().1 == State::Free);
            if free_slots[d].is_none() {
                self.result.ack_waits += 1;
                continue;
            }
            if d == 1 && self.owner.feedback[n % HISTORY].is_none_or(|f| f.seq != n) {
                continue;
            }
            *start = Some(116 + n as u64 * 4 + if d == 1 { 64 } else { 0 });
        }
        // A failed readiness probe cannot block an eligible IN head forever.
        let mut chosen = SubmissionOrder::Deadline.choose(starts);
        if chosen == Some(1) {
            let seq = self.owner.next[1];
            let f = self.owner.feedback[seq % HISTORY].unwrap();
            let frames = f.c.packets[..32]
                .iter()
                .map(|p| usize::from(p.actual / 24))
                .sum();
            let lead = starts[1].unwrap().saturating_sub(self.now / 1000);
            if lead < 16 {
                self.record_output_bound(starts[1].unwrap() * 1000, false);
                self.fail("output_lead_deadline");
                return;
            }
            let ready = self.hal.stage_output(f.first, frames).unwrap();
            if OutputPolicy::WhenReady.defer(ready, lead) {
                self.result.deferrals += 1;
                starts[1] = None;
                chosen = SubmissionOrder::Deadline.choose(starts);
            }
        }
        let Some(d) = chosen else {
            self.result.max_burst = self.result.max_burst.max(self.owner.burst);
            self.owner.burst = 0;
            self.owner.wait_work_until =
                fixed_replay::work(self.now, self.cfg.owner_work, self.cfg.owner_absence);
            self.owner.at =
                fixed_replay::resume(self.owner.wait_work_until + 8000, self.cfg.owner_absence);
            return;
        };
        let seq = self.owner.next[d];
        let slot = free_slots[d].unwrap();
        let start = starts[d].unwrap() * 1000;
        if d == 0 && start / 1000 < self.now / 1000 + 2 {
            self.fail("input_start_deadline");
            return;
        }
        let mut payload = Payload::default();
        if d == 0 {
            for p in &mut payload.c.packets[..32] {
                p.requested = 312;
            }
        } else {
            let f = self.owner.feedback[seq % HISTORY].unwrap();
            for (p, input) in payload.c.packets[..32].iter_mut().zip(&f.c.packets[..32]) {
                p.requested = input.actual;
            }
            let frames = payload.c.packets[..32]
                .iter()
                .map(|p| usize::from(p.requested / 24))
                .sum();
            self.hal
                .output(f.first, frames, &mut payload.bytes)
                .unwrap();
            payload.crc = crate::pool::crc32(&payload.bytes);
            self.owner.feedback[seq % HISTORY] = None;
        }
        let t = ticket(seq, d, slot);
        let ready = start
            + BLOCK
            + self.cfg.callback_delay
            + if self.cfg.reordered && seq.is_multiple_of(7) {
                5000
            } else {
                0
            };
        let flight = NativeFlight {
            seq,
            slot,
            start,
            ready,
            ticket: t,
            delivered: false,
        };
        let end = fixed_replay::work(self.now, self.cfg.owner_work, self.cfg.owner_absence);
        self.owner.job = Some(OwnerJob {
            prepared: Some(payload),
            direction: d,
            flight,
            end,
        });
        self.owner.at = NEVER;
        self.owner.next[d] += 1;
        self.owner.burst += 1;
        self.result.max_burst = self.result.max_burst.max(self.owner.burst);
    }
    fn submit_return(&mut self, job: OwnerJob) {
        assert_eq!(
            fixed_replay::resume(self.now, self.cfg.owner_absence),
            self.now
        );
        let d = job.direction;
        let f = job.flight;
        if let Some(payload) = job.prepared {
            if d == 0 && self.now > f.start {
                self.fail("input_return_deadline");
                return;
            }
            assert!(self.slots[d][f.slot]
                .prepare(self.owner.previous[d][f.slot], f.ticket, |p| *p = payload));
            self.owner.previous[d][f.slot] = f.ticket.generation;
            assert!(self.native[d][f.slot].is_none());
            self.native[d][f.slot] = Some(f);
            let cost = if self.now >= 1_616_000 {
                self.cfg.submit
            } else {
                0
            };
            self.owner.job = Some(OwnerJob {
                prepared: None,
                end: fixed_replay::work(self.now, cost, self.cfg.owner_absence),
                ..job
            });
            return;
        }
        self.owner.job = None;
        self.owner.at = self.now;
        self.owner.last_end[d] = f.start + BLOCK;
        if d == 0 {
            if self.now > f.start {
                self.fail("input_return_deadline");
            }
        } else {
            let lead = (f.start / 1000).saturating_sub(self.now / 1000);
            self.result.lead = self.result.lead.min(lead);
            self.record_output_bound(f.start, true);
            if lead < 16 {
                self.fail("output_lead_deadline");
            }
        }
    }
    fn record_output_bound(&mut self, start: u64, returned: bool) {
        if !self.sampled_absence || self.result.first_out_start.is_some() {
            return;
        }
        if returned {
            self.result.first_out_return = Some(self.now);
        } else {
            self.result.first_out_check = Some(self.now);
        }
        self.result.first_out_start = Some(start);
        let w = self.cfg.owner_absence.unwrap();
        let boundary = w.start + self.result.coverage.unwrap();
        assert_eq!(
            boundary, start,
            "first post-stall OUT differs from coverage frontier"
        );
        if returned {
            assert!(self.now >= self.result.earliest_out_return.unwrap());
        }
    }
    fn next_callback(&self) -> Option<(usize, NativeFlight, u64)> {
        if self.completion.job.is_some() {
            return None;
        }
        self.native
            .iter()
            .enumerate()
            .flat_map(|(d, slots)| {
                slots[..self.cfg.depth]
                    .iter()
                    .filter_map(move |f| f.filter(|f| !f.delivered).map(|f| (d, f)))
            })
            .map(|(d, f)| {
                (
                    d,
                    f,
                    fixed_replay::resume(
                        f.ready.max(self.completion.at) + self.cfg.wake,
                        self.completion_window(),
                    ),
                )
            })
            .min_by_key(|(d, f, at)| (*at, *d, f.seq))
    }
    fn callback(&mut self, d: usize, f: NativeFlight) {
        assert!(self.now >= f.ready);
        assert!(self.completion.job.is_none() && self.now >= self.completion.at);
        if self.owner.job.is_some_and(|j| {
            j.direction == d && j.flight.ticket == f.ticket && j.prepared.is_none()
        }) {
            self.result.early_callbacks += 1;
        }
        assert!(self.slots[d][f.slot].complete(f.ticket, |p| {
            p.c.status = 0;
            p.c.matched = 1;
            p.c.callback_ticks = self.now * 24;
            for (i, packet) in p.c.packets[..32].iter_mut().enumerate() {
                packet.actual = if d == 0 {
                    ([5, 7, 6, 6, 6, 6, 6, 6][i % 8] * 24) as u16
                } else {
                    packet.requested
                };
                packet.ticks = (f.start + (i as u64 + 1) * 125) * 24;
                if d == 0 {
                    for chunk in
                        p.bytes[i * 312..i * 312 + usize::from(packet.actual)].chunks_exact_mut(3)
                    {
                        chunk.copy_from_slice(&ftu_protocol::encode_s24(0.25));
                    }
                }
            }
        }));
        self.native[d][f.slot].as_mut().unwrap().delivered = true;
        let data = self.slots[d][f.slot].claim(f.ticket).unwrap();
        let cost = 50 + if d == 0 { self.cfg.copy } else { 0 };
        let end = fixed_replay::work(self.now, cost, self.completion_window());
        self.completion.job = Some(CopyJob {
            direction: d,
            flight: f,
            data,
            end,
        });
    }
    fn release_later(&mut self, d: usize, f: NativeFlight) {
        assert!(self.slots[d][f.slot].publish(f.ticket));
        self.completion.at = fixed_replay::work(
            self.completion.at.max(self.now),
            self.cfg.handoff,
            self.completion_window(),
        );
        assert!(self.completion.ack_ticket[d][f.slot].is_none());
        self.completion.ack_ticket[d][f.slot] = Some(f.ticket);
        self.completion.ack[d][f.slot] = self.completion.at;
    }
    fn publish_ready(&mut self) {
        loop {
            let seq = self.completion.next;
            let at = seq % HISTORY;
            let Some(mut held) = self.completion.held[at] else {
                break;
            };
            if held.flight.seq != seq {
                self.fail("capture_order_overrun");
                return;
            }
            let first = self.completion.frames;
            if !held.published {
                self.completion.io.ordered(seq, first, &held.c).unwrap();
                let mut cursor = first;
                for group in held.c.packets[..32].chunks_exact(8) {
                    self.completion
                        .clock
                        .observe_usb_group(1, cursor, &std::array::from_fn(|i| group[i].ticks))
                        .unwrap();
                    self.completion
                        .io
                        .clock(self.completion.clock.snapshot())
                        .unwrap();
                    cursor += group.iter().map(|p| u64::from(p.actual / 24)).sum::<u64>();
                }
                held.published = true;
                self.completion.held[at] = Some(held);
            }
            let f = Feedback {
                seq,
                first,
                c: held.c,
            };
            if self.completion.counts.push(f).is_err() {
                break;
            } // retain held slot: bounded backpressure
            self.completion.frames += held.c.packets[..32]
                .iter()
                .map(|p| u64::from(p.actual / 24))
                .sum::<u64>();
            self.completion.next += 1;
            self.completion.held[at] = None;
            self.result.published += 1;
            if self
                .cfg
                .owner_absence
                .is_some_and(|w| (w.start..w.end).contains(&self.now))
            {
                self.result.during_absence += 1;
            }
            self.release_later(0, held.flight);
            self.notify_owner();
        }
    }
    fn copy_done(&mut self, job: CopyJob) {
        self.completion.job = None;
        self.completion.at = self.completion.at.max(self.now);
        let d = job.direction;
        let f = job.flight;
        let p = job.data;
        assert_eq!(p.c.matched, 1);
        assert!(p.c.packets[..32].iter().all(|v| v.status == 0
            && crate::scheduler::valid_length(v.actual)
            && (d == 0 || v.actual == v.requested)));
        assert!(p.c.packets[32..]
            .iter()
            .all(|v| v.actual == 0 && v.requested == 0));
        self.result.completed[d] += 1;
        self.result.frames[d] += p.c.packets[..32]
            .iter()
            .map(|v| u64::from(v.actual / 24))
            .sum::<u64>();
        if d == 1 {
            assert_eq!(crate::pool::crc32(&p.bytes), p.crc);
            self.release_later(d, f);
        } else {
            self.completion.io.capture(f.seq, &p.c, &p.bytes).unwrap();
            let at = f.seq % HISTORY;
            assert!(self.completion.held[at].is_none());
            self.completion.held[at] = Some(Held {
                flight: f,
                c: p.c,
                published: false,
            });
            self.publish_ready();
        }
    }
    fn completion_events(&mut self, next: u64, complete: u64) {
        for d in 0..2 {
            for s in 0..self.cfg.depth {
                if self.completion.ack[d][s] == next {
                    let t = self.completion.ack_ticket[d][s].take().unwrap();
                    assert_eq!(self.slots[d][s].state().0, t.generation);
                    assert!(self.slots[d][s].release(t));
                    self.completion.ack[d][s] = NEVER;
                    self.notify_owner();
                }
            }
        }
        if complete == next {
            self.copy_done(self.completion.job.unwrap());
        }
        if self.retry_publication == next {
            let available = self
                .completion
                .at
                .max(self.completion.job.map_or(0, |j| j.end));
            if available > next || self.completion.job.is_some() {
                self.retry_publication =
                    fixed_replay::resume(available.max(next + 1), self.completion_window());
            } else {
                self.retry_publication = NEVER;
                self.publish_ready();
            }
        }
        if let Some((d, f, at)) = self.next_callback() {
            if at == next {
                self.callback(d, f);
            }
        }
    }
    fn drain(&mut self) {
        // Stop admission, wait for already-entered owner/copy work and paid
        // handoffs to finish, then cancel remaining local preparation. No new
        // native call may start. Abort callbacks below are scripted; native
        // quiescence/abort latency still needs a separate boundary qualification.
        let quiescent = self
            .now
            .max(self.owner.job.map_or(0, |j| j.end))
            .max(self.completion.job.map_or(0, |j| j.end))
            .max(self.completion.at);
        self.observe_time(quiescent);
        self.owner.job = None;
        self.completion.job = None;
        for d in 0..2 {
            for (s, slot) in self.slots[d].iter().enumerate() {
                let (g, mut state) = slot.state();
                let t = Ticket {
                    key: (d * MAX_DEPTH + s) as u32,
                    epoch: 1,
                    generation: g,
                };
                if state == State::Submitted && !self.cfg.lost_abort {
                    assert!(slot.complete(t, |p| p.c.status = -1));
                    state = State::Complete;
                }
                if state == State::Complete {
                    assert!(slot.claim(t).is_some());
                    state = State::Claimed;
                }
                if state == State::Claimed {
                    assert!(slot.publish(t));
                    state = State::Published;
                }
                if state == State::Published {
                    assert!(slot.release(t));
                    state = State::Released;
                }
                if state == State::Released {
                    assert!(slot.acknowledge(t));
                }
            }
        }
        self.result.quarantine = false;
        for arena in &mut self.slots {
            self.result.quarantine |= !arena.finish(true);
        }
    }
    fn run(mut self) -> Outcome {
        assert_eq!((self.hal.work.prepare_us, self.hal.work.capture_us), (0, 0));
        crate::pool_simulation::without_allocations(|| {
            for _ in 0..1_000_000 {
                if self.result.reason.is_some() {
                    break;
                }
                if self.result.completed == [self.cfg.limit; 2]
                    && self.completion.next == self.cfg.limit
                {
                    break;
                }
                let callback = self.next_callback();
                let ack = self.completion.ack.iter().flatten().copied().min().unwrap();
                let owner = self.owner.job.map_or(self.owner.at, |j| j.end);
                let complete = self.completion.job.map_or(NEVER, |j| j.end);
                let next = owner
                    .min(complete)
                    .min(ack)
                    .min(callback.map_or(NEVER, |(_, _, at)| at))
                    .min(self.cfg.stop.unwrap_or(NEVER))
                    .min(self.retry_publication);
                assert!(next != NEVER && next < 60_000_000, "virtual time stalled");
                self.observe_time(next);
                if self.cfg.stop == Some(next) {
                    self.fail("stop");
                    break;
                }
                // HAL reads win equality. Sweep owner/completion order separately.
                if owner == next && self.cfg.owner_first {
                    if let Some(job) = self.owner.job {
                        self.submit_return(job);
                    } else {
                        self.submit_turn();
                    }
                }
                self.completion_events(next, complete);
                if owner == next && !self.cfg.owner_first {
                    if let Some(job) = self.owner.job {
                        self.submit_return(job);
                    } else {
                        self.submit_turn();
                    }
                }
            }
            if self.result.reason.is_none() {
                assert_eq!(self.result.completed, [self.cfg.limit; 2]);
                assert!(self.completion.clock.snapshot().valid);
                assert_eq!(self.completion.frames, self.result.frames[0]);
            }
            self.result.missing = self.hal.steady_input_missing;
            self.result.output_missing = self.hal.steady_output_missing;
            self.drain();
        });
        self.result
    }
}
fn execute(buffer: usize, phase: u64, cfg: Config) -> Outcome {
    Fixture::new(buffer, phase, cfg).run()
}
#[test]
fn independent_joint_smoke() {
    for cfg in [
        Config {
            owner_absence: None,
            submit: 0,
            ..Default::default()
        },
        Config::default(),
    ] {
        let result = execute(64, 7, cfg);
        println!("INDEPENDENT_SMOKE {result:?}");
        assert!(!result.quarantine);
        assert!(result.max_burst <= 10);
        if cfg.owner_absence.is_none() {
            assert!(result.accepted());
        }
    }
}

#[test]
fn independent_joint_phase_and_offset_screen() {
    for depth in [10, 12] {
        for buffer in [64, 128, 256, 512, 1024, 4096] {
            let mut accepted = 0;
            let mut faults = 0;
            let mut missing = 0;
            let mut out_missing = 0;
            let mut min_lead = u64::MAX;
            let mut covered = 0;
            let mut classified = 0;
            for phase in 0..64 {
                for offset in [0, 500, 1500, 2500] {
                    let cfg = Config {
                        depth,
                        owner_absence: Some(Window::new(
                            ABSENCE.start + offset,
                            ABSENCE.end + offset,
                        )),
                        ..Default::default()
                    };
                    let r = execute(buffer, phase, cfg);
                    accepted += usize::from(r.accepted());
                    faults += usize::from(r.reason.is_some());
                    missing += r.missing;
                    out_missing += r.output_missing;
                    min_lead = min_lead.min(r.lead);
                    covered += usize::from(r.during_absence > 0);
                    if let Some(prediction) = r.bound_requires_failure {
                        if prediction {
                            assert_eq!(r.reason, Some("output_lead_deadline"));
                        }
                        classified += 1;
                    }
                    assert!(
                        matches!(r.reason, None | Some("output_lead_deadline")),
                        "unexpected fault: {r:?}"
                    );
                    assert!(r.during_absence > 0);
                    assert_eq!(r.missing, 0, "capture failed: {r:?}");
                    assert_eq!(r.output_missing, 0, "output packing failed: {r:?}");
                    assert!(!r.quarantine);
                    assert!(r.max_burst <= 10);
                }
            }
            println!("JOINT_PHASE depth={depth} buffer={buffer} cases=256 accepted={accepted} faults={faults} input_missing={missing} output_missing={out_missing} min_lead={min_lead} independent_progress={covered} classified={classified}");
            assert_eq!(classified, 256);
            if depth == 10 {
                assert!(faults > 0);
            } else {
                assert_eq!(accepted, 256);
            }
        }
    }
}
#[test]
fn independent_joint_dense_offsets_and_negative_controls() {
    for depth in [10, 12] {
        for duration in [12500, 18000, 20500, 24000, 32000] {
            let mut accepted = 0;
            let mut faults = 0;
            let mut classified = 0;
            let mut min_lead = u64::MAX;
            for offset in 0..32 {
                let start = ABSENCE.start + offset * 125;
                let r = execute(
                    64,
                    7,
                    Config {
                        depth,
                        owner_absence: Some(Window::new(start, start + duration)),
                        ..Default::default()
                    },
                );
                accepted += usize::from(r.accepted());
                faults += usize::from(r.reason.is_some());
                min_lead = min_lead.min(r.lead);
                if let Some(prediction) = r.bound_requires_failure {
                    if prediction {
                        assert_eq!(r.reason, Some("output_lead_deadline"));
                    }
                    classified += 1;
                }
                println!("JOINT_BOUND depth={depth} duration={duration} offset={offset} coverage={:?} returned={:?} start={:?} predicted={:?} reason={:?}",r.coverage,r.first_out_return,r.first_out_start,r.bound_requires_failure,r.reason);
                if duration == 32000 {
                    assert!(!r.accepted());
                }
            }
            println!("JOINT_OFFSET depth={depth} duration={duration} cases=32 accepted={accepted} faults={faults} min_lead={min_lead} classified={classified}");
        }
    }
    for depth in [10, 12] {
        for phase in [0, 7, 31, 63] {
            let r = execute(
                64,
                phase,
                Config {
                    depth,
                    owner_absence: None,
                    submit: 2000,
                    limit: 6000,
                    ..Default::default()
                },
            );
            assert_eq!(r.reason, Some("output_lead_deadline"));
            assert!(r.failure_us.unwrap() > 116_000 + 600 * BLOCK);
            assert!(!r.accepted());
            println!("JOINT_SATURATION depth={depth} phase={phase} result={r:?}");
        }
    }
}
#[test]
fn independent_joint_cost_delay_and_hal_disturbance_screen() {
    for depth in [10, 12] {
        for case in 0..10 {
            let mut accepted = 0;
            let mut input = 0;
            let mut output = 0;
            let mut faults = 0;
            for buffer in [64, 512, 4096] {
                for phase in [0, 7, 31, 63] {
                    let mut cfg = Config {
                        depth,
                        ..Default::default()
                    };
                    match case {
                        0 => {
                            cfg.copy = 250;
                            cfg.handoff = 100;
                            cfg.owner_work = 100;
                        }
                        1 => {
                            cfg.wake = 250;
                            cfg.owner_wake = 250;
                        }
                        2 => {
                            cfg.callback_delay = 5000;
                            cfg.reordered = false;
                        }
                        3 => {
                            cfg.hal_pause = true;
                        }
                        4 => {
                            cfg.correlated = true;
                        }
                        5 => {
                            cfg.copy = 4000;
                        }
                        6 => {
                            cfg.reordered = true;
                        }
                        7 => {
                            cfg.callback_delay = 5000;
                            cfg.reordered = true;
                        }
                        8 => {
                            cfg.owner_first = true;
                        }
                        _ => {
                            cfg.hal_late = true;
                        }
                    }
                    let r = execute(buffer, phase, cfg);
                    accepted += usize::from(r.accepted());
                    input += r.missing;
                    output += r.output_missing;
                    faults += usize::from(r.reason.is_some());
                    assert!(!r.quarantine);
                    if case == 4 && buffer == 64 {
                        assert!(!r.accepted());
                    }
                    if case == 5 {
                        assert!(!r.accepted());
                    }
                }
            }
            println!("JOINT_COST depth={depth} case={case} cases=12 accepted={accepted} faults={faults} input_missing={input} output_missing={output}");
        }
    }
}
#[test]
fn independent_joint_stop_and_lost_abort_are_distinct() {
    for stop in [100010, 120010, 150000, 1624000, 1643325] {
        for lost_abort in [false, true] {
            let r = execute(
                64,
                7,
                Config {
                    stop: Some(stop),
                    lost_abort,
                    ..Default::default()
                },
            );
            assert_eq!(r.reason, Some("stop"));
            // At the first stop only local preparation exists, not USB ownership.
            assert_eq!(r.quarantine, lost_abort && stop > 100010);
        }
    }
}
#[test]
fn independent_count_backpressure_holds_slot_until_record_can_be_sent() {
    let mut f = Fixture::new(64, 7, Config::default());
    let mut p = Payload::default();
    for packet in &mut p.c.packets[..32] {
        packet.actual = 144;
        packet.requested = 312;
    }
    for (i, packet) in p.c.packets[..32].iter_mut().enumerate() {
        packet.ticks = (116000 + (i as u64 + 1) * 125) * 24;
    }
    let flight = NativeFlight {
        seq: 0,
        slot: 0,
        start: 116000,
        ready: 120000,
        ticket: ticket(0, 0, 0),
        delivered: true,
    };
    let slot = &f.slots[0][0];
    assert!(slot.prepare(0, flight.ticket, |v| *v = p));
    assert!(slot.complete(flight.ticket, |_| {}));
    assert!(slot.claim(flight.ticket).is_some());
    f.completion.io.capture(0, &p.c, &p.bytes).unwrap();
    f.completion.held[0] = Some(Held {
        flight,
        c: p.c,
        published: false,
    });
    for seq in 0..HISTORY {
        f.completion
            .counts
            .push(Feedback {
                seq,
                first: 0,
                c: p.c,
            })
            .unwrap();
    }
    f.observe_time(120150);
    f.publish_ready();
    assert!(f.completion.held[0].unwrap().published);
    assert_eq!(f.slots[0][0].state().1, State::Claimed);
    assert_eq!(f.completion.ack[0][0], NEVER);
    for _ in 0..HISTORY {
        assert!(f.owner.counts.pop().is_some());
    }
    f.publish_ready();
    assert!(f.completion.held[0].is_none());
    assert_eq!(f.slots[0][0].state().1, State::Published);
    assert_eq!(f.owner.counts.pop().unwrap().seq, 0);
    f.drain();
}
#[test]
fn independent_joint_delay_witnesses() {
    for depth in [10, 12] {
        for (delay, reordered) in [(0, false), (5000, false), (0, true), (5000, true)] {
            let r = execute(
                64,
                7,
                Config {
                    depth,
                    callback_delay: delay,
                    reordered,
                    ..Default::default()
                },
            );
            println!("JOINT_DELAY depth={depth} delay={delay} reorder={reordered} result={r:?}");
        }
    }
}

#[test]
fn independent_joint_delayed_callback_offsets() {
    for depth in [10, 12] {
        let mut accepted = 0;
        let mut faults = 0;
        let mut min_lead = u64::MAX;
        for buffer in [64, 512, 4096] {
            for phase in [0, 7, 31, 63] {
                for offset in 0..32 {
                    let start = ABSENCE.start + 125 * offset;
                    let r = execute(
                        buffer,
                        phase,
                        Config {
                            depth,
                            callback_delay: 5000,
                            owner_absence: Some(Window::new(start, start + 20500)),
                            ..Default::default()
                        },
                    );
                    accepted += usize::from(r.accepted());
                    faults += usize::from(r.reason.is_some());
                    min_lead = min_lead.min(r.lead);
                    assert_eq!(r.missing, 0);
                    assert_eq!(r.output_missing, 0);
                    assert!(!r.quarantine);
                }
            }
        }
        println!("JOINT_DELAY_OFFSETS depth={depth} cases=384 accepted={accepted} faults={faults} min_lead={min_lead}");
    }
}

#[test]
fn independent_publication_retry_cannot_overlap_copy_or_handoff() {
    let mut f = Fixture::new(
        64,
        7,
        Config {
            owner_absence: None,
            ..Default::default()
        },
    );
    for seq in 0..2 {
        let mut data = Payload::default();
        data.c.matched = 1;
        for (i, p) in data.c.packets[..32].iter_mut().enumerate() {
            p.requested = 312;
            p.actual = 144;
            p.ticks = (116000 + seq as u64 * 4000 + (i as u64 + 1) * 125) * 24;
        }
        let flight = NativeFlight {
            seq,
            slot: seq,
            start: 116000 + seq as u64 * 4000,
            ready: 120000 + seq as u64 * 4000,
            ticket: ticket(seq, 0, seq),
            delivered: true,
        };
        let slot = &f.slots[0][seq];
        assert!(slot.prepare(0, flight.ticket, |p| *p = data));
        assert!(slot.complete(flight.ticket, |_| {}));
        assert!(slot.claim(flight.ticket).is_some());
        if seq == 0 {
            f.completion.io.capture(seq, &data.c, &data.bytes).unwrap();
            f.completion.held[seq] = Some(Held {
                flight,
                c: data.c,
                published: false,
            });
        } else {
            f.completion.job = Some(CopyJob {
                direction: 0,
                flight,
                data,
                end: 124150,
            });
        }
    }
    f.retry_publication = 124150;
    f.observe_time(124150);
    f.completion_events(124150, 124150);
    assert_eq!(f.completion.at, 124200);
    assert_eq!(f.completion.ack[0][0], 124175);
    assert_eq!(f.completion.ack[0][1], 124200);
    assert_eq!(f.retry_publication, 124200);
    assert_eq!(f.result.published, 2);
    f.drain();
    assert!(!f.result.quarantine);
}
#[test]
fn independent_owner_backpressure_does_not_pop_an_occupied_count_cell() {
    let mut f = Fixture::new(64, 7, Config::default());
    let count = Feedback {
        seq: 0,
        first: 0,
        c: Completion::default(),
    };
    f.owner.feedback[0] = Some(count);
    f.completion.counts.push(count).unwrap();
    f.counts_and_ack();
    assert_eq!(f.owner.count_seq, 0);
    f.owner.feedback[0] = None;
    f.counts_and_ack();
    assert_eq!(f.owner.count_seq, 1);
    assert!(f.owner.counts.pop().is_none());
    f.drain();
}

#[test]
fn independent_callback_can_finish_before_submit_returns() {
    let r = execute(
        64,
        7,
        Config {
            owner_absence: None,
            submit: 50000,
            ..Default::default()
        },
    );
    assert!(r.early_callbacks > 0, "{r:?}");
    assert_eq!(r.reason, Some("input_return_deadline"));
    assert!(!r.quarantine);
    println!("JOINT_EARLY_CALLBACK result={r:?}");
}
