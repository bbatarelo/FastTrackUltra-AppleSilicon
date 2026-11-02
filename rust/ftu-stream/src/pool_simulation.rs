//! Recycled native slots with adversarial completion order and failures.
use crate::transport::{Transport, BASELINE, POOL_ABI_VERSION};
use crate::{continuous::ftu_stream_run, native::*, pool::*, scheduler::*};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    ffi::c_void,
};
thread_local! {static TRACK:Cell<bool>=const{Cell::new(false)};static ALLOCS:Cell<usize>=const{Cell::new(0)};}
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if TRACK.try_with(|v| v.get()).unwrap_or(false) {
            let _ = ALLOCS.try_with(|v| v.set(v.get() + 1));
        }
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if TRACK.try_with(|v| v.get()).unwrap_or(false) {
            let _ = ALLOCS.try_with(|v| v.set(v.get() + 1));
        }
        System.realloc(p, l, n)
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
/// Test-only pump hypothesis; no native ABI or live policy uses this type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Dispatch {
    #[default]
    Legacy,
    Separate {
        ready_first: bool,
        callback_us: u64,
        entry_us: u64,
        batch: usize,
    },
}
pub(crate) const READY_DISPATCH: Dispatch = Dispatch::Separate {
    ready_first: true,
    callback_us: 50,
    entry_us: 0,
    batch: 10,
};
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DispatchStats {
    pub calls: u64,
    pub idle_calls: u64,
    pub idle_us: u64,
    pub callbacks: u64,
    pub callback_us: u64,
    pub entry_us: u64,
    pub max_batch: usize,
    pub max_ready_delay_us: u64,
    pub stalls: u64,
    pub stall_us: u64,
}
#[derive(Clone)]
struct Transfer {
    ticket: u64,
    start: u64,
    lengths: [u16; 64],
    reported: bool,
    delivered_ticks: Option<u64>,
    aborted: bool,
}
struct Fake {
    prepared: usize,
    pump_calls: u64,
    dispatched_frame: u64,
    dispatch_stats: DispatchStats,
    submissions_since_pump: usize,
    max_submissions_between_pumps: usize,
    pump_step_ms: u64,
    delay_ms: u64,
    stall_frame: Option<u64>,
    stall_ms: u64,
    submit_cost_ms: u64,
    subframe_us: u64,
    work: PcmWorkload,
    fixed_traced: bool,
    frame: u64,
    transfers: [Vec<Option<Transfer>>; 2],
    data: [Vec<Vec<u8>>; 2],
    geometry: Transport,
    capture_constant: f32,
    drift_ppm: i64,
    wall: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    packet_phase: u64,
    fault: u8,
    cap_delivered: usize,
    stop: *mut i32,
    cleaned: bool,
    policy_active: bool,
    order: [Vec<u64>; 2],
    epoch: u64,
}
impl Fake {
    fn new(fault: u8, stop: *mut i32, count: usize) -> Self {
        Self {
            prepared: DEPTH,
            pump_calls: 0,
            dispatched_frame: 100,
            dispatch_stats: DispatchStats::default(),
            submissions_since_pump: 0,
            max_submissions_between_pumps: 0,
            pump_step_ms: 1,
            delay_ms: 0,
            stall_frame: None,
            stall_ms: 0,
            submit_cost_ms: 0,
            subframe_us: 0,
            work: PcmWorkload::default(),
            fixed_traced: false,
            frame: 100,
            transfers: std::array::from_fn(|_| vec![None; 4]),
            geometry: BASELINE,
            capture_constant: 0.,
            drift_ppm: 0,
            wall: None,
            packet_phase: 0,
            data: std::array::from_fn(|_| vec![vec![0; BLOCK_BYTES]; 4]),
            fault,
            cap_delivered: 0,
            stop,
            cleaned: false,
            policy_active: false,
            order: [Vec::with_capacity(count), Vec::with_capacity(count)],
            epoch: 1,
        }
    }
    fn current_us(&self) -> u64 {
        let local = self.frame * 1000 + self.subframe_us;
        self.wall.as_ref().map_or(local, |w| {
            local.max(w.load(std::sync::atomic::Ordering::SeqCst))
        })
    }
    fn set_us(&mut self, us: u64) {
        assert!(us >= self.current_us(), "virtual clock moved backwards");
        if let Some(w) = self.work.fixed_absence {
            if !self.fixed_traced && us >= w.end {
                crate::capture_trace::record("fixed_begin", w.start, 0, w.end, 0, 0);
                crate::capture_trace::record("fixed_end", w.end, 0, w.start, 0, 0);
                self.fixed_traced = true;
            }
        }
        self.frame = us / 1000;
        self.subframe_us = us % 1000;
        if let Some(w) = &self.wall {
            w.store(us, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn sync_wall(&mut self) {
        let now = self.frame * 1000 + self.subframe_us;
        let at = self.wall.as_ref().map_or(now, |w| {
            now.max(w.load(std::sync::atomic::Ordering::SeqCst))
        });
        self.set_us(crate::fixed_replay::resume(at, self.work.fixed_absence));
    }
    fn advance_us(&mut self, us: u64) {
        let at = self.current_us();
        self.set_us(crate::fixed_replay::work(at, us, self.work.fixed_absence));
    }
    fn wait_until(&mut self, at: u64) {
        self.set_us(crate::fixed_replay::resume(
            at.max(self.current_us()),
            self.work.fixed_absence,
        ));
    }
}
impl Fake {
    fn next_request(&self) -> Option<(u64, usize, usize)> {
        let mut best = None;
        for (d, slots) in self.transfers.iter().enumerate() {
            for (slot, t) in slots
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.as_ref().map(|t| (i, t)))
            {
                if t.reported || t.delivered_ticks.is_some() || (self.fault == 11 && t.ticket > 4) {
                    continue;
                }
                let delay = if self.work.uniform_delay || (t.ticket - 1).is_multiple_of(2) {
                    if self.fault == 8 {
                        10
                    } else {
                        self.delay_ms
                    }
                } else {
                    0
                };
                let due = if t.aborted {
                    0
                } else {
                    (t.start + self.geometry.block_ms() + delay) * 1000
                };
                let key = (due, d, slot);
                if best.is_none_or(|old| key < old) {
                    best = Some(key);
                }
            }
        }
        best
    }
    fn ready_request(&self) -> Option<(u64, usize, usize)> {
        self.next_request()
            .filter(|r| r.0 <= self.frame * 1000 + self.subframe_us)
    }
    fn dispatch_stall(&mut self) {
        if self.stall_frame.is_some_and(|at| self.frame >= at) {
            self.advance_us(self.stall_ms * 1000);
            self.stall_frame = None;
        }
        if self.fault == 3 && self.cap_delivered >= 4 {
            self.advance_us(200_000);
            self.fault = 0;
        }
        if self.work.stall_at_frame != 0 && self.frame >= self.work.stall_at_frame {
            let us = self.work.stall_us;
            self.work.stall_at_frame = 0;
            crate::capture_trace::record(
                "stall_begin",
                self.frame * 1000 + self.subframe_us,
                0,
                us,
                0,
                0,
            );
            self.advance_us(us);
            crate::capture_trace::record(
                "stall_end",
                self.frame * 1000 + self.subframe_us,
                0,
                us,
                0,
                0,
            );
            self.dispatch_stats.stalls += 1;
            self.dispatch_stats.stall_us += us;
        }
    }
    fn pump_separate(&mut self) {
        self.dispatch_pending(true);
    }
    fn dispatch_pending(&mut self, allow_wait: bool) -> u64 {
        let Dispatch::Separate {
            ready_first,
            callback_us,
            entry_us,
            batch,
        } = self.work.dispatch
        else {
            unreachable!()
        };
        assert!((1..=10).contains(&batch));
        self.dispatch_stats.calls += 1;
        crate::capture_trace::record(
            "pump_enter",
            self.frame * 1000 + self.subframe_us,
            0,
            0,
            0,
            0,
        );
        self.advance_us(entry_us);
        self.dispatch_stats.entry_us += entry_us;
        self.dispatch_stall();
        if allow_wait && (!ready_first || self.ready_request().is_none()) {
            // Empty dispatch ALWAYS waits; no spinning on an empty queue. The
            // wait is deliberately still the same coarse 1/4/8 ms interval.
            assert!(self.pump_step_ms > 0);
            self.dispatch_stats.idle_calls += 1;
            crate::capture_trace::record(
                "idle_begin",
                self.frame * 1000 + self.subframe_us,
                0,
                self.pump_step_ms * 1000,
                0,
                0,
            );
            let now = self.frame * 1000 + self.subframe_us;
            let timeout = now + self.pump_step_ms * 1000;
            let due = self.next_request().map_or(timeout, |r| r.0.max(now));
            let wake = if self.work.wake_on_ready {
                timeout.min(due)
            } else {
                timeout
            };
            self.dispatch_stats.idle_us += wake - now;
            // Lateness is relative to the nominal wake, not added after an
            // overlapping absence. Per-dispatch entry work remains separate.
            self.wait_until(wake + self.work.wake_late_us);
            crate::capture_trace::record(
                "wake",
                self.frame * 1000 + self.subframe_us,
                0,
                wake,
                due,
                u64::from(self.work.wake_on_ready && due <= timeout),
            );
            crate::capture_trace::record(
                "idle_end",
                self.frame * 1000 + self.subframe_us,
                0,
                0,
                0,
                0,
            );
            self.dispatch_stall();
        }
        let began = self.frame * 1000 + self.subframe_us;
        let mut dispatched = 0;
        while dispatched < batch
            && (dispatched == 0 || self.frame * 1000 + self.subframe_us - began < 2000)
        {
            let Some((due, d, slot)) = self.ready_request() else {
                break;
            };
            self.advance_us(callback_us);
            self.dispatch_stall();
            let returned = self.frame * 1000 + self.subframe_us;
            self.transfers[d][slot].as_mut().unwrap().delivered_ticks = Some(returned * 24);
            crate::capture_trace::record(
                "deliver",
                returned,
                self.transfers[d][slot].as_ref().unwrap().ticket - 1,
                d as u64,
                due,
                slot as u64,
            );
            self.dispatch_stats.callbacks += 1;
            self.dispatch_stats.callback_us += callback_us;
            self.dispatch_stats.max_ready_delay_us = self
                .dispatch_stats
                .max_ready_delay_us
                .max(returned.saturating_sub(due));
            dispatched += 1;
        }
        self.dispatch_stats.max_batch = self.dispatch_stats.max_batch.max(dispatched);
        crate::capture_trace::record(
            "pump_exit",
            self.frame * 1000 + self.subframe_us,
            0,
            dispatched as u64,
            0,
            0,
        );
        if self.fault == 4 && self.frame >= 280 {
            unsafe {
                std::ptr::write_volatile(self.stop, 1);
            }
        }
        began * 24
    }
}

unsafe fn fake<'a>(ctx: *mut c_void) -> &'a mut Fake {
    &mut *ctx.cast::<Fake>()
}
unsafe extern "C" fn config(
    _: *mut c_void,
    out: *mut u8,
    capacity: usize,
    length: *mut usize,
) -> i32 {
    let b = include_bytes!("../../ftu-protocol/tests/fixtures/config-0.bin");
    assert!(capacity >= b.len());
    std::ptr::copy_nonoverlapping(b.as_ptr(), out, b.len());
    *length = b.len();
    0
}
unsafe extern "C" fn rates(_: *mut c_void, out: *mut Rates) {
    *out = Rates {
        status: [0; 2],
        hz: [48000; 2],
        actual: [3; 2],
        raw: [0x80, 0xbb, 0, 0x80, 0xbb, 0],
    };
}
unsafe extern "C" fn prepare(ctx: *mut c_void, count: u32) -> i32 {
    assert!((2..=DEPTH as u32).contains(&count));
    fake(ctx).prepared = count as usize;
    0
}
unsafe extern "C" fn prepare_geometry(ctx: *mut c_void, geometry: *const Transport) -> i32 {
    if geometry.is_null() || !(*geometry).valid() {
        return -1;
    }
    let f = fake(ctx);
    let g = *geometry;
    assert!(f.transfers.iter().flatten().all(Option::is_none));
    f.geometry = g;
    f.prepared = g.slots();
    f.transfers = std::array::from_fn(|_| vec![None; g.slots()]);
    f.data = std::array::from_fn(|_| vec![vec![0; g.bytes()]; g.slots()]);
    for slot in &mut f.data[0] {
        for sample in slot.chunks_exact_mut(3) {
            sample.copy_from_slice(&ftu_protocol::encode_s24(f.capture_constant));
        }
    }
    0
}
unsafe extern "C" fn activate(_: *mut c_void) -> i32 {
    0
}
unsafe extern "C" fn clock(ctx: *mut c_void, out: *mut Clock) -> i32 {
    let f = fake(ctx);
    f.sync_wall();
    *out = Clock {
        frame: f.frame,
        ticks: f.frame * 24000 + f.subframe_us * 24,
    };
    0
}
unsafe extern "C" fn buffer(ctx: *mut c_void, d: u32, slot: u32) -> *mut u8 {
    let f = fake(ctx);
    assert!(
        (slot as usize) < f.prepared,
        "native slot beyond prepared count"
    );
    assert!(f.transfers[d as usize][slot as usize]
        .as_ref()
        .is_none_or(|t| t.reported));
    f.data[d as usize][slot as usize].as_mut_ptr()
}
unsafe extern "C" fn old_submit(_: *mut c_void, _: u32, _: u32, _: u64, _: *const u16) -> i32 {
    panic!("one-shot submission not permitted")
}
unsafe extern "C" fn old_completion(_: *mut c_void, _: u32, _: u32, _: *mut Completion) -> i32 {
    panic!("one-shot completion not permitted")
}
unsafe extern "C" fn submit(
    ctx: *mut c_void,
    d: u32,
    slot: u32,
    ticket: u64,
    start: u64,
    lengths: *const u16,
) -> i32 {
    let f = fake(ctx);
    let d = d as usize;
    let slot = slot as usize;
    assert!(slot < f.prepared, "submit beyond prepared count");
    f.sync_wall();
    f.submissions_since_pump += 1;
    f.max_submissions_between_pumps = f
        .max_submissions_between_pumps
        .max(f.submissions_since_pump);
    let cost = if f.frame >= f.work.cost_start_frame {
        f.work.submit_us[d]
    } else {
        0
    };
    crate::capture_trace::record(
        "submit_begin",
        f.frame * 1000 + f.subframe_us,
        ticket - 1,
        d as u64,
        start,
        slot as u64,
    );
    f.advance_us(f.submit_cost_ms * 1000 + cost);
    assert!(f.transfers[d][slot].as_ref().is_none_or(|t| t.reported));
    if f.fault == 1 && d == 1 && ticket == 7 {
        return -99;
    }
    if f.fault == 13 && d == 0 && ticket == 11 {
        return 0xe00002c0u32 as i32;
    }
    let mut len = [0; 64];
    len[..f.geometry.entries()]
        .copy_from_slice(std::slice::from_raw_parts(lengths, f.geometry.entries()));
    crate::capture_trace::record(
        "submit",
        f.frame * 1000 + f.subframe_us,
        ticket - 1,
        d as u64,
        start,
        slot as u64,
    );
    f.transfers[d][slot] = Some(Transfer {
        ticket,
        start,
        lengths: len,
        reported: false,
        delivered_ticks: None,
        aborted: false,
    });
    0
}
unsafe extern "C" fn completion(
    ctx: *mut c_void,
    d: u32,
    slot: u32,
    out: *mut PoolCompletion,
) -> i32 {
    let f = fake(ctx);
    let d = d as usize;
    let slot = slot as usize;
    assert!(slot < f.prepared, "poll beyond prepared count");
    let Some(t) = f.transfers[d][slot].as_mut() else {
        return 0;
    };
    let seq = t.ticket - 1;
    let delay = if f.work.uniform_delay || seq.is_multiple_of(2) {
        if f.fault == 8 {
            10
        } else {
            f.delay_ms
        }
    } else {
        0
    };
    let available = if f.work.dispatch == Dispatch::Legacy {
        t.aborted || f.dispatched_frame >= t.start + f.geometry.block_ms() + delay
    } else {
        t.delivered_ticks.is_some()
    };
    if t.reported || !available {
        return 0;
    }
    if f.fault == 11 && seq >= 4 {
        return 0;
    }
    crate::capture_trace::record(
        "consume",
        f.frame * 1000 + f.subframe_us,
        seq,
        d as u64,
        t.delivered_ticks.unwrap_or(0) / 24,
        slot as u64,
    );
    let bad = t.aborted || (f.fault == 2 && d == 0 && seq == 8);
    let status = if bad { 0xe00002ebu32 as i32 } else { 0 };
    let mut c = Completion {
        status,
        matched: if f.fault == 6 && d == 0 && seq == 4 {
            0
        } else {
            1
        },
        callback_ticks: t.delivered_ticks.unwrap_or(f.dispatched_frame * 24000),
        packets: [Packet::default(); 64],
    };
    for (i, p) in c.packets[..f.geometry.entries()].iter_mut().enumerate() {
        *p = Packet {
            status,
            requested: t.lengths[i],
            actual: if d == 0 {
                {
                    let packet = seq * f.geometry.entries() as u64 + i as u64 + f.packet_phase;
                    let drift = |n: u64| n as i64 * 6 * f.drift_ppm / 1_000_000;
                    ((packet % 3 + 5) as i64 + drift(packet + 1) - drift(packet)) as u16 * 24
                }
            } else {
                t.lengths[i]
            },
            ticks: (t.start + i as u64 / 8) * 24000,
        };
    }
    if f.fault == 19 && d == 0 && seq == 0 && !t.aborted {
        c.status = 0xe00002e8u32 as i32;
        c.packets[0].status = c.status;
        c.packets[f.geometry.entries() - 1].status = 0x6c6c6974; // raw 'llit' sentinel, not normalized
        c.packets[f.geometry.entries() - 1].actual = 0;
    }
    if f.fault == 16 && d == 0 && seq == 4 {
        c.packets[0].actual = 24;
        c.packets[1].actual = 312;
    }
    if f.fault == 5 && d == 0 && seq == 4 {
        c.packets[0].actual = 0;
    }
    if f.fault == 10 && d == 0 && seq == 4 {
        c.packets[1].ticks += 1;
    }
    if f.fault == 17 && d == 0 && seq == 4 {
        // Relative pattern from the first real stream-tone failure: successful
        // packets processed at three distinct times within the final group.
        c.packets[f.geometry.entries() - 8].ticks -= 20809;
        c.packets[f.geometry.entries() - 7].ticks -= 17620;
    }
    if f.fault == 18 && d == 0 {
        for (i, p) in c.packets[..f.geometry.entries()].iter_mut().enumerate() {
            p.ticks -= (7 - i as u64 % 8) * 3000;
        }
    }
    if f.fault == 12 && d == 0 && seq == 10 {
        for p in &mut c.packets[..f.geometry.entries()] {
            p.ticks += 6 * 24000;
        }
    }
    if f.fault == 14 && d == 0 && seq == 6 {
        for p in &mut c.packets[..f.geometry.entries()] {
            p.ticks -= 8 * 24000;
        }
    }
    if f.fault == 26 && f.geometry.entries() < 64 {
        c.packets[f.geometry.entries()].actual = 312;
    }
    t.reported = true;
    f.order[d].push(seq);
    if d == 0 {
        f.cap_delivered += 1;
    }
    *out = PoolCompletion {
        ticket: if f.fault == 9 && d == 0 && seq == 4 {
            t.ticket - 1
        } else {
            t.ticket
        },
        packet_count: if f.fault == 24 {
            0
        } else {
            f.geometry.entries() as u32
        },
        reserved: u32::from(f.fault == 25),
        result: c,
        pump: Pump::default(),
    };
    1
}
unsafe extern "C" fn pump(ctx: *mut c_void) {
    let f = fake(ctx);
    f.pump_calls += 1;
    f.submissions_since_pump = 0;
    f.sync_wall();
    if f.work.dispatch != Dispatch::Legacy {
        f.pump_separate();
        return;
    }
    f.advance_us(f.pump_step_ms * 1000);
    if f.stall_frame.is_some_and(|at| f.frame >= at) {
        f.frame += f.stall_ms;
        f.stall_frame = None;
    }
    if f.work.stall_at_frame != 0 && f.frame >= f.work.stall_at_frame {
        f.advance_us(f.work.stall_us);
        f.work.stall_at_frame = 0;
    }
    f.advance_us(0);
    if f.fault == 3 && f.cap_delivered >= 4 {
        f.frame += 200;
        f.fault = 0;
    }
    f.dispatched_frame = f.frame;
    if f.fault == 4 && f.frame >= 280 {
        std::ptr::write_volatile(f.stop, 1);
    }
}
unsafe fn dispatch_ready(ctx: *mut c_void, allow_wait: bool) -> DispatchResult {
    let f = fake(ctx);
    f.sync_wall();
    f.pump_calls += 1;
    f.submissions_since_pump = 0;
    let before = f.dispatch_stats.callbacks;
    let service_begin = f.dispatch_pending(allow_wait);
    DispatchResult {
        progressed: f.dispatch_stats.callbacks > before,
        service_begin,
    }
}
unsafe extern "C" fn abort(ctx: *mut c_void, d: u32) -> i32 {
    assert!(
        !fake(ctx).policy_active,
        "restore scheduling before abort/drain"
    );
    for t in fake(ctx).transfers[d as usize].iter_mut().flatten() {
        if !t.reported {
            t.aborted = true;
        }
    }
    0
}
unsafe extern "C" fn cleanup(ctx: *mut c_void) -> i32 {
    let f = fake(ctx);
    assert!(f.transfers.iter().flatten().flatten().all(|t| t.reported));
    assert!(!f.policy_active, "restore scheduling before native cleanup");
    f.cleaned = true;
    if f.fault == 15 {
        0xe00002d5u32 as i32
    } else {
        0
    }
}
unsafe extern "C" fn info(ctx: *mut c_void, out: *mut Info) {
    let f = fake(ctx);
    *out = Info {
        timebase_numer: 125,
        timebase_denom: 3,
        restored_mask: if f.cleaned { 3 } else { 0 },
        open_mask: if f.cleaned { 0 } else { 3 },
        callback_fault: u32::from(f.fault == 7 && f.cap_delivered >= 4),
        ..Info::default()
    };
}
unsafe extern "C" fn now_ticks(ctx: *mut c_void) -> u64 {
    let f = fake(ctx);
    f.sync_wall();
    f.frame * 24000 + f.subframe_us * 24
}
const OPS: PoolOps = PoolOps {
    version: POOL_ABI_VERSION,
    size: std::mem::size_of::<PoolOps>() as u32,
    prepare_geometry,
    base: Ops {
        config,
        rates,
        prepare,
        activate,
        clock,
        buffer,
        submit: old_submit,
        completion: old_completion,
        pump,
        abort,
        cleanup,
        info,
    },
    submit,
    completion,
    now_ticks,
    worker_sample: None,
    last_pump: None,
};
fn simulate(fault: u8, count: usize, tone: bool) -> (PoolSession, Fake, [Vec<Trace>; 2], usize) {
    simulate_depth(fault, count, tone, DEPTH)
}
fn simulate_depth(
    fault: u8,
    count: usize,
    tone: bool,
    depth: usize,
) -> (PoolSession, Fake, [Vec<Trace>; 2], usize) {
    simulate_geometry(fault, count, tone, Transport::new(8, depth).unwrap())
}
fn simulate_geometry(
    fault: u8,
    count: usize,
    tone: bool,
    geometry: Transport,
) -> (PoolSession, Fake, [Vec<Trace>; 2], usize) {
    simulate_geometry_policy(
        fault,
        count,
        tone,
        geometry,
        crate::output_admission::OutputPolicy::Eager,
    )
}
fn simulate_geometry_policy(
    fault: u8,
    count: usize,
    tone: bool,
    geometry: Transport,
    policy: crate::output_admission::OutputPolicy,
) -> (PoolSession, Fake, [Vec<Trace>; 2], usize) {
    simulate_geometry_order(
        fault,
        count,
        tone,
        geometry,
        policy,
        crate::submission_order::SubmissionOrder::InputFirst,
    )
}
fn simulate_geometry_order(
    fault: u8,
    count: usize,
    tone: bool,
    geometry: Transport,
    policy: crate::output_admission::OutputPolicy,
    order: crate::submission_order::SubmissionOrder,
) -> (PoolSession, Fake, [Vec<Trace>; 2], usize) {
    simulate_geometry_dispatch(
        fault,
        count,
        tone,
        geometry,
        policy,
        order,
        Dispatch::Legacy,
    )
}
fn simulate_geometry_dispatch(
    fault: u8,
    count: usize,
    tone: bool,
    geometry: Transport,
    policy: crate::output_admission::OutputPolicy,
    order: crate::submission_order::SubmissionOrder,
    dispatch: Dispatch,
) -> (PoolSession, Fake, [Vec<Trace>; 2], usize) {
    simulate_geometry_service(fault, count, tone, geometry, policy, order, dispatch, false)
}
#[allow(clippy::too_many_arguments)]
fn simulate_geometry_service(
    fault: u8,
    count: usize,
    tone: bool,
    geometry: Transport,
    policy: crate::output_admission::OutputPolicy,
    order: crate::submission_order::SubmissionOrder,
    dispatch: Dispatch,
    service_ready: bool,
) -> (PoolSession, Fake, [Vec<Trace>; 2], usize) {
    let mut stop = 0;
    let mut f = Fake::new(fault, &mut stop, count);
    f.work.dispatch = dispatch;
    unsafe {
        assert_eq!(prepare_geometry((&mut f as *mut Fake).cast(), &geometry), 0);
    }
    let mut s = PoolSession::with_geometry(
        f.epoch,
        Info {
            timebase_numer: 125,
            timebase_denom: 3,
            ..Info::default()
        },
        geometry,
    )
    .with_output_policy(policy)
    .with_submission_order(order);
    if service_ready {
        s = s.with_ready_dispatch(dispatch_ready);
    }
    let mut records = [vec![Trace::default(); count], vec![Trace::default(); count]];
    let waveform = if tone { tone_table() } else { Vec::new() };
    ALLOCS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    unsafe {
        let ctx = (&mut f as *mut Fake).cast();
        s.run(&OPS, ctx, &mut records, &waveform, &stop);
        s.drain(&OPS, ctx, &mut records);
    }
    TRACK.with(|v| v.set(false));
    let allocated = ALLOCS.with(|v| v.get());
    (s, f, records, allocated)
}
#[test]
fn thousands_of_reuses_without_loop_allocations() {
    let (s, _, r, n) = simulate(0, 2500, false);
    assert_eq!(s.reason, None);
    assert_eq!(s.frames[0], s.frames[1]);
    assert_eq!(s.reused, [2496; 2]);
    assert_eq!(s.pending(), [0; 2]);
    assert_eq!(n, 0);
    assert!(s.clock.snapshot().valid);
    for d in r {
        for (seq, r) in d.iter().enumerate() {
            assert_eq!(r.slot, seq % 4);
            assert_eq!(r.flight.unwrap().ticket, seq as u64 + 1);
        }
    }
}
#[test]
fn reordered_callbacks_keep_correct_generations() {
    let (s, f, _, n) = simulate(8, 256, false);
    assert_eq!(s.reason, None);
    assert_eq!(&f.order[0][..2], &[1, 0]);
    assert_eq!(&f.order[1][..2], &[1, 0]);
    assert_eq!(s.frames[0], s.frames[1]);
    assert_eq!(n, 0);
}
#[test]
fn failures_stop_and_drain_recycled_pool() {
    for (fault, reason) in [
        (1, "submit_failed"),
        (2, "request_completion_error"),
        (3, "stale_schedule"),
        (4, "signal"),
        (5, "packet_completion_error"),
        (6, "request_completion_error"),
        (7, "native_callback_fault"),
        (10, "USB timestamp group not monotonic"),
        (12, "clock gap: interval too long"),
        (13, "submit_failed"),
        (14, "timestamp not increasing"),
    ] {
        let (s, _, _, n) = simulate(fault, 128, false);
        assert_eq!(s.reason, Some(reason), "fault {fault}");
        assert_eq!(s.pending(), [0; 2]);
        assert!(!s.drain_failed);
        assert_eq!(n, 0);
        if matches!(fault, 10 | 12 | 14) {
            assert!(!s.clock.snapshot().valid);
        }
    }
}
#[test]
fn stale_ticket_never_releases_new_owner() {
    let (s, _, _, _) = simulate(9, 32, false);
    assert_eq!(s.reason, Some("stale_completion_ticket"));
    assert!(s.drain_failed);
    assert!(s.pending()[0] > 0);
}
#[test]
fn repeating_tone_survives_buffer_reuse() {
    let (s, _, r, n) = simulate(0, 1250, true);
    assert_eq!(s.reason, None);
    assert_eq!(n, 0);
    for r in &r[1] {
        assert_eq!(r.crc_before, r.crc_after);
        assert!(r.peaks.iter().all(|&v| v <= 83886));
        assert_eq!(&r.peaks[2..], &[0; 6]);
    }
    assert_eq!(s.frames[0], s.frames[1]);
}
#[test]
fn full_ffi_cycles_have_fresh_clock_epochs_and_auditable_evidence() {
    let path = std::env::temp_dir().join(format!(
        "ftu-pool-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    for epoch in 1..=6 {
        let dest = path.join(format!("session-{epoch:03}"));
        std::fs::create_dir(&dest).unwrap();
        let out = std::ffi::CString::new(dest.to_str().unwrap()).unwrap();
        let mut stop = 0;
        let duration = if epoch == 3 { 10000 } else { 256 };
        let mut f = Fake::new(
            match epoch {
                4 => 15,
                5 => 17,
                6 => 18,
                _ => 8,
            },
            &mut stop,
            duration as usize / 8,
        );
        f.epoch = epoch;
        let code = unsafe {
            ftu_stream_run(
                &OPS,
                (&mut f as *mut Fake).cast(),
                out.as_ptr(),
                duration,
                u32::from(epoch == 3),
                epoch,
                &stop,
            )
        };
        assert_eq!(code, if epoch == 4 { 1 } else { 0 });
        assert!(f.cleaned);
        let analyzer = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/ftu-probe/analyze_stream.py");
        let audit = std::process::Command::new("python3")
            .arg("-B")
            .arg(analyzer)
            .arg(&dest)
            .output()
            .unwrap();
        assert!(
            audit.status.success(),
            "{} {}",
            String::from_utf8_lossy(&audit.stdout),
            String::from_utf8_lossy(&audit.stderr)
        );
    }
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn processing_timestamp_variation_preserves_stream_and_clock() {
    for fault in [17, 18] {
        let (s, _, _, n) = simulate(fault, 256, false);
        assert_eq!(s.reason, None);
        assert_eq!(s.frames[0], s.frames[1]);
        assert_eq!(s.pending(), [0; 2]);
        assert_eq!(s.clock.snapshot().observations, 256 * 8);
        assert!(s.clock.snapshot().valid);
        assert_eq!(n, 0);
    }
}

#[test]
fn whole_frames_within_descriptor_capacity_are_mirrored_without_clipping() {
    let (s, _, r, n) = simulate(16, 128, false);
    assert_eq!(s.reason, None);
    assert_eq!(s.frames[0], s.frames[1]);
    assert_eq!(r[1][4].lengths[0], 24);
    assert_eq!(r[1][4].lengths[1], 312);
    assert_eq!(n, 0);
}

// Exercise the actual long-lived FFI lifecycle on the same native contract.
unsafe extern "C" fn live_fake_open(_: u32, _: u32, _: u64) -> *mut c_void {
    let mut f = Fake::new(0, std::ptr::null_mut(), 50000);
    f.capture_constant = 0.25;
    for slot in &mut f.data[0] {
        for sample in slot.chunks_exact_mut(3) {
            sample.copy_from_slice(&ftu_protocol::encode_s24(0.25));
        }
    }
    Box::into_raw(Box::new(f)).cast()
}
unsafe extern "C" fn live_fake_ops(_: *mut c_void) -> *const PoolOps {
    &OPS
}
unsafe extern "C" fn live_fake_close(ctx: *mut c_void) -> i32 {
    let f = Box::from_raw(ctx.cast::<Fake>());
    assert!(f.cleaned);
    assert!(!f.policy_active);
    0
}
unsafe extern "C" fn live_fake_set_rate(
    ctx: *mut c_void,
    direction: u32,
    hz: u32,
    actual: *mut u32,
) -> i32 {
    assert!(direction < 2);
    assert_eq!(hz, 48000);
    assert!(!fake(ctx).policy_active);
    assert!(fake(ctx).transfers.iter().flatten().all(Option::is_none));
    *actual = 3;
    0
}
unsafe extern "C" fn live_fake_configure(
    ctx: *mut c_void,
    schedule: *const crate::live::ThreadSchedule,
) -> i32 {
    if let Some(s) = schedule.as_ref() {
        assert_eq!(fake(ctx).prepared, crate::geometry::LIVE_QUEUE_DEPTH);
        assert_eq!(
            (s.period_ns, s.computation_ns, s.constraint_ns),
            (8_000_000, 1_000_000, 2_000_000)
        );
        assert!(!fake(ctx).policy_active);
        fake(ctx).policy_active = true;
    } else {
        fake(ctx).policy_active = false;
    }
    0
}
unsafe extern "C" fn restore_policy_error(
    ctx: *mut c_void,
    schedule: *const crate::live::ThreadSchedule,
) -> i32 {
    // The fake has no OS scheduling policy. Clear its ordering marker so the
    // existing abort/cleanup checks still establish restore-before-drain.
    live_fake_configure(ctx, schedule);
    if schedule.is_null() {
        -1
    } else {
        0
    }
}
unsafe extern "C" fn apply_policy_error(
    ctx: *mut c_void,
    schedule: *const crate::live::ThreadSchedule,
) -> i32 {
    if schedule.is_null() {
        live_fake_configure(ctx, schedule)
    } else {
        -1
    }
}
#[test]
fn live_thread_policy_apply_and_restore_failures_are_distinct() {
    use crate::live::*;
    for restore in [false, true] {
        let factory = Factory {
            open: live_fake_open,
            ops: live_fake_ops,
            close: live_fake_close,
            set_rate: live_fake_set_rate,
            configure_thread: if restore {
                restore_policy_error
            } else {
                apply_policy_error
            },
        };
        unsafe {
            let handle = ftu_live_create(&factory, 0x2080, 123);
            let started = ftu_live_start(handle, 1);
            assert_eq!(started == 0, restore);
            assert_eq!(ftu_live_stop(handle, 1), 0);
            let mut status = Status::default();
            ftu_live_status(handle, &mut status);
            assert_eq!(status.phase, 4);
            assert_eq!(status.fault, if restore { 13 } else { 12 });
            assert_eq!(status.reason, if restore { 5 } else { 4 });
            assert_eq!(status.pending, [0, 0]);
            assert_eq!(status.submitted, status.completed);
            assert_eq!(status.cleanup_status, 0);
            assert_eq!(ftu_live_destroy(handle), 0);
        }
    }
}
#[test]
fn live_session_start_clients_stop_restart_and_epoch_isolation() {
    use crate::live::*;
    use crate::timeline::ZeroTimestamp;
    let factory = Factory {
        open: live_fake_open,
        ops: live_fake_ops,
        close: live_fake_close,
        set_rate: live_fake_set_rate,
        configure_thread: live_fake_configure,
    };
    unsafe {
        let handle = ftu_live_create(&factory, 0x2080, 123);
        assert!(!handle.is_null());
        for epoch in 1..=3 {
            assert_eq!(ftu_live_start(handle, 7), 0);
            assert_eq!(ftu_live_start(handle, 7), 0);
            assert_eq!(ftu_live_start(handle, 8), 0);
            let mut z = ZeroTimestamp {
                epoch: 0,
                frame: 0,
                ticks: 0,
            };
            assert_eq!(ftu_live_timestamp(handle, &mut z), 0);
            assert_eq!(z.epoch, epoch);
            // ZTS may still be frame zero at startup. Address the live capture
            // cursor rather than assuming that a timestamp period has elapsed.
            let mut status = Status::default();
            ftu_live_status(handle, &mut status);
            assert!(status.captured >= 512);
            let mut data = [0.; 64 * 8];
            assert_eq!(
                ftu_live_read(handle, status.captured - 512, 64, data.as_mut_ptr()),
                0
            );
            assert!(data.iter().all(|&x| x == 0.25));
            assert_eq!(
                ftu_live_write(handle, status.captured + 4096, 64, data.as_ptr()),
                0
            );
            assert_eq!(ftu_live_write(handle, u64::MAX, 64, data.as_ptr()), -1);
            assert_eq!(ftu_live_stop(handle, 7), 0);
            assert_eq!(ftu_live_timestamp(handle, &mut z), 0);
            assert_eq!(ftu_live_stop(handle, 8), 0);
            assert_eq!(ftu_live_timestamp(handle, &mut z), -2);
            let mut status = Status::default();
            ftu_live_status(handle, &mut status);
            assert_eq!(status.phase, 0);
            assert_eq!(status.pending, [0, 0]);
            assert_eq!(status.submitted, status.completed);
            assert_eq!(status.cleanup_status, 0);
            assert_eq!(ftu_live_stop(handle, 8), 0);
            ftu_live_status(handle, &mut status);
            assert_eq!(status.phase, 0);
        }
        assert_eq!(ftu_live_destroy(handle), 0);
    }
}

#[cfg(not(feature = "independent-4ms"))]
static RECONNECT_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(not(feature = "independent-4ms"))]
static RECONNECT_OPENED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(not(feature = "independent-4ms"))]
static RECONNECT_REGISTRY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(not(feature = "independent-4ms"))]
static RECONNECT_OPENS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(not(feature = "independent-4ms"))]
unsafe extern "C" fn reconnect_open(product: u32, location: u32, registry: u64) -> *mut c_void {
    use std::sync::atomic::Ordering::SeqCst;
    assert_eq!(product, 0x2080);
    RECONNECT_OPENED.store(location, SeqCst);
    RECONNECT_REGISTRY.store(registry, SeqCst);
    RECONNECT_OPENS.fetch_add(1, SeqCst);
    live_fake_open(product, location, registry)
}
#[cfg(not(feature = "independent-4ms"))]
fn reconnect_factory() -> crate::live::Factory {
    crate::live::Factory {
        open: reconnect_open,
        ops: live_fake_ops,
        close: live_fake_close,
        set_rate: live_fake_set_rate,
        configure_thread: live_fake_configure,
    }
}
/// Removal while streaming, retired client IDs and their delayed Stop, then
/// rebind to a new port/enumeration and a fresh epoch. Baseline worker only:
/// the independent worker needs the linked route fixture, which is not wired
/// to the Factory path (see live::tests::reconnect for both profiles).
#[cfg(not(feature = "independent-4ms"))]
#[test]
fn live_detach_drains_running_session_and_rebind_starts_fresh_epoch() {
    use crate::live::*;
    use crate::timeline::ZeroTimestamp;
    use std::sync::atomic::Ordering::SeqCst;
    let _serial = RECONNECT_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let factory = reconnect_factory();
    unsafe {
        let handle = ftu_live_create_pinned(&factory, 0x2080, 0x0210_0000, 0x900);
        let mut z = ZeroTimestamp {
            epoch: 0,
            frame: 0,
            ticks: 0,
        };
        let mut data = [0.; 64 * 8];
        assert_eq!(ftu_live_start(handle, 7), 0);
        assert_eq!(ftu_live_start(handle, 8), 0);
        assert_eq!(RECONNECT_OPENED.load(SeqCst), 0x0210_0000);
        assert_eq!(RECONNECT_REGISTRY.load(SeqCst), 0x900);
        let mut previous = 1;
        for (cycle, location) in [0x2021_1100_u32, 0x2021_1100, 0x0210_0000, 0x0300_0000]
            .into_iter()
            .enumerate()
        {
            let registry = 0x901 + cycle as u64;
            assert_eq!(ftu_live_timestamp(handle, &mut z), 0);
            assert_eq!(z.epoch, previous);
            let mut status = Status::default();
            ftu_live_status(handle, &mut status);
            let stale = status.captured;
            let old_binding = ftu_live_binding(handle);
            // Removal: drained under the ordinary stop protocol, IO refused.
            // Clients 7 and 8 become retired.
            assert_eq!(ftu_live_detach(handle), 0);
            ftu_live_status(handle, &mut status);
            assert_eq!(status.phase, 0);
            assert_eq!(status.pending, [0, 0]);
            assert_eq!(status.submitted, status.completed);
            assert_eq!(status.cleanup_status, 0);
            assert_eq!(ftu_live_timestamp(handle, &mut z), -2);
            assert_eq!(
                ftu_live_read(handle, stale - 512, 64, data.as_mut_ptr()),
                -2
            );
            assert_eq!(ftu_live_write(handle, stale + 4096, 64, data.as_ptr()), -2);
            assert_eq!(ftu_live_start(handle, 9), -6);
            assert_eq!(ftu_live_rebind(handle, 0x2080, location, registry), 0);
            // A Start queued against the old publication never opens the new unit.
            let opens = RECONNECT_OPENS.load(SeqCst);
            assert_eq!(ftu_live_start_bound(handle, 9, old_binding), -6);
            // Reused IDs whose old session was never acknowledged are refused,
            // so their delayed Stop cannot stop a new session. No USB open.
            assert_eq!(ftu_live_start(handle, 7), -9);
            assert_eq!(ftu_live_start(handle, 8), -9);
            assert_eq!(RECONNECT_OPENS.load(SeqCst), opens);
            // A distinct new ID starts the new enumeration in a fresh epoch.
            assert_eq!(ftu_live_start_bound(handle, 9, ftu_live_binding(handle)), 0);
            assert_eq!(RECONNECT_OPENS.load(SeqCst), opens + 1);
            assert_eq!(RECONNECT_OPENED.load(SeqCst), location);
            assert_eq!(RECONNECT_REGISTRY.load(SeqCst), registry);
            assert_eq!(ftu_live_timestamp(handle, &mut z), 0);
            assert_eq!(
                z.epoch,
                previous + 1,
                "cycle {cycle}: new zero-timestamp seed"
            );
            // The delayed old Stop(7) only consumes 7's retirement...
            assert_eq!(ftu_live_stop(handle, 7), 0);
            ftu_live_status(handle, &mut status);
            assert_eq!(
                status.phase, 2,
                "cycle {cycle}: retired Stop left the session"
            );
            // ...after which 7 is an ordinary fresh client again.
            assert_eq!(ftu_live_start(handle, 7), 0);
            // 8 is acknowledged by Stop or by RemoveDeviceClient.
            if cycle % 2 == 0 {
                assert_eq!(ftu_live_stop(handle, 8), 0);
            } else {
                assert_eq!(ftu_live_remove_client(handle, 8), 0);
            }
            // Repeated unknown Stops never alter the running session.
            for _ in 0..3 {
                assert_eq!(ftu_live_stop(handle, 8), 0);
                assert_eq!(ftu_live_stop(handle, 4242), 0);
                assert_eq!(ftu_live_remove_client(handle, 8), 1);
            }
            assert_eq!(ftu_live_timestamp(handle, &mut z), 0);
            assert_eq!(z.epoch, previous + 1);
            ftu_live_status(handle, &mut status);
            assert_eq!((status.phase, status.location), (2, location));
            assert!(status.captured >= 512);
            assert_eq!(
                ftu_live_read(handle, status.captured - 512, 64, data.as_mut_ptr()),
                0
            );
            assert!(data.iter().all(|&x| x == 0.25));
            previous = z.epoch;
            // Restore clients 7 and 8 for the next removal.
            assert_eq!(ftu_live_stop(handle, 9), 0);
            assert_eq!(ftu_live_start(handle, 8), 0);
            assert_eq!(RECONNECT_OPENS.load(SeqCst), opens + 1, "joins never open");
        }
        assert_eq!(ftu_live_stop(handle, 7), 0);
        assert_eq!(ftu_live_stop(handle, 8), 0);
        let mut status = Status::default();
        ftu_live_status(handle, &mut status);
        assert_eq!(status.phase, 0);
        assert_eq!(ftu_live_destroy(handle), 0);
    }
}
/// Unacknowledged retirements accumulate across reconnects only up to the
/// fixed 64-slot client budget (active + retired); then distinct new IDs are
/// refused (-2) without opening USB until a Stop/RemoveDeviceClient frees one.
#[cfg(not(feature = "independent-4ms"))]
#[test]
fn live_retired_clients_stay_bounded_across_unacknowledged_reconnects() {
    use crate::live::*;
    use std::sync::atomic::Ordering::SeqCst;
    let _serial = RECONNECT_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let factory = reconnect_factory();
    unsafe {
        let handle = ftu_live_create_pinned(&factory, 0x2080, 0x10, 0xa00);
        let mut next = 100_u32;
        for round in 0..3_u64 {
            if round > 0 {
                assert_eq!(ftu_live_rebind(handle, 0x2080, 0x10, 0xa00 + round), 0);
            }
            for _ in 0..20 {
                assert_eq!(ftu_live_start(handle, next), 0);
                next += 1;
            }
            assert_eq!(ftu_live_detach(handle), 0);
        }
        // 60 retired: four more active clients fill the budget.
        assert_eq!(ftu_live_rebind(handle, 0x2080, 0x10, 0xa03), 0);
        for _ in 0..4 {
            assert_eq!(ftu_live_start(handle, next), 0);
            next += 1;
        }
        assert_eq!(ftu_live_start(handle, next), -2);
        assert_eq!(ftu_live_start(handle, 100), -9);
        // Detaching moves all 64 into retirement; still within the budget.
        assert_eq!(ftu_live_detach(handle), 0);
        assert_eq!(ftu_live_rebind(handle, 0x2080, 0x10, 0xa04), 0);
        let opens = RECONNECT_OPENS.load(SeqCst);
        assert_eq!(ftu_live_start(handle, next), -2);
        assert_eq!(
            RECONNECT_OPENS.load(SeqCst),
            opens,
            "a full budget never opens"
        );
        // One acknowledgement frees exactly one slot.
        assert_eq!(ftu_live_remove_client(handle, 100), 0);
        assert_eq!(ftu_live_start(handle, next), 0);
        assert_eq!(RECONNECT_OPENS.load(SeqCst), opens + 1);
        assert_eq!(ftu_live_start(handle, next + 1), -2);
        // Old Stops of the remaining retirements never touch the new session.
        for id in 101..next {
            assert_eq!(ftu_live_stop(handle, id), 0);
        }
        let mut status = Status::default();
        ftu_live_status(handle, &mut status);
        assert_eq!(status.phase, 2);
        assert_eq!(ftu_live_start(handle, 100), 0, "acknowledged IDs are fresh");
        assert_eq!(ftu_live_stop(handle, 100), 0);
        assert_eq!(ftu_live_stop(handle, next), 0);
        ftu_live_status(handle, &mut status);
        assert_eq!(status.phase, 0);
        assert_eq!(ftu_live_destroy(handle), 0);
    }
}

