// Reconnect contract through the public FFI on whichever worker this build
// links (baseline or independent-4ms). The fake open records the binding the
// worker was given and never yields a running transport, so these cases are
// identical for both profiles. Running-session cases live in pool_simulation.
use std::sync::atomic::AtomicPtr;

static SERIAL: Mutex<()> = Mutex::new(());
static OPENS: AtomicU64 = AtomicU64::new(0);
static OPENED: AtomicU64 = AtomicU64::new(0);
static OPENED_REGISTRY: AtomicU64 = AtomicU64::new(0);
static INSIDE: AtomicU32 = AtomicU32::new(0);
static OVERLAP: AtomicBool = AtomicBool::new(false);
static HOLD: AtomicBool = AtomicBool::new(false);
static ENTERED: AtomicBool = AtomicBool::new(false);
static RETAIN: AtomicBool = AtomicBool::new(false);
static RETAINED: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

unsafe extern "C" fn recording_open(product: u32, location: u32, registry: u64) -> *mut c_void {
    if INSIDE.fetch_add(1, SeqCst) != 0 {
        OVERLAP.store(true, SeqCst);
    }
    OPENS.fetch_add(1, SeqCst);
    OPENED.store((u64::from(product) << 32) | u64::from(location), SeqCst);
    OPENED_REGISTRY.store(registry, SeqCst);
    ENTERED.store(true, SeqCst);
    let deadline = Instant::now() + Duration::from_secs(10);
    while HOLD.load(SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    INSIDE.fetch_sub(1, SeqCst);
    if RETAIN.load(SeqCst) {
        // Non-null context without operations: the worker quarantines it.
        let context = Box::into_raw(Box::new(0_u8));
        RETAINED.store(context, SeqCst);
        return context.cast();
    }
    std::ptr::null_mut()
}
unsafe extern "C" fn no_ops(_: *mut c_void) -> *const PoolOps {
    std::ptr::null()
}
unsafe extern "C" fn refuse_close(_: *mut c_void) -> i32 {
    panic!("no context in these cases may be closed")
}
unsafe extern "C" fn refuse_schedule(_: *mut c_void, _: *const ThreadSchedule) -> i32 {
    -1
}
unsafe extern "C" fn refuse_rate(_: *mut c_void, _: u32, _: u32, _: *mut u32) -> i32 {
    -1
}
const RECORDING: Factory = Factory {
    open: recording_open,
    ops: no_ops,
    close: refuse_close,
    configure_thread: refuse_schedule,
    set_rate: refuse_rate,
};
fn serial() -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    OPENS.store(0, SeqCst);
    OPENED.store(0, SeqCst);
    OPENED_REGISTRY.store(0, SeqCst);
    OVERLAP.store(false, SeqCst);
    HOLD.store(false, SeqCst);
    ENTERED.store(false, SeqCst);
    RETAIN.store(false, SeqCst);
    guard
}
unsafe fn status(h: *mut Live) -> Status {
    let mut s = Status::default();
    ftu_live_status(h, &mut s);
    s
}
// Start reaches the worker, which opens exactly the current binding (location
// and USB enumeration) in a new epoch; the fake keeps it offline -> Fault.
unsafe fn start_opens(h: *mut Live, client: u32, location: u32, registry: u64) {
    let opens = OPENS.load(SeqCst);
    let epoch = status(h).epoch;
    assert_eq!(ftu_live_start_bound(h, client, ftu_live_binding(h)), -5);
    assert_eq!(OPENS.load(SeqCst), opens + 1);
    assert_eq!(
        OPENED.load(SeqCst),
        (0x2080_u64 << 32) | u64::from(location)
    );
    assert_eq!(OPENED_REGISTRY.load(SeqCst), registry);
    let s = status(h);
    assert_eq!(
        (s.phase, s.fault, s.epoch, s.location),
        (FAULT, 1, epoch + 1, location)
    );
    assert_eq!(ftu_live_stop(h, client), 0);
}

