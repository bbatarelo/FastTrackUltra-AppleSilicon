//! Exercise the real live worker across initializer failures, without hardware.
use super::*;
use crate::{
    live::*,
    startup::{Snapshot, Startup},
};
use std::sync::Mutex;
#[repr(C)]
struct Context {
    // Existing pool callbacks operate on this first field.
    pool: Fake,
    case: u32,
    events: Vec<&'static str>,
    writes: u32,
    verified: bool,
}
static CLOSED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
unsafe fn context<'a>(p: *mut c_void) -> &'a mut Context {
    &mut *p.cast::<Context>()
}
unsafe extern "C" fn open(_: u32, case: u32, _: u64) -> *mut c_void {
    Box::into_raw(Box::new(Context {
        pool: Fake::new(0, std::ptr::null_mut(), 16),
        case,
        events: vec![],
        writes: 0,
        verified: false,
    }))
    .cast()
}
unsafe extern "C" fn get_rates(p: *mut c_void, out: *mut Rates) {
    rates(p, out);
    let c = context(p);
    if c.pool.cleaned {
        c.events.push("post-get");
        return;
    }
    if c.writes == 0 {
        c.events.push("before-get");
        if c.case == 1 {
            (*out).hz[0] = 44100;
        }
    } else {
        c.events.push("verify-get");
        c.verified = true;
        match c.case {
            8 | 9 => (*out).hz[(c.case - 8) as usize] = 96000,
            10 | 11 => (*out).status[(c.case - 10) as usize] = -55,
            12 | 13 => (*out).actual[(c.case - 12) as usize] = 2,
            _ => {}
        }
    }
}
unsafe extern "C" fn prepare_start(p: *mut c_void, n: u32) -> i32 {
    context(p).events.push("prepare");
    if context(p).case == 2 {
        -22
    } else {
        prepare(p, n)
    }
}
unsafe extern "C" fn activate_start(p: *mut c_void) -> i32 {
    context(p).events.push("activate");
    if context(p).case == 3 {
        -33
    } else {
        0
    }
}
unsafe extern "C" fn set_rate(p: *mut c_void, d: u32, hz: u32, actual: *mut u32) -> i32 {
    let c = context(p);
    assert_eq!(hz, 48000);
    assert_eq!(d, c.writes); // IN, then OUT; never repeated.
    assert!(c.events.contains(&"activate"));
    assert!(!c.pool.policy_active);
    assert!(c.pool.transfers.iter().flatten().all(Option::is_none));
    c.events.push(if d == 0 { "set-in" } else { "set-out" });
    c.writes += 1;
    if c.case == 14 + d {
        return -66;
    } // Native left actual sentinel untouched.
    *actual = if c.case == 6 + d { 2 } else { 3 };
    if c.case == 4 + d {
        -44
    } else {
        0
    }
}
unsafe extern "C" fn policy(p: *mut c_void, s: *const ThreadSchedule) -> i32 {
    let c = context(p);
    if s.is_null() {
        c.events.push("restore-policy");
        return 0;
    }
    assert!(c.verified && c.writes == 2);
    c.events.push("policy");
    // Halt after successful initialization; existing live tests cover actual IO.
    -1
}
unsafe extern "C" fn teardown(p: *mut c_void) -> i32 {
    context(p).events.push("cleanup");
    cleanup(p)
}
unsafe extern "C" fn close(p: *mut c_void) -> i32 {
    let mut c = Box::from_raw(p.cast::<Context>());
    assert!(c.pool.cleaned && !c.pool.policy_active);
    assert!(c.pool.transfers.iter().flatten().all(Option::is_none));
    assert_eq!(c.pool.pump_calls, 0);
    c.events.push("close");
    *CLOSED.lock().unwrap() = c.events.clone();
    0
}
unsafe extern "C" fn prepare_start_geometry(
    p: *mut c_void,
    g: *const crate::transport::Transport,
) -> i32 {
    context(p).events.push("prepare");
    if context(p).case == 2 {
        -22
    } else {
        prepare_geometry(p, g)
    }
}
const START_OPS: PoolOps = PoolOps {
    prepare_geometry: prepare_start_geometry,
    base: Ops {
        rates: get_rates,
        prepare: prepare_start,
        activate: activate_start,
        cleanup: teardown,
        ..OPS.base
    },
    ..OPS
};
unsafe extern "C" fn operations(_: *mut c_void) -> *const PoolOps {
    &START_OPS
}
const FACTORY: Factory = Factory {
    open,
    ops: operations,
    close,
    configure_thread: policy,
    set_rate,
};

