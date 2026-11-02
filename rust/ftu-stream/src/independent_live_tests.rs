//! The production Owner/Completer, linked to real C routing and fake USB.
use super::*;
unsafe extern "C" {
    fn ftu_fixture_create(slots: u32, hooks: *const Hooks) -> *mut c_void;
    fn ftu_fixture_buffer(f: *mut c_void, d: u32, s: u32) -> *mut u8;
    fn ftu_fixture_submit(
        f: *mut c_void,
        d: u32,
        s: u32,
        t: u64,
        start: u64,
        l: *const u16,
        token: *mut u64,
    ) -> i32;
    fn ftu_fixture_command(f: *mut c_void, now: u64, kind: u32, d: u32, s: u32) -> i32;
    fn ftu_fixture_ack(f: *mut c_void, d: u32, s: u32, token: u64) -> i32;
    fn ftu_fixture_stop(f: *mut c_void, timeout: u32) -> i32;
    fn ftu_fixture_destroy(f: *mut c_void) -> i32;
    fn ftu_fixture_stat(f: *mut c_void, key: u32) -> u64;
    fn ftu_fixture_snapshot(f: *mut c_void, out: *mut u64);
    fn ftu_fixture_fault(f: *mut c_void, fault: u32);
    fn ftu_fixture_clock(f: *mut c_void, ppm: i32);
}
unsafe extern "C" fn ack(f: *mut c_void, d: u32, s: u32, t: u64) -> bool {
    ftu_fixture_ack(f, d, s, t) == 1
}
unsafe extern "C" fn stop(f: *mut c_void, ms: u32) -> bool {
    ftu_fixture_stop(f, ms) == 1
}
unsafe extern "C" fn destroy(f: *mut c_void) -> bool {
    ftu_fixture_destroy(f) == 1
}
unsafe extern "C" fn fault(f: *mut c_void) -> u32 {
    ftu_fixture_stat(f, 6) as u32
}
unsafe extern "C" fn no_cleanup_snapshot(_: *mut c_void, out: *mut u64) {
    std::ptr::write_bytes(out, 0, 16);
}
const FAKE: Api = Api {
    buffer: ftu_fixture_buffer,
    submit: ftu_fixture_submit,
    release: ftu_route_release,
    ack,
    stop,
    destroy,
    fault,
    snapshot: ftu_fixture_snapshot,
    cleanup_snapshot: no_cleanup_snapshot,
};
unsafe extern "C" fn unused_open(_: u32, _: u32, _: u64) -> *mut c_void {
    std::ptr::null_mut()
}
unsafe extern "C" fn unused_ops(_: *mut c_void) -> *const PoolOps {
    std::ptr::null()
}
unsafe extern "C" fn unused_close(_: *mut c_void) -> i32 {
    -1
}
unsafe extern "C" fn unused_schedule(_: *mut c_void, _: *const ThreadSchedule) -> i32 {
    -1
}
unsafe extern "C" fn unused_rate(_: *mut c_void, _: u32, _: u32, _: *mut u32) -> i32 {
    -1
}
fn fixture() -> (Owner, Live) {
    let shared = Arc::new(Shared::new());
    shared.reset(1);
    let info = Info {
        timebase_numer: 125,
        timebase_denom: 3,
        ..Info::default()
    };
    let o = Owner::new(shared.clone(), 1, info, FAKE, |hooks| unsafe {
        ftu_fixture_create(12, hooks)
    })
    .unwrap();
    let live = Live::new(
        shared,
        Factory {
            open: unused_open,
            ops: unused_ops,
            close: unused_close,
            configure_thread: unused_schedule,
            set_rate: unused_rate,
        },
        0x2080,
        123,
    );
    (o, live)
}
fn step(o: &mut Owner, ms: u64) -> Result<bool, &'static str> {
    crate::pool_simulation::without_allocations(|| {
        o.step(|| {
            Ok(Clock {
                frame: ms,
                ticks: ms * 24000,
            })
        })
    })
}
fn dispatch(o: &Owner, ms: u64) -> i32 {
    for d in 0..2 {
        for s in 0..DEPTH {
            if let Some(f) = o.flights[d][s] {
                let start = o.base + (f.ticket.generation - 1) * 4 + if d == 1 { 64 } else { 0 };
                if start + 4 <= ms && o.link.slots[d][s].state().1 == State::Submitted {
                    let r =
                        unsafe { ftu_fixture_command(o.route, ms * 1000, 1, d as u32, s as u32) };
                    if r != 0 {
                        return r;
                    }
                }
            }
        }
    }
    unsafe { ftu_fixture_command(o.route, ms * 1000, 0, 0, 0) }
}
fn stream(buffer: usize, stall: bool, ppm: i32) {
    let (mut o, mut live) = fixture();
    unsafe { ftu_fixture_clock(o.route, ppm) };
    let mut next = 0;
    let mut active = false;
    let mut steady_in = 0;
    let mut steady_out = 0;
    let mut write = vec![0.03125f32; buffer * 8];
    let mut read = vec![0f32; buffer * 8];
    for ms in 100u64..3100 {
        let now = (ms.saturating_sub(116)) * 48;
        if live.shared.phase.load(SeqCst) == RUNNING {
            if !active {
                next = now;
                active = true;
            }
            while next <= now {
                let out = next + u64::from(OUTPUT_SAFETY) + buffer as u64;
                for (i, v) in write.iter_mut().enumerate() {
                    *v = ((out + i as u64 / 8) % 10000 + 1) as f32 / 8_388_608.;
                }
                assert_eq!(
                    unsafe { ftu_live_write(&mut live, out, buffer as u32, write.as_ptr()) },
                    0
                );
                if next >= u64::from(INPUT_SAFETY) + buffer as u64 {
                    let before = live.shared.capture_missing.load(SeqCst);
                    assert_eq!(
                        unsafe {
                            ftu_live_read(
                                &mut live,
                                next - u64::from(INPUT_SAFETY) - buffer as u64,
                                buffer as u32,
                                read.as_mut_ptr(),
                            )
                        },
                        0
                    );
                    if next >= 48000 {
                        steady_in += live.shared.capture_missing.load(SeqCst) - before;
                        assert!(read.iter().all(|v| *v == 0.25));
                    }
                }
                next += buffer as u64;
            }
        }
        assert_eq!(dispatch(&o, ms), 0);
        if !(stall && (1623..1644).contains(&ms)) {
            let before = live.shared.playback_missing.load(SeqCst);
            for _ in 0..10 {
                if !step(&mut o, ms).unwrap() {
                    break;
                }
            }
            if now >= 48000 {
                steady_out += live.shared.playback_missing.load(SeqCst) - before;
            }
        }
    }
    let timing = live.shared.timing.snapshot();
    let lead = timing.metrics[1][Metric::SubmitReturnLeadMs as usize];
    assert!(lead.count > 0 && lead.min >= 16);
    assert!(active);
    assert_eq!((steady_in, steady_out), (0, 0));
    assert!(o.min_lead >= 16);
    let fit = live.shared.clock.read().unwrap();
    assert_eq!(fit.epoch, 1);
    assert!(fit.frame > 100000);
    let completed = o.link.completed[0].load(SeqCst);
    assert!(completed > 700);
    assert_eq!(live.shared.reason.load(SeqCst), 0);
    assert!(o.stop());
    assert_eq!(live.shared.pending[0].load(SeqCst), 0);
    assert!(live.shared.clock.read().is_none());
    println!("buffer={buffer} stall={stall} ppm={ppm} callbacks={completed} min_out_lead={} steady_in={steady_in} steady_out={steady_out}",o.min_lead);
}
#[test]
fn production_streams_at_64_and_128_with_independent_capture() {
    for b in [64, 128] {
        for stall in [false, true] {
            stream(b, stall, 0);
        }
    }
}
#[test]
fn production_clock_uses_native_timestamps() {
    for ppm in [-250, 250] {
        stream(128, false, ppm);
    }
}
#[test]
fn synchronous_rejections_retire_both_ownership_layers() {
    for code in [2, 7] {
        let (mut o, _) = fixture();
        unsafe { ftu_fixture_fault(o.route, code) };
        assert_eq!(step(&mut o, 100), Err("submit_failed"));
        assert!(o.link.slots[0].iter().all(|s| s.state().1 == State::Free));
        assert!(o.stop());
    }
}
#[test]
fn callback_before_submit_return_waits_for_owner_ack() {
    let (mut o, _) = fixture();
    unsafe { ftu_fixture_fault(o.route, 5) };
    assert_eq!(step(&mut o, 100), Ok(true));
    assert_eq!(o.link.slots[0][0].state().1, State::Released);
    assert!(o.flights[0][0].is_some());
    o.receive().unwrap();
    assert_eq!(o.link.slots[0][0].state().1, State::Free);
    assert!(o.stop());
}
#[test]
fn removal_and_malformed_packets_stop_with_known_ownership() {
    for code in [3, 6] {
        let (mut o, _) = fixture();
        assert!(step(&mut o, 100).unwrap());
        unsafe { ftu_fixture_fault(o.route, code) };
        assert_eq!(dispatch(&o, 120), 0);
        assert!(o.link.error.load(Acquire) != 0);
        assert_eq!(step(&mut o, 120), Err("native_completion_error"));
        assert!(o.stop());
    }
}
#[test]
fn callback_identity_fault_retains_native_and_rust() {
    let (mut o, _) = fixture();
    assert!(step(&mut o, 100).unwrap());
    unsafe { ftu_fixture_fault(o.route, 4) };
    assert_ne!(dispatch(&o, 120), 0);
    assert!(!o.stop());
    assert!(!o.ctx.is_null());
    assert!(!o.route.is_null());
}
#[test]
fn missing_abort_retains_until_late_completion() {
    let (mut o, _) = fixture();
    assert!(step(&mut o, 100).unwrap());
    unsafe { ftu_fixture_fault(o.route, 1) };
    assert!(!o.stop());
    assert!(!o.ctx.is_null());
    unsafe { ftu_fixture_fault(o.route, 0) };
    assert_eq!(dispatch(&o, 120), 0);
    assert!(o.stop());
}
#[test]
fn post_submit_clock_failure_keeps_admitted_ownership() {
    let (mut o, _) = fixture();
    let mut calls = 0;
    let r = o.step(|| {
        calls += 1;
        if calls == 4 {
            Err("clock_query_failed")
        } else {
            Ok(Clock {
                frame: 100,
                ticks: 2400000,
            })
        }
    });
    assert_eq!(r, Err("clock_query_failed"));
    assert!(o.flights[0][0].is_some());
    assert!(o.stop());
}
#[test]
fn elapsed_submission_is_rejected() {
    let (mut o, _) = fixture();
    let mut calls = 0;
    let r = o.step(|| {
        calls += 1;
        Ok(Clock {
            frame: if calls == 4 { 117 } else { 100 },
            ticks: 2400000,
        })
    });
    assert_eq!(r, Err("stale_schedule"));
    assert!(o.stop());
}
#[test]
fn stop_after_clock_start_does_not_publish_again() {
    let (mut o, live) = fixture();
    for ms in 100..300 {
        assert_eq!(dispatch(&o, ms), 0);
        for _ in 0..10 {
            if !step(&mut o, ms).unwrap() {
                break;
            }
        }
    }
    assert_eq!(live.shared.phase.load(SeqCst), RUNNING);
    live.shared.stop.store(true, SeqCst);
    assert!(o.stop());
    assert!(live.shared.clock.read().is_none());
}
#[test]
fn normal_stop_drains_both_directions() {
    let (mut o, live) = fixture();
    for ms in 100..400 {
        assert_eq!(dispatch(&o, ms), 0);
        for _ in 0..10 {
            if !step(&mut o, ms).unwrap() {
                break;
            }
        }
    }
    assert!(live.shared.captured.load(SeqCst) > 10000);
    assert!(o.stop());
}
#[test]
fn lost_completions_hit_watchdog_and_drain() {
    let (mut o, _) = fixture();
    for _ in 0..12 {
        assert!(step(&mut o, 100).unwrap());
    }
    assert_eq!(step(&mut o, 181), Err("completion_deadline"));
    assert!(o.stop());
}
#[test]
fn completion_panic_is_contained_and_context_retained() {
    let (mut o, _) = fixture();
    assert!(step(&mut o, 100).unwrap());
    unsafe { ftu_fixture_fault(o.route, 8) };
    assert_eq!(dispatch(&o, 120), 0);
    assert!(o.link.fault.load(Acquire));
    assert_eq!(step(&mut o, 120), Err("native_callback_fault"));
    assert!(!o.stop());
    assert!(!o.ctx.is_null());
}
// ---- Runtime-fault record (ftuE): the actual Owner guards, one stage each.