#[test]
fn live_journal_recycles_without_duration_storage_or_allocations() {
    struct Sink(
        crate::timing::Timing,
        crate::worker_timing::WorkerTiming,
        crate::worker_delivery::Delivery,
        crate::worker_pumps::Pumps,
    );

    impl StreamIo for Sink {
        fn worker_timing(&self) -> bool {
            true
        }
        fn worker_event(
            &mut self,
            mut e: crate::worker_timing::Event,
            _pump: Pump,
            _feedback_pump: Pump,
        ) {
            e.captured = 48000;
            self.1.observe(e);
            self.2.admission(e, _pump, _feedback_pump);
        }
        fn worker_completion(
            &mut self,
            d: usize,
            slot: usize,
            flight: Flight,
            c: &PoolCompletion,
            consumed: u64,
            draining: bool,
        ) {
            self.2
                .completion(d, slot, flight, c, consumed, 48000, draining);
        }
        fn worker_sample(&mut self, s: WorkerSample) {
            self.2.sample(s);
        }
        fn worker_pump(&mut self, p: PumpObservation, draining: bool) {
            self.3.observe(p, 48000, draining);
        }
        fn submission_timing(&self) -> bool {
            true
        }
        fn submitted(&mut self, start: u64, checked: u64, returned: Option<u64>) {
            self.0.observe(
                crate::timing::Metric::SubmitCheckLeadMs,
                crate::timing::distance(start, checked),
                48_000,
            );
            if let Some(returned) = returned {
                self.0.observe(
                    crate::timing::Metric::SubmitReturnLeadMs,
                    crate::timing::distance(start, returned),
                    48_000,
                );
            }
        }
    }

    unsafe extern "C" fn failed_sample(ctx: *mut c_void, out: *mut WorkerSample) {
        // Deliberately unavailable timestamps: Rust cadence must not trust them.
        *out = WorkerSample {
            cpu_status: 7,
            policy_status: 8,
            ..WorkerSample::default()
        };
        let _ = ctx;
    }
    let mut signal = 0;
    let mut f = Fake::new(0, &mut signal, 10000);
    f.prepared = DEPTH;
    let ctx = (&mut f as *mut Fake).cast::<c_void>();
    let mut s = PoolSession::with_depth(
        1,
        Info {
            timebase_numer: 125,
            timebase_denom: 3,
            ..Info::default()
        },
        DEPTH,
    );
    let mut records = [
        vec![Trace::default(); FEEDBACK_SLOTS],
        vec![Trace::default(); FEEDBACK_SLOTS],
    ];
    let mut journal = Journal {
        records: &mut records,
        recycle: true,
    };
    let sampled_ops = PoolOps {
        worker_sample: Some(failed_sample),
        ..OPS
    };
    let mut sink = Sink(
        crate::timing::Timing::new(),
        crate::worker_timing::WorkerTiming::new(),
        crate::worker_delivery::Delivery::new(),
        crate::worker_pumps::Pumps::new(),
    );
    sink.3.reset(1);
    sink.3.configure(125, 3);
    sink.2.reset(1);
    sink.2.configure(125, 3);
    sink.1.reset(1);
    sink.1.configure(125, 3, DEPTH);
    ALLOCS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    unsafe {
        s.run_with(&sampled_ops, ctx, &mut journal, &mut sink, None, &|| {
            fake(ctx).cap_delivered >= 5000
        });
        s.drain_with(&OPS, ctx, &mut journal, &mut sink);
    }
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(|v| v.get()), 0);
    assert_eq!(s.reason, Some("signal"));
    assert_eq!(s.pending(), [0; 2]);
    assert!(s.reused[0] >= 5000 - DEPTH && s.reused[1] > 4900);
    assert!(s.max_pending.iter().all(|&n| n <= DEPTH));
    assert_eq!(records[0].len() + records[1].len(), 64);
    let pumps = sink.3.snapshot();
    assert_eq!(pumps.header[6], f.pump_calls);
    assert_eq!(pumps.header[7], f.pump_calls); // Optional native hook absent; never a transport error.
    assert!(pumps.counts[1][0] > 0 && pumps.counts[2][0] > 0);
    let snapshot = sink.0.snapshot();
    let check = &snapshot.metrics[1][crate::timing::Metric::SubmitCheckLeadMs as usize];
    let returned = &snapshot.metrics[1][crate::timing::Metric::SubmitReturnLeadMs as usize];
    assert_eq!(check.count, s.next[1] as u64);
    assert_eq!(returned.count, check.count);
    assert!(returned.min > 1);
    assert_eq!(returned.dropped, 0);
    let delivery = sink.2.snapshot();
    assert!(
        (40..=43).contains(&delivery.header[17]),
        "sampler cadence must survive bad telemetry"
    );
    assert_eq!(delivery.header[18], delivery.header[17]);
    assert_eq!(delivery.header[21], delivery.header[17]);
    assert_eq!(delivery.header[1], 1);
    assert_eq!(delivery.header[14], 0);
    for d in 0..2 {
        assert_eq!(delivery.header[6 + d], s.completed[d] as u64);
        assert_eq!(delivery.header[10 + d], s.next[d] as u64);
        assert_eq!(
            delivery.metrics[d][1][0] + delivery.header[24 + d],
            s.completed[d] as u64
        );
    }
    let worker = sink.1.snapshot();
    assert_eq!(worker.valid, 1);
    assert_eq!(worker.dropped, 0);
    assert_eq!(worker.observed, (s.next[0] + s.next[1]) as u64);
    assert_eq!(
        worker.recent_overwritten,
        worker.observed - crate::worker_timing::RECENT as u64
    );
}

