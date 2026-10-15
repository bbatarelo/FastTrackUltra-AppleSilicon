//! Fake OS boundary: exercise the actual Rust session loop without USB.
use super::*;
#[derive(Clone)]
struct Transfer {
    start: u64,
    lengths: [u16; ENTRIES],
    done: bool,
    reported: bool,
    aborted: bool,
}
struct Fake {
    frame: u64,
    transfers: [Vec<Option<Transfer>>; 2],
    data: [Vec<Vec<u8>>; 2],
    fault: u8,
    cap_delivered: usize,
    stop: *mut i32,
    max_pending: [usize; 2],
    cleaned: bool,
    completion_order: [Vec<usize>; 2],
}
impl Fake {
    fn new(count: usize, fault: u8, stop: *mut i32) -> Self {
        Self {
            frame: 100,
            transfers: [vec![None; count], vec![None; count]],
            data: [
                vec![vec![0; BLOCK_BYTES]; count],
                vec![vec![0; BLOCK_BYTES]; count],
            ],
            fault,
            cap_delivered: 0,
            stop,
            max_pending: [0; 2],
            cleaned: false,
            completion_order: [Vec::new(), Vec::new()],
        }
    }
}
unsafe fn fake<'a>(ctx: *mut c_void) -> &'a mut Fake {
    &mut *ctx.cast::<Fake>()
}
unsafe extern "C" fn config(
    _: *mut c_void,
    bytes: *mut u8,
    capacity: usize,
    length: *mut usize,
) -> i32 {
    let fixture = include_bytes!("../../ftu-protocol/tests/fixtures/config-0.bin");
    assert!(capacity >= fixture.len());
    std::ptr::copy_nonoverlapping(fixture.as_ptr(), bytes, fixture.len());
    *length = fixture.len();
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
    assert_eq!(fake(ctx).data[0].len(), count as usize);
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
unsafe extern "C" fn buffer(ctx: *mut c_void, d: u32, i: u32) -> *mut u8 {
    fake(ctx).data[d as usize][i as usize].as_mut_ptr()
}
unsafe extern "C" fn submit(
    ctx: *mut c_void,
    d: u32,
    i: u32,
    start: u64,
    lengths: *const u16,
) -> i32 {
    let f = fake(ctx);
    let d = d as usize;
    let i = i as usize;
    if f.fault == 1 && d == 1 && i == 1 {
        return -99;
    }
    let mut sizes = [0; ENTRIES];
    sizes.copy_from_slice(std::slice::from_raw_parts(lengths, ENTRIES));
    f.transfers[d][i] = Some(Transfer {
        start,
        lengths: sizes,
        done: false,
        reported: false,
        aborted: false,
    });
    let pending = f.transfers[d]
        .iter()
        .flatten()
        .filter(|t| !t.reported)
        .count();
    f.max_pending[d] = f.max_pending[d].max(pending);
    0
}
unsafe extern "C" fn completion(ctx: *mut c_void, d: u32, i: u32, out: *mut Completion) -> i32 {
    let f = fake(ctx);
    let d = d as usize;
    let i = i as usize;
    let Some(t) = f.transfers[d][i].as_mut() else {
        return 0;
    };
    if !t.done || t.reported {
        return 0;
    }
    let bad = t.aborted || (f.fault == 2 && d == 0 && i == 2);
    let status = if bad { 0xe00002ebu32 as i32 } else { 0 };
    let mut c = Completion {
        status,
        matched: if f.fault == 6 && d == 0 && i == 0 {
            0
        } else {
            1
        },
        callback_ticks: f.frame * 24000,
        packets: [Packet::default(); ENTRIES],
    };
    for (slot, p) in c.packets.iter_mut().enumerate() {
        *p = Packet {
            status,
            requested: t.lengths[slot],
            actual: if d == 0 {
                ((i * ENTRIES + slot) % 3 + 5) as u16 * 24
            } else {
                t.lengths[slot]
            },
            ticks: (t.start + slot as u64 / 8) * 24000,
        };
    }
    if f.fault == 5 && d == 0 && i == 0 {
        c.packets[0].actual = 0;
    }
    t.reported = true;
    f.completion_order[d].push(i);
    *out = c;
    if d == 0 {
        f.cap_delivered += 1;
    }
    1
}
unsafe extern "C" fn pump(ctx: *mut c_void) {
    let f = fake(ctx);
    f.frame += 1;
    if f.fault == 3 && f.cap_delivered == 1 {
        f.frame += 200;
        f.fault = 0;
    }
    if f.fault == 4 && f.frame >= 140 {
        std::ptr::write_volatile(f.stop, 1);
    }
    for dir in &mut f.transfers {
        for (i, transfer) in dir.iter_mut().enumerate() {
            if let Some(t) = transfer {
                // Delay each even callback until its odd successor arrived.
                let delay = if f.fault == 8 && i % 2 == 0 { 10 } else { 0 };
                if f.frame >= t.start + 8 + delay {
                    t.done = true;
                }
            }
        }
    }
}
unsafe extern "C" fn abort(ctx: *mut c_void, d: u32) -> i32 {
    for t in fake(ctx).transfers[d as usize].iter_mut().flatten() {
        if !t.reported {
            t.aborted = true;
            t.done = true;
        }
    }
    0
}
unsafe extern "C" fn cleanup(ctx: *mut c_void) -> i32 {
    let f = fake(ctx);
    assert!(f.transfers.iter().flatten().flatten().all(|t| t.reported));
    f.cleaned = true;
    0
}
unsafe extern "C" fn info(ctx: *mut c_void, out: *mut Info) {
    let f = fake(ctx);
    *out = Info {
        timebase_numer: 125,
        timebase_denom: 3,
        callback_fault: u32::from(f.fault == 7 && f.cap_delivered > 0),
        restored_mask: if f.cleaned { 3 } else { 0 },
        open_mask: if f.cleaned { 0 } else { 3 },
        ..Info::default()
    };
}
const OPS: Ops = Ops {
    config,
    rates,
    prepare,
    activate,
    clock,
    buffer,
    submit,
    completion,
    pump,
    abort,
    cleanup,
    info,
};
fn simulate(fault: u8, tone: bool) -> (Session, Fake) {
    simulate_count(16, fault, tone)
}
fn simulate_count(count: usize, fault: u8, tone: bool) -> (Session, Fake) {
    let mut stop = 0;
    let mut f = Fake::new(count, fault, &mut stop);
    let mut s = Session::new(count);
    unsafe {
        let ctx = (&mut f as *mut Fake).cast();
        s.run(&OPS, ctx, (count * 8) as u32, tone, &stop);
        s.drain(&OPS, ctx);
    }
    assert_eq!(s.pending(), 0);
    assert!(!s.drain_failed);
    assert!(f.max_pending.into_iter().all(|n| n <= DEPTH));
    (s, f)
}
#[test]
fn real_session_loop_conserves_feedback_and_sends_zero() {
    let (s, f) = simulate(0, false);
    assert_eq!(s.reason, None);
    assert_eq!(s.queues[1].completed, 16);
    for i in 0..16 {
        let input = s.records[0][i].completion.unwrap();
        let output = s.records[1][i].completion.unwrap();
        assert_eq!(
            s.records[1][i].start.unwrap() - s.records[0][i].start.unwrap(),
            FEEDBACK_DELAY_MS
        );
        for j in 0..ENTRIES {
            assert_eq!(input.packets[j].actual, output.packets[j].requested);
        }
        assert!(f.data[1][i].iter().all(|&b| b == 0));
    }
}
#[test]
fn output_submit_failure_has_no_callback_debt() {
    let (s, _) = simulate(1, false);
    assert_eq!(s.reason, Some("submit_failed"));
    assert_eq!(s.queues[1].submitted, 1);
    assert_eq!(s.queues[1].completed, 1);
}
#[test]
fn failed_capture_is_not_reused_for_output() {
    let (s, _) = simulate(2, false);
    assert_eq!(s.reason, Some("request_completion_error"));
    assert!(s.queues[1].submitted <= 2);
}
#[test]
fn delayed_dispatch_stops_stale_schedule() {
    let (s, _) = simulate(3, false);
    assert_eq!(s.reason, Some("stale_schedule"));
}
#[test]
fn signal_stops_both_directions_and_drains() {
    let (s, _) = simulate(4, false);
    assert_eq!(s.reason, Some("signal"));
    for q in s.queues {
        assert_eq!(q.submitted, q.completed);
        assert!(q.stopped);
    }
}

#[test]
fn full_tone_session_has_bounded_packed_left_then_right() {
    let (s, f) = simulate_count(256, 0, true);
    assert_eq!(s.reason, None);
    let mut cursor = 0u64;
    let mut heard = [0usize; 8];
    for i in 0..256 {
        let frames = s.records[1][i]
            .lengths
            .iter()
            .map(|&n| n as usize / 24)
            .sum::<usize>();
        for frame in 0..frames {
            for (ch, heard_count) in heard.iter_mut().enumerate() {
                let at = frame * 24 + ch * 3;
                let sample = ftu_protocol::decode_s24(f.data[1][i][at..at + 3].try_into().unwrap());
                assert!(sample.abs() <= 83886);
                if ch >= 2 {
                    assert_eq!(sample, 0);
                }
                if sample != 0 {
                    *heard_count += 1;
                    assert!(if ch == 0 {
                        (4800..33600).contains(&cursor)
                    } else {
                        (43200..72000).contains(&cursor)
                    });
                }
            }
            cursor += 1;
        }
    }
    assert!(heard[0] > 20000 && heard[1] > 20000);
    assert_eq!(cursor, s.next_sample);
}

#[test]
fn missing_capture_feedback_stops_before_output() {
    let (s, _) = simulate(5, false);
    assert_eq!(s.reason, Some("packet_completion_error"));
    assert_eq!(s.queues[1].submitted, 0);
}

#[test]
fn mismatched_callback_stops_before_reading_payload() {
    let (s, _) = simulate(6, false);
    assert_eq!(s.reason, Some("request_completion_error"));
    assert_eq!(s.queues[1].submitted, 0);
}

#[test]
fn native_callback_fault_stops_both_queues() {
    let (s, _) = simulate(7, false);
    assert_eq!(s.reason, Some("native_callback_fault"));
    assert_eq!(s.queues[1].submitted, 0);
}

#[test]
fn out_of_order_callbacks_preserve_feedback_block_identity() {
    let (s, f) = simulate(8, false);
    assert_eq!(s.reason, None);
    for order in &f.completion_order {
        assert_eq!(&order[..2], &[1, 0]);
    }
    for i in 0..16 {
        let input = s.records[0][i].completion.unwrap();
        let output = s.records[1][i].completion.unwrap();
        for j in 0..ENTRIES {
            assert_eq!(input.packets[j].actual, output.packets[j].requested);
        }
    }
}

#[test]
fn full_ffi_path_writes_independently_auditable_evidence() {
    let path = std::env::temp_dir().join(format!(
        "ftu-duplex-ffi-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let output = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let mut stop = 0;
    let mut f = Fake::new(16, 8, &mut stop);
    let code = unsafe {
        ftu_duplex_run(
            &OPS,
            (&mut f as *mut Fake).cast(),
            output.as_ptr(),
            128,
            0,
            &stop,
        )
    };
    assert_eq!(code, 0);
    assert!(f.cleaned);
    let analyzer =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/ftu-probe/analyze_duplex.py");
    let audit = std::process::Command::new("python3")
        .arg("-B")
        .arg(analyzer)
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        audit.status.success(),
        "{} {}",
        String::from_utf8_lossy(&audit.stdout),
        String::from_utf8_lossy(&audit.stderr)
    );
    std::fs::remove_dir_all(path).unwrap();
}
