// Production control + Rust worker + real C completion router; only device
// responses/clock are fake. No new Start or Stop releases the retained clients.
#[test]
fn clean_failed_restart_uses_remaining_budget_and_resumes_retained_audio() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        let hal = Hal::start(h, 128);
        assert_eq!(ftu_live_start(h, 7), 0);
        assert_eq!(ftu_live_start(h, 8), 0);
        let first = inject(h, &JUMP_OUT).epoch;
        STARTUP_STALLS.store(1, SeqCst);
        assert_eq!(
            monitor(h).0,
            -5,
            "the failed restart is not reported as success"
        );
        let s = st(h);
        assert_eq!(
            (s.phase, s.fault, s.reason, s.epoch),
            (FAULT, 6, 55, first + 1)
        );
        let r = record(h);
        assert_eq!(
            (r.words[rf::STAGE], r.words[rf::NEXT + 1]),
            (rf::STAGE_READINESS, 0)
        );
        assert_eq!(r.words[rf::SCHEDULED] - r.words[rf::CHECKED_FRAME], 13);
        assert_eq!(
            (
                r.words[rf::DRAIN_FAILED],
                r.words[rf::CLEANUP],
                r.words[rf::CLOSE]
            ),
            (0, 0, 0)
        );
        assert_eq!(OPEN_CONTEXTS.load(SeqCst), 0);
        let v = view(h);
        assert_eq!(
            (v[rp::STATE], v[rp::CLIENTS]),
            (rp::PENDING, 2),
            "clean failed launch must not settle permanently"
        );
        assert_eq!(
            (
                v[rp::ATTEMPTS],
                v[rp::FAILED],
                v[rp::RECOVERED_COUNT],
                v[rp::REFUSED_COUNT]
            ),
            (1, 1, 0, 0)
        );
        assert_eq!(
            (v[rp::FAULT_EPOCH], v[rp::LAST_STATUS] as i64),
            (first + 1, -5)
        );
        assert_eq!(v[rp::DUE] - v[rp::PENDING_SINCE], 1000);
        assert_eq!(
            ftu_live_recover(h, ftu_live_binding(h)),
            2,
            "production backoff"
        );
        assert_eq!(
            ftu_live_start(h, 7),
            -2,
            "retained Start cannot bypass backoff"
        );
        assert_eq!(ftu_live_start(h, 99), -2, "new Start cannot bypass backoff");
        assert_eq!(st(h).epoch, first + 1);
        assert_eq!(monitor(h).0, 0);
        assert_eq!((st(h).phase, st(h).epoch), (RUNNING, first + 2));
        let v = view(h);
        assert_eq!(
            (
                v[rp::ATTEMPTS],
                v[rp::FAILED],
                v[rp::RECOVERED_COUNT],
                v[rp::CLIENTS]
            ),
            (2, 1, 1, 2)
        );
        assert_eq!(v[rp::CONSECUTIVE], 2);
        assert_eq!(record(h), r, "failure remains visible after recovery");
        advance(h, 1600);
        let (oc, om, ic, im) = steady_missing(&*h);
        assert!(oc > 0 && ic > 0);
        assert_eq!((om, im), (0, 0));
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(OPENED_REGISTRY.load(SeqCst), REGISTRY);
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert_eq!(st(h).phase, RUNNING);
        assert_eq!(ftu_live_stop(h, 8), 0);
        let stats = hal.finish();
        assert!(stats.seeds.contains(&first) && stats.seeds.contains(&(first + 2)));
        assert!(
            !stats.seeds.contains(&(first + 1)),
            "failed start publishes no running timeline"
        );
        assert!(stats.writes[0] > 0 && stats.reads[0] > 0);
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

#[test]
fn failed_restart_persistent_deadlines_exhaust_without_extra_opens() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        set_policy(
            h,
            Policy {
                backoff_ms: [0, 40, 80],
                healthy_ms: 3_600_000,
                ..rp::POLICY
            },
        );
        assert_eq!(ftu_live_start(h, 7), 0);
        let first = inject(h, &JUMP_OUT).epoch;
        let opens = OPENS.load(SeqCst);
        STARTUP_STALLS.store(3, SeqCst);
        for attempt in 1..=3 {
            assert_eq!(monitor(h).0, -5);
            assert_eq!(st(h).epoch, first + attempt);
            let v = view(h);
            assert_eq!(
                (v[rp::ATTEMPTS], v[rp::FAILED], v[rp::RECOVERED_COUNT]),
                (attempt, attempt, 0)
            );
            assert_eq!(v[rp::CONSECUTIVE], attempt);
            if attempt < 3 {
                assert_eq!(v[rp::STATE], rp::PENDING);
            }
        }
        let v = view(h);
        assert_eq!(
            (v[rp::STATE], v[rp::EXHAUSTED_COUNT], v[rp::CLIENTS]),
            (rp::EXHAUSTED, 1, 1)
        );
        assert_eq!(v[rp::REFUSED_COUNT], 0);
        for _ in 0..10 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), -11);
            assert_eq!(ftu_live_start(h, 7), -2);
            assert_eq!(ftu_live_start(h, 8), -2);
        }
        assert_eq!(OPENS.load(SeqCst), opens + 3);
        assert_eq!(OPEN_CONTEXTS.load(SeqCst), 0);
        assert!(!OVERLAP.load(SeqCst));
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

