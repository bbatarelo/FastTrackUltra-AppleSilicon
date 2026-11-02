// Runtime-fault record and automatic recovery through the production route:
// public FFI -> control lock -> production independent worker (factory open,
// startup initializer, Owner, guards, teardown, cleanup, close) -> linked C
// completion route. The Seam supplies only the fake route, a virtual bus clock
// and owner-thread completion delivery; no production guard is relaxed. A
// separate thread plays the CoreAudio IO thread with continuous ZTS/IO calls.
use crate::recovery::{self as rp, Policy};
use std::sync::atomic::AtomicI32;

static SERIAL: Mutex<()> = Mutex::new(());
// Virtual USB bus time in ms (frames of the fake clock). Monotonic for a test.
static VIRTUAL: AtomicU64 = AtomicU64::new(100);
// One-shot preemption (bus ms) injected inside the next OUT / IN packing pass.
static JUMP_OUT: AtomicU64 = AtomicU64::new(0);
static JUMP_IN: AtomicU64 = AtomicU64::new(0);
// Reproduce the field failure: a new recovery worker loses 33 ms before
// submitting its first OUT request, then stops cleanly before RUNNING.
static STARTUP_STALLS: AtomicU64 = AtomicU64::new(0);
static STALL_THIS_OPEN: AtomicBool = AtomicBool::new(false);
static HOLD_STARTUP_CLOCK: AtomicBool = AtomicBool::new(false);
static HOLD_CLOCK_READY: AtomicBool = AtomicBool::new(false);
static HOLD_HANDLE: AtomicU64 = AtomicU64::new(0);
// Fake USB enumeration currently present; a pinned open of another is refused.
static PRESENT: AtomicU64 = AtomicU64::new(0);
static OPENS: AtomicU64 = AtomicU64::new(0);
static REFUSED_OPENS: AtomicU64 = AtomicU64::new(0);
static OPENED_REGISTRY: AtomicU64 = AtomicU64::new(0);
static OPEN_CONTEXTS: AtomicU32 = AtomicU32::new(0);
static OVERLAP: AtomicBool = AtomicBool::new(false);
static CLEANUP_STATUS: AtomicI32 = AtomicI32::new(0);
static CLOSE_STATUS: AtomicI32 = AtomicI32::new(0);
static RESTORE_STATUS: AtomicI32 = AtomicI32::new(0);
static ROUTE_FAULT: AtomicU32=AtomicU32::new(0);
static ROUTE_FAULT_APPLIED: AtomicBool=AtomicBool::new(false);
static REMOVED: AtomicBool=AtomicBool::new(false);
static NATIVE_REMOVED_MASK: AtomicU32=AtomicU32::new(0);
// Raw rate reply order: bit0 OUT, bit1 IN; zero selects the usual both-error fixture.
static BAD_RATE_REPLIES: AtomicU32=AtomicU32::new(0);
// HAL lockstep: the owner waits until the IO thread served each virtual ms.
static HAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static HAL_DONE: AtomicU64 = AtomicU64::new(0);
const REGISTRY: u64 = 0x1_0100_d550;
const LOCATION: u32 = 0x2021_1100;

struct RecCtx {
    cleaned: bool,
}
unsafe extern "C" fn rec_open(_: u32, _: u32, registry: u64) -> *mut c_void {
    OPENS.fetch_add(1, SeqCst);
    OPENED_REGISTRY.store(registry, SeqCst);
    if registry == 0 || registry != PRESENT.load(SeqCst) {
        REFUSED_OPENS.fetch_add(1, SeqCst);
        return std::ptr::null_mut();
    }
    if OPEN_CONTEXTS.fetch_add(1, SeqCst) != 0 {
        OVERLAP.store(true, SeqCst);
    }
    STALL_THIS_OPEN.store(STARTUP_STALLS.fetch_update(SeqCst, SeqCst,
        |n| n.checked_sub(1)).is_ok(), SeqCst);
    Box::into_raw(Box::new(RecCtx { cleaned: false })).cast()
}
unsafe extern "C" fn rec_ops(_: *mut c_void) -> *const PoolOps {
    &REC_OPS
}
unsafe extern "C" fn rec_close(p: *mut c_void) -> i32 {
    drop(Box::from_raw(p.cast::<RecCtx>()));
    OPEN_CONTEXTS.fetch_sub(1, SeqCst);
    CLOSE_STATUS.load(SeqCst)
}
unsafe extern "C" fn rec_schedule(_: *mut c_void, s: *const ThreadSchedule) -> i32 {
    if s.is_null() {
        RESTORE_STATUS.load(SeqCst)
    } else {
        0
    }
}
unsafe extern "C" fn rec_rate(_: *mut c_void, _: u32, hz: u32, actual: *mut u32) -> i32 {
    assert_eq!(hz, 48000);
    *actual = 3;
    0
}
const REC_FACTORY: Factory = Factory {
    open: rec_open,
    ops: rec_ops,
    close: rec_close,
    configure_thread: rec_schedule,
    set_rate: rec_rate,
};
unsafe extern "C" fn rec_config(_: *mut c_void, out: *mut u8, capacity: usize, length: *mut usize) -> i32 {
    let b = include_bytes!("../../ftu-protocol/tests/fixtures/config-0.bin");
    assert!(capacity >= b.len());
    std::ptr::copy_nonoverlapping(b.as_ptr(), out, b.len());
    *length = b.len();
    0
}
unsafe extern "C" fn rec_rates(_: *mut c_void, out: *mut Rates) {
    *out = Rates {
        status: std::array::from_fn(|e| if REMOVED.load(SeqCst) && (BAD_RATE_REPLIES.load(SeqCst)==0 || BAD_RATE_REPLIES.load(SeqCst)&(1<<e)!=0) { 0xe00002c0u32 as i32 } else { 0 }),
        hz: [48000; 2],
        actual: [3; 2],
        raw: [0x80, 0xbb, 0, 0x80, 0xbb, 0],
    };
}
unsafe extern "C" fn rec_prepare(_: *mut c_void, _: u32) -> i32 {
    0
}
unsafe extern "C" fn rec_prepare_geometry(_: *mut c_void, g: *const Transport) -> i32 {
    if g.is_null() || !(*g).valid() || (*g).slots() != DEPTH || (*g).block_ms() != 4 {
        return -1;
    }
    0
}
unsafe extern "C" fn rec_activate(_: *mut c_void) -> i32 {
    0
}
unsafe extern "C" fn rec_clock(_: *mut c_void, out: *mut Clock) -> i32 {
    let ms = VIRTUAL.load(SeqCst);
    *out = Clock {
        frame: ms,
        ticks: ms * 24000,
    };
    0
}
// The independent worker never uses the legacy pool data path.
unsafe extern "C" fn rec_buffer(_: *mut c_void, _: u32, _: u32) -> *mut u8 {
    panic!("legacy pool buffer")
}
unsafe extern "C" fn rec_old_submit(_: *mut c_void, _: u32, _: u32, _: u64, _: *const u16) -> i32 {
    panic!("legacy submit")
}
unsafe extern "C" fn rec_old_completion(_: *mut c_void, _: u32, _: u32, _: *mut Completion) -> i32 {
    panic!("legacy completion")
}
unsafe extern "C" fn rec_pump(_: *mut c_void) {
    panic!("legacy pump")
}
unsafe extern "C" fn rec_abort(_: *mut c_void, _: u32) -> i32 {
    panic!("legacy abort")
}
unsafe extern "C" fn rec_cleanup(p: *mut c_void) -> i32 {
    (*p.cast::<RecCtx>()).cleaned = true;
    CLEANUP_STATUS.load(SeqCst)
}
unsafe extern "C" fn rec_info(p: *mut c_void, out: *mut Info) {
    let cleaned = (*p.cast::<RecCtx>()).cleaned;
    *out = Info {
        timebase_numer: 125,
        timebase_denom: 3,
        restored_mask: if cleaned { restored_mask() } else { 0 },
        open_mask: if cleaned { 0 } else { 3 },
        ..Info::default()
    };
}
unsafe extern "C" fn rec_submit(_: *mut c_void, _: u32, _: u32, _: u64, _: u64, _: *const u16) -> i32 {
    panic!("legacy pool submit")
}
unsafe extern "C" fn rec_completion(_: *mut c_void, _: u32, _: u32, _: *mut PoolCompletion) -> i32 {
    panic!("legacy pool completion")
}
unsafe extern "C" fn rec_now(_: *mut c_void) -> u64 {
    VIRTUAL.load(SeqCst) * 24000
}
const REC_OPS: PoolOps = PoolOps {
    version: crate::transport::POOL_ABI_VERSION,
    size: std::mem::size_of::<PoolOps>() as u32,
    prepare_geometry: rec_prepare_geometry,
    base: Ops {
        config: rec_config,
        rates: rec_rates,
        prepare: rec_prepare,
        activate: rec_activate,
        clock: rec_clock,
        buffer: rec_buffer,
        submit: rec_old_submit,
        completion: rec_old_completion,
        pump: rec_pump,
        abort: rec_abort,
        cleanup: rec_cleanup,
        info: rec_info,
    },
    submit: rec_submit,
    completion: rec_completion,
    now_ticks: rec_now,
    worker_sample: None,
    last_pump: None,
};