static QUARANTINE_FAULT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static QUARANTINE_CONTEXT: std::sync::atomic::AtomicPtr<Fake> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static QUARANTINE_CLOSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
unsafe extern "C" fn quarantine_open(_: u32, _: u32, _: u64) -> *mut c_void {
    use std::sync::atomic::Ordering::SeqCst;
    let ptr = Box::into_raw(Box::new(Fake::new(
        QUARANTINE_FAULT.load(SeqCst),
        std::ptr::null_mut(),
        50000,
    )));
    QUARANTINE_CONTEXT.store(ptr, SeqCst);
    ptr.cast()
}
unsafe extern "C" fn quarantine_close(_: *mut c_void) -> i32 {
    QUARANTINE_CLOSED.store(true, std::sync::atomic::Ordering::SeqCst);
    0
}
#[test]
fn live_quarantine_retains_context_and_permanently_refuses_restart() {
    use crate::live::*;
    use std::sync::atomic::Ordering::SeqCst;
    let factory = Factory {
        open: quarantine_open,
        ops: live_fake_ops,
        close: quarantine_close,
        set_rate: live_fake_set_rate,
        configure_thread: live_fake_configure,
    };
    for fault in [15, 11] {
        unsafe {
            QUARANTINE_FAULT.store(fault, SeqCst);
            QUARANTINE_CLOSED.store(false, SeqCst);
            let handle = ftu_live_create(&factory, 0x2080, 123);
            let started = ftu_live_start(handle, 1);
            if fault == 15 {
                assert_eq!(started, 0);
            } else {
                assert_ne!(started, 0);
            }
            assert_ne!(ftu_live_stop(handle, 1), 0);
            let mut status = Status::default();
            ftu_live_status(handle, &mut status);
            assert_eq!(status.phase, 5);
            assert_ne!(ftu_live_start(handle, 2), 0);
            assert_ne!(ftu_live_destroy(handle), 0);
            assert!(!QUARANTINE_CLOSED.load(SeqCst));
            if fault == 11 {
                assert!(status.pending[0] > 0);
            }
            // Fake backend has no asynchronous kernel owner. The failed public
            // destroy above proves production retains it; reclaim test-only objects
            // after Start/Stop have joined their finished worker.
            drop(Box::from_raw(
                QUARANTINE_CONTEXT.swap(std::ptr::null_mut(), SeqCst),
            ));
            drop(Box::from_raw(handle));
        }
    }
    // Negative control for removal while streaming: a native cleanup error
    // during detach (as a vanished device may report) quarantines. Detach does
    // not suppress it; the handle is never rebound, reopened or closed.
    unsafe {
        QUARANTINE_FAULT.store(15, SeqCst);
        QUARANTINE_CLOSED.store(false, SeqCst);
        let handle = ftu_live_create_pinned(&factory, 0x2080, 0x10, 0xb00);
        assert_eq!(ftu_live_start(handle, 1), 0);
        assert_eq!(ftu_live_detach(handle), -3);
        let mut status = Status::default();
        ftu_live_status(handle, &mut status);
        assert_eq!(status.phase, 5);
        assert_eq!(ftu_live_rebind(handle, 0x2080, 0x20, 0xb01), -3);
        assert_eq!(ftu_live_start(handle, 2), -6);
        // The retired client's delayed Stop is only an acknowledgement.
        assert_eq!(ftu_live_stop(handle, 1), 0);
        ftu_live_status(handle, &mut status);
        assert_eq!((status.phase, status.location), (5, 0x10));
        assert_ne!(ftu_live_destroy(handle), 0);
        assert!(!QUARANTINE_CLOSED.load(SeqCst));
        drop(Box::from_raw(
            QUARANTINE_CONTEXT.swap(std::ptr::null_mut(), SeqCst),
        ));
        drop(Box::from_raw(handle));
    }
}