#[test]
fn failed_restart_backoff_is_cancelled_by_stop_or_detach() {
    let _h = Harness::new();
    unsafe {
        for detach in [false, true] {
            PRESENT.store(REGISTRY, SeqCst);
            let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
            set_policy(
                h,
                Policy {
                    backoff_ms: [0, 60_000, 60_000],
                    ..rp::POLICY
                },
            );
            assert_eq!(ftu_live_start(h, 7), 0);
            inject(h, &JUMP_OUT);
            STARTUP_STALLS.store(1, SeqCst);
            assert_eq!(monitor(h).0, -5);
            assert_eq!(view(h)[rp::STATE], rp::PENDING);
            let opens = OPENS.load(SeqCst);
            let binding = ftu_live_binding(h);
            assert_eq!(ftu_live_stop(h, 99), 0, "unknown Stop doesn't cancel");
            assert_eq!(view(h)[rp::STATE], rp::PENDING);
            if detach {
                assert_eq!(ftu_live_detach(h), 0);
                PRESENT.store(REGISTRY + 1, SeqCst);
                assert_eq!(ftu_live_rebind(h, 0x2080, LOCATION, REGISTRY + 1), 0);
                assert_eq!(ftu_live_recover(h, binding), -6);
                assert_eq!(ftu_live_start(h, 7), -9);
                assert_eq!(ftu_live_remove_client(h, 7), 0);
            } else {
                assert_eq!(ftu_live_stop(h, 7), 0);
            }
            assert_eq!(view(h)[rp::CANCELLED_COUNT], 1);
            for _ in 0..5 {
                assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 1);
            }
            assert_eq!(OPENS.load(SeqCst), opens);
            assert_eq!(OPEN_CONTEXTS.load(SeqCst), 0);
            assert_eq!(ftu_live_destroy(h), 0);
        }
    }
}

#[test]
fn failed_restart_revalidates_binding_and_admitted_io_before_next_attempt() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        set_policy(
            h,
            Policy {
                backoff_ms: [0; 3],
                ..rp::POLICY
            },
        );
        assert_eq!(ftu_live_start(h, 7), 0);
        inject(h, &JUMP_OUT);
        STARTUP_STALLS.store(1, SeqCst);
        assert_eq!(monitor(h).0, -5);
        assert_eq!(view(h)[rp::STATE], rp::PENDING);
        let opens = OPENS.load(SeqCst);
        (&*h).shared.calls.fetch_add(1, SeqCst);
        for _ in 0..5 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), 4);
        }
        assert_eq!(OPENS.load(SeqCst), opens);
        (&*h).shared.calls.fetch_sub(1, SeqCst);
        assert_eq!(ftu_live_recover(h, ftu_live_binding(h) + 1), -6);
        assert_eq!(OPENS.load(SeqCst), opens);
        PRESENT.store(REGISTRY + 7, SeqCst);
        assert_eq!(
            monitor(h).0,
            -5,
            "pinned open cannot adopt replacement registry"
        );
        assert_eq!(OPENED_REGISTRY.load(SeqCst), REGISTRY);
        let opens = OPENS.load(SeqCst);
        for _ in 0..5 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), -10);
        }
        assert_eq!(OPENS.load(SeqCst), opens);
        assert_eq!(
            (view(h)[rp::FAILED], view(h)[rp::REFUSAL]),
            (2, rp::R_ATTEMPT_FAILED)
        );
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