// Seam: the linked fake route; a preempted packing pass advances bus time.
unsafe extern "C" fn late_buffer(f: *mut c_void, d: u32, s: u32) -> *mut u8 {
    let jump = if d == 1 { &JUMP_OUT } else { &JUMP_IN };
    let late = jump.swap(0, SeqCst);
    if late != 0 {
        VIRTUAL.fetch_add(late, SeqCst);
    }
    ftu_fixture_buffer(f, d, s)
}
fn seam_attach(hooks: &Hooks) -> *mut c_void {
    // Runs on the worker thread: the fixture's owner thread is the worker.
    unsafe { ftu_fixture_create(DEPTH as u32, hooks) }
}
fn seam_clock() -> Result<Clock, &'static str> {
    if HOLD_CLOCK_READY.swap(false, SeqCst) {
        let h = HOLD_HANDLE.load(SeqCst) as *mut Live;
        assert!(!h.is_null());
        let deadline = Instant::now() + Duration::from_secs(10);
        // Block after Owner's stop check until the actual launch timeout
        // requests cancellation, then return a clock that fails its OUT gate.
        while !unsafe { (&*h).shared.stop.load(SeqCst) } {
            assert!(Instant::now() < deadline, "launch never timed out");
            thread::sleep(Duration::from_millis(1));
        }
        VIRTUAL.fetch_add(32, SeqCst);
    }
    let ms = VIRTUAL.load(SeqCst);
    Ok(Clock {
        frame: ms,
        ticks: ms * 24000,
    })
}
fn seam_service(o: &Owner) {
    let fault=ROUTE_FAULT.swap(0,SeqCst);
    if fault!=0 { unsafe { ftu_fixture_fault(o.route,fault); } ROUTE_FAULT_APPLIED.store(true,SeqCst); }
    if o.base != 0 && o.next[1] == 0 && VIRTUAL.load(SeqCst) >= o.base + 18
        && STALL_THIS_OPEN.swap(false, SeqCst) {
        // This service normally advances one ms; add 32 to make a 33 ms gap.
        if HOLD_STARTUP_CLOCK.load(SeqCst) {
            HOLD_CLOCK_READY.store(true, SeqCst);
        } else { VIRTUAL.fetch_add(32, SeqCst); }
    }
    let ms = VIRTUAL.fetch_add(1, SeqCst) + 1;
    assert_eq!(dispatch(o, ms), 0);
    if HAL_ACTIVE.load(SeqCst) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while HAL_DONE.load(SeqCst) < ms && Instant::now() < deadline {
            thread::yield_now();
        }
    }
}
fn restored_mask()->u32 {
    if !REMOVED.load(SeqCst) { 3 } else {
        let removed=NATIVE_REMOVED_MASK.load(SeqCst);if removed==0 { 0 } else { 3^removed }
    }
}
unsafe extern "C" fn rec_cleanup_snapshot(ctx:*mut c_void,out:*mut u64) {
    let mut w=[0u64;16];
    if (*ctx.cast::<RecCtx>()).cleaned {
        w[0]=1;w[1]=1023;w[2]=NATIVE_REMOVED_MASK.load(SeqCst).into();
        w[3]=restored_mask().into();
        w[5]=CLEANUP_STATUS.load(SeqCst) as i64 as u64;
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(),out,16);
}
unsafe extern "C" fn rec_stop(route:*mut c_void,ms:u32)->bool {
    let result=stop(route,ms);let mut w=[0u64;16];ftu_fixture_snapshot(route,w.as_mut_ptr());
    let mask=(0..2).fold(0u32,|mask,d|mask | if w[14+d] as u32==0xe00002c0 { 1<<d } else { 0 });
    NATIVE_REMOVED_MASK.store(mask,SeqCst);result
}
const SEAMED: Seam = Seam {
    api: Api {
        buffer: late_buffer,
        stop: rec_stop,
        cleanup_snapshot: rec_cleanup_snapshot,
        ..FAKE
    },
    attach: seam_attach,
    clock: seam_clock,
    service: seam_service,
};