#[test]
fn detach_refuses_starts_and_rebind_moves_the_next_open() {
    let _serial = serial();
    unsafe {
        assert!(ftu_live_create_pinned(&RECORDING, 0x2080, 0x0210_0000, 0).is_null());
        let h = ftu_live_create_pinned(&RECORDING, 0x2080, 0x0210_0000, 0x1001);
        start_opens(h, 1, 0x0210_0000, 0x1001);
        assert_eq!(ftu_live_detach(h), 0);
        assert_eq!(ftu_live_start(h, 2), -6);
        assert_eq!(OPENS.load(SeqCst), 1);
        // A delayed Stop from the removed unit's client stays idempotent.
        assert_eq!(ftu_live_stop(h, 1), 0);
        assert_eq!(ftu_live_detach(h), 0, "detach is idempotent");
        assert_eq!(ftu_live_rebind(h, 0x2081, 0x2021_1100, 0x1002), -7);
        assert_eq!(
            ftu_live_rebind(h, 0x2080, 0x2021_1100, 0),
            -1,
            "HAL is pinned"
        );
        assert_eq!(status(h).location, 0x0210_0000);
        assert_eq!(ftu_live_rebind(h, 0x2080, 0x2021_1100, 0x1002), 0);
        assert_eq!(ftu_live_rebind(h, 0x2080, 0x2021_1100, 0x1002), -8);
        start_opens(h, 3, 0x2021_1100, 0x1002);
        // Repeated reconnects: monotonic epochs, one open per Start, no growth.
        for i in 0..64_u32 {
            assert_eq!(ftu_live_detach(h), 0);
            assert_eq!(ftu_live_start(h, 4), -6);
            let registry = 0x2000 + u64::from(i);
            assert_eq!(ftu_live_rebind(h, 0x2080, 0x100 + i % 5, registry), 0);
            start_opens(h, 4, 0x100 + i % 5, registry);
        }
        assert_eq!(OPENS.load(SeqCst), 2 + 64);
        assert_eq!(status(h).epoch, 2 + 64);
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

// Same port, same product: only the registry entry ID tells a replacement
// enumeration apart. Without detach/rebind the worker is never given it.
#[test]
fn worker_open_is_pinned_to_the_bound_enumeration() {
    let _serial = serial();
    unsafe {
        let h = ftu_live_create_pinned(&RECORDING, 0x2080, 0x0210_0000, 0x51);
        start_opens(h, 1, 0x0210_0000, 0x51);
        // A replug at the same location creates entry 0x52; no rebind happened,
        // so the next open still demands 0x51 and a pinned factory refuses 0x52.
        start_opens(h, 1, 0x0210_0000, 0x51);
        assert_eq!(ftu_live_detach(h), 0);
        assert_eq!(ftu_live_rebind(h, 0x2080, 0x0210_0000, 0x52), 0);
        start_opens(h, 1, 0x0210_0000, 0x52);
        assert_eq!(ftu_live_destroy(h), 0);
        // Standalone tools stay explicitly unpinned.
        let tool = ftu_live_create(&RECORDING, 0x2080, 0x10);
        start_opens(tool, 1, 0x10, 0);
        assert_eq!(ftu_live_destroy(tool), 0);
    }
}

// A Start queued against the previous publication (binding read before the
// detach/rebind) is refused once it reaches the lock: it never opens USB.
#[test]
fn stale_binding_start_never_opens_the_new_unit() {
    let _serial = serial();
    unsafe {
        let h = ftu_live_create_pinned(&RECORDING, 0x2080, 0x10, 0x61);
        let old = ftu_live_binding(h);
        assert_eq!(ftu_live_detach(h), 0);
        assert_eq!(ftu_live_start_bound(h, 1, old), -6);
        assert_eq!(ftu_live_rebind(h, 0x2080, 0x20, 0x62), 0);
        assert_ne!(ftu_live_binding(h), old);
        assert_eq!(ftu_live_start_bound(h, 1, old), -6);
        assert_eq!(OPENS.load(SeqCst), 0);
        // Retirement acknowledgement of an ID that was never active is inert.
        assert_eq!(ftu_live_remove_client(h, 1), 1);
        start_opens(h, 1, 0x20, 0x62);
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

#[test]
fn detach_is_serialized_behind_an_inflight_start() {
    let _serial = serial();
    unsafe {
        let h = ftu_live_create_pinned(&RECORDING, 0x2080, 0x10, 0x71);
        let handle = h as usize;
        HOLD.store(true, SeqCst);
        let start = thread::spawn(move || ftu_live_start(handle as *mut Live, 1));
        while !ENTERED.load(SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
        let detached = Arc::new(AtomicBool::new(false));
        let flag = detached.clone();
        let detach = thread::spawn(move || {
            let rc = ftu_live_detach(handle as *mut Live);
            flag.store(true, SeqCst);
            rc
        });
        thread::sleep(Duration::from_millis(100));
        // The worker is still inside open: detach must wait for Start's lock.
        assert!(!detached.load(SeqCst));
        assert_eq!(status(h).phase, STARTING);
        HOLD.store(false, SeqCst);
        assert_eq!(start.join().unwrap(), -5);
        assert_eq!(detach.join().unwrap(), 0);
        assert_eq!(ftu_live_start(h, 2), -6);
        assert_eq!(OPENS.load(SeqCst), 1);
        assert_eq!(ftu_live_rebind(h, 0x2080, 0x20, 0x72), 0);
        start_opens(h, 2, 0x20, 0x72);
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

#[test]
fn quarantine_survives_detach_and_refuses_rebind() {
    let _serial = serial();
    unsafe {
        let h = ftu_live_create_pinned(&RECORDING, 0x2080, 0x30, 0x81);
        RETAIN.store(true, SeqCst);
        assert_ne!(ftu_live_start(h, 1), 0);
        RETAIN.store(false, SeqCst);
        let s = status(h);
        assert_eq!((s.phase, s.fault), (QUARANTINED, 2));
        assert_eq!(ftu_live_detach(h), -3);
        assert_eq!(ftu_live_rebind(h, 0x2080, 0x40, 0x82), -3);
        assert_eq!(ftu_live_start(h, 2), -6);
        assert_eq!(status(h).phase, QUARANTINED);
        assert_eq!(status(h).location, 0x30, "never moved over retained state");
        assert_eq!(OPENS.load(SeqCst), 1, "never reopened");
        assert_ne!(ftu_live_destroy(h), 0);
        // Test-only reclaim: the worker has been joined and the fake context has
        // no asynchronous owner; production retains both.
        drop(Box::from_raw(RETAINED.swap(std::ptr::null_mut(), SeqCst)));
        drop(Box::from_raw(h));
    }
}
