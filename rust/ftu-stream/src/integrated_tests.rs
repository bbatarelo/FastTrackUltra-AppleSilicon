// Linked offline integration. Actual native router and CFRunLoop thread; virtual
// service costs. The original independent_tests.rs remains the frozen reference.
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
    event: NativeEvent,
    admission: Admission,
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
    token: u64,
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
    prefill_counts: bool,
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
            prefill_counts: false,
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
    returns: bridge::Consumer<Return, 32>,
    at: u64,
    job: Option<OwnerJob>,
    burst: usize,
    last_end: [u64; 2],
    count_seq: usize,
    wait_work_until: u64,
}

use std::sync::atomic::Ordering::{Acquire, Release};
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
};
#[repr(C)]
#[derive(Clone, Copy)]
struct NativeEvent {
    token: u64,
    ticket: u64,
    logical_us: u64,
    direction: u32,
    slot: u32,
    entries: u32,
    reserved: u32,
    result: Completion,
    data: *const u8,
    bytes: u64,
}
#[repr(C)]
struct Hooks {
    context: *mut c_void,
    complete: extern "C" fn(*mut c_void, *mut c_void, *const NativeEvent),
    poll: extern "C" fn(*mut c_void, *mut c_void, u64, u32),
}
unsafe extern "C" {
    fn ftu_fixture_create(slots: u32, hooks: *const Hooks) -> *mut c_void;
    fn ftu_fixture_buffer(f: *mut c_void, d: u32, s: u32) -> *mut u8;
    fn ftu_fixture_submit(
        f: *mut c_void,
        d: u32,
        s: u32,
        t: u64,
        start: u64,
        lengths: *const u16,
        token: *mut u64,
    ) -> i32;
    fn ftu_fixture_command(f: *mut c_void, now: u64, kind: u32, d: u32, s: u32) -> i32;
    fn ftu_fixture_ack(f: *mut c_void, d: u32, s: u32, token: u64) -> i32;
    fn ftu_fixture_release(route: *mut c_void, token: u64) -> i32;
    fn ftu_fixture_fault(f: *mut c_void, fault: u32);
    fn ftu_fixture_clock(f: *mut c_void, ppm: i32);
    fn ftu_fixture_real_time(f: *mut c_void, base: u64);
    fn ftu_integrated_allocations() -> u64;
    fn ftu_fixture_stop(f: *mut c_void, timeout: u32) -> i32;
    fn ftu_fixture_destroy(f: *mut c_void) -> i32;
    fn ftu_fixture_stat(f: *mut c_void, key: u32) -> u64;
}
#[derive(Clone, Copy, Default)]
struct Admission {
    seq: usize,
    start: u64,
    crc: u32,
}
#[derive(Clone, Copy, Debug)]
struct Return {
    ticket: Ticket,
    token: u64,
}
// All fields are atomic: the version check never hides a non-atomic data race.
// Commands have an acquire/release dispatch barrier; the version also protects
// readers while the physical shutdown timer is running.
struct Summary {
    version: AtomicU64,
    fields: [AtomicU64; 14],
}
impl Summary {
    fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            fields: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    fn store(&self, v: [u64; 14]) {
        self.version.fetch_add(1, SeqCst);
        for (dst, val) in self.fields.iter().zip(v) {
            dst.store(val, SeqCst);
        }
        self.version.fetch_add(1, SeqCst);
    }
    fn load(&self) -> [u64; 14] {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let before = self.version.load(SeqCst);
            if !before.is_multiple_of(2) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "summary writer failed to quiesce"
                );
                std::thread::yield_now();
                continue;
            }
            let v = std::array::from_fn(|i| self.fields[i].load(SeqCst));
            if before == self.version.load(SeqCst) {
                return v;
            }
        }
    }
}
struct Link {
    slots: [Box<[Slot<Admission>]>; 2],
    summary: Summary,
    fault: AtomicU64,
    error: AtomicU64,
    stopping: AtomicBool,
    retry: AtomicU64,
}
struct Context {
    link: Arc<Link>,
    state: UnsafeCell<Completer>,
}
// Only C's single completion thread enters these hooks. The owner never makes
// a reference into state. On a panic, all further access stops and ownership is
// retained. Native is authoritative for data/frame-list lifetime.
extern "C" fn completed(ctx: *mut c_void, route: *mut c_void, event: *const NativeEvent) {
    guarded(ctx, |s| s.receive(route, unsafe { *event }));
}
extern "C" fn polled(ctx: *mut c_void, route: *mut c_void, now: u64, stopping: u32) {
    guarded(ctx, |s| s.poll(route, now, stopping != 0));
}
fn guarded(ctx: *mut c_void, f: impl FnOnce(&mut Completer)) {
    let c = unsafe { &*(ctx.cast::<Context>()) };
    if c.link.fault.load(Acquire) != 0 {
        return;
    }
    if catch_unwind(AssertUnwindSafe(|| {
        crate::pool_simulation::without_allocations(|| f(unsafe { &mut *c.state.get() }));
    }))
    .is_err()
    {
        // Fault-path hooks may allocate; the zero-allocation gate applies only
        // to successful streaming. Unwind never crosses the C ABI.
        c.link.fault.store(1, Release);
    }
}
struct Completer {
    cfg: Config,
    link: Arc<Link>,
    counts: bridge::Producer<Feedback, HISTORY>,
    returns: bridge::Producer<Return, 32>,
    held: [Option<Held>; HISTORY],
    next: usize,
    frames: u64,
    at: u64,
    job: Option<CopyJob>,
    ack: [[u64; MAX_DEPTH]; 2],
    tickets: [[Option<Ticket>; MAX_DEPTH]; 2],
    tokens: [[u64; MAX_DEPTH]; 2],
    clock: SampleClock,
    io: AudioIo,
    completed: [usize; 2],
    totals: [u64; 2],
    during: usize,
    notifications: u64,
    now: u64,
    stopping: bool,
}
impl Completer {
    fn window(&self) -> Option<Window> {
        if self.cfg.correlated {
            self.cfg.owner_absence
        } else {
            None
        }
    }
    fn report(&self) {
        let due = self
            .ack
            .iter()
            .flatten()
            .copied()
            .min()
            .unwrap()
            .min(self.job.map_or(NEVER, |j| j.end));
        let fit = self.clock.snapshot();
        self.link.summary.store([
            self.at,
            self.job.map_or(NEVER, |j| j.end),
            due,
            self.completed[0] as u64,
            self.completed[1] as u64,
            self.totals[0],
            self.totals[1],
            self.next as u64,
            self.during as u64,
            u64::from(fit.valid),
            self.notifications,
            self.frames,
            u64::from(self.stopping),
            self.io.shared.captured.load(SeqCst),
        ]);
    }
    fn receive(&mut self, route: *mut c_void, e: NativeEvent) {
        let d = e.direction as usize;
        let slot = e.slot as usize;
        assert!(
            d < 2
                && slot < self.cfg.depth
                && e.entries == 32
                && e.bytes == BYTES as u64
                && !e.data.is_null()
        );
        let t = Ticket {
            key: (d * MAX_DEPTH + slot) as u32,
            epoch: 1,
            generation: e.ticket,
        };
        assert!(self.link.slots[d][slot].complete(t, |_| {}));
        let a = self.link.slots[d][slot].claim(t).unwrap();
        assert_eq!(a.seq as u64 + 1, e.ticket);
        assert_eq!(
            self.tokens[d][slot], 0,
            "native slot reused without release"
        );
        self.tokens[d][slot] = e.token;
        self.tickets[d][slot] = Some(t);
        self.stopping |= self.link.stopping.load(Acquire);
        if e.result.status != 0 || self.stopping {
            if e.result.status != 0 && !self.stopping {
                self.link.error.store(1, Release);
            }
            // Abort callbacks are ownership returns, never capture publication.
            assert!(self.link.slots[d][slot].publish(t));
            self.release(route, d, slot, false);
            return;
        }
        assert!(self.job.is_none() && e.logical_us >= self.at);
        self.now = e.logical_us;
        self.job = Some(CopyJob {
            event: e,
            admission: a,
            end: fixed_replay::work(
                self.now,
                50 + if d == 0 { self.cfg.copy } else { 0 },
                self.window(),
            ),
        });
    }
    fn release(&mut self, route: *mut c_void, d: usize, s: usize, notify: bool) {
        let t = self.tickets[d][s].unwrap();
        let token = self.tokens[d][s];
        assert_eq!(unsafe { ftu_fixture_release(route, token) }, 1);
        assert!(self.link.slots[d][s].release(t));
        // One combined return record, after both releases. Capacity32 >= all24
        // native slots. A record cannot be regenerated before its owner ACK.
        if notify {
            self.returns.push(Return { ticket: t, token }).unwrap();
            self.notifications += 1;
        }
        self.tickets[d][s] = None;
        self.tokens[d][s] = 0;
        self.ack[d][s] = NEVER;
    }
    fn later(&mut self, d: usize, f: NativeFlight) {
        assert!(self.link.slots[d][f.slot].publish(f.ticket));
        self.at = fixed_replay::work(self.at.max(self.now), self.cfg.handoff, self.window());
        self.ack[d][f.slot] = self.at;
    }
    fn publish(&mut self) {
        loop {
            let seq = self.next;
            let at = seq % HISTORY;
            let Some(mut held) = self.held[at] else { break };
            assert_eq!(held.flight.seq, seq);
            let first = self.frames;
            if !held.published {
                self.io.ordered(seq, first, &held.c).unwrap();
                let mut cursor = first;
                for group in held.c.packets[..32].chunks_exact(8) {
                    self.clock
                        .observe_usb_group(1, cursor, &std::array::from_fn(|i| group[i].ticks))
                        .unwrap();
                    self.io.clock(self.clock.snapshot()).unwrap();
                    cursor += group.iter().map(|p| u64::from(p.actual / 24)).sum::<u64>();
                }
                held.published = true;
                self.held[at] = Some(held);
            }
            if self
                .counts
                .push(Feedback {
                    seq,
                    first,
                    c: held.c,
                })
                .is_err()
            {
                break;
            }
            self.frames += held.c.packets[..32]
                .iter()
                .map(|p| u64::from(p.actual / 24))
                .sum::<u64>();
            self.next += 1;
            self.held[at] = None;
            assert_eq!(self.tokens[0][held.flight.slot], held.token);
            if self
                .cfg
                .owner_absence
                .is_some_and(|w| (w.start..w.end).contains(&self.now))
            {
                self.during += 1;
            }
            self.later(0, held.flight);
            self.notifications += 1;
        }
    }
    fn copied(&mut self, job: CopyJob) {
        self.job = None;
        self.at = self.at.max(self.now);
        let e = job.event;
        let a = job.admission;
        let d = e.direction as usize;
        let slot = e.slot as usize;
        let c = e.result;
        assert_eq!(c.matched, 1);
        assert!(c.packets[..32].iter().all(|p| p.status == 0
            && crate::scheduler::valid_length(p.actual)
            && p.actual <= p.requested
            && (d == 0 || p.actual == p.requested)));
        assert!(c.packets[32..]
            .iter()
            .all(|p| p.actual == 0 && p.requested == 0));
        // This borrow is valid only while native HELD; no pointer is exposed to
        // the owner, and release happens after copy/publication/clock/count.
        let data = unsafe { std::slice::from_raw_parts(e.data, BYTES) };
        self.completed[d] += 1;
        self.totals[d] += c.packets[..32]
            .iter()
            .map(|p| u64::from(p.actual / 24))
            .sum::<u64>();
        let f = NativeFlight {
            seq: a.seq,
            slot,
            start: a.start,
            ready: 0,
            ticket: self.tickets[d][slot].unwrap(),
            delivered: true,
        };
        if d == 1 {
            assert_eq!(
                crate::pool::crc32(data),
                a.crc,
                "pending OUT buffer mutated"
            );
            self.later(d, f);
        } else {
            self.io.capture(a.seq, &c, data).unwrap();
            assert!(self.held[a.seq % HISTORY].is_none());
            self.held[a.seq % HISTORY] = Some(Held {
                flight: f,
                c,
                published: false,
                token: e.token,
            });
            self.publish();
        }
    }
    fn poll(&mut self, route: *mut c_void, now: u64, stopping: bool) {
        self.now = now;
        if stopping || self.stopping || self.link.stopping.load(Acquire) {
            self.stopping = true;
            self.link.stopping.store(true, Release);
            self.job = None;
            self.held.fill(None);
            for d in 0..2 {
                for s in 0..self.cfg.depth {
                    if let Some(t) = self.tickets[d][s] {
                        if self.link.slots[d][s].state().1 == State::Claimed {
                            assert!(self.link.slots[d][s].publish(t));
                        }
                        self.release(route, d, s, false);
                    }
                }
            }
            self.report();
            return;
        }
        for d in 0..2 {
            for s in 0..self.cfg.depth {
                if self.ack[d][s] <= now {
                    self.release(route, d, s, true);
                }
            }
        }
        if let Some(j) = self.job {
            if j.end <= now {
                self.copied(j);
            }
        }
        let retry = self.link.retry.load(Acquire);
        if retry <= now {
            let available = self.at.max(self.job.map_or(0, |j| j.end));
            if available > now || self.job.is_some() {
                self.link.retry.store(
                    fixed_replay::resume(available.max(now + 1), self.window()),
                    Release,
                );
            } else {
                self.link.retry.store(NEVER, Release);
                self.publish();
            }
        }
        self.report();
    }
}
struct Fixture {
    cfg: Config,
    link: Arc<Link>,
    native: [[Option<NativeFlight>; MAX_DEPTH]; 2],
    tokens: [[u64; MAX_DEPTH]; 2],
    ctx: *mut Context,
    backend: *mut c_void,
    owner: Submitter,
    hal: PcmHal,
    now: u64,
    result: Outcome,
    sampled_absence: bool,
    summary: [u64; 14],
    seen_notifications: u64,
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
        // Preserve the reference's pre-stream anchor for nominal HAL service.
        // The clock assertion below requires it to be replaced by capture.
        // A separate native-fed startup test begins with no anchor.
        let (mut producer, consumer) = bridge::channel();
        if cfg.prefill_counts {
            for seq in 0..HISTORY {
                producer
                    .push(Feedback {
                        seq,
                        first: 0,
                        c: Completion::default(),
                    })
                    .unwrap();
            }
        }
        let (returns, returned) = bridge::channel();
        let link = Arc::new(Link {
            slots: std::array::from_fn(|d| {
                (0..cfg.depth)
                    .map(|s| Slot::new(1, (d * MAX_DEPTH + s) as u32, Admission::default()))
                    .collect()
            }),
            summary: Summary::new(),
            fault: AtomicU64::new(0),
            error: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            retry: AtomicU64::new(NEVER),
        });
        let completion = Completer {
            cfg,
            link: link.clone(),
            counts: producer,
            returns,
            held: [None; HISTORY],
            next: 0,
            frames: 0,
            at: 100000,
            job: None,
            ack: [[NEVER; MAX_DEPTH]; 2],
            tickets: [[None; MAX_DEPTH]; 2],
            tokens: [[0; MAX_DEPTH]; 2],
            clock: SampleClock::new(1, 125, 3),
            io: capture,
            completed: [0; 2],
            totals: [0; 2],
            during: 0,
            notifications: 0,
            now: 100000,
            stopping: false,
        };
        completion.report();
        let ctx = Box::into_raw(Box::new(Context {
            link: link.clone(),
            state: UnsafeCell::new(completion),
        }));
        let hooks = Hooks {
            context: ctx.cast(),
            complete: completed,
            poll: polled,
        };
        let backend = unsafe { ftu_fixture_create(cfg.depth as u32, &hooks) };
        if backend.is_null() {
            unsafe { drop(Box::from_raw(ctx)) };
            panic!("native route creation failed");
        }
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
            summary: link.summary.load(),
            link,
            ctx,
            backend,
            tokens: [[0; MAX_DEPTH]; 2],
            seen_notifications: 0,
            native: [[None; MAX_DEPTH]; 2],
            owner: Submitter {
                next: [0; 2],
                previous: [[0; MAX_DEPTH]; 2],
                feedback: [None; HISTORY],
                counts: consumer,
                returns: returned,
                at: 100_000,
                job: None,
                burst: 0,
                last_end: [116_000, 180_000],
                count_seq: 0,
                wait_work_until: 100_000,
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

    fn sync_summary(&mut self) {
        self.summary = self.link.summary.load();
        let s = self.summary;
        self.result.completed = [s[3] as usize, s[4] as usize];
        self.result.frames = [s[5], s[6]];
        self.result.published = s[7] as usize;
        self.result.during_absence = s[8] as usize;
        if s[10] != self.seen_notifications {
            self.notify_owner();
            self.seen_notifications = s[10];
        }
        if self.link.fault.load(Acquire) != 0 {
            self.fail("completion_fault");
        }
        if self.link.error.load(Acquire) != 0 {
            self.fail("native_completion_error");
        }
    }
    fn counts_and_ack(&mut self) {
        while self.owner.feedback[self.owner.count_seq % HISTORY].is_none() {
            let Some(f) = self.owner.counts.pop() else {
                break;
            };
            assert_eq!(f.seq, self.owner.count_seq);
            self.owner.feedback[f.seq % HISTORY] = Some(f);
            self.owner.count_seq += 1;
            self.link.retry.store(
                fixed_replay::resume(
                    self.now
                        .max(self.summary[0])
                        .max(if self.summary[1] == NEVER {
                            0
                        } else {
                            self.summary[1]
                        }),
                    self.completion_window(),
                ),
                Release,
            );
        }
        while let Some(r) = self.owner.returns.pop() {
            let d = r.ticket.key as usize / MAX_DEPTH;
            let s = r.ticket.key as usize % MAX_DEPTH;
            assert_eq!(self.native[d][s].unwrap().ticket, r.ticket);
            assert_eq!(self.tokens[d][s], r.token);
            assert_eq!(
                unsafe { ftu_fixture_ack(self.backend, d as u32, s as u32, r.token) },
                1
            );
            assert!(self.link.slots[d][s].acknowledge(r.ticket));
            self.tokens[d][s] = 0;
            self.native[d][s] = None;
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
            free_slots[d] = self.link.slots[d]
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
            assert!(self.link.slots[d][f.slot].prepare(
                self.owner.previous[d][f.slot],
                f.ticket,
                |a| *a = Admission {
                    seq: f.seq,
                    start: f.start,
                    crc: payload.crc
                }
            ));
            self.owner.previous[d][f.slot] = f.ticket.generation;
            assert!(self.native[d][f.slot].is_none());
            self.native[d][f.slot] = Some(f);
            let lengths: [u16; 32] = std::array::from_fn(|i| payload.c.packets[i].requested);
            let data = unsafe { ftu_fixture_buffer(self.backend, d as u32, f.slot as u32) };
            assert!(!data.is_null());
            // Only the owner writes a native buffer before admission.
            unsafe { std::ptr::copy_nonoverlapping(payload.bytes.as_ptr(), data, BYTES) };
            let mut token = 0;
            let status = unsafe {
                ftu_fixture_submit(
                    self.backend,
                    d as u32,
                    f.slot as u32,
                    f.ticket.generation,
                    f.start / 1000,
                    lengths.as_ptr(),
                    &mut token,
                )
            };
            self.tokens[d][f.slot] = token;
            if status != 0 {
                // Synchronous rejection without callback. A conflict poisons
                // native and must retain both lifetimes instead of fabricating ACK.
                if unsafe { ftu_fixture_stat(self.backend, 6) } == 0
                    && self.link.slots[d][f.slot].state().1 == State::Submitted
                {
                    let slot = &self.link.slots[d][f.slot];
                    assert!(slot.complete(f.ticket, |_| {}));
                    assert!(slot.claim(f.ticket).is_some());
                    assert!(slot.publish(f.ticket));
                    assert!(slot.release(f.ticket));
                    if token != 0 {
                        assert_eq!(
                            unsafe {
                                ftu_fixture_ack(self.backend, d as u32, f.slot as u32, token)
                            },
                            1
                        );
                    }
                    assert!(slot.acknowledge(f.ticket));
                    self.native[d][f.slot] = None;
                    self.tokens[d][f.slot] = 0;
                }
                self.fail("native_submit_rejected");
                return;
            }

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
        if self.summary[1] != NEVER {
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
                        f.ready.max(self.summary[0]) + self.cfg.wake,
                        self.completion_window(),
                    ),
                )
            })
            .min_by_key(|(d, f, at)| (*at, *d, f.seq))
    }

    fn command(&mut self, kind: u32, d: usize, slot: usize) {
        let rc =
            unsafe { ftu_fixture_command(self.backend, self.now, kind, d as u32, slot as u32) };
        if rc != 0 {
            self.fail("native_route_fault");
        }
        self.sync_summary();
    }
    fn drain(&mut self) {
        let error_before = self.link.error.load(Acquire);
        self.link.stopping.store(true, Release);
        self.owner.job = None; // calls are synchronous; only logical paid cost may remain
        if self.cfg.lost_abort {
            unsafe { ftu_fixture_fault(self.backend, 1) };
        }
        let joined =
            unsafe { ftu_fixture_stop(self.backend, if self.cfg.lost_abort { 10 } else { 5000 }) }
                == 1;
        if joined {
            for d in 0..2 {
                for s in 0..self.cfg.depth {
                    let slot = &self.link.slots[d][s];
                    let (g, state) = slot.state();
                    if state == State::Released {
                        assert!(slot.acknowledge(Ticket {
                            key: (d * MAX_DEPTH + s) as u32,
                            epoch: 1,
                            generation: g
                        }));
                    }
                }
            }
        }
        assert_eq!(
            self.link.error.load(Acquire),
            error_before,
            "clean abort classified as a streaming error"
        );
        let free = self
            .link
            .slots
            .iter()
            .flatten()
            .all(|s| s.state().1 == State::Free);
        self.result.quarantine = !(joined && free && self.link.fault.load(Acquire) == 0);
        if !self.result.quarantine {
            assert_eq!(unsafe { ftu_fixture_stat(self.backend, 0) }, unsafe {
                ftu_fixture_stat(self.backend, 1)
            });
            assert_eq!(
                unsafe { ftu_fixture_stat(self.backend, 1) }
                    + unsafe { ftu_fixture_stat(self.backend, 8) },
                unsafe { ftu_fixture_stat(self.backend, 2) }
            );
            assert_eq!(unsafe { ftu_fixture_destroy(self.backend) }, 1);
            self.backend = std::ptr::null_mut();
            unsafe {
                drop(Box::from_raw(self.ctx));
            }
            self.ctx = std::ptr::null_mut();
        }
        // Otherwise retain C route, native storage, Context, channels and Arc
        // audio buffers together. Process-isolated fault tests own that leak.
    }
    fn run(mut self) -> Outcome {
        let native_allocations = unsafe { ftu_integrated_allocations() };
        assert_eq!((self.hal.work.prepare_us, self.hal.work.capture_us), (0, 0));
        crate::pool_simulation::without_allocations(|| {
            for _ in 0..1_000_000 {
                if self.result.reason.is_some() {
                    break;
                }
                if self.result.completed == [self.cfg.limit; 2]
                    && self.result.published == self.cfg.limit
                {
                    break;
                }
                let callback = self.next_callback();
                let owner = self.owner.job.map_or(self.owner.at, |j| j.end);
                let next = owner
                    .min(self.summary[2])
                    .min(callback.map_or(NEVER, |(_, _, at)| at))
                    .min(self.cfg.stop.unwrap_or(NEVER))
                    .min(self.link.retry.load(Acquire));
                assert!(next != NEVER && next < 60_000_000, "virtual time stalled");
                self.observe_time(next);
                if self.cfg.stop == Some(next) {
                    self.fail("stop");
                    break;
                }
                if owner == next && self.cfg.owner_first {
                    self.owner_event();
                }
                self.command(0, 0, 0);
                if let Some((d, f, at)) = self.next_callback() {
                    if at == next {
                        if self.owner.job.is_some_and(|j| {
                            j.direction == d && j.flight.ticket == f.ticket && j.prepared.is_none()
                        }) {
                            self.result.early_callbacks += 1;
                        }
                        self.native[d][f.slot].as_mut().unwrap().delivered = true;
                        self.command(1, d, f.slot);
                    }
                }
                if owner == next && !self.cfg.owner_first {
                    self.owner_event();
                }
            }
            if self.result.reason.is_none() {
                assert_eq!(self.result.completed, [self.cfg.limit; 2]);
                assert_eq!(self.summary[9], 1);
                assert_eq!(self.summary[11], self.result.frames[0]);
                let mut z = ZeroTimestamp {
                    epoch: 0,
                    frame: 0,
                    ticks: 0,
                };
                assert_eq!(unsafe { ftu_live_timestamp(&mut self.hal.live, &mut z) }, 0);
                assert_eq!(z.epoch, 1);
                assert_ne!(
                    z.ticks, 1_000_000,
                    "reference seed anchor was never replaced"
                );
                assert!(z.ticks > 116000 * 24, "seeded timestamp escaped");
            }
            self.result.missing = self.hal.steady_input_missing;
            self.result.output_missing = self.hal.steady_output_missing;
        });
        assert_eq!(
            unsafe { ftu_integrated_allocations() },
            native_allocations,
            "native streaming allocation/free"
        );
        self.drain(); // teardown allocations excluded, as in native qualification
        self.result
    }
    fn owner_event(&mut self) {
        if let Some(job) = self.owner.job {
            self.submit_return(job);
        } else {
            self.submit_turn();
        }
    }
}
fn execute(buffer: usize, phase: u64, cfg: Config) -> Outcome {
    Fixture::new(buffer, phase, cfg).run()
}
#[test]
fn integrated_joint_smoke() {
    for cfg in [
        Config {
            owner_absence: None,
            submit: 0,
            ..Default::default()
        },
        Config::default(),
    ] {
        let result = execute(64, 7, cfg);
        println!("INTEGRATED_SMOKE {result:?}");
        assert!(!result.quarantine);
        assert!(result.max_burst <= 10);
        if cfg.owner_absence.is_none() {
            assert!(result.accepted());
        }
    }
}