struct Harness {
    _serial: std::sync::MutexGuard<'static, ()>,
}
impl Harness {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        VIRTUAL.store(100, SeqCst);
        for a in [&JUMP_OUT, &JUMP_IN, &OPENS, &REFUSED_OPENS, &OPENED_REGISTRY, &HAL_DONE] {
            a.store(0, SeqCst);
        }
        STARTUP_STALLS.store(0, SeqCst);
        STALL_THIS_OPEN.store(false, SeqCst);
        HOLD_STARTUP_CLOCK.store(false, SeqCst);HOLD_CLOCK_READY.store(false, SeqCst);
        HOLD_HANDLE.store(0, SeqCst);
        PRESENT.store(REGISTRY, SeqCst);
        ROUTE_FAULT.store(0,SeqCst);ROUTE_FAULT_APPLIED.store(false,SeqCst);REMOVED.store(false,SeqCst);NATIVE_REMOVED_MASK.store(0,SeqCst);BAD_RATE_REPLIES.store(0,SeqCst);
        OPEN_CONTEXTS.store(0, SeqCst);
        OVERLAP.store(false, SeqCst);
        for a in [&CLEANUP_STATUS, &CLOSE_STATUS, &RESTORE_STATUS] {
            a.store(0, SeqCst);
        }
        HAL_ACTIVE.store(false, SeqCst);
        *SEAM.lock().unwrap_or_else(|e| e.into_inner()) = Some(SEAMED);
        Self { _serial: serial }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        HAL_ACTIVE.store(false, SeqCst);
        *SEAM.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

// CoreAudio IO-thread stand-in: ZTS on every pass (also while stopped), and
// write/read for every new virtual millisecond, resynchronizing on a new seed.
#[derive(Default, Debug)]
struct HalStats {
    seeds: Vec<u64>,
    not_running: u64,
    writes: [u64; 3], // ok, not running (-2), rejected window (-3)
    reads: [u64; 2],  // ok, not running
    // Extrapolated sample time below the previous pass within one seed
    // (reported only; anchors are re-fitted, so a step is not a failure).
    backward: u64,
}
struct Hal {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<HalStats>,
}
impl Hal {
    fn start(h: *mut Live, buffer: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = h as usize;
        HAL_ACTIVE.store(true, SeqCst);
        let thread = thread::spawn(move || unsafe {
            let h = handle as *mut Live;
            let mut s = HalStats::default();
            let (mut seed, mut next, mut served, mut last) = (None, 0u64, 0u64, 0u64);
            let write = vec![0.03125f32; buffer * 8];
            let mut read = vec![0f32; buffer * 8];
            while !flag.load(SeqCst) {
                let v = VIRTUAL.load(SeqCst);
                let mut ts = ZeroTimestamp {
                    epoch: 0,
                    frame: 0,
                    ticks: 0,
                };
                if ftu_live_timestamp(h, &mut ts) != 0 {
                    s.not_running += 1;
                    // A bounded seqlock read can fail inside the same epoch.
                    // Resync IO on the next valid anchor without inventing a
                    // seed transition; only an observed new seed counts.
                    next = u64::MAX;
                    HAL_DONE.store(v, SeqCst);
                    thread::sleep(Duration::from_micros(100));
                    continue;
                }
                if seed != Some(ts.epoch) {
                    // Changed seed: a new timeline. Resynchronize, never splice.
                    seed = Some(ts.epoch);
                    s.seeds.push(ts.epoch);
                    next = u64::MAX;
                    last = 0;
                }
                if v == served && next != u64::MAX {
                    thread::yield_now();
                    continue;
                }
                served = v;
                let ticks = v * 24000;
                let now = if ticks >= ts.ticks {
                    ts.frame + (ticks - ts.ticks) / 500
                } else {
                    ts.frame.saturating_sub((ts.ticks - ticks) / 500)
                };
                if now < last {
                    s.backward += 1;
                }
                last = now;
                if next == u64::MAX {
                    next = now;
                }
                while next <= now {
                    let out = next + u64::from(OUTPUT_SAFETY) + buffer as u64;
                    match ftu_live_write(h, out, buffer as u32, write.as_ptr()) {
                        0 => s.writes[0] += 1,
                        -2 => s.writes[1] += 1,
                        -3 => s.writes[2] += 1,
                        rc => panic!("unexpected write status {rc}"),
                    }
                    if next >= u64::from(INPUT_SAFETY) + buffer as u64 {
                        let first = next - u64::from(INPUT_SAFETY) - buffer as u64;
                        match ftu_live_read(h, first, buffer as u32, read.as_mut_ptr()) {
                            0 => s.reads[0] += 1,
                            -2 => s.reads[1] += 1,
                            rc => panic!("unexpected read status {rc}"),
                        }
                    }
                    next += buffer as u64;
                }
                HAL_DONE.store(v, SeqCst);
            }
            s
        });
        Self { stop, thread }
    }
    fn finish(self) -> HalStats {
        HAL_ACTIVE.store(false, SeqCst);
        self.stop.store(true, SeqCst);
        self.thread.join().unwrap()
    }
}

unsafe fn st(h: *mut Live) -> Status {
    let mut s = Status::default();
    ftu_live_status(h, &mut s);
    s
}
unsafe fn record(h: *mut Live) -> rf::Snapshot {
    let mut r = rf::Snapshot::default();
    ftu_live_runtime_fault(h, &mut r);
    r
}
unsafe fn view(h: *mut Live) -> [u64; rp::WORDS] {
    let mut v = RecoverySnapshot {
        words: [0; rp::WORDS],
    };
    ftu_live_recovery(h, &mut v);
    assert_eq!(v.words[1], 1, "coherent recovery view");
    v.words
}
unsafe fn set_policy(h: *mut Live, policy: Policy) {
    (*h).control.lock().unwrap().recovery.policy = policy;
}
unsafe fn wait_phase(h: *mut Live, phases: &[u32]) -> Status {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let s = st(h);
        if phases.contains(&s.phase) {
            return s;
        }
        assert!(Instant::now() < deadline, "phase {} never reached {phases:?}", s.phase);
        thread::sleep(Duration::from_millis(1));
    }
}
// Let the running owner advance `ms` of virtual bus time.
unsafe fn advance(h: *mut Live, ms: u64) {
    let target = VIRTUAL.load(SeqCst) + ms;
    let deadline = Instant::now() + Duration::from_secs(120);
    while VIRTUAL.load(SeqCst) < target {
        assert_eq!(st(h).phase, RUNNING, "stream stopped while advancing");
        assert!(Instant::now() < deadline, "virtual time stalled");
        thread::sleep(Duration::from_millis(1));
    }
}
// Running stream, then one preempted packing pass; returns the faulted status.
unsafe fn inject(h: *mut Live, jump: &AtomicU64) -> Status {
    advance(h, 200);
    jump.store(64, SeqCst);
    wait_phase(h, &[FAULT, QUARANTINED])
}
// Monitor stand-in: same call the plug-in makes. While stopped, bus time keeps
// passing. Returns the first decisive code and the backoffs it observed.
unsafe fn monitor(h: *mut Live) -> (i32, Vec<u64>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut backoffs = vec![];
    loop {
        let rc = ftu_live_recover(h, ftu_live_binding(h));
        if !matches!(rc, 2..=4) {
            return (rc, backoffs);
        }
        let w = view(h);
        if w[rp::STATE] == rp::PENDING && w[rp::TIME_FLAGS] & rp::T_PENDING != 0 {
            let b = w[rp::DUE] - w[rp::PENDING_SINCE];
            if backoffs.last() != Some(&b) {
                backoffs.push(b);
            }
        }
        assert!(Instant::now() < deadline, "recovery never decided");
        VIRTUAL.fetch_add(5, SeqCst);
        thread::sleep(Duration::from_millis(2));
    }
}
fn steady_missing(live: &Live) -> (u64, i64, u64, i64) {
    let t = live.shared.timing.snapshot();
    let out = t.metrics[1][Metric::OutputMissing as usize];
    let input = t.metrics[1][Metric::ReadMissing as usize];
    (out.count, out.max, input.count, input.max)
}

