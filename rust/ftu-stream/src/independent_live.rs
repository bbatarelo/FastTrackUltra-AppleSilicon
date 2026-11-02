//! Experimental hardware runtime: owner submits; native dispatcher publishes IN.
//! All allocation precedes admission. Quarantine retains both contexts on doubt.
use super::*;
use crate::clock::SampleClock;
use crate::independent_completion::{self as bridge, Slot, State, Ticket};
use crate::output_admission::OutputPolicy;
use crate::runtime_fault::{self as rf, Site};
use crate::submission_order::SubmissionOrder;
use std::sync::atomic::Ordering::{Acquire, Release};
use std::{
    cell::UnsafeCell,
    panic::{catch_unwind, AssertUnwindSafe},
};
#[cfg(panic = "abort")]
compile_error!("The HAL completion boundary requires panic=unwind");
const DEPTH: usize = 12;
const HISTORY: usize = 64;
const BYTES: usize = 32 * 312;
#[repr(C)]
#[derive(Clone, Copy)]
struct Event {
    token: u64,
    ticket: u64,
    host_ticks: u64,
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
    complete: extern "C" fn(*mut c_void, *mut c_void, *const Event),
    poll: extern "C" fn(*mut c_void, *mut c_void, u64, u32),
    completion_qos: u32,
}
unsafe extern "C" {
    fn ftu_live_route_attach(ctx: *mut c_void, hooks: *const Hooks) -> *mut c_void;
    fn ftu_route_buffer(route: *mut c_void, d: u32, s: u32) -> *mut u8;
    fn ftu_route_submit(
        route: *mut c_void,
        d: u32,
        s: u32,
        ticket: u64,
        start: u64,
        lengths: *const u16,
        token: *mut u64,
    ) -> i32;
    fn ftu_route_release(route: *mut c_void, token: u64) -> bool;
    fn ftu_route_ack(route: *mut c_void, d: u32, s: u32, token: u64) -> bool;
    fn ftu_route_stop(route: *mut c_void, ms: u32) -> bool;
    fn ftu_route_destroy(route: *mut c_void) -> bool;
    fn ftu_live_route_fault(route: *mut c_void) -> u32;
    fn ftu_route_snapshot(route: *mut c_void, out: *mut u64);
    fn ftu_live_cleanup_snapshot(ctx: *mut c_void, out: *mut u64);
}
#[derive(Clone, Copy)]
struct Api {
    buffer: unsafe extern "C" fn(*mut c_void, u32, u32) -> *mut u8,
    submit: unsafe extern "C" fn(*mut c_void, u32, u32, u64, u64, *const u16, *mut u64) -> i32,
    release: unsafe extern "C" fn(*mut c_void, u64) -> bool,
    ack: unsafe extern "C" fn(*mut c_void, u32, u32, u64) -> bool,
    stop: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    destroy: unsafe extern "C" fn(*mut c_void) -> bool,
    fault: unsafe extern "C" fn(*mut c_void) -> u32,
    snapshot: unsafe extern "C" fn(*mut c_void, *mut u64),
    cleanup_snapshot: unsafe extern "C" fn(*mut c_void, *mut u64),
}
const NATIVE: Api = Api {
    buffer: ftu_route_buffer,
    submit: ftu_route_submit,
    release: ftu_route_release,
    ack: ftu_route_ack,
    stop: ftu_route_stop,
    destroy: ftu_route_destroy,
    fault: ftu_live_route_fault,
    snapshot: ftu_route_snapshot,
    cleanup_snapshot: ftu_live_cleanup_snapshot,
};
#[derive(Clone, Copy, Default)]
struct Admission {
    seq: usize,
    crc: u32,
    start: u64,
    checked: u64,
    ticks: u64,
    lengths: [u16; 32],
}
#[derive(Clone, Copy, Debug)]
struct Feedback {
    seq: usize,
    first: u64,
    c: Completion,
}
#[derive(Clone, Copy, Debug)]
struct Return {
    ticket: Ticket,
    token: u64,
}
#[derive(Clone, Copy)]
struct Held {
    ticket: Ticket,
    token: u64,
    seq: usize,
    c: Completion,
    published: bool,
}
#[derive(Clone, Copy)]
struct Failure {
    admission: Admission,
    direction: usize,
    slot: usize,
    c: Completion,
    reason: u32,
}
struct Link {
    slots: [Box<[Slot<Admission>]>; 2],
    fault: AtomicBool,
    error: AtomicU32,
    stopping: AtomicBool,
    completed: [AtomicU64; 2],
    frames: [AtomicU64; 2],
}
struct Context {
    link: Arc<Link>,
    state: UnsafeCell<Completer>,
}
struct Completer {
    link: Arc<Link>,
    api: Api,
    epoch: u64,
    io: AudioIo,
    clock: SampleClock,
    counts: bridge::Producer<Feedback, HISTORY>,
    returns: bridge::Producer<Return, 32>,
    failures: bridge::Producer<Failure, 2>,
    held: [Option<Held>; HISTORY],
    owned: [[Option<Return>; DEPTH]; 2],
    next: usize,
    frames: u64,
    retired: bool,
}
const _: () = {
    fn require_send<T: Send>() {}
    let _ = require_send::<Completer>;
};
fn ticket(epoch: u64, seq: usize, d: usize, s: usize) -> Ticket {
    Ticket {
        epoch,
        key: (d * DEPTH + s) as u32,
        generation: seq as u64 + 1,
    }
}
extern "C" fn complete(ctx: *mut c_void, route: *mut c_void, event: *const Event) {
    guarded(ctx, |s| s.receive(route, unsafe { *event }));
}
extern "C" fn poll(ctx: *mut c_void, route: *mut c_void, _ticks: u64, stopping: u32) {
    guarded(ctx, |s| s.poll(route, stopping != 0));
}
fn guarded(ctx: *mut c_void, f: impl FnOnce(&mut Completer) -> Result<(), &'static str>) {
    let c = unsafe { &*ctx.cast::<Context>() };
    if c.link.fault.load(Acquire) {
        return;
    }
    let r = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(test)]
        {
            crate::pool_simulation::without_allocations(|| f(unsafe { &mut *c.state.get() }))
        }
        #[cfg(not(test))]
        {
            f(unsafe { &mut *c.state.get() })
        }
    }));
    match r {
        Ok(Ok(())) => (),
        Ok(Err(reason)) => {
            let _ = c
                .link
                .error
                .compare_exchange(0, reason_code(Some(reason)), SeqCst, SeqCst);
        }
        Err(_) => {
            c.link.fault.store(true, Release);
        }
    }
}
impl Completer {
    fn stopping(&self) -> bool {
        self.link.stopping.load(Acquire)
            || self.io.shared.stop.load(SeqCst)
            || self.link.error.load(Acquire) != 0
    }
    fn release(&mut self, route: *mut c_void, d: usize, s: usize, notify: bool) {
        let r = self.owned[d][s].unwrap();
        assert!(self.link.slots[d][s].publish(r.ticket));
        assert!(unsafe { (self.api.release)(route, r.token) });
        assert!(self.link.slots[d][s].release(r.ticket));
        if notify {
            self.returns.push(r).unwrap();
        }
        self.owned[d][s] = None;
    }
    fn receive(&mut self, route: *mut c_void, e: Event) -> Result<(), &'static str> {
        let (d, s) = (e.direction as usize, e.slot as usize);
        assert!(
            d < 2 && s < DEPTH && e.entries == 32 && e.bytes == BYTES as u64 && !e.data.is_null()
        );
        let t = Ticket {
            key: (d * DEPTH + s) as u32,
            epoch: self.epoch,
            generation: e.ticket,
        };
        assert!(self.link.slots[d][s].complete(t, |_| {}));
        let a = self.link.slots[d][s].claim(t).unwrap();
        assert_eq!(a.seq as u64 + 1, e.ticket);
        assert!(self.owned[d][s].is_none());
        self.owned[d][s] = Some(Return {
            ticket: t,
            token: e.token,
        });
        if self.stopping() {
            self.release(route, d, s, false);
            return Ok(());
        }
        if e.result.status != 0 {
            self.release(route, d, s, true);
            let _ = self.failures.push(Failure {
                admission: a,
                direction: d,
                slot: s,
                c: e.result,
                reason: 23,
            });
            return Err("request_completion_error");
        }
        let c = e.result;
        if c.matched != 1
            || c.packets[..32].iter().enumerate().any(|(i, p)| {
                p.requested != a.lengths[i]
                    || p.status != 0
                    || !valid_length(p.actual)
                    || p.actual > p.requested
                    || (d == 1 && p.actual != p.requested)
            })
        {
            let _ = self.failures.push(Failure {
                admission: a,
                direction: d,
                slot: s,
                c,
                reason: 22,
            });
            return Err("packet_completion_error");
        }
        let frames = c.packets[..32]
            .iter()
            .map(|p| u64::from(p.actual / 24))
            .sum::<u64>();
        let bytes = unsafe { std::slice::from_raw_parts(e.data, BYTES) };
        if d == 1 && crate::pool::crc32(bytes) != a.crc {
            return Err("output_payload_changed");
        }
        self.link.completed[d].fetch_add(1, SeqCst);
        self.link.frames[d].fetch_add(frames, SeqCst);
        if d == 1 {
            self.release(route, d, s, true);
        } else {
            self.io.capture(a.seq, &c, bytes)?;
            assert!(self.held[a.seq % HISTORY].is_none());
            self.held[a.seq % HISTORY] = Some(Held {
                ticket: t,
                token: e.token,
                seq: a.seq,
                c,
                published: false,
            });
            self.publish(route)?;
        }
        Ok(())
    }
    fn publish(&mut self, route: *mut c_void) -> Result<(), &'static str> {
        // At most the bounded native pool can be retained in ordered publication.
        for _ in 0..DEPTH {
            let at = self.next % HISTORY;
            let Some(mut held) = self.held[at] else { break };
            assert_eq!(held.seq, self.next);
            if !held.published {
                self.io.ordered(held.seq, self.frames, &held.c)?;
                let mut cursor = self.frames;
                for group in held.c.packets[..32].chunks_exact(8) {
                    self.clock.observe_usb_group(
                        self.epoch,
                        cursor,
                        &std::array::from_fn(|i| group[i].ticks),
                    )?;
                    self.io.clock(self.clock.snapshot())?;
                    cursor += group.iter().map(|p| u64::from(p.actual / 24)).sum::<u64>();
                }
                held.published = true;
                self.held[at] = Some(held);
            }
            if self
                .counts
                .push(Feedback {
                    seq: held.seq,
                    first: self.frames,
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
            let s = held.ticket.key as usize % DEPTH;
            assert_eq!(self.owned[0][s].unwrap().token, held.token);
            self.release(route, 0, s, true);
        }
        Ok(())
    }
    fn poll(&mut self, route: *mut c_void, stopping: bool) -> Result<(), &'static str> {
        if stopping || self.stopping() {
            self.link.stopping.store(true, Release);
            if !self.retired {
                self.io.shared.clock.publish(None);
                self.retired = true;
            }
            self.held.fill(None);
            for d in 0..2 {
                for s in 0..DEPTH {
                    if self.owned[d][s].is_some() {
                        self.release(route, d, s, false);
                    }
                }
            }
            Ok(())
        } else {
            self.publish(route)
        }
    }
}
struct Owner {
    link: Arc<Link>,
    api: Api,
    route: *mut c_void,
    ctx: *mut Context,
    io: AudioIo,
    epoch: u64,
    base: u64,
    next: [usize; 2],
    previous: [[u64; DEPTH]; 2],
    flights: [[Option<Return>; DEPTH]; 2],
    feedback: [Option<Feedback>; HISTORY],
    count_seq: usize,
    counts: bridge::Consumer<Feedback, HISTORY>,
    returns: bridge::Consumer<Return, 32>,
    failures: bridge::Consumer<Failure, 2>,
    retry: crate::output_retry::Tracker,
    min_lead: u64,
    // Runtime-fault diagnostics (ftuE); fixed-size, no allocation in step.
    // The failing guard of the current step, if it was a precise check.
    site: Option<Site>,
    // `now` of the last step that returned Ok, and the returned clock of the
    // last submission that passed every guard. Absent until measured.
    prior_step: Option<Clock>,
    prior_progress: Option<Clock>,
    steps: u64,
}
fn site(stage: u64, d: usize, seq: usize, slot: Option<usize>, start: u64, checked: Clock) -> Site {
    Site {
        stage,
        direction: d,
        sequence: seq as u64,
        slot,
        scheduled: Some(start),
        checked: Some((checked.frame, checked.ticks)),
        returned: None,
        required: 0,
        submit_status: None,
    }
}
impl Owner {
    fn new(
        shared: Arc<Shared>,
        epoch: u64,
        info: Info,
        api: Api,
        attach: impl FnOnce(&Hooks) -> *mut c_void,
    ) -> Option<Self> {
        let geometry = Transport::new(4, DEPTH).unwrap();
        let (counts, consumer) = bridge::channel();
        let (returns, returned) = bridge::channel();
        let (failures, failed) = bridge::channel();
        let link = Arc::new(Link {
            slots: std::array::from_fn(|d| {
                (0..DEPTH)
                    .map(|s| Slot::new(epoch, (d * DEPTH + s) as u32, Admission::default()))
                    .collect()
            }),
            fault: AtomicBool::new(false),
            error: AtomicU32::new(0),
            stopping: AtomicBool::new(false),
            completed: std::array::from_fn(|_| AtomicU64::new(0)),
            frames: std::array::from_fn(|_| AtomicU64::new(0)),
        });
        let state = Completer {
            link: link.clone(),
            api,
            epoch,
            io: AudioIo::with_geometry(shared.clone(), epoch, info, geometry),
            clock: SampleClock::new(epoch, info.timebase_numer, info.timebase_denom),
            counts,
            returns,
            failures,
            held: [None; HISTORY],
            owned: [[None; DEPTH]; 2],
            next: 0,
            frames: 0,
            retired: false,
        };
        let output_io = AudioIo::with_geometry(shared, epoch, info, geometry);
        let ctx = Box::into_raw(Box::new(Context {
            link: link.clone(),
            state: UnsafeCell::new(state),
        }));
        let hooks = Hooks {
            context: ctx.cast(),
            complete,
            poll,
            completion_qos: 1,
        };
        let route = attach(&hooks);
        if route.is_null() {
            unsafe { drop(Box::from_raw(ctx)) };
            return None;
        }
        Some(Self {
            link,
            api,
            route,
            ctx,
            io: output_io,
            epoch,
            base: 0,
            next: [0; 2],
            previous: [[0; DEPTH]; 2],
            flights: [[None; DEPTH]; 2],
            feedback: [None; HISTORY],
            count_seq: 0,
            counts: consumer,
            returns: returned,
            failures: failed,
            retry: crate::output_retry::Tracker::new(
                epoch,
                OutputPolicy::WhenReady,
                info.timebase_numer,
                info.timebase_denom,
            ),
            min_lead: u64::MAX,
            site: None,
            prior_step: None,
            prior_progress: None,
            steps: 0,
        })
    }
    /// Primary ftuE record for a stop decided now, before any teardown.
    fn runtime_fault(&self, reason: u32, uptime_ms: u64) -> rf::Snapshot {
        rf::Snapshot::primary(&rf::Primary {
            epoch: self.epoch,
            reason,
            site: self.site,
            base: (self.base != 0).then_some(self.base),
            prior_step: self.prior_step.map(|c| (c.frame, c.ticks)),
            prior_progress: self.prior_progress.map(|c| (c.frame, c.ticks)),
            steps: self.steps,
            next: self.next.map(|n| n as u64),
            completed: std::array::from_fn(|d| self.link.completed[d].load(SeqCst)),
            pending: self
                .flights
                .each_ref()
                .map(|f| f.iter().filter(|f| f.is_some()).count() as u64),
            captured: self.io.shared.captured.load(SeqCst),
            prepared: self.io.shared.prepared.load(SeqCst),
            timebase: (self.io.timebase.timebase_numer != 0
                && self.io.timebase.timebase_denom != 0)
                .then_some((
                    self.io.timebase.timebase_numer,
                    self.io.timebase.timebase_denom,
                )),
            min_out_lead: (self.min_lead != u64::MAX).then_some(self.min_lead),
            uptime_ms,
            queue_depth: DEPTH as u64,
            block_ms: self.io.transport.block_ms(),
            replay_ms: self.io.transport.replay_ms(),
        })
    }
    fn receive(&mut self) -> Result<(), &'static str> {
        if self.link.fault.load(Acquire) || unsafe { (self.api.fault)(self.route) } != 0 {
            return Err("native_callback_fault");
        }
        if self.link.error.load(Acquire) != 0 {
            return Err("native_completion_error");
        }
        while let Some(r) = self.returns.pop() {
            let d = r.ticket.key as usize / DEPTH;
            let s = r.ticket.key as usize % DEPTH;
            let expected = self.flights[d][s].unwrap();
            assert_eq!(expected.ticket, r.ticket);
            assert_eq!(expected.token, r.token);
            assert!(unsafe { (self.api.ack)(self.route, d as u32, s as u32, r.token) });
            assert!(self.link.slots[d][s].acknowledge(r.ticket));
            self.flights[d][s] = None;
        }
        while self.feedback[self.count_seq % HISTORY].is_none() {
            let Some(f) = self.counts.pop() else { break };
            assert_eq!(f.seq, self.count_seq);
            self.feedback[f.seq % HISTORY] = Some(f);
            self.count_seq += 1;
        }
        self.progress();
        Ok(())
    }
    fn progress(&self) {
        for d in 0..2 {
            self.io.shared.submitted[d].store(self.next[d] as u64, SeqCst);
            self.io.shared.completed[d].store(self.link.completed[d].load(SeqCst), SeqCst);
            self.io.shared.pending[d].store(
                self.flights[d].iter().filter(|f| f.is_some()).count() as u64,
                SeqCst,
            );
        }
    }
    fn step(
        &mut self,
        mut clock: impl FnMut() -> Result<Clock, &'static str>,
    ) -> Result<bool, &'static str> {
        // Every guard below that fails records its own stage and clock; any
        // other error leaves `site` empty and is reported as a generic stop.
        self.site = None;
        self.steps = self.steps.wrapping_add(1);
        self.receive()?;
        if self.link.stopping.load(Acquire) || self.io.shared.stop.load(SeqCst) {
            return Err("signal");
        }
        let now = clock()?;
        if self.base == 0 {
            self.base = now.frame.checked_add(16).ok_or("timeline_exhausted")?;
        }
        // Lost callbacks can leave every slot occupied even with valid sources.
        // Bound that condition using the actual bus clock; never invent returns.
        let mut stale = None;
        'watch: for (d, flights) in self.flights.iter().enumerate() {
            for (s, f) in flights.iter().enumerate() {
                let Some(f) = f else { continue };
                let seq = (f.ticket.generation - 1) as usize;
                let start = self
                    .io
                    .transport
                    .start_frame(self.base, seq, d == 1)
                    .ok_or("timeline_exhausted")?;
                if now.frame > start.saturating_add(64) {
                    stale = Some(Site {
                        required: 64,
                        ..site(rf::STAGE_WATCHDOG, d, seq, Some(s), start, now)
                    });
                    break 'watch;
                }
            }
        }
        if stale.is_some() {
            self.site = stale;
            return Err("completion_deadline");
        }
        let free: [Option<usize>; 2] = std::array::from_fn(|d| {
            self.link.slots[d]
                .iter()
                .position(|s| s.state().1 == State::Free)
        });
        let mut starts = [None; 2];
        for d in 0..2 {
            if free[d].is_some()
                && (d == 0
                    || self.feedback[self.next[d] % HISTORY].is_some_and(|f| f.seq == self.next[d]))
            {
                starts[d] = Some(
                    self.io
                        .transport
                        .start_frame(self.base, self.next[d], d == 1)
                        .ok_or("timeline_exhausted")?,
                );
            }
        }
        let mut chosen = SubmissionOrder::Deadline.choose(starts);
        if chosen == Some(1) {
            let f = self.feedback[self.next[1] % HISTORY].unwrap();
            let frames = f.c.packets[..32]
                .iter()
                .map(|p| usize::from(p.actual / 24))
                .sum();
            let lead = starts[1].unwrap().saturating_sub(now.frame);
            if lead < 16 {
                self.site = Some(Site {
                    required: 16,
                    ..site(
                        rf::STAGE_READINESS,
                        1,
                        self.next[1],
                        None,
                        starts[1].unwrap(),
                        now,
                    )
                });
                return Err("output_lead_deadline");
            }
            let captured = self.io.shared.captured.load(SeqCst);
            self.retry.begin(captured, now.ticks);
            let ready = self.io.stage_output(f.first, frames)?;
            let defer = OutputPolicy::WhenReady.defer(ready, lead);
            self.retry.decision(captured, ready, defer, now.ticks);
            self.io.shared.retry.publish(&self.retry.snapshot);
            if defer {
                starts[1] = None;
                chosen = SubmissionOrder::Deadline.choose(starts);
            }
        }
        let Some(d) = chosen else {
            self.prior_step = Some(now);
            return Ok(false);
        };
        let seq = self.next[d];
        let s = free[d].unwrap();
        let start = starts[d].unwrap();
        let late = if d == 0 {
            "stale_schedule"
        } else {
            "output_lead_deadline"
        };
        let required = if d == 0 { 2 } else { 16 };
        let checked = clock()?;
        if start < checked.frame.saturating_add(required) {
            self.site = Some(Site {
                required,
                ..site(rf::STAGE_BEFORE_PACKING, d, seq, Some(s), start, checked)
            });
            return Err(late);
        }
        let data = unsafe { (self.api.buffer)(self.route, d as u32, s as u32) };
        if data.is_null() {
            return Err("native_callback_fault");
        }
        let bytes = unsafe { std::slice::from_raw_parts_mut(data, BYTES) };
        let mut lengths = [312u16; 32];
        let mut crc = 0;
        if d == 1 {
            let f = self.feedback[seq % HISTORY].unwrap();
            for (length, p) in lengths.iter_mut().zip(f.c.packets) {
                *length = p.actual;
            }
            let frames = lengths.iter().map(|n| usize::from(*n / 24)).sum();
            bytes.fill(0);
            self.io.output(f.first, frames, bytes)?;
            crc = crate::pool::crc32(bytes);
        }
        // A preempted packing pass must not submit against a stale bus reading.
        let checked = clock()?;
        if start < checked.frame.saturating_add(required) {
            self.site = Some(Site {
                required,
                ..site(rf::STAGE_AFTER_PACKING, d, seq, Some(s), start, checked)
            });
            return Err(late);
        }
        let t = ticket(self.epoch, seq, d, s);
        assert!(
            self.link.slots[d][s].prepare(self.previous[d][s], t, |a| *a = Admission {
                seq,
                crc,
                start,
                checked: checked.frame,
                ticks: checked.ticks,
                lengths
            })
        );
        self.previous[d][s] = t.generation;
        let mut token = 0;
        let status = unsafe {
            (self.api.submit)(
                self.route,
                d as u32,
                s as u32,
                t.generation,
                start,
                lengths.as_ptr(),
                &mut token,
            )
        };
        self.flights[d][s] = Some(Return { ticket: t, token });
        let returned = clock();
        if d == 1 {
            self.io.submitted(
                start,
                checked.frame,
                returned.as_ref().ok().map(|c| c.frame),
            );
        }
        let returned = match returned {
            Ok(r) => r,
            Err(e) => {
                // Submitted, but no return clock exists: generic, never zeroed.
                self.site = Some(site(rf::STAGE_GENERIC, d, seq, Some(s), start, checked));
                return Err(e);
            }
        };
        if status != 0 {
            self.site = Some(Site {
                returned: Some((returned.frame, returned.ticks)),
                submit_status: Some(status),
                ..site(rf::STAGE_SUBMIT, d, seq, Some(s), start, checked)
            });
            self.io.shared.submit_status.store(status, SeqCst);
            self.io.shared.submit_direction.store(d as u32, SeqCst);
            for (a, v) in self.io.shared.submit_detail.iter().zip([
                seq as u64,
                start,
                checked.frame,
                returned.frame,
            ]) {
                a.store(v, SeqCst);
            }
            if unsafe { (self.api.fault)(self.route) } == 0
                && (token == 0 || unsafe { (self.api.ack)(self.route, d as u32, s as u32, token) })
            {
                let slot = &self.link.slots[d][s];
                assert!(slot.complete(t, |_| {}));
                assert!(slot.claim(t).is_some());
                assert!(slot.publish(t));
                assert!(slot.release(t));
                assert!(slot.acknowledge(t));
                self.flights[d][s] = None;
            }
            return Err("submit_failed");
        }
        self.next[d] += 1;
        if d == 1 {
            self.feedback[seq % HISTORY] = None;
            self.min_lead = self.min_lead.min(start.saturating_sub(returned.frame));
        }
        self.progress();
        let required = if d == 0 { 1 } else { 16 };
        if start < returned.frame.saturating_add(required) {
            self.site = Some(Site {
                required,
                returned: Some((returned.frame, returned.ticks)),
                ..site(rf::STAGE_AFTER_RETURN, d, seq, Some(s), start, checked)
            });
            return Err(late);
        }
        self.prior_step = Some(now);
        self.prior_progress = Some(returned);
        Ok(true)
    }
    fn stop(&mut self) -> bool {
        let mut report = self.io.shared.shutdown.begin(self.epoch);
        let result = self.stop_report(&mut report);
        report.words[4] = 1;
        self.io.shared.shutdown.publish(&report);
        result
    }
    fn stop_report(&mut self, report: &mut crate::shutdown::Snapshot) -> bool {
        self.link.stopping.store(true, Release);
        let joined = unsafe { (self.api.stop)(self.route, 1500) };
        report.words[5] = u64::from(joined);
        report.words[9] = u64::from(self.link.fault.load(Acquire));
        unsafe {
            (self.api.snapshot)(self.route, report.words[12..28].as_mut_ptr());
        }
        if !joined || self.link.fault.load(Acquire) {
            return false;
        }
        // Native stop consumed RETURNED acknowledgements. Retire matching Rust
        // generations only after join; no callback can mutate them anymore.
        for d in 0..2 {
            for s in 0..DEPTH {
                let (generation, state) = self.link.slots[d][s].state();
                if state == State::Released {
                    let t = Ticket {
                        key: (d * DEPTH + s) as u32,
                        epoch: self.epoch,
                        generation,
                    };
                    if !self.link.slots[d][s].acknowledge(t) {
                        return false;
                    }
                } else if state != State::Free {
                    return false;
                }
            }
        }
        report.words[6] = 1;
        report.words[7] = 1;
        if !unsafe { (self.api.destroy)(self.route) } {
            // Failed destroy retains the route, so its quarantine is readable.
            unsafe {
                (self.api.snapshot)(self.route, report.words[12..28].as_mut_ptr());
            }
            return false;
        }
        report.words[8] = 1;
        unsafe { drop(Box::from_raw(self.ctx)) };
        self.ctx = std::ptr::null_mut();
        self.route = std::ptr::null_mut();
        self.flights = [[None; DEPTH]; 2];
        self.progress();
        true
    }
}
// No Drop closes a native context: panic/timeout intentionally retain context,
// Arc<Shared>, slots and source references while any callback could still exist.