#[test]
fn integrated_joint_phase_and_offset_screen() {
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
            println!("LINKED_PHASE depth={depth} buffer={buffer} cases=256 accepted={accepted} faults={faults} input_missing={missing} output_missing={out_missing} min_lead={min_lead} integrated_progress={covered} classified={classified}");
            assert_eq!(classified, 256);
            if depth == 10 {
                assert_eq!((accepted, faults, min_lead), (128, 128, 14));
            } else {
                assert_eq!((accepted, faults, min_lead), (256, 0, 22));
            }
        }
    }
}
#[test]
fn integrated_joint_dense_offsets_and_negative_controls() {
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
                println!("LINKED_BOUND depth={depth} duration={duration} offset={offset} coverage={:?} returned={:?} start={:?} predicted={:?} reason={:?}",r.coverage,r.first_out_return,r.first_out_start,r.bound_requires_failure,r.reason);
                if duration == 32000 {
                    assert!(!r.accepted());
                }
            }
            println!("LINKED_OFFSET depth={depth} duration={duration} cases=32 accepted={accepted} faults={faults} min_lead={min_lead} classified={classified}");
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
            println!("LINKED_SATURATION depth={depth} phase={phase} result={r:?}");
        }
    }
}
#[test]
fn integrated_joint_cost_delay_and_hal_disturbance_screen() {
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
            let expected = [
                [
                    [12, 0, 0, 0],
                    [12, 0, 0, 0],
                    [0, 12, 0, 0],
                    [12, 0, 0, 0],
                    [0, 12, 394, 0],
                    [0, 12, 0, 0],
                    [12, 0, 0, 0],
                    [0, 12, 0, 0],
                    [12, 0, 0, 0],
                    [12, 0, 0, 0],
                ],
                [
                    [12, 0, 0, 0],
                    [12, 0, 0, 0],
                    [12, 0, 0, 0],
                    [12, 0, 0, 0],
                    [5, 0, 394, 0],
                    [0, 12, 0, 0],
                    [12, 0, 0, 0],
                    [0, 12, 0, 0],
                    [12, 0, 0, 0],
                    [12, 0, 0, 0],
                ],
            ];
            assert_eq!(
                [accepted, faults, input as usize, output as usize],
                expected[usize::from(depth == 12)][case],
                "retained cost/PCM gate changed"
            );
            println!("LINKED_COST depth={depth} case={case} cases=12 accepted={accepted} faults={faults} input_missing={input} output_missing={output}");
        }
    }
}
#[test]
fn integrated_joint_delay_witnesses() {
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
            if delay == 5000 && (depth == 10 || reordered) {
                assert_eq!(r.reason, Some("output_lead_deadline"));
                assert!(!r.accepted());
            } else {
                assert!(r.accepted());
            }
            assert_eq!((r.missing, r.output_missing), (0, 0));
            println!("LINKED_DELAY depth={depth} delay={delay} reorder={reordered} result={r:?}");
        }
    }
}