#[test]
fn deadline_fault_recovers_retained_clients_without_a_new_start() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        let hal = Hal::start(h, 128);
        assert_eq!(ftu_live_start(h, 7), 0);
        assert_eq!(ftu_live_start(h, 8), 0, "second client joins the running session");
        let first = st(h).epoch;
        let opens = OPENS.load(SeqCst);
        let s = inject(h, &JUMP_OUT);
        // Cleanly drained fault 6 / reason 55, nothing pending, still alive.
        assert_eq!((s.phase, s.fault, s.reason, s.epoch), (FAULT, 6, 55, first));
        assert_eq!((s.pending, s.cleanup_status), ([0, 0], 0));
        let mut ts = ZeroTimestamp {
            epoch: 0,
            frame: 0,
            ticks: 0,
        };
        assert_eq!(ftu_live_timestamp(h, &mut ts), -2, "no false running state");
        let r = record(h);
        let w = &r.words;
        assert!(r.present());
        assert_eq!((w[rf::EPOCH], w[rf::REASON], w[rf::STAGE]), (first, 55, rf::STAGE_AFTER_PACKING));
        assert_eq!((w[rf::DIRECTION], w[rf::REQUIRED_LEAD]), (1, 16));
        assert!(r.flag(rf::F_DIRECTION | rf::F_SLOT | rf::F_SCHEDULED | rf::F_CHECKED));
        assert!(r.flag(rf::F_PRIOR_STEP | rf::F_PRIOR_PROGRESS | rf::F_BASE | rf::F_TIMEBASE));
        assert!(!r.flag(rf::F_RETURNED) && !r.flag(rf::F_SUBMIT_STATUS));
        // The failed check really was late; the record is not a success sample.
        assert!(w[rf::SCHEDULED] < w[rf::CHECKED_FRAME] + 16);
        assert_eq!(w[rf::CHECKED_TICKS], w[rf::CHECKED_FRAME] * 24000);
        assert!(w[rf::PRIOR_STEP_FRAME] < w[rf::CHECKED_FRAME]);
        assert_eq!((w[rf::FINISHED], w[rf::DRAIN_FAILED], w[rf::CLEANUP]), (1, 0, 0));
        assert!(r.flag(rf::F_CLOSE) && w[rf::CLOSE] == 0);
        assert_eq!((w[rf::TERMINAL_PHASE], w[rf::TERMINAL_FAULT]), (4, 6));
        assert_eq!((w[rf::QUEUE_DEPTH], w[rf::BLOCK_MS], w[rf::REPLAY_MS]), (12, 4, 64));
        // The lifetime first-completion latch is a different record.
        assert_eq!((&*h).shared.first_failure.snapshot().header[2], 0);

        // No new Start: the monitor's recovery reopens the same binding.
        let (rc, _) = monitor(h);
        assert_eq!(rc, 0);
        let s = st(h);
        assert_eq!((s.phase, s.epoch, s.fault, s.reason), (RUNNING, first + 1, 0, 0));
        assert_eq!(OPENS.load(SeqCst), opens + 1);
        assert_eq!(OPENED_REGISTRY.load(SeqCst), REGISTRY);
        assert!(!OVERLAP.load(SeqCst), "old context closed before the reopen");
        assert_eq!(ftu_live_timestamp(h, &mut ts), 0);
        assert_eq!(ts.epoch, first + 1, "fresh seed for the new timeline");
        let v = view(h);
        assert_eq!((v[rp::STATE], v[rp::ATTEMPTS], v[rp::RECOVERED_COUNT]), (rp::RECOVERED, 1, 1));
        assert_eq!((v[rp::CLIENTS], v[rp::FAULT_EPOCH], v[rp::RECOVERED_EPOCH]), (2, first, first + 1));
        // Steady audio in the recovered epoch: nothing missing in either direction.
        advance(h, 1600);
        let (out_count, out_max, in_count, in_max) = steady_missing(&*h);
        assert!(out_count > 0 && in_count > 0);
        assert_eq!((out_max, in_max), (0, 0));
        // The record persists through the restart.
        assert_eq!(record(h), r);
        // A retained client's Stop still means stop.
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert_eq!(st(h).phase, RUNNING);
        assert_eq!(ftu_live_stop(h, 8), 0);
        assert_eq!(st(h).phase, IDLE);
        assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 1, "nothing revived");
        // A normal client Start never clears the record and restores the budget.
        assert_eq!(ftu_live_start(h, 9), 0);
        assert_eq!(record(h), r);
        assert_eq!(view(h)[rp::CONSECUTIVE], 0);
        assert_eq!(ftu_live_stop(h, 9), 0);
        let stats = hal.finish();
        assert!(stats.seeds.windows(2).all(|p| p[1] > p[0]));
        assert!(stats.seeds.contains(&first) && stats.seeds.contains(&(first + 1)));
        assert!(stats.not_running > 0, "IO calls continued across the stop");
        assert!(stats.writes[0] > 0 && stats.reads[0] > 0);
        println!(
            "hal={stats:?} backward_steps={} record={:?}",
            stats.backward,
            &r.words[..]
        );
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