// PoolSession supplies first-failure/teardown reporting only in this runtime.
// Its legacy scheduling setters accept the ten-slot prototype, not this owner.
// Owner owns the actual readiness policy and joint submission order. Keep the
// report geometry and initial retry metadata correct without configuring the
// unused legacy scheduler or widening its qualification guards.
fn diagnostic_session(epoch: u64, info: Info) -> PoolSession {
    let mut session = PoolSession::with_geometry(epoch, info, LIVE);
    session.retry = crate::output_retry::Tracker::new(
        epoch,
        OutputPolicy::WhenReady,
        info.timebase_numer,
        info.timebase_denom,
    );
    session
}

/// Test-only runner seam (linked native-integration builds only): the
/// production worker, factory lifecycle and guards, with the linked fake route
/// in place of the IOKit route, a virtual bus clock, and one owner-thread
/// service call per stream pass that delivers fake completions. Never present
/// in a driver or library build; no production check reads it.
#[cfg(all(test, feature = "native-integration"))]
#[derive(Clone, Copy)]
struct Seam {
    api: Api,
    attach: fn(&Hooks) -> *mut c_void,
    clock: fn() -> Result<Clock, &'static str>,
    service: fn(&Owner),
}
#[cfg(all(test, feature = "native-integration"))]
static SEAM: std::sync::Mutex<Option<Seam>> = std::sync::Mutex::new(None);