// Failure of teardown during a recovery launch cannot enter the new retry path.
unsafe fn unsafe_failed_restart(kind: u32) {
    let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
    assert_eq!(ftu_live_start(h, 7), 0);
    inject(h, &JUMP_OUT);
    STARTUP_STALLS.store(1, SeqCst);
    match kind {
        0 => CLEANUP_STATUS.store(-9, SeqCst),
        1 => CLOSE_STATUS.store(-7, SeqCst),
        _ => RESTORE_STATUS.store(-1, SeqCst),
    }
    assert_eq!(monitor(h).0, if kind < 2 { -3 } else { -5 });
    let opens = OPENS.load(SeqCst);
    for _ in 0..5 {
        assert_eq!(
            ftu_live_recover(h, ftu_live_binding(h)),
            if kind < 2 { -3 } else { -10 }
        );
    }
    assert_eq!(OPENS.load(SeqCst), opens);
    assert_eq!(view(h)[rp::FAILED], 1);
    assert_eq!(ftu_live_start(h, 7), -2);
    assert_eq!(ftu_live_stop(h, 7), if kind < 2 { -3 } else { 0 });
    if kind < 2 {
        assert_ne!(ftu_live_destroy(h), 0);
    } else {
        assert_eq!(ftu_live_destroy(h), 0);
    }
}
#[test]
fn failed_restart_cleanup_failure_keeps_quarantine() {
    let _h = Harness::new();
    unsafe {
        unsafe_failed_restart(0);
    }
}
#[test]
fn failed_restart_close_failure_keeps_quarantine() {
    let _h = Harness::new();
    unsafe {
        unsafe_failed_restart(1);
    }
}
#[test]
fn failed_restart_thread_policy_failure_is_not_retried() {
    let _h = Harness::new();
    unsafe {
        unsafe_failed_restart(2);
    }
}

#[test]
fn failed_restart_timeout_cannot_be_reclassified_by_late_deadline() {
    let _h = Harness::new();
    unsafe {
        let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
        assert_eq!(ftu_live_start(h, 7), 0);
        inject(h, &JUMP_OUT);
        STARTUP_STALLS.store(1, SeqCst);
        HOLD_HANDLE.store(h as u64, SeqCst);
        HOLD_STARTUP_CLOCK.store(true, SeqCst);
        let began = Instant::now();
        assert_eq!(monitor(h).0, -5);
        assert!(began.elapsed() >= Duration::from_secs(2));
        let r = record(h);
        assert_eq!(
            (r.words[rf::REASON], r.words[rf::STAGE]),
            (55, rf::STAGE_READINESS)
        );
        assert!(
            rp::classify(&r, st(h).epoch, st(h).fault).is_ok(),
            "late guard otherwise looks retryable"
        );
        let v = view(h);
        assert_eq!(
            (v[rp::STATE], v[rp::REFUSAL]),
            (rp::REFUSED, rp::R_ATTEMPT_FAILED)
        );
        let opens = OPENS.load(SeqCst);
        for _ in 0..5 {
            assert_eq!(ftu_live_recover(h, ftu_live_binding(h)), -10);
        }
        assert_eq!(OPENS.load(SeqCst), opens);
        assert_eq!(OPEN_CONTEXTS.load(SeqCst), 0);
        assert_eq!(ftu_live_stop(h, 7), 0);
        assert_eq!(ftu_live_destroy(h), 0);
    }
}

#[test]
fn failed_restart_due_starts_use_same_budget_and_admit_client_only_on_success() {
    let _h = Harness::new();
    unsafe {
        for client in [7, 8] {
            let h = ftu_live_create_pinned(&REC_FACTORY, 0x2080, LOCATION, REGISTRY);
            set_policy(
                h,
                Policy {
                    backoff_ms: [0; 3],
                    healthy_ms: 3_600_000,
                    ..rp::POLICY
                },
            );
            assert_eq!(ftu_live_start(h, 7), 0);
            inject(h, &JUMP_OUT);
            STARTUP_STALLS.store(1, SeqCst);
            assert_eq!(monitor(h).0, -5);
            let failed = st(h).epoch;
            assert_eq!(ftu_live_start(h, client), 0);
            assert_eq!((st(h).phase, st(h).epoch), (RUNNING, failed + 1));
            let v = view(h);
            assert_eq!(
                (
                    v[rp::ATTEMPTS],
                    v[rp::FAILED],
                    v[rp::RECOVERED_COUNT],
                    v[rp::CONSECUTIVE]
                ),
                (2, 1, 1, 2)
            );
            assert_eq!(v[rp::CLIENTS], if client == 7 { 1 } else { 2 });
            inject(h, &JUMP_OUT);
            STARTUP_STALLS.store(1, SeqCst);
            assert_eq!(monitor(h).0, -5);
            assert_eq!(
                (view(h)[rp::STATE], view(h)[rp::ATTEMPTS]),
                (rp::EXHAUSTED, 3)
            );
            assert_eq!(ftu_live_stop(h, client), 0);
            if client != 7 {
                assert_eq!(ftu_live_stop(h, 7), 0);
            }
            assert_eq!(
                ftu_live_start(h, 9),
                0,
                "fresh client session resets exhausted budget"
            );
            assert_eq!(view(h)[rp::CONSECUTIVE], 0);
            assert_eq!(ftu_live_stop(h, 9), 0);
            assert_eq!(ftu_live_destroy(h), 0);
        }
    }
}