#[test]
fn last_stop_and_detach_cancel_pending_recovery_and_never_revive() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        set_policy(h, Policy { backoff_ms: [60_000; 3], ..rp::POLICY });
        assert_eq!(ftu_live_start(h, 7), 0);
        inject(h, &JUMP_OUT);
        let (rc, backoffs) = monitor_until_pending(h);
        assert_eq!((rc, backoffs), (2, vec![60_000]));
        let opens = OPENS.load(SeqCst);
        // An unknown/stale Stop does not affect the retained client.
        assert_eq!(ftu_live_stop(h, 99), 0);
        assert_eq!((view(h)[rp::STATE], view(h)[rp::CLIENTS]), (rp::PENDING, 1));
        // The retained client's Stop cancels; nothing is revived afterwards.
        assert_eq!(ftu_live_stop(h, 7), 0);
        let v = view(h);
        assert_eq!((v[rp::STATE], v[rp::CANCELLED_COUNT], v[rp::CLIENTS]), (rp::CANCELLED, 1, 0));
        set_policy(h, rp::POLICY);
        for _ in 0..5 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 1);
        }
        assert_eq!((st(h).phase, OPENS.load(SeqCst)), (FAULT, opens));
        // Detach during a pending recovery: retired, cancelled, never reopened.
        set_policy(h, Policy { backoff_ms: [60_000; 3], ..rp::POLICY });
        assert_eq!(ftu_live_start(h, 7), 0);
        inject(h, &JUMP_OUT);
        assert_eq!(monitor_until_pending(h).0, 2);
        let opens = OPENS.load(SeqCst);
        let old = ftu_live_binding(h);
        assert_eq!(ftu_live_detach(h), 0);
        let v = view(h);
        assert_eq!((v[rp::STATE], v[rp::CLIENTS], v[rp::RETIRED]), (rp::CANCELLED, 0, 1));
        assert_eq!(ftu_live_recover(h, old), -6);
        // Replacement enumeration after rebind: no client, nothing to revive.
        PRESENT.store(REGISTRY + 1, SeqCst);
        assert_eq!(ftu_live_rebind(h, 0x2080, LOCATION, REGISTRY + 1), 0);
        assert_eq!(ftu_live_recover(h, old), -6, "stale publication");
        assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 1);
        assert_eq!(ftu_live_start(h, 7), -9, "retired ID not revived");
        assert_eq!(OPENS.load(SeqCst), opens);
        assert_eq!(ftu_live_remove_client(h, 7), 0, "acknowledges the retirement only");
        assert_eq!(view(h)[rp::RETIRED], 0, "acknowledgement is published immediately");
        assert_eq!(ftu_live_stop(h, 7), 0, "late Stop remains harmless");
        assert_eq!(st(h).phase, FAULT);
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(ftu_live_destroy(h), 0);
    }
}
// Calls the monitor until the policy is waiting on its backoff.
unsafe fn monitor_until_pending(h: *mut Live) -> (i32, Vec<u64>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let rc = ftu_live_recover(h, ftu_live_binding(h));
        let w = view(h);
        if rc == 2 && w[rp::STATE] == rp::PENDING {
            return (rc, vec![w[rp::DUE] - w[rp::PENDING_SINCE]]);
        }
        assert!(matches!(rc, 2..=4), "unexpected {rc}");
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn recovery_waits_for_admitted_io_and_starts_reuse_the_policy() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        assert_eq!(ftu_live_start(h, 7), 0);
        let first = inject(h, &JUMP_OUT).epoch;
        wait_joinable(h);
        let opens = OPENS.load(SeqCst);
        // A realtime call admitted before the fault is still inside the HAL.
        (&*h).shared.calls.fetch_add(1, SeqCst);
        for _ in 0..20 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 4);
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!((OPENS.load(SeqCst), st(h).epoch), (opens, first));
        (&*h).shared.calls.fetch_sub(1, SeqCst);
        assert_eq!(monitor(h).0, 0);
        assert_eq!((st(h).phase, st(h).epoch), (RUNNING, first + 1));
        // A repeated Start of a retained client runs the same policy.
        set_policy(h, Policy { backoff_ms: [0; 3], ..rp::POLICY });
        inject(h, &JUMP_OUT);
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let rc = ftu_live_start(h, 7);
            if rc == 0 {
                break;
            }
            assert_eq!(rc, -2);
            assert!(Instant::now() < deadline);
            VIRTUAL.fetch_add(5, SeqCst);
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!((st(h).phase, st(h).epoch), (RUNNING, first + 2));
        assert_eq!(ftu_live_start(h, 8), 0);
        assert_eq!(view(h)[rp::CLIENTS], 2);
        // A refused fault: new and retained Starts both stay refused.
        RESTORE_STATUS.store(-1, SeqCst);
        let s = inject(h, &JUMP_OUT);
        RESTORE_STATUS.store(0, SeqCst);
        assert_eq!((s.phase, s.fault), (FAULT, 13));
        wait_joinable(h);
        assert_eq!(ftu_live_start(h, 9), -2);
        assert_eq!(ftu_live_start(h, 7), -2);
        assert_eq!(view(h)[rp::REFUSAL], rp::R_FAULT_CODE);
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert_eq!(ftu_live_stop(h, 8), 0);
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(ftu_live_destroy(h), 0);
    }
}
// The worker publishes FAULT as its last action; wait until it has returned.
unsafe fn wait_joinable(h: *mut Live) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while (*h)
        .control
        .lock()
        .unwrap()
        .worker
        .as_ref()
        .is_some_and(|w| !w.is_finished())
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn repeated_faults_back_off_exhaust_and_reset_only_after_healthy_run() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        let policy = Policy {
            budget: 3,
            backoff_ms: [0, 40, 80],
            healthy_ms: 3_600_000,
        };
        set_policy(h, policy);
        assert_eq!(ftu_live_start(h, 7), 0);
        for attempt in 0..3 {
            inject(h, &JUMP_OUT);
            let (rc, backoffs) = monitor(h);
            assert_eq!(rc, 0, "attempt {attempt}");
            if attempt == 0 {
                assert!(backoffs.is_empty(), "first attempt is immediate");
            } else {
                assert_eq!(backoffs, vec![policy.backoff_ms[attempt]]);
            }
            // RUNNING alone never restores the budget.
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 1);
            assert_eq!(view(h)[rp::CONSECUTIVE], attempt as u64 + 1);
        }
        let opens = OPENS.load(SeqCst);
        let s = inject(h, &JUMP_OUT);
        wait_joinable(h);
        for _ in 0..5 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), -11);
        }
        let v = view(h);
        assert_eq!((v[rp::STATE], v[rp::EXHAUSTED_COUNT], v[rp::ATTEMPTS]), (rp::EXHAUSTED, 1, 3));
        assert_eq!((OPENS.load(SeqCst), st(h).phase, st(h).epoch), (opens, FAULT, s.epoch));
        assert_eq!(record(h).words[rf::EPOCH], s.epoch, "the visible fault is the last one");
        assert_eq!(record(h).words[rf::OVERWRITTEN], 3);
        // A healthy run resets the consecutive budget; counters persist.
        assert_eq!(ftu_live_stop(h, 7), 0);
        set_policy(h, Policy { healthy_ms: 150, ..policy });
        assert_eq!(ftu_live_start(h, 7), 0);
        inject(h, &JUMP_OUT);
        assert_eq!(monitor(h).0, 0);
        assert_eq!(view(h)[rp::CONSECUTIVE], 1);
        thread::sleep(Duration::from_millis(250));
        assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 1);
        let v = view(h);
        assert_eq!((v[rp::STATE], v[rp::CONSECUTIVE], v[rp::HEALTHY_RESETS]), (rp::HEALTHY, 0, 1));
        inject(h, &JUMP_OUT);
        let (rc, backoffs) = monitor(h);
        assert!(rc == 0 && backoffs.is_empty());
        let v = view(h);
        assert_eq!((v[rp::ATTEMPTS], v[rp::RECOVERED_COUNT], v[rp::CONSECUTIVE]), (5, 5, 1));
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