unsafe extern "C" fn failed_submit_open(_: u32, _: u32, _: u64) -> *mut c_void {
    Box::into_raw(Box::new(Fake::new(1, std::ptr::null_mut(), 50000))).cast()
}
#[test]
fn live_failed_submission_preserves_native_error_and_scheduling_evidence() {
    use crate::live::*;
    let factory = Factory {
        open: failed_submit_open,
        ops: live_fake_ops,
        close: live_fake_close,
        set_rate: live_fake_set_rate,
        configure_thread: live_fake_configure,
    };
    unsafe {
        let handle = ftu_live_create(&factory, 0x2080, 123);
        assert_ne!(ftu_live_start(handle, 1), 0);
        assert_eq!(ftu_live_stop(handle, 1), 0);
        let mut s = Status::default();
        ftu_live_status(handle, &mut s);
        assert_eq!(s.phase, 4);
        assert_eq!(s.reason, 26);
        assert_eq!(s.submit_status, -99);
        assert_eq!(s.submit_direction, 1);
        assert_eq!(s.submit_sequence, 6);
        assert_eq!(s.submit_start, 180 + 6 * crate::transport::LIVE.block_ms());
        assert!(s.submit_checked < s.submit_start);
        assert_eq!(s.submit_checked, s.submit_returned);
        assert_eq!(s.pending, [0, 0]);
        assert_eq!(s.submitted, s.completed);
        assert_eq!(ftu_live_destroy(handle), 0);
    }
}