#[test]
fn initializer_failure_never_admits_audio_and_retains_report_after_cleanup() {
    // Case zero reaches the native attach after successful rate verification.
    // This fixture is a legacy Fake, not a native USB context. Independent
    // route attach/dispatch is covered by the linked native fixture instead.
    let first = u32::from(cfg!(feature = "independent-4ms"));
    for case in first..16 {
        unsafe {
            let h = ftu_live_create(&FACTORY, 0x2080, case);
            assert_ne!(ftu_live_start(h, 1), 0, "case {case}");
            assert_eq!(ftu_live_stop(h, 1), 0);
            let mut status = Status::default();
            ftu_live_status(h, &mut status);
            assert_eq!(status.phase, 4);
            assert_eq!(status.submitted, [0; 2]);
            assert_eq!(status.pending, [0; 2]);
            assert_eq!(status.cleanup_status, 0);
            let mut s = Snapshot::new(0);
            ftu_live_startup(h, &mut s);
            assert_eq!(&s.header[..3], &[1, 1, 1]);
            let e = CLOSED.lock().unwrap().clone();
            assert_eq!(&e[e.len() - 3..], &["cleanup", "post-get", "close"]);
            let mut expected = vec!["before-get"];
            if case != 1 {
                expected.push("prepare");
            }
            if case != 1 && case != 2 {
                expected.push("activate");
            }
            if ![1, 2, 3].contains(&case) {
                expected.push("set-in");
                if ![4, 6, 14].contains(&case) {
                    expected.push("set-out");
                    if ![5, 7, 15].contains(&case) {
                        expected.push("verify-get");
                    }
                }
            }
            if case == 0 {
                expected.extend(["policy", "restore-policy"]);
                assert_eq!(s.header[4], 1);
                assert_eq!(status.reason, 4); // Policy failure after successful initializer.
            } else {
                assert_eq!(s.header[4], 2);
                assert_eq!(
                    status.reason,
                    match case {
                        1 => 1,
                        2 => 2,
                        3 => 3,
                        8..=13 => 39,
                        _ => 38,
                    }
                );
                assert_eq!(
                    status.fault,
                    match case {
                        1 => 3,
                        2 => 4,
                        3 => 5,
                        _ => 14,
                    }
                );
            }
            expected.extend(["cleanup", "post-get", "close"]);
            assert_eq!(e, expected, "case {case}");
            if case >= 4 && !(8..=13).contains(&case) {
                let d = ((case - 4) % 2) as usize;
                assert_eq!(&s.writes[d][..2], &[1, 1]);
                assert_eq!(
                    s.writes[d][3],
                    if case >= 14 {
                        u32::MAX as u64
                    } else if case >= 6 {
                        2
                    } else {
                        3
                    }
                );
                if [4, 5, 14, 15].contains(&case) {
                    assert_eq!(s.writes[d][2] as i64, if case >= 14 { -66 } else { -44 });
                }
                if d == 0 {
                    assert_eq!(s.writes[1], [0; 4]);
                }
                assert_eq!(s.header[9], 0);
            }
            assert_eq!(ftu_live_destroy(h), 0);
        }
    }
}

#[test]
fn cancellation_between_control_calls_prevents_remaining_calls() {
    // Check cancellation before either SET, before GET, and after GET.
    for cancel_at in 0..4 {
        unsafe {
            let p = open(0x2080, 0, 0);
            context(p).events.push("activate");
            let report = Startup::new();
            let mut s = Snapshot::new(7);
            let calls = Cell::new(0);
            let result =
                crate::startup::initialize(p, &START_OPS, FACTORY, &report, &mut s, || {
                    let n = calls.get();
                    calls.set(n + 1);
                    n == cancel_at
                });
            assert_eq!(result, Err("signal"));
            let observed = report.snapshot();
            assert_eq!(observed.header[4], 3);
            assert_eq!(context(p).writes, cancel_at.min(2));
            assert_eq!(context(p).verified, cancel_at == 3);
            assert_eq!(observed.header[9], u64::from(cancel_at == 3));
            let c = Box::from_raw(p.cast::<Context>());
            assert!(!c.pool.policy_active);
            assert!(c.pool.transfers.iter().flatten().all(Option::is_none));
        }
    }
}