#[test]
fn integrated_joint_delayed_callback_offsets() {
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
        assert_eq!(
            (accepted, faults, min_lead),
            if depth == 10 {
                (0, 384, 9)
            } else {
                (384, 0, 17)
            }
        );
        println!("LINKED_DELAY_OFFSETS depth={depth} cases=384 accepted={accepted} faults={faults} min_lead={min_lead}");
    }
}

#[test]
fn integrated_logical_callback_during_paid_submit_cost() {
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
    println!("LINKED_EARLY_CALLBACK result={r:?}");
}

// Direct submissions below deliberately bypass joint admission only to exercise
// ownership/startup faults. They are not accepted timing/latency scenarios.
impl Fixture {
    fn input(&mut self, seq: usize, slot: usize) -> u64 {
        let t = ticket(seq, 0, slot);
        let start = 116000 + seq as u64 * 4000;
        assert!(
            self.link.slots[0][slot].prepare(self.owner.previous[0][slot], t, |a| *a =
                Admission { seq, start, crc: 0 })
        );
        self.owner.previous[0][slot] = t.generation;
        let mut token = 0;
        assert_eq!(
            unsafe {
                ftu_fixture_submit(
                    self.backend,
                    0,
                    slot as u32,
                    t.generation,
                    start / 1000,
                    [312u16; 32].as_ptr(),
                    &mut token,
                )
            },
            0
        );
        self.tokens[0][slot] = token;
        self.native[0][slot] = Some(NativeFlight {
            seq,
            slot,
            start,
            ready: start + 4000,
            ticket: t,
            delivered: false,
        });
        token
    }
    fn tick(&mut self, now: u64, kind: u32, slot: usize) {
        self.now = now;
        self.command(kind, 0, slot);
    }
}
#[test]
fn integrated_startup_clock_is_native_fed() {
    assert_eq!(std::mem::size_of::<NativeEvent>(), 1096);
    for ppm in [-250, 0, 250] {
        let mut f = Fixture::new(64, 7, Config::default());
        f.hal.live.shared.clock.publish(None);
        f.hal.live.shared.set_phase(STARTING);
        unsafe { ftu_fixture_clock(f.backend, ppm) };
        let mut z = ZeroTimestamp {
            epoch: 0,
            frame: 0,
            ticks: 0,
        };
        assert_eq!(unsafe { ftu_live_timestamp(&mut f.hal.live, &mut z) }, -2);
        for seq in 0..40 {
            f.input(seq, 0);
            let due = 120000 + seq as u64 * 4000;
            f.tick(due, 1, 0);
            f.tick(due + 150, 0, 0);
            f.tick(due + 175, 0, 0);
            f.counts_and_ack();
            f.owner.feedback[seq % HISTORY] = None;
        }
        assert_eq!(unsafe { ftu_live_timestamp(&mut f.hal.live, &mut z) }, 0);
        let anchor = f.hal.live.shared.clock.read().unwrap();
        let expected = 500.0 * (1.0 + f64::from(ppm) / 1_000_000.0);
        assert!(
            (anchor.ticks_per_frame - expected).abs() < 0.002,
            "{} vs {}",
            anchor.ticks_per_frame,
            expected
        );
        assert_eq!(f.summary[11], 40 * 192);
        assert_eq!(f.summary[13], 40 * 192);
        assert_eq!(z.epoch, 1);
        assert!(z.ticks > 116000 * 24);
        f.drain();
        assert!(!f.result.quarantine);
        println!(
            "LINKED_CLOCK ppm={ppm} ticks_per_frame={} captured=7680",
            anchor.ticks_per_frame
        );
    }
}
#[test]
fn integrated_count_backpressure_keeps_native_buffer_held() {
    let mut f = Fixture::new(
        64,
        7,
        Config {
            prefill_counts: true,
            ..Default::default()
        },
    );
    let token = f.input(0, 0);
    f.tick(120000, 1, 0);
    f.tick(120150, 0, 0);
    assert_eq!(f.link.slots[0][0].state().1, State::Claimed);
    assert_eq!(unsafe { ftu_fixture_stat(f.backend, 5) }, 1);
    assert_eq!(f.summary[13], 192);
    assert_eq!(f.summary[7], 0);
    assert_eq!(unsafe { ftu_fixture_ack(f.backend, 0, 0, token) }, 0);
    for _ in 0..HISTORY {
        assert!(f.owner.counts.pop().is_some());
    }
    f.link.retry.store(120150, Release);
    f.tick(120150, 0, 0);
    assert_eq!(f.summary[7], 1);
    assert_eq!(f.summary[13], 192);
    assert_eq!(f.link.slots[0][0].state().1, State::Published);
    f.tick(120175, 0, 0);
    f.counts_and_ack();
    assert_eq!(f.link.slots[0][0].state().1, State::Free);
    assert_eq!(f.owner.feedback[0].unwrap().seq, 0);
    f.drain();
    assert!(!f.result.quarantine);
}
#[test]
fn integrated_stop_releases_under_count_backpressure() {
    let mut f = Fixture::new(
        64,
        7,
        Config {
            prefill_counts: true,
            ..Default::default()
        },
    );
    f.input(0, 0);
    f.tick(120000, 1, 0);
    f.tick(120150, 0, 0);
    assert_eq!(f.link.slots[0][0].state().1, State::Claimed);
    f.drain();
    assert!(!f.result.quarantine);
}
#[test]
fn integrated_submit_rejection_retires_both_states() {
    for fault in [2, 7] {
        let mut f = Fixture::new(64, 7, Config::default());
        unsafe { ftu_fixture_fault(f.backend, fault) };
        f.submit_turn();
        let j = f.owner.job.unwrap();
        f.observe_time(j.end);
        f.submit_return(j);
        assert_eq!(f.result.reason, Some("native_submit_rejected"));
        assert_eq!(f.link.slots[0][0].state().1, State::Free);
        f.drain();
        assert!(!f.result.quarantine);
    }
}
#[test]
fn integrated_physical_callback_before_submit_return() {
    let mut f = Fixture::new(64, 7, Config::default());
    unsafe { ftu_fixture_fault(f.backend, 5) };
    f.input(0, 0);
    f.sync_summary();
    assert_eq!(f.link.slots[0][0].state().1, State::Claimed);
    assert_eq!(unsafe { ftu_fixture_stat(f.backend, 0) }, 1);
    f.tick(120150, 0, 0);
    f.tick(120175, 0, 0);
    f.counts_and_ack();
    assert_eq!(f.link.slots[0][0].state().1, State::Free);
    f.drain();
    assert!(!f.result.quarantine);
}
#[test]
fn integrated_stop_at_each_ownership_stage() {
    for stop in [100010, 120010, 150000, 1624000, 1643325] {
        let r = execute(
            64,
            7,
            Config {
                stop: Some(stop),
                ..Default::default()
            },
        );
        assert_eq!(r.reason, Some("stop"));
        assert!(!r.quarantine);
    }
}
#[test]
fn integrated_lost_abort_retains_until_real_late_callback() {
    let mut f = Fixture::new(64, 7, Config::default());
    f.input(0, 0);
    f.link.stopping.store(true, Release);
    unsafe { ftu_fixture_fault(f.backend, 1) };
    assert_eq!(unsafe { ftu_fixture_stop(f.backend, 10) }, 0);
    assert_eq!(unsafe { ftu_fixture_destroy(f.backend) }, 0);
    assert_eq!(f.link.slots[0][0].state().1, State::Submitted);
    // Sources/context remain live. A late physical callback still reaches Rust.
    f.tick(120000, 1, 0);
    assert_eq!(f.link.slots[0][0].state().1, State::Released);
    assert_eq!(f.summary[7], 0);
    f.drain();
    assert!(!f.result.quarantine);
}
#[test]
fn integrated_device_removal_stops_without_publication() {
    let mut f = Fixture::new(64, 7, Config::default());
    f.input(0, 0);
    unsafe { ftu_fixture_fault(f.backend, 6) };
    f.tick(120000, 1, 0);
    assert_eq!(f.result.reason, Some("native_completion_error"));
    assert_eq!(f.summary[7], 0);
    f.drain();
    assert!(!f.result.quarantine);
}
// Run these retained-thread tests in separate processes (the runner does so).
#[test]
fn integrated_malformed_completion_is_quarantined() {
    let mut f = Fixture::new(64, 7, Config::default());
    f.input(0, 0);
    unsafe { ftu_fixture_fault(f.backend, 3) };
    f.tick(120000, 1, 0);
    f.tick(120150, 0, 0);
    assert_eq!(f.result.reason, Some("completion_fault"));
    f.drain();
    assert!(f.result.quarantine);
    assert_eq!(unsafe { ftu_fixture_stat(f.backend, 5) }, 1);
    assert_eq!(unsafe { ftu_fixture_destroy(f.backend) }, 0);
}
#[test]
fn integrated_bad_native_identity_is_quarantined() {
    let mut f = Fixture::new(64, 7, Config::default());
    f.input(0, 0);
    unsafe { ftu_fixture_fault(f.backend, 4) };
    f.tick(120000, 1, 0);
    assert_eq!(f.result.reason, Some("native_route_fault"));
    assert_eq!(f.link.slots[0][0].state().1, State::Submitted);
    f.drain();
    assert!(f.result.quarantine);
    assert_eq!(unsafe { ftu_fixture_destroy(f.backend) }, 0);
}