/// Exercise the production pool at the proposed live depth, including all
/// millisecond phases across two callback-delay periods. The independent event
/// model also covers 125 us phases; this fake intentionally uses coarse clocks.
#[test]
fn three_slot_pool_conserves_feedback_under_bounded_stalls_and_reordering() {
    for phase in 0..16 {
        for (delay, stall, cost) in [(0, 0, 0), (2, 8, 1), (10, 0, 0), (2, 32, 1)] {
            let mut stop = 0;
            let mut f = Fake::new(0, &mut stop, 320);
            f.prepared = 3;
            f.delay_ms = delay;
            f.stall_frame = Some(116 + 1200 + phase);
            f.stall_ms = stall;
            f.submit_cost_ms = cost;
            let mut session = PoolSession::with_depth(
                1,
                Info {
                    timebase_numer: 125,
                    timebase_denom: 3,
                    ..Info::default()
                },
                3,
            );
            let mut records = [vec![Trace::default(); 320], vec![Trace::default(); 320]];
            ALLOCS.with(|v| v.set(0));
            TRACK.with(|v| v.set(true));
            unsafe {
                let ctx = (&mut f as *mut Fake).cast();
                session.run(&OPS, ctx, &mut records, &[], &stop);
                session.drain(&OPS, ctx, &mut records);
            }
            TRACK.with(|v| v.set(false));
            assert_eq!(ALLOCS.with(|v| v.get()), 0);
            assert_eq!(session.pending(), [0, 0]);
            assert!(!session.drain_failed);
            assert!(session.max_pending.iter().all(|&n| n <= 3));
            if stall == 32 {
                assert_eq!(session.reason, Some("stale_schedule"));
            } else {
                assert_eq!(
                    session.reason, None,
                    "phase {phase}, delay {delay}, stall {stall}, cost {cost}"
                );
                assert_eq!(session.frames[0], session.frames[1]);
                assert_eq!(session.reused, [317, 317]);
                for (seq, r) in records[1].iter().enumerate() {
                    assert_eq!(r.slot, seq % 3);
                    assert_eq!(r.crc_before, r.crc_after);
                    assert_eq!(
                        r.lengths,
                        records[0][seq]
                            .completion
                            .unwrap()
                            .result
                            .packets
                            .map(|p| p.actual)
                    );
                }
            }
        }
    }
}

#[test]
fn three_slot_failures_drain_and_stale_tickets_keep_the_owner() {
    for (fault, reason) in [
        (1, "submit_failed"),
        (2, "request_completion_error"),
        (3, "stale_schedule"),
        (5, "packet_completion_error"),
        (6, "request_completion_error"),
        (7, "native_callback_fault"),
        (13, "submit_failed"),
    ] {
        let (s, _, _, n) = simulate_depth(fault, 128, false, 3);
        assert_eq!(s.reason, Some(reason));
        assert_eq!(s.pending(), [0, 0]);
        assert!(!s.drain_failed);
        assert_eq!(n, 0);
    }
    let (s, _, _, n) = simulate_depth(9, 32, false, 3);
    assert_eq!(s.reason, Some("stale_completion_ticket"));
    assert!(s.drain_failed && s.pending()[0] > 0);
    assert_eq!(n, 0);
}

#[test]
fn worker_events_separate_delivery_consumption_preparation_and_submission() {
    use crate::worker_timing::{self as wt, Event};
    struct Events {
        ctx: *mut c_void,
        prepare_delay: bool,
        count: usize,
        events: [Event; 128],
    }
    impl StreamIo for Events {
        fn worker_timing(&self) -> bool {
            true
        }
        fn worker_event(&mut self, e: Event, _pump: Pump, _feedback_pump: Pump) {
            self.events[self.count] = e;
            self.count += 1;
        }
        fn output(&mut self, first: u64, _: usize, _: &mut [u8]) -> Result<(), &'static str> {
            if self.prepare_delay && first >= 1500 {
                unsafe {
                    fake(self.ctx).frame += 1;
                }
                self.prepare_delay = false;
            }
            Ok(())
        }
    }
    unsafe extern "C" fn delayed_consumption(
        ctx: *mut c_void,
        d: u32,
        slot: u32,
        out: *mut PoolCompletion,
    ) -> i32 {
        let got = completion(ctx, d, slot, out);
        if got == 1 && d == 1 && (*out).ticket == 5 {
            fake(ctx).frame += 1;
        }
        got
    }
    for scenario in 0..5 {
        let mut stop = 0;
        let mut f = Fake::new(0, &mut stop, 64);
        if scenario == 1 {
            f.delay_ms = 2;
        }
        if scenario == 4 {
            f.submit_cost_ms = 1;
        }
        let ctx = (&mut f as *mut Fake).cast();
        let ops = PoolOps {
            completion: if scenario == 2 {
                delayed_consumption
            } else {
                completion
            },
            ..OPS
        };
        let mut session = PoolSession::new(
            1,
            Info {
                timebase_numer: 125,
                timebase_denom: 3,
                ..Info::default()
            },
        );
        let mut records = [vec![Trace::default(); 64], vec![Trace::default(); 64]];
        let mut journal = Journal {
            records: &mut records,
            recycle: false,
        };
        let mut sink = Events {
            ctx,
            prepare_delay: scenario == 3,
            count: 0,
            events: [Event::default(); 128],
        };
        ALLOCS.with(|a| a.set(0));
        TRACK.with(|a| a.set(true));
        unsafe {
            session.run_with(&ops, ctx, &mut journal, &mut sink, Some(64), &|| false);
            session.drain_with(&ops, ctx, &mut journal, &mut sink);
        }
        TRACK.with(|a| a.set(false));
        assert_eq!(ALLOCS.with(|a| a.get()), 0);
        assert_eq!(session.reason, None);
        assert_eq!(session.pending(), [0, 0]);
        assert_eq!(sink.count, 128);
        for e in sink.events {
            assert_eq!(e.ticket, e.sequence + 1);
            assert_eq!(e.slot, e.sequence % 4);
            assert_eq!(
                e.flags & (wt::CHECKED | wt::PREPARED | wt::SUBMITTED),
                wt::CHECKED | wt::PREPARED | wt::SUBMITTED
            );
            assert!(
                e.loop_enter_ticks <= e.poll_end_ticks && e.poll_end_ticks <= e.check_begin_ticks
            );
            assert!(
                e.check_begin_ticks <= e.check_end_ticks
                    && e.check_end_ticks <= e.prepare_end_ticks
            );
            assert!(
                e.prepare_end_ticks <= e.submit_begin_ticks
                    && e.submit_begin_ticks <= e.submit_end_ticks
            );
            if e.flags & wt::PREVIOUS != 0 {
                assert_eq!(e.previous_sequence + 4, e.sequence);
                assert!(e.previous_consumed_ticks >= e.previous_callback_ticks);
            }
            if e.direction == 1 {
                assert_eq!(e.feedback_sequence, e.sequence);
                assert!(e.flags & wt::RETURNED != 0);
                assert!(e.query_end_ticks >= e.submit_end_ticks);
            } else {
                assert_eq!(e.flags & (wt::RETURNED | wt::FEEDBACK | wt::QUERIED), 0);
            }
            assert_eq!(
                e.submit_end_ticks - e.submit_begin_ticks,
                if scenario == 4 { 24000 } else { 0 }
            );
        }
        let events = &sink.events;
        let preparation_max = events
            .iter()
            .map(|e| e.prepare_end_ticks - e.check_end_ticks)
            .max()
            .unwrap();
        assert_eq!(preparation_max, if scenario == 3 { 24000 } else { 0 });
        if scenario == 1 {
            let e = events
                .iter()
                .find(|e| {
                    e.direction == 1 && e.previous_sequence == 4 && e.flags & wt::PREVIOUS != 0
                })
                .unwrap();
            assert!(e.previous_callback_ticks - e.previous_last_packet_ticks >= 3 * 24000);
            assert_eq!(e.previous_consumed_ticks - e.previous_callback_ticks, 0);
        }
        if scenario == 2 {
            let e = events
                .iter()
                .find(|e| {
                    e.direction == 1 && e.previous_sequence == 4 && e.flags & wt::PREVIOUS != 0
                })
                .unwrap();
            assert_eq!(e.previous_consumed_ticks - e.previous_callback_ticks, 24000);
        }
    }
}