/// Step with a scripted clock: the k-th clock() call in the step reads
/// `frames[k]` (the last value repeats). Calls: now, before packing, after
/// packing, after native submit returned.
fn scripted(o: &mut Owner, frames: &[u64]) -> Result<bool, &'static str> {
    let mut calls = 0;
    crate::pool_simulation::without_allocations(|| {
        o.step(|| {
            let f = frames[calls.min(frames.len() - 1)];
            calls += 1;
            Ok(Clock {
                frame: f,
                ticks: f * 24000,
            })
        })
    })
}
/// Base 116: IN 0..12 submitted (IN 0 completed at 120, IN 12 starts 164),
/// every IN slot busy, OUT 0 (start 180) has feedback but no PCM; deferred.
fn out_due() -> (Owner, Live) {
    let (mut o, live) = fixture();
    for _ in 0..12 {
        assert!(step(&mut o, 100).unwrap());
    }
    assert!(!step(&mut o, 100).unwrap());
    assert_eq!(dispatch(&o, 120), 0);
    assert!(step(&mut o, 120).unwrap());
    assert!(!step(&mut o, 120).unwrap());
    assert_eq!(o.next, [13, 0]);
    (o, live)
}
fn words(o: &Owner, reason: u32) -> rf::Snapshot {
    o.runtime_fault(reason, 77)
}
#[test]
fn out_guards_record_their_exact_stage_and_clock() {
    use rf::*;
    // (clock script, stage, slot known, return clock known)
    let cases: [(&[u64], u64, bool, bool); 4] = [
        (&[170], STAGE_READINESS, false, false),
        (&[160, 170], STAGE_BEFORE_PACKING, true, false),
        (&[160, 160, 170], STAGE_AFTER_PACKING, true, false),
        (&[160, 160, 160, 170], STAGE_AFTER_RETURN, true, true),
    ];
    for (frames, stage, slot, returned) in cases {
        let (mut o, _live) = out_due();
        assert_eq!(scripted(&mut o, frames), Err("output_lead_deadline"));
        let r = words(&o, 55);
        let w = &r.words;
        assert_eq!(
            (w[EPOCH], w[REASON], w[STAGE]),
            (1, 55, stage),
            "stage {stage}"
        );
        assert_eq!((w[DIRECTION], w[SEQUENCE], w[REQUIRED_LEAD]), (1, 0, 16));
        assert_eq!(
            (w[SCHEDULED], w[CHECKED_FRAME], w[CHECKED_TICKS]),
            (
                180,
                if returned { 160 } else { 170 },
                if returned { 160 } else { 170 } * 24000
            )
        );
        assert!(r.flag(F_DIRECTION | F_SCHEDULED | F_CHECKED | F_BASE | F_TIMEBASE));
        assert_eq!(r.flag(F_SLOT), slot);
        assert_eq!(r.flag(F_RETURNED), returned);
        if returned {
            assert_eq!((w[RETURNED_FRAME], w[RETURNED_TICKS]), (170, 170 * 24000));
            // The late submission was admitted and is pending; its own lead is
            // the minimum observed return lead, measured, not assumed.
            assert!(r.flag(F_MIN_OUT_LEAD) && w[MIN_OUT_LEAD] == 10);
            assert_eq!((w[NEXT + 1], w[PENDING + 1]), (1, 1));
        } else {
            assert_eq!((w[RETURNED_FRAME], w[RETURNED_TICKS]), (0, 0));
            assert!(!r.flag(F_MIN_OUT_LEAD));
            assert_eq!((w[NEXT + 1], w[PENDING + 1]), (0, 0));
        }
        assert!(!r.flag(F_SUBMIT_STATUS));
        // Prior owner progress is the last passing step/submission, at 120.
        assert!(r.flag(F_PRIOR_STEP | F_PRIOR_PROGRESS));
        assert_eq!(
            (w[PRIOR_STEP_FRAME], w[PRIOR_STEP_TICKS]),
            (120, 120 * 24000)
        );
        assert_eq!(
            (w[PRIOR_PROGRESS_FRAME], w[PRIOR_PROGRESS_TICKS]),
            (120, 120 * 24000)
        );
        assert_eq!((w[BASE], w[STEPS], w[NEXT], w[PENDING]), (116, 16, 13, 12));
        assert_eq!((w[COMPLETED], w[COMPLETED + 1]), (1, 0));
        assert_eq!(
            (w[TIMEBASE_NUMER], w[TIMEBASE_DENOM], w[UPTIME_MS]),
            (125, 3, 77)
        );
        assert_eq!((w[QUEUE_DEPTH], w[BLOCK_MS], w[REPLAY_MS]), (12, 4, 64));
        assert_eq!((w[FINISHED], w[PRESENT]), (0, 1));
        assert!(o.stop(), "the refused step leaves ownership drainable");
    }
}
#[test]
fn in_stale_watchdog_and_generic_stops_are_distinguished() {
    use rf::*;
    for (frames, stage, required, returned) in [
        (&[100u64, 115][..], STAGE_BEFORE_PACKING, 2, false),
        (&[100, 100, 115][..], STAGE_AFTER_PACKING, 2, false),
        (&[100, 100, 100, 116][..], STAGE_AFTER_RETURN, 1, true),
    ] {
        let (mut o, _live) = fixture();
        assert_eq!(scripted(&mut o, frames), Err("stale_schedule"));
        let r = words(&o, 20);
        let w = &r.words;
        assert_eq!(
            (w[STAGE], w[DIRECTION], w[SEQUENCE], w[SLOT]),
            (stage, 0, 0, 0)
        );
        assert_eq!((w[SCHEDULED], w[REQUIRED_LEAD]), (116, required));
        assert_eq!(r.flag(F_RETURNED), returned);
        // No step had passed yet: absent, not zero.
        assert!(!r.flag(F_PRIOR_STEP) && !r.flag(F_PRIOR_PROGRESS));
        assert!(o.stop());
    }
    // Completion watchdog over a lost IN request.
    let (mut o, _live) = fixture();
    for _ in 0..12 {
        assert!(step(&mut o, 100).unwrap());
    }
    assert_eq!(step(&mut o, 181), Err("completion_deadline"));
    let r = words(&o, 21);
    let w = &r.words;
    assert_eq!(
        (w[STAGE], w[DIRECTION], w[SEQUENCE], w[SLOT]),
        (STAGE_WATCHDOG, 0, 0, 0)
    );
    assert_eq!(
        (w[SCHEDULED], w[CHECKED_FRAME], w[REQUIRED_LEAD]),
        (116, 181, 64)
    );
    assert_eq!((w[PENDING], w[PRIOR_STEP_FRAME]), (12, 100));
    assert!(o.stop());
    // Clock failure: no precise check clock exists; nothing is invented.
    let (mut o, _live) = fixture();
    assert_eq!(
        o.step(|| Err("clock_query_failed")),
        Err("clock_query_failed")
    );
    let r = words(&o, 29);
    assert_eq!(
        (r.words[STAGE], r.words[FLAGS]),
        (STAGE_GENERIC, F_TIMEBASE)
    );
    assert_eq!((r.words[CHECKED_FRAME], r.words[SCHEDULED]), (0, 0));
    assert!(o.stop());
    // Submitted, but the return clock failed: generic with the admission data.
    let (mut o, _live) = fixture();
    assert_eq!(scripted_failing_return(&mut o), Err("clock_query_failed"));
    let r = words(&o, 29);
    assert_eq!(r.words[STAGE], STAGE_GENERIC);
    assert!(r.flag(F_DIRECTION | F_SCHEDULED | F_CHECKED) && !r.flag(F_RETURNED));
    assert!(o.stop());
    // Native submit error.
    let (mut o, _live) = fixture();
    unsafe { ftu_fixture_fault(o.route, 2) };
    assert_eq!(step(&mut o, 100), Err("submit_failed"));
    let r = words(&o, 26);
    assert_eq!(r.words[STAGE], STAGE_SUBMIT);
    assert!(r.flag(F_SUBMIT_STATUS | F_RETURNED) && r.words[SUBMIT_STATUS] != 0);
    assert!(o.stop());
    // Callback fault surfaces through receive: generic.
    let (mut o, _live) = fixture();
    assert!(step(&mut o, 100).unwrap());
    unsafe { ftu_fixture_fault(o.route, 3) };
    assert_eq!(dispatch(&o, 120), 0);
    assert_eq!(step(&mut o, 120), Err("native_completion_error"));
    assert_eq!(words(&o, 22).words[STAGE], STAGE_GENERIC);
    assert!(o.stop());
}
fn scripted_failing_return(o: &mut Owner) -> Result<bool, &'static str> {
    let mut calls = 0;
    o.step(|| {
        calls += 1;
        if calls == 4 {
            Err("clock_query_failed")
        } else {
            Ok(Clock {
                frame: 100,
                ticks: 2400000,
            })
        }
    })
}
#[test]
fn record_survives_reset_and_clean_teardown_and_is_not_the_first_failure() {
    let (mut o, live) = out_due();
    assert_eq!(
        scripted(&mut o, &[160, 160, 170]),
        Err("output_lead_deadline")
    );
    let shared = &live.shared;
    shared
        .runtime
        .publish(&o.runtime_fault(55, shared.uptime_ms()));
    let primary = shared.runtime.snapshot();
    assert!(o.stop());
    assert!(shared.runtime.finish(
        1,
        rf::Terminal {
            drain_failed: false,
            cleanup: 0,
            close: Some(0),
            phase: FAULT,
            fault: 6
        }
    ));
    let finished = shared.runtime.snapshot();
    // Finishing adds outcomes (and the close flag); it never rewrites the primary.
    let mut expected = primary.words;
    expected[rf::FLAGS] |= rf::F_CLOSE;
    assert_eq!(finished.words[..rf::FINISHED], expected[..rf::FINISHED]);
    assert_eq!(finished.words[rf::FINISHED], 1);
    // A later epoch's reset (normal restart or recovery) keeps it.
    shared.reset(2);
    assert_eq!(shared.runtime.snapshot(), finished);
    assert!(!shared.runtime.finish(
        2,
        rf::Terminal {
            drain_failed: true,
            cleanup: -1,
            close: None,
            phase: QUARANTINED,
            fault: 7
        }
    ));
    assert_eq!(shared.runtime.snapshot(), finished);
    assert_eq!(
        shared.first_failure.snapshot().header[2],
        0,
        "separate latch"
    );
    let mut out = rf::Snapshot::default();
    unsafe { ftu_live_runtime_fault(&live as *const Live as *mut Live, &mut out) };
    assert_eq!(out, finished);
}

mod recovery {
    use super::*;
    include!("independent_recovery_tests.rs");
}

#[test]
fn completion_error_keeps_packet_evidence() {
    let (mut o, _) = fixture();
    assert!(step(&mut o, 100).unwrap());
    unsafe { ftu_fixture_fault(o.route, 3) };
    assert_eq!(dispatch(&o, 120), 0);
    let failure = o.failures.pop().unwrap();
    assert_eq!(failure.reason, 22);
    assert_eq!(failure.admission.start, 116);
    assert_eq!(failure.c.packets[0].actual, 313);
    assert!(o.stop());
}