#[test]
fn ineligible_and_uncertain_faults_are_refused_and_visible() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        // IN stale schedule (reason 20) is not the recoverable OUT deadline.
        assert_eq!(ftu_live_start(h, 7), 0);
        let s = inject(h, &JUMP_IN);
        assert_eq!((s.phase, s.reason), (FAULT, 20));
        let r = record(h);
        assert_eq!((r.words[rf::STAGE], r.words[rf::DIRECTION], r.words[rf::REQUIRED_LEAD]), (rf::STAGE_AFTER_PACKING, 0, 2));
        let opens = OPENS.load(SeqCst);
        assert_eq!(monitor(h).0, -10);
        assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), -10);
        let v = view(h);
        assert_eq!((v[rp::STATE], v[rp::REFUSAL], v[rp::REFUSED_COUNT]), (rp::REFUSED, rp::R_REASON, 1));
        assert_eq!(OPENS.load(SeqCst), opens);
        assert_eq!(ftu_live_stop(h, 7), 0);
        // Replacement enumeration behind a retained client: the pinned reopen is
        // refused by the factory, counted as a failed attempt, never retried.
        assert_eq!(ftu_live_start(h, 7), 0);
        let fault = inject(h, &JUMP_OUT).epoch;
        PRESENT.store(REGISTRY + 7, SeqCst);
        let refused = REFUSED_OPENS.load(SeqCst);
        assert_eq!(monitor(h).0, -5);
        assert_eq!(REFUSED_OPENS.load(SeqCst), refused + 1);
        assert_eq!(OPENED_REGISTRY.load(SeqCst), REGISTRY, "never the replacement");
        let opens = OPENS.load(SeqCst);
        for _ in 0..5 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), -10);
        }
        let v = view(h);
        assert_eq!((v[rp::REFUSAL], v[rp::FAILED], v[rp::LAST_STATUS] as i64), (rp::R_ATTEMPT_FAILED, 1, -5));
        assert_eq!((OPENS.load(SeqCst), st(h).fault), (opens, 1));
        assert_eq!(record(h).words[rf::EPOCH], fault, "startup failure is not a runtime record");
        assert_eq!(ftu_live_stop(h, 7), 0);
        PRESENT.store(REGISTRY, SeqCst);
        ROUTE_FAULT.store(0,SeqCst);ROUTE_FAULT_APPLIED.store(false,SeqCst);REMOVED.store(false,SeqCst);
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(ftu_live_destroy(h), 0);
        // Close failure after the fault: quarantine, never reopened.
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        assert_eq!(ftu_live_start(h, 7), 0);
        CLOSE_STATUS.store(-7, SeqCst);
        let s = inject(h, &JUMP_OUT);
        CLOSE_STATUS.store(0, SeqCst);
        assert_eq!((s.phase, s.fault), (QUARANTINED, 8));
        let r = record(h);
        assert_eq!((r.words[rf::FINISHED], r.words[rf::TERMINAL_PHASE], r.words[rf::CLOSE] as i64), (1, 5, -7));
        let opens = OPENS.load(SeqCst);
        assert_eq!(monitor(h).0, -3);
        assert_eq!(view(h)[rp::REFUSAL], rp::R_QUARANTINE);
        assert_eq!(ftu_live_start(h, 8), -2);
        assert_eq!(OPENS.load(SeqCst), opens);
        assert_eq!(ftu_live_stop(h, 7), -3, "join only; retain quarantine");
        assert_ne!(ftu_live_destroy(h), 0, "quarantine retained");
        // Failed cleanup: uncertain native ownership, quarantine, never reopened.
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        assert_eq!(ftu_live_start(h, 7), 0);
        CLEANUP_STATUS.store(-9, SeqCst);
        let s = inject(h, &JUMP_OUT);
        CLEANUP_STATUS.store(0, SeqCst);
        assert_eq!((s.phase, s.fault, s.cleanup_status), (QUARANTINED, 7, -9));
        let r = record(h);
        assert_eq!((r.words[rf::FINISHED], r.words[rf::CLEANUP] as i64), (1, -9));
        assert!(!r.flag(rf::F_CLOSE), "close never attempted");
        let opens = OPENS.load(SeqCst);
        assert_eq!(monitor(h).0, -3);
        assert_eq!(OPENS.load(SeqCst), opens);
        // Stop joins the finished worker but preserves the quarantined handle
        // and uncertain native context. Do not leave an unjoined fixture thread.
        assert_eq!(ftu_live_stop(h, 7), -3);
        assert_ne!(ftu_live_destroy(h), 0, "quarantine retained");
    }
}