#[cfg(test)]
pub(crate) fn assert_no_alloc(f: impl FnOnce()) {
    ALLOCS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    f();
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(|v| v.get()), 0);
}

#[test]
fn first_completion_failure_keeps_exact_owned_record_through_drain_and_next_start() {
    use crate::first_failure::{Finish, FirstFailure};
    for (fault, reason, seq) in [(19, 23, 0), (2, 23, 8), (5, 22, 4), (6, 23, 4)] {
        let (s, _, records, allocs) = simulate(fault, 128, false);
        assert_eq!(allocs, 0);
        let f = s
            .completion_failure
            .as_ref()
            .expect("fatal completed request");
        assert_eq!(
            (f.direction, f.reason, f.trace.flight.unwrap().seq),
            (0, reason, seq)
        );
        assert_eq!(f.trace.completion.unwrap().ticket, seq as u64 + 1);
        assert_eq!(
            f.trace.completion.unwrap().result.status,
            records[0][seq].completion.unwrap().result.status
        );
        let before = Info {
            timebase_numer: 125,
            timebase_denom: 3,
            open_mask: 3,
            ..Info::default()
        };
        let mut end = Finish {
            epoch: 7,
            activated: true,
            before_rates: Rates {
                hz: [48000; 2],
                actual: [3; 2],
                ..Rates::default()
            },
            after_rates: None,
            before_info: before,
            after_info: None,
            cleanup: 0,
            close: None,
        };
        let recorder = FirstFailure::new();
        assert!(recorder.latch(&s, &end));
        let initial = recorder.snapshot();
        assert_eq!(
            (
                initial.header[3],
                initial.header[4],
                initial.header[6],
                initial.header[23],
                initial.header[30]
            ),
            (7, reason.into(), seq as u64, 1, 0)
        );
        if fault == 19 {
            assert_eq!(initial.header[14] as i64, -536870168);
            assert_eq!(initial.packets[63][0], 0x6c6c6974);
            assert_eq!(s.next[1], 0);
        }
        // Early/quarantine exit keeps primary evidence. Terminal enrichment
        // preserves the original reason and raw packets, including unmatched.
        end.cleanup = -1000001;
        recorder.finish(&s, &end, true);
        let final_record = recorder.snapshot();
        assert_eq!(final_record.header[30], 1);
        assert_eq!(final_record.header[19] as i64, -1000001);
        assert_eq!(initial.packets, final_record.packets);
        end.epoch = 8;
        assert!(!recorder.latch(&s, &end));
        recorder.finish(&s, &end, false);
        assert_eq!(recorder.snapshot(), final_record);
    }
    for fault in [0, 1, 4, 7, 9, 11] {
        let (s, _, _, _) = simulate(fault, 32, false);
        assert!(
            s.completion_failure.is_none(),
            "non-completion failure {fault}"
        );
    }
}

#[cfg(test)]
#[path = "startup_simulation.rs"]
mod startup_simulation;

#[test]
fn all_transfer_profiles_recycle_and_conserve_capture_counts_without_allocations() {
    for (ms, slots) in [(8, 4), (4, 8), (4, 10), (2, 16), (1, 32)] {
        let geometry = Transport::new(ms, slots).unwrap();
        let (s, _, records, allocations) = simulate_geometry(0, 2048, true, geometry);
        assert_eq!(s.reason, None, "{geometry:?}");
        assert_eq!(allocations, 0);
        assert_eq!(s.pending(), [0; 2]);
        assert!(!s.drain_failed);
        assert_eq!(s.frames[0], s.frames[1]);
        assert_eq!(s.reused, [2048 - slots; 2]);
        assert!(s.clock.snapshot().valid);
        for (seq, output) in records[1].iter().enumerate() {
            assert_eq!(output.slot, seq % slots);
            assert_eq!(
                output.flight.unwrap().start - records[0][seq].flight.unwrap().start,
                64
            );
            assert_eq!(output.crc_before, output.crc_after);
            let c = records[0][seq].completion.unwrap();
            assert_eq!(c.packet_count as usize, geometry.entries());
            assert_eq!(output.lengths, c.result.packets.map(|p| p.actual));
            assert!(output.lengths[geometry.entries()..].iter().all(|&n| n == 0));
        }
        for (fault, reason) in [
            (1, "submit_failed"),
            (2, "request_completion_error"),
            (5, "packet_completion_error"),
            (6, "request_completion_error"),
            (7, "native_callback_fault"),
            (24, "native_completion_geometry"),
            (25, "native_completion_geometry"),
        ] {
            let (s, _, _, n) = simulate_geometry(fault, 128, false, geometry);
            assert_eq!(s.reason, Some(reason), "{geometry:?}, fault {fault}");
            assert_eq!(s.pending(), [0; 2]);
            assert!(!s.drain_failed);
            assert_eq!(n, 0);
        }
        if ms < 8 {
            let (s, _, _, n) = simulate_geometry(26, 128, false, geometry);
            assert_eq!(s.reason, Some("native_completion_geometry"));
            assert_eq!(s.pending(), [0; 2]);
            assert!(!s.drain_failed);
            assert_eq!(n, 0);
        }
        let (s, _, _, n) = simulate_geometry(9, 32, false, geometry);
        assert_eq!(s.reason, Some("stale_completion_ticket"));
        assert!(s.drain_failed && s.pending()[0] > 0);
        assert_eq!(n, 0);
    }
}

/// Integer-ms fixture: ceiling the measured 4.364 ms callback tail to 5 ms,
/// and the 7.826 ms outer pump / 7.523 ms timeout to 8 ms. These are separate
/// scenarios, not additive claims about independent events in the recording.
/// The old 9 ms + 4 ms stress case is retained alongside the new measurements.
#[test]
fn shorter_profile_measured_tail_and_stall_screen() {
    struct Observe;
    impl StreamIo for Observe {
        fn submission_timing(&self) -> bool {
            true
        }
    }
    use crate::output_admission::OutputPolicy::{Eager, WhenReady};
    for (geometry, policy) in [
        (crate::transport::SHORTER, Eager),
        (crate::transport::SHORTER_TEN, Eager),
        (crate::transport::SHORTER_TEN, WhenReady),
    ] {
        let slots = geometry.slots();
        for (delay, stall) in [(5, 0), (0, 8), (9, 0), (9, 4), (9, 8), (9, 32)] {
            let mut minimum = u64::MAX;
            let mut below_gate = 0;
            let mut accepted = 0;
            let mut stopped = 0;
            for phase in 0..16 {
                for ppm in [-200, 0, 200] {
                    let mut stop = 0;
                    let mut f = Fake::new(0, &mut stop, 384);
                    unsafe {
                        assert_eq!(prepare_geometry((&mut f as *mut Fake).cast(), &geometry), 0);
                    }
                    f.delay_ms = delay;
                    f.stall_ms = stall;
                    f.stall_frame = Some(116 + 800 + phase);
                    f.packet_phase = phase;
                    f.drift_ppm = ppm;
                    let mut session = PoolSession::with_geometry(
                        1,
                        Info {
                            timebase_numer: 125,
                            timebase_denom: 3,
                            ..Info::default()
                        },
                        geometry,
                    )
                    .with_output_policy(policy);
                    let mut records = [vec![Trace::default(); 384], vec![Trace::default(); 384]];
                    ALLOCS.with(|v| v.set(0));
                    TRACK.with(|v| v.set(true));
                    unsafe {
                        let ctx = (&mut f as *mut Fake).cast();
                        session.run_with(
                            &OPS,
                            ctx,
                            &mut Journal {
                                records: &mut records,
                                recycle: false,
                            },
                            &mut Observe,
                            Some(384),
                            &|| false,
                        );
                        session.drain(&OPS, ctx, &mut records);
                    }
                    TRACK.with(|v| v.set(false));
                    assert_eq!(ALLOCS.with(|v| v.get()), 0);
                    assert_eq!(session.pending(), [0; 2]);
                    assert!(!session.drain_failed);
                    if session.reason.is_some() {
                        stopped += 1;
                        assert!(
                            session.reason == Some("stale_schedule")
                                || (policy == WhenReady
                                    && session.reason == Some("output_lead_deadline"))
                        );
                    } else {
                        assert_eq!(session.frames[0], session.frames[1]);
                        for (seq, r) in records[1].iter().enumerate() {
                            assert_eq!(r.crc_before, r.crc_after);
                            assert_eq!(
                                r.lengths,
                                records[0][seq]
                                    .completion
                                    .unwrap()
                                    .result
                                    .packets
                                    .map(|p| p.actual)
                            );
                        }
                    }
                    let lead = records[1]
                        .iter()
                        .filter(|r| r.submit == Some(0))
                        .map(|r| {
                            r.flight
                                .unwrap()
                                .start
                                .saturating_sub(r.submit_return_frame)
                        })
                        .min()
                        .unwrap();
                    minimum = minimum.min(lead);
                    below_gate += usize::from(lead < 16);
                    accepted += usize::from(session.reason.is_none() && lead >= 16);
                }
            }
            println!("4ms/{slots}slots ready_policy={} delay_ms={delay} stall_ms={stall} scenarios=48 min_out_lead_ms={minimum} below_16ms={below_gate} stopped={stopped} accepted={accepted}",u8::from(policy == WhenReady));
            if stall == 32 {
                // Beyond-budget runs must be rejected. The native one-ms
                // schedule guard is distinct from the sixteen-ms acceptance gate.
                assert_eq!(accepted, 0);
                if slots == 8 {
                    assert_eq!(stopped, 48);
                }
            } else {
                assert_eq!(stopped, 0);
            }
            if slots == 8 && delay == 9 && matches!(stall, 4 | 8) {
                assert!(
                    below_gate > 0,
                    "retain the rejection; do not weaken the 16 ms gate"
                );
            }
            if stall == 0 || delay == 0 || (slots == 10 && stall < 32) {
                assert!(minimum >= 16);
            }
        }
    }
}

