//! Recycled native slots with adversarial completion order and failures.
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
#[derive(Clone)]
struct Transfer {
    ticket: u64,
    start: u64,
    lengths: [u16; 64],
    reported: bool,
    aborted: bool,
}
struct Fake {
    prepared: usize,
    delay_ms: u64,
    stall_frame: Option<u64>,
    stall_ms: u64,
    submit_cost_ms: u64,
    frame: u64,
    transfers: [[Option<Transfer>; 4]; 2],
    data: [[Vec<u8>; 4]; 2],
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
            delay_ms: 0,
            stall_frame: None,
            stall_ms: 0,
            submit_cost_ms: 0,
            frame: 100,
            transfers: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            data: std::array::from_fn(|_| std::array::from_fn(|_| vec![0; BLOCK_BYTES])),
            fault,
            cap_delivered: 0,
            stop,
            cleaned: false,
            policy_active: false,
            order: [Vec::with_capacity(count), Vec::with_capacity(count)],
            epoch: 1,
        }
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
unsafe extern "C" fn activate(_: *mut c_void) -> i32 {
    0
}
unsafe extern "C" fn clock(ctx: *mut c_void, out: *mut Clock) -> i32 {
    let f = fake(ctx);
    *out = Clock {
        frame: f.frame,
        ticks: f.frame * 24000,
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
    f.frame += f.submit_cost_ms;
    assert!(f.transfers[d][slot].as_ref().is_none_or(|t| t.reported));
    if f.fault == 1 && d == 1 && ticket == 7 {
        return -99;
    }
    if f.fault == 13 && d == 0 && ticket == 11 {
        return 0xe00002c0u32 as i32;
    }
    let mut len = [0; 64];
    len.copy_from_slice(std::slice::from_raw_parts(lengths, 64));
    f.transfers[d][slot] = Some(Transfer {
        ticket,
        start,
        lengths: len,
        reported: false,
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
    let delay = if seq.is_multiple_of(2) {
        if f.fault == 8 {
            10
        } else {
            f.delay_ms
        }
    } else {
        0
    };
    if t.reported || (!t.aborted && f.frame < t.start + 8 + delay) {
        return 0;
    }
    if f.fault == 11 && seq >= 4 {
        return 0;
    }
    let bad = t.aborted || (f.fault == 2 && d == 0 && seq == 8);
    let status = if bad { 0xe00002ebu32 as i32 } else { 0 };
    let mut c = Completion {
        status,
        matched: if f.fault == 6 && d == 0 && seq == 4 {
            0
        } else {
            1
        },
        callback_ticks: f.frame * 24000,
        packets: [Packet::default(); 64],
    };
    for (i, p) in c.packets.iter_mut().enumerate() {
        *p = Packet {
            status,
            requested: t.lengths[i],
            actual: if d == 0 {
                ((seq * 64 + i as u64) % 3 + 5) as u16 * 24
            } else {
                t.lengths[i]
            },
            ticks: (t.start + i as u64 / 8) * 24000,
        };
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
        c.packets[56].ticks -= 20809;
        c.packets[57].ticks -= 17620;
    }
    if f.fault == 18 && d == 0 {
        for (i, p) in c.packets.iter_mut().enumerate() {
            p.ticks -= (7 - i as u64 % 8) * 3000;
        }
    }
    if f.fault == 12 && d == 0 && seq == 10 {
        for p in &mut c.packets {
            p.ticks += 6 * 24000;
        }
    }
    if f.fault == 14 && d == 0 && seq == 6 {
        for p in &mut c.packets {
            p.ticks -= 8 * 24000;
        }
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
        result: c,
    };
    1
}
unsafe extern "C" fn pump(ctx: *mut c_void) {
    let f = fake(ctx);
    f.frame += 1;
    if f.stall_frame.is_some_and(|at| f.frame >= at) {
        f.frame += f.stall_ms;
        f.stall_frame = None;
    }
    if f.fault == 3 && f.cap_delivered >= 4 {
        f.frame += 200;
        f.fault = 0;
    }
    if f.fault == 4 && f.frame >= 280 {
        std::ptr::write_volatile(f.stop, 1);
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
    fake(ctx).frame * 24000
}
const OPS: PoolOps = PoolOps {
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
    let mut stop = 0;
    let mut f = Fake::new(fault, &mut stop, count);
    f.prepared = depth;
    let mut s = PoolSession::with_depth(
        f.epoch,
        Info {
            timebase_numer: 125,
            timebase_denom: 3,
            ..Info::default()
        },
        depth,
    );
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
unsafe extern "C" fn live_fake_open(_: u32, _: u32) -> *mut c_void {
    let mut f = Fake::new(0, std::ptr::null_mut(), 50000);
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

#[test]
fn live_journal_recycles_without_duration_storage_or_allocations() {
    struct Sink(crate::timing::Timing, crate::worker_timing::WorkerTiming);

    impl StreamIo for Sink {
        fn worker_timing(&self) -> bool {
            true
        }
        fn worker_event(&mut self, mut e: crate::worker_timing::Event) {
            e.captured = 48000;
            self.1.observe(e);
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
    let mut signal = 0;
    let mut f = Fake::new(0, &mut signal, 10000);
    f.prepared = crate::geometry::LIVE_QUEUE_DEPTH;
    let ctx = (&mut f as *mut Fake).cast::<c_void>();
    let mut s = PoolSession::with_depth(
        1,
        Info {
            timebase_numer: 125,
            timebase_denom: 3,
            ..Info::default()
        },
        crate::geometry::LIVE_QUEUE_DEPTH,
    );
    let mut records = [
        vec![Trace::default(); FEEDBACK_SLOTS],
        vec![Trace::default(); FEEDBACK_SLOTS],
    ];
    let mut journal = Journal {
        records: &mut records,
        recycle: true,
    };
    let mut sink = Sink(
        crate::timing::Timing::new(),
        crate::worker_timing::WorkerTiming::new(),
    );
    sink.1.reset(1);
    sink.1.configure(125, 3, crate::geometry::LIVE_QUEUE_DEPTH);
    ALLOCS.with(|v| v.set(0));
    TRACK.with(|v| v.set(true));
    unsafe {
        s.run_with(&OPS, ctx, &mut journal, &mut sink, None, &|| {
            fake(ctx).cap_delivered >= 5000
        });
        s.drain_with(&OPS, ctx, &mut journal, &mut sink);
    }
    TRACK.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(|v| v.get()), 0);
    assert_eq!(s.reason, Some("signal"));
    assert_eq!(s.pending(), [0; 2]);
    assert!(s.reused[0] >= 5000 - crate::geometry::LIVE_QUEUE_DEPTH && s.reused[1] > 4900);
    assert!(s
        .max_pending
        .iter()
        .all(|&n| n <= crate::geometry::LIVE_QUEUE_DEPTH));
    assert_eq!(records[0].len() + records[1].len(), 64);
    let snapshot = sink.0.snapshot();
    let check = &snapshot.metrics[1][crate::timing::Metric::SubmitCheckLeadMs as usize];
    let returned = &snapshot.metrics[1][crate::timing::Metric::SubmitReturnLeadMs as usize];
    assert_eq!(check.count, s.next[1] as u64);
    assert_eq!(returned.count, check.count);
    assert!(returned.min > 1);
    assert_eq!(returned.dropped, 0);
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
unsafe extern "C" fn quarantine_open(_: u32, _: u32) -> *mut c_void {
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
}

unsafe extern "C" fn failed_submit_open(_: u32, _: u32) -> *mut c_void {
    Box::into_raw(Box::new(Fake::new(1, std::ptr::null_mut(), 50000))).cast()
}
#[test]
fn live_failed_submission_preserves_native_error_and_scheduling_evidence() {
    use crate::live::*;
    let factory = Factory {
        open: failed_submit_open,
        ops: live_fake_ops,
        close: live_fake_close,
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
        assert_eq!(s.submit_start, 228);
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
        fn worker_event(&mut self, e: Event) {
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