/// Teardown outcomes for this epoch's runtime record, if it published one.
fn finish_runtime(
    shared: &Shared,
    owned: bool,
    epoch: u64,
    drain_failed: bool,
    cleanup: i32,
    close: Option<i32>,
    phase: u32,
) {
    if owned {
        shared.runtime.finish(
            epoch,
            rf::Terminal {
                drain_failed,
                cleanup,
                close,
                phase,
                fault: shared.fault.load(SeqCst),
            },
        );
    }
}

pub(super) unsafe fn worker(
    shared: Arc<Shared>,
    factory: Factory,
    product: u32,
    location: u32,
    registry: u64,
    epoch: u64,
) {
    let ctx = (factory.open)(product, location, registry);
    if ctx.is_null() {
        shared.fault.store(1, SeqCst);
        shared.set_phase(FAULT);
        return;
    }
    let operations = (factory.ops)(ctx);
    if operations.is_null() {
        shared.fault.store(2, SeqCst);
        shared.phase.store(QUARANTINED, SeqCst);
        return;
    }
    let ops = &*operations;
    if !ops.compatible() {
        shared.fault.store(15, SeqCst);
        let closed = (factory.close)(ctx);
        shared.set_phase(if closed == 0 { FAULT } else { QUARANTINED });
        return;
    }
    let mut info = Info::default();
    let mut rates = Rates::default();
    let mut config = vec![0; 65535];
    let mut length = 0;
    let mut owner: Option<Owner> = None;
    let mut session = diagnostic_session(epoch, info);
    let mut activated = false;
    let mut policy_attempted = false;
    // This epoch published the current ftuE record and must finish it.
    let mut runtime_owned = false;
    let mut startup = crate::startup::Snapshot::new(epoch);
    startup.header[3] = 1;
    shared.startup.publish(&startup);
    let config_status = (ops.base.config)(ctx, config.as_mut_ptr(), config.len(), &mut length);
    (ops.base.info)(ctx, &mut info);
    (ops.base.rates)(ctx, &mut rates);
    let before_rates = rates;
    startup.header[5] = config_status as i64 as u64;
    startup.before(rates);
    shared.startup.publish(&startup);

    if config_status != 0
        || length > config.len()
        || !ftu_protocol::validate_profile(&config[..length], false)
        || !ftu_protocol::validate_profile(&config[..length], true)
        || !rates.valid()
        || info.callback_fault != 0
        || info.timebase_numer == 0
        || info.timebase_denom == 0
    {
        startup.header[4] = 2;
        shared.startup.publish(&startup);
        session.stop("setup_precondition");
        shared.fault.store(3, SeqCst);
    } else {
        shared
            .worker_timing
            .configure_geometry(info.timebase_numer, info.timebase_denom, LIVE);
        shared
            .delivery
            .configure_geometry(info.timebase_numer, info.timebase_denom, LIVE);
        shared
            .pumps
            .configure(info.timebase_numer, info.timebase_denom);
        session = diagnostic_session(epoch, info);
        shared.retry.publish(&session.retry.snapshot);
        startup.header[3] = 2;
        startup.header[10] = 1;
        shared.startup.publish(&startup);
        let prepared = (ops.prepare_geometry)(ctx, &LIVE);
        startup.header[6] = prepared as i64 as u64;
        startup.header[10] = 2;
        if prepared != 0 {
            startup.header[4] = 2;
            session.stop("prepare_failed");
            shared.fault.store(4, SeqCst);
        } else if shared.stop.load(SeqCst) {
            startup.header[4] = 3;
            session.stop("signal");
        } else {
            startup.header[3] = 3;
            startup.header[11] = 1;
            shared.startup.publish(&startup);
            let activation = (ops.base.activate)(ctx);
            startup.header[7] = activation as i64 as u64;
            startup.header[11] = 2;
            if activation != 0 {
                startup.header[4] = 2;
                session.stop("activate_failed");
                shared.fault.store(5, SeqCst);
            } else {
                activated = true;
                match crate::startup::initialize(
                    ctx,
                    ops,
                    factory,
                    &shared.startup,
                    &mut startup,
                    || shared.stop.load(SeqCst),
                ) {
                    Err(reason) => {
                        session.stop(reason);
                        if reason != "signal" {
                            shared.fault.store(14, SeqCst);
                        }
                    }
                    Ok(()) => {
                        #[cfg(all(test, feature = "native-integration"))]
                        let seam = *SEAM.lock().unwrap_or_else(|e| e.into_inner());
                        let (api, attach): (Api, &dyn Fn(&Hooks) -> *mut c_void) =
                            (NATIVE, &|hooks: &Hooks| ftu_live_route_attach(ctx, hooks));
                        #[cfg(all(test, feature = "native-integration"))]
                        let (api, attach): (
                            Api,
                            &dyn Fn(&Hooks) -> *mut c_void,
                        ) = match &seam {
                            Some(s) => (s.api, &s.attach),
                            None => (api, attach),
                        };
                        owner = Owner::new(shared.clone(), epoch, info, api, attach);
                        policy_attempted = owner.is_some();
                        if owner.is_some() && (factory.configure_thread)(ctx, &THREAD_SCHEDULE) != 0
                        {
                            session.stop("thread_policy_failed");
                            shared.fault.store(12, SeqCst);
                        } else {
                            if let Some(o) = owner.as_mut() {
                                let clock = || {
                                    #[cfg(all(test, feature = "native-integration"))]
                                    if let Some(s) = &seam {
                                        return (s.clock)();
                                    }
                                    let mut c = Clock::default();
                                    if (ops.base.clock)(ctx, &mut c) == 0 {
                                        Ok(c)
                                    } else {
                                        Err("clock_query_failed")
                                    }
                                };
                                let streamed = catch_unwind(AssertUnwindSafe(|| 'stream: loop {
                                    #[cfg(all(test, feature = "native-integration"))]
                                    if let Some(s) = &seam {
                                        (s.service)(o);
                                    }
                                    for _ in 0..crate::submission_order::CATCH_UP_ATTEMPTS {
                                        match o.step(clock) {
                                            Ok(true) => (),
                                            Ok(false) => {
                                                thread::sleep(Duration::from_millis(1));
                                                continue 'stream;
                                            }
                                            Err(reason) => {
                                                session.stop(reason);
                                                // Publish before any teardown can
                                                // disturb owner/queue state.
                                                if reason != "signal" {
                                                    let error = o.link.error.load(Acquire);
                                                    let code = if error != 0 {
                                                        error
                                                    } else {
                                                        reason_code(Some(reason))
                                                    };
                                                    shared.runtime.publish(
                                                        &o.runtime_fault(code, shared.uptime_ms()),
                                                    );
                                                    runtime_owned = true;
                                                }
                                                break 'stream;
                                            }
                                        }
                                    }
                                    thread::yield_now();
                                }));
                                if streamed.is_err() {
                                    o.link.fault.store(true, Release);
                                    shared.fault.store(11, SeqCst);
                                    shared.phase.store(QUARANTINED, SeqCst);
                                    session.stop("native_callback_fault");
                                    // A panic has no precise guard clock.
                                    o.site = None;
                                    shared.runtime.publish(&o.runtime_fault(
                                        reason_code(Some("native_callback_fault")),
                                        shared.uptime_ms(),
                                    ));
                                    runtime_owned = true;
                                }
                                session.next = o.next;
                                session.completed = std::array::from_fn(|d| {
                                    o.link.completed[d].load(SeqCst) as usize
                                });
                                session.frames =
                                    std::array::from_fn(|d| o.link.frames[d].load(SeqCst));
                                if let Some(f) = o.failures.pop() {
                                    let a = f.admission;
                                    let mut lengths = [0; 64];
                                    lengths[..32].copy_from_slice(&a.lengths);
                                    session.completion_failure = Some(CompletionFailure {
                                        direction: f.direction,
                                        reason: f.reason,
                                        trace: Trace {
                                            flight: Some(Flight {
                                                seq: a.seq,
                                                ticket: a.seq as u64 + 1,
                                                start: a.start,
                                            }),
                                            slot: f.slot,
                                            submitted_at: a.checked,
                                            admission_ticks: a.ticks,
                                            submit: Some(0),
                                            completion: Some(PoolCompletion {
                                                ticket: a.seq as u64 + 1,
                                                packet_count: 32,
                                                reserved: 0,
                                                result: f.c,
                                                pump: Pump::default(),
                                            }),
                                            lengths,
                                            ..Trace::default()
                                        },
                                    });
                                }
                                // Preserve a specific completion/clock failure code.
                                let error = o.link.error.load(Acquire);
                                if error != 0 {
                                    shared.reason.store(error, SeqCst);
                                }
                            } else {
                                session.stop("native_route_attach_failed");
                            }
                        }
                    }
                }
            }
        }
        shared.startup.publish(&startup);
    }
    let mut failure_finish = crate::first_failure::Finish {
        epoch,
        activated,
        before_rates,
        after_rates: None,
        before_info: session.info,
        after_info: None,
        cleanup: 0,
        close: None,
    };
    let owns_failure_record = shared.first_failure.latch(&session, &failure_finish);
    if shared.phase.load(SeqCst) != QUARANTINED {
        shared.set_phase(STOPPING);
    }
    // Restore normal scheduling before abort/drain and native teardown.
    if policy_attempted && (factory.configure_thread)(ctx, std::ptr::null()) != 0 {
        // A normal stop's "signal" is not the cause of a restore failure.
        // Preserve any earlier transport error, but make the policy failure
        // independently distinguishable from an apply failure (fault 12).
        if session.reason == Some("signal") {
            session.reason = None;
        }
        session.stop("thread_policy_restore_failed");
        shared.fault.store(13, SeqCst);
    }
    if let Some(o) = owner.as_mut() {
        session.drain_failed = !o.stop();
        session.completed = std::array::from_fn(|d| o.link.completed[d].load(SeqCst) as usize);
        session.frames = std::array::from_fn(|d| o.link.frames[d].load(SeqCst));
    }
    if shared.reason.load(SeqCst) == 0 {
        shared.reason.store(reason_code(session.reason), SeqCst);
    }
    if let Some(f) = session.submission_failure {
        shared.submit_status.store(f.status, SeqCst);
        shared.submit_direction.store(f.direction, SeqCst);
        for (a, v) in shared
            .submit_detail
            .iter()
            .zip([f.sequence, f.start, f.checked, f.returned])
        {
            a.store(v, SeqCst);
        }
    }
    // No runtime callback can reference a timeline after the worker returns;
    // the permanent Shared object outlives admitted HAL calls independently.
    if !session.drain_failed {
        shared.clock.publish(None);
    }
    for d in 0..2 {
        shared.submitted[d].store(session.next[d] as u64, SeqCst);
        shared.completed[d].store(session.completed[d] as u64, SeqCst);
        if !session.drain_failed {
            shared.pending[d].store(0, SeqCst);
        }
    }
    let cleanup = if session.drain_failed {
        PENDING_OWNERSHIP
    } else {
        (ops.base.cleanup)(ctx)
    };
    shared.cleanup.store(cleanup, SeqCst);
    failure_finish.cleanup = cleanup;
    let mut removed_mask = 0;
    if let Some(o) = owner.as_ref() {
        let mut report = shared.shutdown.snapshot();
        if report.words[1] == 1 && report.words[3] == epoch {
            unsafe {
                (o.api.cleanup_snapshot)(ctx, report.words[28..44].as_mut_ptr());
            }
            report.words[10] = u64::from(report.words[28] == 1);
            if report.words[8] == 1 && report.words[10] == 1 {
                removed_mask = report.words[30] as u32;
            }
            shared.shutdown.publish(&report);
        }
    }

    if cleanup != 0 || session.drain_failed {
        shared
            .first_failure
            .finish(&session, &failure_finish, owns_failure_record);
        // Quarantine at most this one native context. Never _Exit the HAL host,
        // free uncertain USB memory, or retry over outstanding callback debt.
        shared.fault.store(7, SeqCst);
        let drain_failed = session.drain_failed;
        finish_runtime(
            &shared,
            runtime_owned,
            epoch,
            drain_failed,
            cleanup,
            None,
            QUARANTINED,
        );
        shared.phase.store(QUARANTINED, SeqCst);
        return;
    }
    (ops.base.info)(ctx, &mut info);
    (ops.base.rates)(ctx, &mut rates);
    failure_finish.after_info = Some(info);
    failure_finish.after_rates = Some(rates);
    // A removed endpoint cannot restore/read back its alternate or rate. Keep
    // their actual failed observations; only a successfully destroyed route can
    // report a removal mask. Connected endpoints still require restoration.
    let bad_postcondition = activated
        && ((info.restored_mask | removed_mask) != 3
            || info.open_mask != 0
            || !(0..2).all(|d| {
                // Native direction 0=IN, 1=OUT; rate replies are OUT then IN.
                let e = 1 - d;
                removed_mask & (1 << d) != 0
                    || (rates.status[e] == 0 && rates.actual[e] == 3 && rates.hz[e] == 48000)
            }));
    if bad_postcondition {
        shared.fault.store(10, SeqCst);
    } else if removed_mask != 0 && shared.fault.load(SeqCst) == 0 {
        // Ownership retired safely, but the old USB binding is gone. Do not
        // auto-recover even if a deadline (55) happened to fail first. Rebind
        // requires the monitor's normal removal/arrival and fresh enumeration.
        shared.fault.store(16, SeqCst);
    }
    let closed = (factory.close)(ctx);
    failure_finish.close = Some(closed);
    shared
        .first_failure
        .finish(&session, &failure_finish, owns_failure_record);
    if closed != 0 {
        shared.cleanup.store(closed, SeqCst);
        shared.fault.store(8, SeqCst);
        finish_runtime(
            &shared,
            runtime_owned,
            epoch,
            false,
            cleanup,
            Some(closed),
            QUARANTINED,
        );
        shared.phase.store(QUARANTINED, SeqCst);
        return;
    }
    if shared.phase.load(SeqCst) == QUARANTINED {
        finish_runtime(
            &shared,
            runtime_owned,
            epoch,
            false,
            cleanup,
            Some(closed),
            QUARANTINED,
        );
        return;
    }
    if session.reason == Some("signal")
        && shared.stop.load(SeqCst)
        && !bad_postcondition
        && shared.fault.load(SeqCst) == 0
    {
        finish_runtime(
            &shared,
            runtime_owned,
            epoch,
            false,
            cleanup,
            Some(closed),
            IDLE,
        );
        shared.set_phase(IDLE);
    } else {
        if shared.fault.load(SeqCst) == 0 {
            shared.fault.store(6, SeqCst);
        }
        // Finish before FAULT is visible, so a reader of the phase finds the
        // complete record. The monitor additionally waits for the join.
        finish_runtime(
            &shared,
            runtime_owned,
            epoch,
            false,
            cleanup,
            Some(closed),
            FAULT,
        );
        shared.set_phase(FAULT);
    }
}

#[cfg(all(test, feature = "native-integration"))]
#[path = "independent_live_tests.rs"]
mod tests;