#[test]
fn integrated_real_timer_services_rust_without_owner_pump() {
    let mut f = Fixture::new(64, 7, Config::default());
    f.input(0, 0);
    f.tick(120000, 1, 0); // physical Mach callback, copy still pending
    assert_eq!(f.summary[7], 0);
    unsafe { ftu_fixture_real_time(f.backend, 120000) };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        f.sync_summary();
        if f.link.slots[0][0].state().1 == State::Released {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real timer did not service Rust"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    f.counts_and_ack();
    assert_eq!(f.summary[7], 1);
    f.drain();
    assert!(!f.result.quarantine);
}
#[test]
fn integrated_stale_return_cannot_reuse_new_generation() {
    let mut f = Fixture::new(64, 7, Config::default());
    let old = f.input(0, 0);
    f.tick(120000, 1, 0);
    f.tick(120150, 0, 0);
    f.tick(120175, 0, 0);
    assert_eq!(f.link.slots[0][0].state().1, State::Released);
    // Native release alone grants no owner reuse.
    assert!(unsafe { ftu_fixture_buffer(f.backend, 0, 0) }.is_null());
    f.counts_and_ack();
    let new = f.input(1, 0);
    assert_ne!(old, new);
    assert_eq!(unsafe { ftu_fixture_ack(f.backend, 0, 0, old) }, 0);
    assert!(!f.link.slots[0][0].acknowledge(ticket(0, 0, 0)));
    assert_eq!(f.link.slots[0][0].state(), (2, State::Submitted));
    f.drain();
    assert!(!f.result.quarantine);
}

#[test]
fn integrated_unrecoverable_abort_retains_context() {
    let r = execute(
        64,
        7,
        Config {
            stop: Some(120010),
            lost_abort: true,
            ..Default::default()
        },
    );
    assert_eq!(r.reason, Some("stop"));
    assert!(r.quarantine);
}
#[test]
fn integrated_physical_early_callback_uses_joint_admission() {
    let mut f = Fixture::new(64, 7, Config::default());
    f.submit_turn();
    let job = f.owner.job.unwrap();
    f.observe_time(job.end);
    f.command(0, 0, 0);
    unsafe { ftu_fixture_fault(f.backend, 5) };
    f.submit_return(job);
    f.sync_summary();
    assert_eq!(unsafe { ftu_fixture_stat(f.backend, 0) }, 1);
    assert_eq!(f.link.slots[0][0].state().1, State::Claimed);
    // The true callback has arrived while the C call was entered; no owner
    // ACK has run. Retire the owner's logical paid cost before consuming ACK.
    let returned = f.owner.job.unwrap();
    f.observe_time(returned.end);
    f.submit_return(returned);
    f.tick(100175, 0, 0);
    f.tick(100200, 0, 0);
    f.counts_and_ack();
    assert_eq!(f.link.slots[0][0].state().1, State::Free);
    f.drain();
    assert!(!f.result.quarantine);
}