#[test]
fn private_pool_abi_mismatch_never_submits_or_pumps() {
    for (version, size) in [
        (POOL_ABI_VERSION - 1, std::mem::size_of::<PoolOps>() as u32),
        (POOL_ABI_VERSION, 136),
    ] {
        let mut stop = 0;
        let mut f = Fake::new(0, &mut stop, 32);
        let ops = PoolOps {
            version,
            size,
            ..OPS
        };
        let mut session = PoolSession::new(1, Info::default());
        let mut records = [vec![Trace::default(); 32], vec![Trace::default(); 32]];
        unsafe {
            session.run(&ops, (&mut f as *mut Fake).cast(), &mut records, &[], &stop);
        }
        assert_eq!(session.reason, Some("pool_abi_mismatch"));
        assert_eq!(session.next, [0; 2]);
        assert_eq!(f.pump_calls, 0);
        assert_eq!(session.pending(), [0; 2]);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PcmWorkload {
    pub fixed_absence: Option<crate::fixed_replay::Window>,
    pub wake_on_ready: bool,
    pub wake_late_us: u64,
    pub capture_us: u64,
    pub service_ready: bool,
    pub dispatch: Dispatch,
    pub order: crate::submission_order::SubmissionOrder,
    pub callback_delay_ms: u64,
    pub uniform_delay: bool,
    pub pump_step_ms: u64,
    pub stall_at_frame: u64,
    pub stall_us: u64,
    pub cost_start_frame: u64,
    pub submit_us: [u64; 2],
    pub prepare_us: u64,
    pub steady_end_frames: u64,
    pub hal_service_delay_us: u64,
}
impl Default for PcmWorkload {
    fn default() -> Self {
        Self {
            fixed_absence: None,
            wake_on_ready: false,
            wake_late_us: 0,
            capture_us: 0,
            dispatch: Dispatch::Legacy,
            service_ready: false,
            order: crate::submission_order::SubmissionOrder::InputFirst,
            callback_delay_ms: 0,
            uniform_delay: false,
            pump_step_ms: 1,
            stall_at_frame: 0,
            stall_us: 0,
            cost_start_frame: 0,
            submit_us: [0; 2],
            prepare_us: 0,
            steady_end_frames: 600 * 192,
            hal_service_delay_us: 0,
        }
    }
}

/// No hardware: same production pool, with an independent nominal HAL clock
/// observed by the PCM fixture before it publishes newly delivered USB input.
pub(crate) fn run_pcm_fixture(
    geometry: Transport,
    wall: std::sync::Arc<std::sync::atomic::AtomicU64>,
    work: PcmWorkload,
    policy: crate::output_admission::OutputPolicy,
    io: &mut impl StreamIo,
) -> PoolSession {
    run_pcm_dispatch_fixture(geometry, wall, work, policy, io).0
}
pub(crate) fn run_pcm_dispatch_fixture(
    geometry: Transport,
    wall: std::sync::Arc<std::sync::atomic::AtomicU64>,
    work: PcmWorkload,
    policy: crate::output_admission::OutputPolicy,
    io: &mut impl StreamIo,
) -> (PoolSession, DispatchStats) {
    assert!(work.fixed_absence.is_none() || work.stall_at_frame == 0);
    let mut stop = 0;
    let mut f = Fake::new(0, &mut stop, 600);
    f.capture_constant = 0.25;
    f.delay_ms = work.callback_delay_ms;
    f.pump_step_ms = work.pump_step_ms;
    f.work = work;
    f.wall = Some(wall);
    unsafe {
        assert_eq!(prepare_geometry((&mut f as *mut Fake).cast(), &geometry), 0);
    }
    let mut session = PoolSession::with_geometry(
        1,
        Info {
            timebase_numer: 125,
            timebase_denom: 3,
            ..Info::default()
        },
        geometry,
    )
    .with_output_policy(policy)
    .with_submission_order(work.order);
    if work.service_ready {
        session = session.with_ready_dispatch(dispatch_ready);
    }
    // Mirror live worker initialization, including failures before the first
    // output admission. A never-published fixture snapshot must not masquerade
    // as missing terminal counters.
    io.output_retry(&session.retry.snapshot);
    let mut records = [
        vec![Trace::default(); geometry.history()],
        vec![Trace::default(); geometry.history()],
    ];
    let mut journal = Journal {
        records: &mut records,
        recycle: true,
    };
    ALLOCS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    let dispatch;
    unsafe {
        let ctx = (&mut f as *mut Fake).cast();
        session.run_with(&OPS, ctx, &mut journal, io, Some(600), &|| false);
        dispatch = f.dispatch_stats;
        session.drain_with(&OPS, ctx, &mut journal, io);
    }
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(|v| v.get()), 0);
    if work.order == crate::submission_order::SubmissionOrder::Deadline {
        assert!(f.max_submissions_between_pumps <= crate::submission_order::CATCH_UP_ATTEMPTS);
        assert!(session.max_catch_up_attempts <= crate::submission_order::CATCH_UP_ATTEMPTS);
        assert!(session.catch_up_rechecks > 0);
    }
    (session, dispatch)
}

#[test]
fn readiness_policy_retains_immutable_payloads_and_fault_ownership() {
    use crate::output_admission::OutputPolicy::WhenReady;
    for order in [
        crate::submission_order::SubmissionOrder::InputFirst,
        crate::submission_order::SubmissionOrder::Deadline,
    ] {
        let g = crate::transport::SHORTER_TEN;
        let (s, _, records, n) = simulate_geometry_order(0, 2048, true, g, WhenReady, order);
        assert_eq!(s.reason, None);
        assert_eq!(s.frames[0], s.frames[1]);
        assert_eq!(s.pending(), [0; 2]);
        assert_eq!(n, 0);
        for (seq, r) in records[1].iter().enumerate() {
            assert_eq!(r.crc_before, r.crc_after);
            assert_eq!(
                r.lengths,
                records[0][seq]
                    .completion
                    .unwrap()
                    .result
                    .packets
                    .map(|p| p.actual)
            );
            assert!(
                r.flight
                    .unwrap()
                    .start
                    .saturating_sub(r.submit_return_frame)
                    >= 16
            );
        }
        for fault in [1, 2, 5, 6, 7, 24, 25, 26] {
            let (s, _, _, n) = simulate_geometry_order(fault, 128, false, g, WhenReady, order);
            assert!(s.reason.is_some());
            assert!(!s.drain_failed);
            assert_eq!(s.pending(), [0; 2]);
            assert_eq!(n, 0);
        }
        let (s, _, _, n) = simulate_geometry_order(9, 32, false, g, WhenReady, order);
        assert_eq!(s.reason, Some("stale_completion_ticket"));
        assert!(s.drain_failed && s.pending()[0] > 0);
        assert_eq!(n, 0);
    }
}

#[test]
fn readiness_deadlines_bound_deferral_and_keep_successful_submit_owned() {
    use crate::output_admission::OutputPolicy::WhenReady;
    struct Missing {
        copied: usize,
    }
    impl StreamIo for Missing {
        fn stage_output(&mut self, _: u64, _: usize) -> Result<bool, &'static str> {
            Ok(false)
        }
        fn output(&mut self, _: u64, _: usize, _: &mut [u8]) -> Result<(), &'static str> {
            self.copied += 1;
            Ok(())
        }
    }
    // No HAL writer must continue bounded silence with explicit fallbacks.
    // Slow native submissions must fail the POST-return gate, retaining ownership.
    for order in [
        crate::submission_order::SubmissionOrder::InputFirst,
        crate::submission_order::SubmissionOrder::Deadline,
    ] {
        for cost in [0, 11] {
            let mut stop = 0;
            let mut f = Fake::new(0, &mut stop, 64);
            let g = crate::transport::SHORTER_TEN;
            unsafe {
                assert_eq!(prepare_geometry((&mut f as *mut Fake).cast(), &g), 0);
            }
            let mut s = PoolSession::with_geometry(
                1,
                Info {
                    timebase_numer: 125,
                    timebase_denom: 3,
                    ..Info::default()
                },
                g,
            )
            .with_output_policy(WhenReady)
            .with_submission_order(order);
            let mut records = [vec![Trace::default(); 64], vec![Trace::default(); 64]];
            let mut missing = Missing { copied: 0 };
            // Charge only OUT submissions, to isolate the return boundary from IN priming.
            unsafe extern "C" fn slow_submit(
                ctx: *mut c_void,
                d: u32,
                slot: u32,
                ticket: u64,
                start: u64,
                lens: *const u16,
            ) -> i32 {
                if d == 1 {
                    fake(ctx).frame += 11;
                }
                submit(ctx, d, slot, ticket, start, lens)
            }
            let ops = PoolOps {
                submit: if cost == 0 { submit } else { slow_submit },
                ..OPS
            };
            ALLOCS.with(|v| v.set(0));
            TRACK.with(|v| v.set(true));
            unsafe {
                let ctx = (&mut f as *mut Fake).cast();
                s.run_with(
                    &ops,
                    ctx,
                    &mut Journal {
                        records: &mut records,
                        recycle: false,
                    },
                    &mut missing,
                    Some(64),
                    &|| false,
                );
                if cost == 11 {
                    assert_eq!(s.reason, Some("output_lead_deadline"));
                    assert!(s.pending()[1] > 0);
                    let failure = s.submission_failure.unwrap();
                    assert!(failure.start.saturating_sub(failure.returned) < 16);
                }
                s.drain(&ops, ctx, &mut records);
            }
            TRACK.with(|v| v.set(false));
            assert_eq!(ALLOCS.with(|v| v.get()), 0);
            assert!(s.output_deferrals > 0 && s.output_deadline_fallbacks > 0);
            assert_eq!(missing.copied as u64, s.output_deadline_fallbacks);
            assert_eq!(s.pending(), [0; 2]);
            assert!(!s.drain_failed);
            if cost == 0 {
                assert_eq!(s.reason, None);
                assert_eq!(s.frames[0], s.frames[1]);
            }
        }
    }
}

#[test]
fn missing_writer_progresses_with_coarse_pumps_at_every_latency_phase() {
    missing_writer_screen(Dispatch::Legacy);
}
#[test]
fn split_dispatch_progresses_without_a_hal_writer() {
    missing_writer_screen(READY_DISPATCH);
}
fn missing_writer_screen(dispatch: Dispatch) {
    use crate::output_admission::OutputPolicy::WhenReady;
    struct Missing;
    impl StreamIo for Missing {
        fn stage_output(&mut self, _: u64, _: usize) -> Result<bool, &'static str> {
            Ok(false)
        }
    }
    for order in [
        crate::submission_order::SubmissionOrder::InputFirst,
        crate::submission_order::SubmissionOrder::Deadline,
    ] {
        for step in [1, 4, 8] {
            for delay in 0..8 {
                let g = crate::transport::SHORTER_TEN;
                let mut stop = 0;
                let mut f = Fake::new(0, &mut stop, 256);
                f.work.dispatch = dispatch;
                f.pump_step_ms = step;
                f.delay_ms = delay;
                unsafe {
                    assert_eq!(prepare_geometry((&mut f as *mut Fake).cast(), &g), 0);
                }
                let mut s = PoolSession::with_geometry(
                    1,
                    Info {
                        timebase_numer: 125,
                        timebase_denom: 3,
                        ..Info::default()
                    },
                    g,
                )
                .with_output_policy(WhenReady)
                .with_submission_order(order);
                let mut records = [vec![Trace::default(); 256], vec![Trace::default(); 256]];
                ALLOCS.with(|v| v.set(0));
                TRACK.with(|v| v.set(true));
                unsafe {
                    let ctx = (&mut f as *mut Fake).cast();
                    s.run_with(
                        &OPS,
                        ctx,
                        &mut Journal {
                            records: &mut records,
                            recycle: false,
                        },
                        &mut Missing,
                        Some(256),
                        &|| false,
                    );
                    s.drain(&OPS, ctx, &mut records);
                }
                TRACK.with(|v| v.set(false));
                assert_eq!(ALLOCS.with(|v| v.get()), 0);
                assert_eq!(s.reason, None, "pump={step} delay={delay}");
                assert_eq!(s.pending(), [0; 2]);
                assert!(!s.drain_failed);
                assert_eq!(s.frames[0], s.frames[1]);
                assert_eq!(s.output_deadline_fallbacks, 256);
                assert!(s.output_deferrals > 0);
                let lead = records[1]
                    .iter()
                    .map(|r| {
                        r.flight
                            .unwrap()
                            .start
                            .saturating_sub(r.submit_return_frame)
                    })
                    .min()
                    .unwrap();
                assert!(lead >= 16);
                println!(
                    "NO_WRITER pump_ms={step} delay_ms={delay} min_lead_ms={lead} fallbacks={}",
                    s.output_deadline_fallbacks
                );
            }
        }
    }
}

#[test]
fn retry_observer_exposes_over_budget_waits_without_affecting_capture_only_progress() {
    use crate::output_admission::OutputPolicy::WhenReady;
    use crate::output_retry::{Snapshot, MAX_NS, OVER_BUDGET, RETRIES};
    struct Missing {
        last: Option<Snapshot>,
        observe: bool,
    }
    impl StreamIo for Missing {
        fn stage_output(&mut self, _: u64, _: usize) -> Result<bool, &'static str> {
            Ok(false)
        }
        fn output_retry(&mut self, s: &Snapshot) {
            if self.observe {
                self.last = Some(*s);
            }
        }
    }
    let mut reference = None;
    for observe in [false, true] {
        let g = crate::transport::SHORTER_TEN;
        let mut stop = 0;
        let mut f = Fake::new(0, &mut stop, 256);
        f.pump_step_ms = 9;
        unsafe {
            assert_eq!(prepare_geometry((&mut f as *mut Fake).cast(), &g), 0);
        }
        let mut session = PoolSession::with_geometry(
            1,
            Info {
                timebase_numer: 125,
                timebase_denom: 3,
                ..Info::default()
            },
            g,
        )
        .with_output_policy(WhenReady);
        let mut records = [vec![Trace::default(); 256], vec![Trace::default(); 256]];
        let mut io = Missing {
            last: None,
            observe,
        };
        ALLOCS.with(|v| v.set(0));
        TRACK.with(|v| v.set(true));
        unsafe {
            let ctx = (&mut f as *mut Fake).cast();
            session.run_with(
                &OPS,
                ctx,
                &mut Journal {
                    records: &mut records,
                    recycle: false,
                },
                &mut io,
                Some(256),
                &|| false,
            );
            session.drain(&OPS, ctx, &mut records);
        }
        TRACK.with(|v| v.set(false));
        assert_eq!(ALLOCS.with(|v| v.get()), 0);
        assert_eq!(session.reason, None);
        assert_eq!(session.pending(), [0; 2]);
        assert!(!session.drain_failed);
        assert_eq!(session.frames[0], session.frames[1]);
        let s = session.retry.snapshot;
        assert!(s.counts[0][OVER_BUDGET] > 0);
        assert_eq!(s.counts[0][OVER_BUDGET], s.counts[0][RETRIES]);
        assert_eq!(s.counts[0][MAX_NS], 9_000_000);
        if observe {
            assert_eq!(io.last, Some(s));
            assert_eq!(reference, Some((s, session.frames, session.completed)));
        } else {
            assert!(io.last.is_none());
            reference = Some((s, session.frames, session.completed));
        }
        println!(
            "RETRY_OBSERVER enabled={} retries={} over_budget={} max_retry_ns={}",
            u8::from(observe),
            s.counts[0][RETRIES],
            s.counts[0][OVER_BUDGET],
            s.counts[0][MAX_NS]
        );
    }
}