// Coherent ftuE/ftuA reads while the worker publishes and recovery runs.
#[test]
fn diagnostic_snapshots_stay_coherent_across_fault_and_recovery() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        assert_eq!(ftu_live_start(h, 7), 0);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = h as usize;
        let reader = thread::spawn(move || {
            let h = handle as *mut Live;
            let mut seen = 0;
            while !flag.load(SeqCst) {
                let r = record(h);
                if r.present() {
                    let w = r.words;
                    assert_eq!(w[rf::REASON], 55);
                    assert_eq!(w[rf::CHECKED_TICKS], w[rf::CHECKED_FRAME] * 24000);
                    if w[rf::FINISHED] == 1 {
                        assert_eq!((w[rf::TERMINAL_PHASE], w[rf::TERMINAL_FAULT]), (4, 6));
                    }
                    seen += 1;
                }
                let mut v = RecoverySnapshot {
                    words: [0; rp::WORDS],
                };
                ftu_live_recovery(h, &mut v);
                if v.words[1] == 1 {
                    assert!(v.words[rp::RECOVERED_COUNT] <= v.words[rp::ATTEMPTS]);
                }
            }
            seen
        });
        set_policy(h, Policy { backoff_ms: [0; 3], ..rp::POLICY });
        for _ in 0..3 {
            inject(h, &JUMP_OUT);
            assert_eq!(monitor(h).0, 0);
        }
        stop.store(true, SeqCst);
        assert!(reader.join().unwrap() > 0);
        assert_eq!(record(h).words[rf::RECORDS], 3);
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

unsafe fn arm_route(h:*mut Live,fault:u32) {
    ROUTE_FAULT_APPLIED.store(false,SeqCst);ROUTE_FAULT.store(fault,SeqCst);
    let deadline=Instant::now()+Duration::from_secs(10);
    while !ROUTE_FAULT_APPLIED.load(SeqCst) {
        assert_eq!(st(h).phase,RUNNING);assert!(Instant::now()<deadline);thread::yield_now();
    }
}
unsafe fn arm_removal(h:*mut Live,fault:u32) {
    arm_route(h,fault);PRESENT.store(0,SeqCst);REMOVED.store(true,SeqCst);
}
#[test]
fn disconnected_abort_drains_rebinds_and_retires_old_clients_repeatedly() {
    let _guard=Harness::new();
    unsafe {
        let h=ftu_live_create_pinned(&REC_FACTORY,0x2080,LOCATION,REGISTRY);
        let hal=Hal::start(h,128);
        for cycle in 0..16u32 {
            let old_binding=ftu_live_binding(h);
            assert_eq!(ftu_live_start(h,7),0);assert_eq!(ftu_live_start(h,8),0);
            advance(h,300);let epoch=st(h).epoch;
            arm_removal(h,9);
            assert_eq!(ftu_live_detach(h),0);
            assert_eq!(OPEN_CONTEXTS.load(SeqCst),0);
            assert_eq!(st(h).pending,[0,0]);assert_ne!(st(h).phase,QUARANTINED);
            let shut=(&*h).shared.shutdown.snapshot();let w=shut.words;
            assert_eq!((w[1],w[2],w[3],w[4],w[5],w[6],w[7],w[8]),(1,1,epoch,1,1,1,1,1));
            assert_eq!((w[18],w[19],w[20],w[22],w[24]),(0,0,0,1,0));
            assert_eq!(w[26] as i32,0xe00002c0u32 as i32);
            assert_eq!((w[30],w[31],w[32],w[33]),(3,0,0,0));
            let registry=REGISTRY+cycle as u64+1;
            PRESENT.store(registry,SeqCst);REMOVED.store(false,SeqCst);
            assert_eq!(ftu_live_rebind(h,0x2080,LOCATION+cycle,registry),0);
            assert_eq!(ftu_live_start_bound(h,9,old_binding),-6);
            assert_eq!(ftu_live_start(h,7),-9,"old client must acknowledge its retirement");
            assert_eq!(ftu_live_start(h,9),0);
            assert_eq!(OPENED_REGISTRY.load(SeqCst),registry);
            assert_eq!((&*h).shared.shutdown.snapshot().words,w,"new Start retains shutdown evidence");
            assert_eq!(ftu_live_stop(h,7),0);assert_eq!(ftu_live_stop(h,8),0);
            assert_eq!(st(h).phase,RUNNING,"late old Stops cannot stop the new generation");
            advance(h,1600);let (a,b,c,d)=steady_missing(&*h);assert!(a>0 && c>0);assert_eq!((b,d),(0,0));
            assert_eq!(ftu_live_stop(h,9),0);assert_eq!(OPEN_CONTEXTS.load(SeqCst),0);
        }
        assert!(!OVERLAP.load(SeqCst));assert_eq!(OPENS.load(SeqCst),32);
        let stats=hal.finish();assert!(stats.writes[0]>0 && stats.reads[0]>0);
        assert!(stats.seeds.windows(2).all(|w|w[1]>w[0]));
        assert_eq!(ftu_live_destroy(h),0);
    }
}
#[test]
fn disconnected_missing_callbacks_still_quarantine_without_clients() {
    let _guard=Harness::new();
    unsafe {
        let h=ftu_live_create_pinned(&REC_FACTORY,0x2080,LOCATION,REGISTRY);
        assert_eq!(ftu_live_start(h,7),0);advance(h,300);arm_removal(h,10);
        assert_eq!(ftu_live_detach(h),-3);
        let s=st(h);assert_eq!(s.phase,QUARANTINED);
        let w=(&*h).shared.shutdown.snapshot().words;
        assert_eq!((w[4],w[5],w[7],w[8],w[22]),(1,0,0,0,0));assert!(w[18]>0);
        assert_eq!(w[10],0,"native cleanup never attempted with pending ownership");
        assert_eq!(w[26] as i32,0xe00002c0u32 as i32);
        assert_eq!(ftu_live_recover(h,ftu_live_binding(h)),-3);
        let v=view(h);assert_eq!((v[rp::STATE],v[rp::REFUSAL],v[rp::FAULT_EPOCH]),(rp::REFUSED,rp::R_QUARANTINE,s.epoch));
        assert_eq!(v[rp::CLIENTS],0);assert_eq!(v[rp::ATTEMPTS],0);
        for _ in 0..3 { assert_eq!(ftu_live_recover(h,0),-3); }
        assert_eq!(view(h)[rp::REFUSED_COUNT],v[rp::REFUSED_COUNT]);
        assert_eq!(ftu_live_rebind(h,0x2080,LOCATION,REGISTRY+1),-3);
        assert_eq!(OPENS.load(SeqCst),1);assert_eq!(OPEN_CONTEXTS.load(SeqCst),1);
        assert_ne!(ftu_live_destroy(h),0);
    }
}

#[test]
fn removal_callback_and_deadline_stop_before_detach_do_not_reopen_the_old_binding() {
    let _guard=Harness::new();
    unsafe {
        let h=ftu_live_create_pinned(&REC_FACTORY,0x2080,LOCATION,REGISTRY);
        for (cycle,mode) in [11u32,9].iter().enumerate() {
            assert_eq!(ftu_live_start(h,7),0);advance(h,300);
            let epoch=st(h).epoch;let opens=OPENS.load(SeqCst);
            if *mode==9 { arm_removal(h,9);JUMP_OUT.store(64,SeqCst); }
            else { REMOVED.store(true,SeqCst);PRESENT.store(0,SeqCst);ROUTE_FAULT.store(*mode,SeqCst); }
            let s=wait_phase(h,&[FAULT,QUARANTINED]);
            assert_eq!((s.phase,s.fault,s.reason),(FAULT,16,if *mode==11 {23} else {55}));
            let (rc,_)=monitor(h);assert_eq!(rc,-10);
            assert_eq!(view(h)[rp::REFUSAL],rp::R_FAULT_CODE);assert_eq!(view(h)[rp::ATTEMPTS],0);
            assert_eq!(OPENS.load(SeqCst),opens);
            let w=(&*h).shared.shutdown.snapshot().words;
            assert_eq!((w[3],w[5],w[8],w[30]),(epoch,1,1,3));
            assert_eq!(w[26] as u32,0xe00002c0);assert_eq!(record(h).words[rf::TERMINAL_FAULT],16);
            assert_eq!(ftu_live_detach(h),0);
            assert_eq!(ftu_live_stop(h,7),0); // acknowledge removed client's retirement
            let registry=REGISTRY+cycle as u64+1;PRESENT.store(registry,SeqCst);REMOVED.store(false,SeqCst);
            assert_eq!(ftu_live_rebind(h,0x2080,LOCATION,registry),0);
        }
        assert_eq!(ftu_live_start(h,9),0);assert_eq!(ftu_live_stop(h,9),0);
        assert_eq!(ftu_live_destroy(h),0);
    }
}
#[test]
fn missing_removal_proof_cannot_excuse_failed_postconditions() {
    let _guard=Harness::new();
    unsafe {
        let h=ftu_live_create_pinned(&REC_FACTORY,0x2080,LOCATION,REGISTRY);
        assert_eq!(ftu_live_start(h,7),0);advance(h,300);
        REMOVED.store(true,SeqCst);ROUTE_FAULT.store(6,SeqCst); // completion NoDevice, abort success
        let s=wait_phase(h,&[FAULT,QUARANTINED]);assert_eq!((s.phase,s.fault),(FAULT,10));
        let w=(&*h).shared.shutdown.snapshot().words;assert_eq!((w[26],w[27],w[30]),(0,0,0));
        assert_eq!(monitor(h).0,-10);assert_eq!(OPENS.load(SeqCst),1);
        assert_eq!(ftu_live_stop(h,7),0);assert_eq!(ftu_live_destroy(h),0);
    }
}

#[test]
fn partial_removal_validates_the_other_endpoint_rate_in_reply_order() {
    let _guard=Harness::new();
    unsafe {
        let h=ftu_live_create_pinned(&REC_FACTORY,0x2080,LOCATION,REGISTRY);
        // (native fault, removed-direction mask, bad RAW rate reply mask, terminal fault).
        // A wrong IN/OUT mapping reverses both outcomes for each direction.
        for (mode,mask,bad_reply,expected) in [(12,1,2,16),(12,1,1,10),(13,2,1,16),(13,2,2,10)] {
            REMOVED.store(false,SeqCst);
            assert_eq!(ftu_live_start(h,7),0);advance(h,300);arm_route(h,mode);
            REMOVED.store(true,SeqCst);BAD_RATE_REPLIES.store(bad_reply,SeqCst);
            assert_eq!(ftu_live_stop(h,7),0);
            let s=st(h);assert_eq!((s.phase,s.fault),(FAULT,expected));
            let w=(&*h).shared.shutdown.snapshot().words;
            assert_eq!((w[8],w[30],w[31]),(1,mask,3^mask));
            assert_eq!(OPEN_CONTEXTS.load(SeqCst),0);
        }
        assert_eq!(ftu_live_destroy(h),0);
    }
}
#[test]
fn removed_status_without_enumeration_change_requires_all_clients_to_stop_before_manual_retry() {
    let _guard=Harness::new();
    unsafe {
        let h=ftu_live_create_pinned(&REC_FACTORY,0x2080,LOCATION,REGISTRY);
        assert_eq!(ftu_live_start(h,7),0);advance(h,300);let epoch=st(h).epoch;
        // Enumeration and normal cleanup/rate observations remain present.
        // Only abort reports gone. This is intentionally conservative evidence.
        arm_route(h,9);JUMP_OUT.store(64,SeqCst);
        assert_eq!(wait_phase(h,&[FAULT,QUARANTINED]).fault,16);
        let w=(&*h).shared.shutdown.snapshot().words;assert_eq!((w[30],w[31]),(3,3));
        for _ in 0..3 { assert_eq!(monitor(h).0,-10); }
        assert_eq!(ftu_live_start(h,7),-2);assert_eq!(ftu_live_start(h,8),-2);
        assert_eq!(OPENS.load(SeqCst),1);assert_eq!(view(h)[rp::ATTEMPTS],0);
        assert_eq!(ftu_live_stop(h,7),0);assert_eq!(OPEN_CONTEXTS.load(SeqCst),0);
        assert_eq!(ftu_live_start(h,8),0,"a fresh client session may explicitly retry a fully closed context");
        assert_eq!(OPENED_REGISTRY.load(SeqCst),REGISTRY);assert_eq!(st(h).epoch,epoch+1);
        assert_eq!(ftu_live_stop(h,8),0);assert_eq!(ftu_live_destroy(h),0);
    }
}

mod failed_restart {
    use super::*;
    include!("failed_recovery_tests.rs");
}