#[test]
fn repoll_during_submission_work_cannot_dispatch_a_native_callback() {
    let mut stop = 0;
    let mut f = Fake::new(0, &mut stop, 16);
    let ctx = (&mut f as *mut Fake).cast();
    let mut out = PoolCompletion::default();
    unsafe {
        assert_eq!(submit(ctx, 0, 0, 1, 116, [312; 64].as_ptr()), 0);
        f.advance_us(100_000);
        assert_eq!(completion(ctx, 0, 0, &mut out), 0);
        pump(ctx);
        assert_eq!(completion(ctx, 0, 0, &mut out), 1);
        assert_eq!(completion(ctx, 0, 0, &mut out), 0);
    }
}

#[test]
fn deadline_catch_up_checks_stop_at_count_or_elapsed_boundary() {
    use crate::output_admission::OutputPolicy::WhenReady;
    use crate::submission_order::SubmissionOrder::Deadline;
    struct Silence;
    impl StreamIo for Silence {}
    // Zero-cost work reaches the two-attempt bound; one slow synchronous call
    // crosses the time bound. Neither may start another request after stop.
    for (cost, stop_check, expected_submits) in [(0, 4, 2), (3000, 3, 1)] {
        let mut stop = 0;
        let mut f = Fake::new(0, &mut stop, 64);
        f.work.submit_us = [cost; 2];
        let g = crate::transport::SHORTER_TEN;
        let ctx = (&mut f as *mut Fake).cast();
        unsafe {
            assert_eq!(prepare_geometry(ctx, &g), 0);
        }
        let mut s = PoolSession::with_geometry(
            1,
            Info {
                timebase_numer: 125,
                timebase_denom: 3,
                ..Default::default()
            },
            g,
        )
        .with_output_policy(WhenReady)
        .with_submission_order(Deadline);
        let mut records = [vec![Trace::default(); 64], vec![Trace::default(); 64]];
        let mut journal = Journal {
            records: &mut records,
            recycle: false,
        };
        let checks = Cell::new(0);
        ALLOCS.with(|v| v.set(0));
        TRACK.with(|v| v.set(true));
        unsafe {
            s.run_with(&OPS, ctx, &mut journal, &mut Silence, Some(64), &|| {
                checks.set(checks.get() + 1);
                checks.get() >= stop_check
            });
            assert_eq!(s.reason, Some("signal"));
            assert_eq!(s.next, [expected_submits, 0]);
            s.drain_with(&OPS, ctx, &mut journal, &mut Silence);
        }
        TRACK.with(|v| v.set(false));
        assert_eq!(ALLOCS.with(|v| v.get()), 0);
        assert_eq!(s.pending(), [0; 2]);
        assert!(!s.drain_failed);
    }
}

#[test]
fn split_dispatch_preserves_ownership_and_fault_quarantine() {
    use crate::output_admission::OutputPolicy::WhenReady;
    use crate::submission_order::SubmissionOrder::Deadline;
    let g = crate::transport::SHORTER_TEN;
    for fault in [0, 1, 2, 5, 6, 7, 9, 24, 25, 26] {
        let (s, f, records, allocations) =
            simulate_geometry_dispatch(fault, 2048, true, g, WhenReady, Deadline, READY_DISPATCH);
        assert_eq!(allocations, 0);
        assert!(f.dispatch_stats.callbacks > 0);
        assert!(f.dispatch_stats.max_batch <= 10);
        if fault == 0 {
            assert_eq!(s.reason, None);
            assert_eq!(s.frames[0], s.frames[1]);
            for (seq, r) in records[1].iter().enumerate() {
                assert_eq!(r.crc_before, r.crc_after);
                assert_eq!(
                    r.lengths,
                    records[0][seq]
                        .completion
                        .unwrap()
                        .result
                        .packets
                        .map(|p| p.actual)
                );
                assert!(
                    r.flight
                        .unwrap()
                        .start
                        .saturating_sub(r.submit_return_frame)
                        >= 16
                );
            }
        } else {
            assert!(s.reason.is_some());
        }
        if fault == 9 {
            assert!(s.drain_failed && s.pending()[0] > 0);
        } else {
            assert!(!s.drain_failed);
            assert_eq!(s.pending(), [0; 2]);
        }
    }
}
fn ready_fake(callback_us: u64, batch: usize, ready_first: bool) -> Fake {
    let mut f = Fake::new(0, std::ptr::null_mut(), 32);
    f.work.dispatch = Dispatch::Separate {
        ready_first,
        callback_us,
        entry_us: 0,
        batch,
    };
    f.pump_step_ms = 8;
    for slots in &mut f.transfers {
        for (i, t) in slots.iter_mut().enumerate() {
            *t = Some(Transfer {
                ticket: i as u64 + 1,
                start: 90,
                lengths: [144; 64],
                reported: false,
                delivered_ticks: None,
                aborted: false,
            });
        }
    }
    f
}
#[test]
fn split_dispatch_requires_delivery_and_never_spins_when_idle() {
    for ready_first in [false, true] {
        let mut f = ready_fake(50, 10, ready_first);
        let ctx = (&mut f as *mut Fake).cast();
        let mut c = PoolCompletion::default();
        // Physical readiness and advancing time do not deliver a callback.
        f.advance_us(1000);
        unsafe {
            assert_eq!(completion(ctx, 0, 0, &mut c), 0);
        }
        assert!(f.transfers[0][0]
            .as_ref()
            .unwrap()
            .delivered_ticks
            .is_none());
        let before = f.frame * 1000 + f.subframe_us;
        f.pump_separate();
        assert_eq!(f.dispatch_stats.callbacks, 8);
        assert_eq!(f.dispatch_stats.idle_calls, u64::from(!ready_first));
        assert_eq!(
            f.frame * 1000 + f.subframe_us - before,
            400 + if ready_first { 0 } else { 8000 }
        );
        unsafe {
            assert_eq!(completion(ctx, 0, 0, &mut c), 1);
            assert_eq!(completion(ctx, 0, 0, &mut c), 0);
        }
        // Delivered-but-not-consumed requests cannot keep an empty pump spinning.
        let before = f.frame * 1000 + f.subframe_us;
        f.pump_separate();
        assert_eq!(f.frame * 1000 + f.subframe_us - before, 8000);
        assert_eq!(f.dispatch_stats.callbacks, 8);
    }
}
#[test]
fn split_dispatch_bounds_callback_work_and_charges_stalls() {
    for (cost, batch, expected) in [
        (0, 2, 2),
        (50, 1, 1),
        (50, 10, 8),
        (1000, 10, 2),
        (3000, 10, 1),
    ] {
        let mut f = ready_fake(cost, batch, true);
        f.pump_separate();
        assert_eq!(f.dispatch_stats.callbacks, expected);
        assert_eq!(f.dispatch_stats.callback_us, expected * cost);
        assert_eq!(f.dispatch_stats.max_batch, expected as usize);
    }
    let mut f = ready_fake(50, 10, true);
    f.work.stall_at_frame = 100;
    f.work.stall_us = 12_500;
    f.pump_separate();
    f.pump_separate();
    assert_eq!(f.dispatch_stats.stalls, 1);
    assert_eq!(f.dispatch_stats.stall_us, 12_500);
    assert_eq!(f.dispatch_stats.callbacks, 8);
    // An entry delay remains charged even when no idle wait is needed.
    f.work.dispatch = Dispatch::Separate {
        ready_first: true,
        callback_us: 50,
        entry_us: 1000,
        batch: 10,
    };
    f.pump_separate();
    assert_eq!(f.dispatch_stats.entry_us, 1000);
}

#[test]
fn split_dispatch_orders_by_readiness_and_accounts_for_a_mid_callback_stall() {
    let mut f = ready_fake(50, 1, true);
    // IN[1] becomes ready earlier than IN[0], then OUT[0] is earlier still.
    f.transfers[0][1].as_mut().unwrap().start = 89;
    f.transfers[1][0].as_mut().unwrap().start = 88;
    f.pump_separate();
    assert!(f.transfers[1][0]
        .as_ref()
        .unwrap()
        .delivered_ticks
        .is_some());
    assert!(f.transfers[0][0]
        .as_ref()
        .unwrap()
        .delivered_ticks
        .is_none());
    f.pump_separate();
    assert!(f.transfers[0][1]
        .as_ref()
        .unwrap()
        .delivered_ticks
        .is_some());
    let mut f = ready_fake(1000, 10, true);
    f.work.stall_at_frame = 101;
    f.work.stall_us = 12_500;
    f.pump_separate();
    assert_eq!(f.dispatch_stats.stalls, 1);
    assert_eq!(f.dispatch_stats.callbacks, 1);
    assert_eq!(f.dispatch_stats.callback_us, 1000);
    assert_eq!(f.frame * 1000 + f.subframe_us, 113_500);
}

#[test]
fn capture_dispatch_preserves_ownership_and_fault_quarantine() {
    use crate::output_admission::OutputPolicy::WhenReady;
    use crate::submission_order::SubmissionOrder::Deadline;
    let g = crate::transport::SHORTER_TEN;
    for fault in [0, 1, 2, 5, 6, 7, 9, 24, 25, 26] {
        let (s, f, records, allocations) = simulate_geometry_service(
            fault,
            2048,
            true,
            g,
            WhenReady,
            Deadline,
            Dispatch::Separate {
                ready_first: true,
                callback_us: 50,
                entry_us: 0,
                batch: 1,
            },
            true,
        );
        assert_eq!(allocations, 0);
        assert!(f.dispatch_stats.callbacks > 0);
        assert!(f.dispatch_stats.max_batch <= 10);
        if fault == 0 {
            assert_eq!(s.reason, None);
            assert_eq!(s.frames[0], s.frames[1]);
            for (seq, r) in records[1].iter().enumerate() {
                assert_eq!(r.crc_before, r.crc_after);
                assert_eq!(
                    r.lengths,
                    records[0][seq]
                        .completion
                        .unwrap()
                        .result
                        .packets
                        .map(|p| p.actual)
                );
                assert!(
                    r.flight
                        .unwrap()
                        .start
                        .saturating_sub(r.submit_return_frame)
                        >= 16
                );
            }
        } else {
            assert!(s.reason.is_some());
        }
        if fault == 9 {
            assert!(s.drain_failed && s.pending()[0] > 0);
        } else {
            assert!(!s.drain_failed);
            assert_eq!(s.pending(), [0; 2]);
        }
    }
}
#[test]
fn capture_dispatch_bounds_work_stops_and_empty_probes() {
    struct Silence;
    impl StreamIo for Silence {}
    for (cost, ready, expected, stop_after) in [
        (0, 20, 10, 99),
        (50, 20, 10, 99),
        (1000, 20, 2, 99),
        (3000, 20, 1, 99),
        (50, 0, 0, 99),
        (50, 20, 2, 3),
    ] {
        let mut f = ready_fake(cost, 1, true);
        for slots in &mut f.transfers {
            let request = slots[0].clone();
            slots.resize(10, request);
            if ready == 0 {
                slots.fill(None);
            }
        }
        let mut s = PoolSession::with_geometry(
            1,
            Info {
                timebase_numer: 125,
                timebase_denom: 3,
                ..Default::default()
            },
            crate::transport::SHORTER_TEN,
        )
        .with_ready_dispatch(dispatch_ready);
        s.anchor.frame = f.frame;
        let mut records = [vec![Trace::default(); 32], vec![Trace::default(); 32]];
        let mut journal = Journal {
            records: &mut records,
            recycle: false,
        };
        let checks = Cell::new(0);
        let calls = f.dispatch_stats.calls;
        unsafe {
            s.service_ready(
                &OPS,
                (&mut f as *mut Fake).cast(),
                &mut journal,
                &mut Silence,
                false,
                None,
                &|| {
                    checks.set(checks.get() + 1);
                    checks.get() >= stop_after
                },
            );
        }
        assert_eq!(f.dispatch_stats.callbacks, expected);
        assert_eq!(f.dispatch_stats.idle_calls, 0);
        assert!(f.dispatch_stats.calls - calls <= 10);
        if ready == 0 {
            assert_eq!(f.dispatch_stats.calls - calls, 1);
        }
        if stop_after == 3 {
            assert_eq!(s.reason, Some("signal"));
        }
    }
}

#[test]
fn fixed_replay_clock_cannot_erase_hal_processing() {
    use std::sync::{
        atomic::{AtomicU64, Ordering::SeqCst},
        Arc,
    };
    let wall = Arc::new(AtomicU64::new(100_500));
    let mut f = Fake::new(0, std::ptr::null_mut(), 1);
    f.wall = Some(wall.clone());
    f.advance_us(50);
    assert_eq!(wall.load(SeqCst), 100_550);
    wall.store(100_750, SeqCst);
    f.wait_until(100_600);
    assert_eq!(wall.load(SeqCst), 100_750);
}

/// Tracking starts after fixtures and thread runtime have been created.
pub(crate) fn without_allocations<T>(f: impl FnOnce() -> T) -> T {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            TRACK.with(|v| v.set(false));
        }
    }
    assert!(!TRACK.with(|v| v.replace(true)));
    ALLOCS.with(|v| v.set(0));
    let guard = Guard;
    let result = f();
    drop(guard);
    assert_eq!(ALLOCS.with(|v| v.get()), 0, "streaming allocation");
    result
}
