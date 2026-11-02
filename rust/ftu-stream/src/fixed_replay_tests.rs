use crate::capture_trace::{Event, Scope};
use crate::fixed_replay::Window;
use crate::pool_simulation::{Dispatch, PcmWorkload};
const ABSENCE: Window = Window::new(1_622_800, 1_643_300);
fn workload() -> PcmWorkload {
    PcmWorkload {
        dispatch: Dispatch::Separate {
            ready_first: true,
            callback_us: 50,
            entry_us: 0,
            batch: 1,
        },
        service_ready: true,
        order: crate::submission_order::SubmissionOrder::Deadline,
        pump_step_ms: 8,
        cost_start_frame: 1616,
        submit_us: [1000; 2],
        ..Default::default()
    }
}
struct Run {
    accepted: bool,
    reason: Option<&'static str>,
    missing: u64,
    output_missing: u64,
    lead: u64,
    events: Vec<Event>,
}
fn run(buffer: usize, phase: u64, work: PcmWorkload) -> Run {
    let geometry = crate::transport::SHORTER_TEN;
    let (live, io) = placement_fixture_for(geometry);
    let wall = Arc::new(AtomicU64::new(100_000));
    let mut hal = PcmHal {
        live,
        io,
        wall: wall.clone(),
        buffer,
        next: phase,
        write: vec![0.; buffer * 8],
        read: vec![0.; buffer * 8],
        steady_callbacks: 0,
        pause_start: 0,
        pause_frames: 0,
        steady_input_missing: 0,
        steady_output_missing: 0,
        min_lead_ms: u64::MAX,
        min_steady_lead_ms: u64::MAX,
        work,
        stage_paid: false,
    };
    let trace = Scope::with_capacity(0, u64::MAX, 32768);
    let session = crate::pool_simulation::run_pcm_fixture(
        geometry,
        wall,
        work,
        crate::output_admission::OutputPolicy::WhenReady,
        &mut hal,
    );
    let events = trace.finish();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "hal_read" && (48000..600 * 192).contains(&e.c))
            .count(),
        hal.steady_callbacks
    );
    assert_eq!(session.pending(), [0; 2]);
    assert!(!session.drain_failed);
    if session.reason.is_none() {
        assert_eq!(session.completed, [600; 2]);
        assert_eq!(session.frames[0], session.frames[1]);
    }
    if let Some(w) = work.fixed_absence {
        for e in &events {
            if matches!(
                e.kind,
                "submit" | "deliver" | "consume" | "stage" | "publish"
            ) {
                assert!(
                    !(w.start..w.end).contains(&e.us),
                    "owner action inside absence: {e:?}"
                );
            }
        }
        assert_eq!(events.iter().filter(|e| e.kind == "fixed_begin").count(), 1);
        assert_eq!(
            events.iter().find(|e| e.kind == "fixed_begin").unwrap().us,
            w.start
        );
        assert_eq!(
            events.iter().find(|e| e.kind == "fixed_end").unwrap().us,
            w.end
        );
    }
    Run {
        accepted: session.reason.is_none()
            && hal.steady_input_missing == 0
            && hal.steady_output_missing == 0
            && hal.min_lead_ms >= 16,
        reason: session.reason,
        missing: hal.steady_input_missing,
        output_missing: hal.steady_output_missing,
        lead: hal.min_lead_ms,
        events,
    }
}
#[derive(Debug)]
struct Request {
    seq: u64,
    slot: u64,
    submitted: u64,
    ready: u64,
    first: u64,
    end: u64,
    actual: u64,
}
fn requests(run: &Run) -> Vec<Request> {
    let mut out = Vec::new();
    for p in run
        .events
        .iter()
        .filter(|e| e.kind == "publish" && e.c == 1)
    {
        let submitted = run
            .events
            .iter()
            .find(|e| e.kind == "submit" && e.a == 0 && e.seq == p.seq)
            .unwrap();
        let delivered = run
            .events
            .iter()
            .find(|e| e.kind == "deliver" && e.a == 0 && e.seq == p.seq)
            .unwrap();
        assert!(submitted.us <= submitted.b * 1000 && submitted.b * 1000 < delivered.b);
        assert!(delivered.b <= delivered.us && delivered.us <= p.us);
        out.push(Request {
            seq: p.seq,
            slot: submitted.c,
            submitted: submitted.us,
            ready: delivered.b,
            first: p.a,
            end: p.b,
            actual: p.us,
        });
    }
    assert!(out
        .windows(2)
        .all(|p| p[0].seq + 1 == p[1].seq && p[0].end == p[1].first));
    out
}
fn misses(run: &Run, reqs: &[Request], times: &[u64]) -> u64 {
    assert_eq!(reqs.len(), times.len());
    let mut missing = 0;
    let original = reqs.iter().zip(times).all(|(r, &t)| r.actual == t);
    for read in run
        .events
        .iter()
        .filter(|e| e.kind == "hal_read" && (48000..600 * 192).contains(&e.c))
    {
        let mut covered = 0;
        let before = missing;
        let newest = reqs
            .iter()
            .zip(times)
            .filter(|(_, t)| **t < read.us)
            .map(|(r, _)| r.end)
            .max()
            .unwrap_or(0);
        assert!(
            newest.saturating_sub(read.seq) <= RING_FRAMES as u64,
            "capture ring overwritten before read"
        );
        for (r, &published) in reqs.iter().zip(times) {
            let overlap = r
                .end
                .min(read.seq + read.a)
                .saturating_sub(r.first.max(read.seq));
            covered += overlap;
            // The HAL read is serviced before publication at the same tick.
            if published >= read.us {
                missing += overlap;
            }
        }
        assert_eq!(
            covered, read.a,
            "capture trace must cover every tested read"
        );
        if original {
            assert_eq!(
                missing - before,
                read.b,
                "per-read reconstruction at {}",
                read.us
            );
        }
    }
    missing
}
/// Schedule-only lower-bound model, not a second USB owner implementation.
/// Uses validated request ranges from the actual pool run; never invents data
/// for a request that was not submitted, completed and validated in that run.
fn independent(run: &Run, reqs: &[Request], cost: u64, correlated: Option<Window>) -> (u64, u64) {
    let mut times = Vec::with_capacity(reqs.len());
    let mut available = 0;
    for r in reqs {
        available = crate::fixed_replay::work(available.max(r.ready), cost, correlated);
        times.push(available);
    }
    let mut reuse_violations = 0;
    for (i, r) in reqs.iter().enumerate() {
        if let Some(next) = reqs.iter().skip(i + 1).find(|n| n.slot == r.slot) {
            reuse_violations += u64::from(times[i] > next.submitted);
        }
    }
    (misses(run, reqs, &times), reuse_violations)
}
#[test]
fn fixed_replay_exact_counterexample_and_independent_bound() {
    let control = run(
        64,
        7,
        PcmWorkload {
            stall_at_frame: 1623,
            stall_us: 12500,
            ..workload()
        },
    );
    assert_eq!(control.reason, None);
    let first = control
        .events
        .iter()
        .find(|e| e.kind == "hal_missing")
        .unwrap();
    assert_eq!((first.us, first.seq), (1_641_480, 72_191));
    let stall = control
        .events
        .iter()
        .find(|e| e.kind == "stall_begin")
        .unwrap();
    assert_eq!(stall.us, 1_630_800);
    // Replay the realized injected stall at fixed wall time, retaining the
    // original preceding idle. Freezing the whole idle+stall is a separate arm:
    // it may preempt an operation exactly at the idle-start boundary.
    let exact = run(
        64,
        7,
        PcmWorkload {
            fixed_absence: Some(Window::new(stall.us, stall.us + 12500)),
            ..workload()
        },
    );
    assert_eq!(exact.missing, control.missing);
    assert_eq!(
        (exact.reason, exact.lead, exact.output_missing),
        (control.reason, control.lead, control.output_missing)
    );
    for kind in ["submit", "deliver", "publish", "hal_missing"] {
        let events = |r: &Run| {
            r.events
                .iter()
                .filter(|e| e.kind == kind)
                .map(|e| (e.us, e.seq, e.a, e.b, e.c))
                .collect::<Vec<_>>()
        };
        let a = events(&exact);
        let b = events(&control);
        assert_eq!(a.len(), b.len());
        for (i, (a, b)) in a.iter().zip(&b).enumerate() {
            assert_eq!(a, b, "fixed replay {kind} event {i}");
        }
    }
    println!(
        "REPLAY_EXACT missing={} lead={} causal_events_identical=1",
        exact.missing, exact.lead
    );
    for wake in [false, true] {
        // Freeze the entire observed absence, not just its injected portion.
        let replay = run(
            64,
            7,
            PcmWorkload {
                fixed_absence: Some(ABSENCE),
                wake_on_ready: wake,
                ..workload()
            },
        );
        assert!(!replay.accepted);
        assert!(replay.missing > 0);
        assert_eq!(replay.reason, None);
        assert_eq!(replay.output_missing, 0);
        assert!(replay.lead >= 16);
        let reqs = requests(&replay);
        assert_eq!(reqs.len(), 600);
        assert_eq!(
            misses(
                &replay,
                &reqs,
                &reqs.iter().map(|r| r.actual).collect::<Vec<_>>()
            ),
            replay.missing
        );
        let required = reqs
            .iter()
            .find(|r| r.first <= 72_191 && 72_191 < r.end)
            .unwrap();
        assert!(required.submitted < ABSENCE.start && required.ready < first.us);
        let coverage = reqs
            .iter()
            .filter(|r| r.submitted < ABSENCE.start)
            .map(|r| r.ready)
            .max()
            .unwrap();
        println!("REPLAY_BOUND wake={} owner_missing={} lead={} in_coverage_end_us={coverage} required_submitted_us={} required_ready_us={}",u8::from(wake),replay.missing,replay.lead,required.submitted,required.ready);
        for cost in [50, 75, 150, 300, 1050] {
            let separate = independent(&replay, &reqs, cost, None);
            assert_eq!(separate.0, 0);
            if cost == 50 {
                assert_eq!(separate.1, 0);
            }
            let shared = independent(&replay, &reqs, cost, Some(ABSENCE));
            assert!(shared.0 > 0);
            println!("REPLAY_INDEPENDENT wake={} callback_plus_copy_us={cost} independent_missing={} independent_reuse={} correlated_missing={} correlated_reuse={}",u8::from(wake),separate.0,separate.1,shared.0,shared.1);
        }
    }
}
#[test]
fn fixed_replay_wake_and_paid_capture_screen() {
    for buffer in [64, 512, 4096] {
        for (label, window) in [
            ("injected", Window::new(1_630_800, 1_643_300)),
            ("combined", ABSENCE),
        ] {
            for wake in [false, true] {
                for capture in [0, 100] {
                    let mut accepted = 0;
                    let mut missing = 0;
                    let mut faults = 0;
                    let mut lead = u64::MAX;
                    let mut out = 0;
                    for phase in 0..64 {
                        let result = run(
                            buffer,
                            phase,
                            PcmWorkload {
                                fixed_absence: Some(window),
                                wake_on_ready: wake,
                                capture_us: capture,
                                ..workload()
                            },
                        );
                        accepted += usize::from(result.accepted);
                        missing = missing.max(result.missing);
                        faults += usize::from(result.reason.is_some());
                        lead = lead.min(result.lead);
                        out = out.max(result.output_missing);
                    }
                    if label == "combined" && buffer <= 512 {
                        assert_eq!(accepted, 0);
                    }
                    if label == "injected" && wake {
                        assert_eq!(accepted, 64);
                    }
                    println!("REPLAY_MATRIX scope={label} buffer={buffer} wake={} capture_us={capture} cases=64 accepted={accepted} faults={faults} max_missing_in={missing} max_missing_out={out} min_lead={lead}",u8::from(wake));
                }
            }
        }
    }
}
#[test]
fn fixed_replay_readiness_offset_and_duration_screen() {
    for duration in [
        12_500, 16_000, 17_000, 17_500, 18_000, 20_500, 24_000, 32_000,
    ] {
        let mut accepted = 0;
        let mut missing = 0;
        let mut faults = 0;
        let mut lead = u64::MAX;
        let mut out = 0;
        for offset in (0..4000).step_by(125) {
            let window = Window::new(1_624_000 - offset, 1_624_000 - offset + duration);
            let r = run(
                64,
                7,
                PcmWorkload {
                    fixed_absence: Some(window),
                    wake_on_ready: true,
                    capture_us: 100,
                    ..workload()
                },
            );
            accepted += usize::from(r.accepted);
            missing = missing.max(r.missing);
            faults += usize::from(r.reason.is_some());
            lead = lead.min(r.lead);
            out = out.max(r.output_missing);
        }
        println!("REPLAY_OFFSET duration_us={duration} offsets=32 accepted={accepted} faults={faults} max_missing_in={missing} max_missing_out={out} min_lead={lead}");
        if duration >= 20_500 {
            assert_eq!(accepted, 0);
        }
    }
}

#[test]
fn fixed_replay_paid_parent_slot_lifetime() {
    let mut witnesses = 0;
    for wake in [false, true] {
        for copy in [25, 100, 250, 1000] {
            let r = run(
                64,
                7,
                PcmWorkload {
                    fixed_absence: Some(ABSENCE),
                    wake_on_ready: wake,
                    capture_us: copy,
                    ..workload()
                },
            );
            if r.reason.is_some() {
                println!("REPLAY_PAID_PARENT wake={} copy_us={copy} parent_fault=1 owner_missing={} output_missing={} lead={} independent_evaluated=0",u8::from(wake),r.missing,r.output_missing,r.lead);
                continue;
            }
            let reqs = requests(&r);
            assert_eq!(
                misses(
                    &r,
                    &reqs,
                    &reqs.iter().map(|r| r.actual).collect::<Vec<_>>()
                ),
                r.missing
            );
            let separate = independent(&r, &reqs, 50 + copy, None);
            assert_eq!(separate, (0, 0));
            let correlated = independent(&r, &reqs, 50 + copy, Some(ABSENCE));
            assert!(correlated.0 > 0);
            assert!(r.missing > 0);
            witnesses += 1;
            println!("REPLAY_PAID_PARENT wake={} copy_us={copy} parent_fault=0 owner_missing={} output_missing={} lead={} independent_evaluated=1 independent_missing={} independent_reuse={} correlated_missing={} correlated_reuse={}",u8::from(wake),r.missing,r.output_missing,r.lead,separate.0,separate.1,correlated.0,correlated.1);
        }
    }
    assert!(
        witnesses > 0,
        "need at least one paid, ownership-compatible counterfactual"
    );
}

#[test]
fn fixed_replay_independent_overload_remains_rejected() {
    let r = run(
        64,
        7,
        PcmWorkload {
            fixed_absence: Some(ABSENCE),
            ..workload()
        },
    );
    let reqs = requests(&r);
    let overload = independent(&r, &reqs, 4050, None);
    assert!(overload.0 > 0 && overload.1 > 0);
    println!(
        "REPLAY_INDEPENDENT_OVERLOAD service_us=4050 missing={} reuse={}",
        overload.0, overload.1
    );
}

#[test]
fn fixed_replay_independent_sampled_matrix() {
    for buffer in [64, 512, 4096] {
        for wake in [false, true] {
            for copy in [25, 100, 250] {
                let mut parent_faults = 0;
                let mut evaluated = 0;
                let mut missing = 0;
                let mut reuse = 0;
                let mut correlated_missing = 0;
                for phase in [0, 1, 7, 15, 31, 32, 62, 63] {
                    let r = run(
                        buffer,
                        phase,
                        PcmWorkload {
                            fixed_absence: Some(ABSENCE),
                            wake_on_ready: wake,
                            capture_us: copy,
                            ..workload()
                        },
                    );
                    if r.reason.is_some() {
                        parent_faults += 1;
                        continue;
                    }
                    let reqs = requests(&r);
                    assert_eq!(reqs.len(), 600);
                    assert_eq!(
                        misses(
                            &r,
                            &reqs,
                            &reqs.iter().map(|r| r.actual).collect::<Vec<_>>()
                        ),
                        r.missing
                    );
                    let separate = independent(&r, &reqs, 50 + copy, None);
                    let correlated = independent(&r, &reqs, 50 + copy, Some(ABSENCE));
                    evaluated += 1;
                    missing += separate.0;
                    reuse += separate.1;
                    correlated_missing += correlated.0;
                }
                assert_eq!((missing, reuse), (0, 0));
                if buffer <= 512 && evaluated > 0 {
                    assert!(correlated_missing > 0);
                }
                println!("REPLAY_SPLIT_SAMPLE buffer={buffer} wake={} copy_us={copy} cases=8 parent_faults={parent_faults} evaluated={evaluated} independent_missing={missing} independent_reuse={reuse} correlated_missing={correlated_missing}",u8::from(wake));
            }
        }
    }
}

#[test]
fn fixed_replay_wake_lateness_and_batched_sources() {
    for (scope, window) in [
        ("injected", Window::new(1630800, 1643300)),
        ("combined", ABSENCE),
    ] {
        for late in [250, 1000, 8000] {
            for batch in [1, 10] {
                let mut accepted = 0;
                let mut faults = 0;
                let mut missing = 0;
                let mut lead = u64::MAX;
                for phase in [0, 1, 7, 15, 31, 32, 62, 63] {
                    let r = run(
                        64,
                        phase,
                        PcmWorkload {
                            fixed_absence: Some(window),
                            wake_on_ready: true,
                            wake_late_us: late,
                            capture_us: 100,
                            dispatch: Dispatch::Separate {
                                ready_first: true,
                                callback_us: 50,
                                entry_us: 0,
                                batch,
                            },
                            ..workload()
                        },
                    );
                    for e in r.events.iter().filter(|e| e.kind == "wake") {
                        assert!(e.us >= e.a + late);
                    }
                    accepted += usize::from(r.accepted);
                    faults += usize::from(r.reason.is_some());
                    missing = missing.max(r.missing);
                    lead = lead.min(r.lead);
                }
                if scope == "combined" {
                    assert_eq!(accepted, 0);
                }
                println!("REPLAY_WAKE_COST scope={scope} wake_late_us={late} batch={batch} phases=8 accepted={accepted} faults={faults} max_missing_in={missing} min_lead={lead}");
            }
        }
    }
}

// Deadline lower bound with slot-return and count handoffs. This retains the
// parent's requested bus frames and earliest submission entries. It propagates
// waits through ALL serialized IN/OUT submissions and rejects at the first
// missed bus/lead deadline. Other owner work and HAL output repacking are not
// simulated; success is not a qualified concurrent transport implementation.
struct Projection {
    ok: bool,
    blocked: u64,
    max_wait: u64,
    lead: u64,
    reason: &'static str,
    missing: Option<u64>,
}
fn returned_slots(parent: &Run, copy_callback: u64, handoff: u64, window: Window) -> Projection {
    let reqs = requests(parent);
    assert_eq!(reqs.len(), 600);
    let mut pubs = vec![0; 600];
    let mut releases = [0; 10];
    let mut owner = 0;
    let mut capture = 0;
    let mut blocked = 0;
    let mut max_wait = 0;
    let mut lead = u64::MAX;
    for s in parent.events.iter().filter(|e| e.kind == "submit") {
        let begin = parent
            .events
            .iter()
            .find(|e| e.kind == "submit_begin" && e.seq == s.seq && e.a == s.a)
            .unwrap();
        let release = if s.a == 0 {
            releases[s.c as usize]
        } else {
            pubs[s.seq as usize] + handoff
        };
        let intended = begin.us.max(owner);
        let at = window.resume(intended.max(release));
        if release > intended {
            blocked += 1;
            max_wait = max_wait.max(release - intended);
        }
        let cost = if begin.us >= 1_616_000 { 1000 } else { 0 };
        owner = window.work(at, cost);
        if copy_callback == 0 && handoff == 0 {
            assert_eq!(
                owner, s.us,
                "zero-cost projection must reproduce native call return"
            );
        }
        let reason = if s.a == 0 && owner > s.b * 1000 {
            "input_bus_deadline"
        } else if s.a == 1 && s.b.saturating_sub(owner / 1000) < 16 {
            "output_lead_deadline"
        } else {
            "none"
        };
        if s.a == 1 {
            lead = lead.min(s.b.saturating_sub(owner / 1000));
        }
        if reason != "none" {
            return Projection {
                ok: false,
                blocked,
                max_wait,
                lead,
                reason,
                missing: None,
            };
        }
        if s.a == 0 {
            let r = &reqs[s.seq as usize];
            assert_eq!(r.seq, s.seq);
            // Only a submitted request can cause a completion/publication.
            assert!(owner <= r.ready - 4000);
            capture = capture.max(r.ready) + copy_callback;
            pubs[s.seq as usize] = capture;
            releases[s.c as usize] = capture + handoff;
        }
    }
    let missing = misses(parent, &reqs, &pubs);
    Projection {
        ok: missing == 0,
        blocked,
        max_wait,
        lead,
        reason: if missing > 0 {
            "capture_missing"
        } else {
            "none"
        },
        missing: Some(missing),
    }
}
#[test]
fn fixed_replay_slot_return_backpressure_projection() {
    for wake in [false, true] {
        let r = run(
            64,
            7,
            PcmWorkload {
                fixed_absence: Some(ABSENCE),
                wake_on_ready: wake,
                ..workload()
            },
        );
        assert_eq!(r.reason, None);
        assert!(returned_slots(&r, 0, 0, ABSENCE).ok);
        for cost in [75, 150, 300, 1050, 4050] {
            for handoff in [1, 25, 100] {
                let p = returned_slots(&r, cost, handoff, ABSENCE);
                if cost == 4050 {
                    assert!(!p.ok);
                }
                println!("REPLAY_HANDOFF wake={} service_us={cost} handoff_us={handoff} projected_ok={} blocked={} max_wait_us={} min_lead={} reason={} missing={:?}",u8::from(wake),u8::from(p.ok),p.blocked,p.max_wait,p.lead,p.reason,p.missing);
            }
        }
    }
}
#[test]
fn fixed_replay_slot_return_offset_decision() {
    for duration in [17_000, 17_500, 18_000, 20_500, 24_000, 32_000] {
        let mut parent_faults = 0;
        let mut evaluated = 0;
        let mut passing = 0;
        let mut blocked = 0;
        for offset in (0..4000).step_by(125) {
            let window = Window::new(1624000 - offset, 1624000 - offset + duration);
            let r = run(
                64,
                7,
                PcmWorkload {
                    fixed_absence: Some(window),
                    wake_on_ready: true,
                    ..workload()
                },
            );
            if r.reason.is_some() {
                parent_faults += 1;
                continue;
            }
            let p = returned_slots(&r, 150, 25, window);
            evaluated += 1;
            passing += usize::from(p.ok);
            blocked += p.blocked;
        }
        println!("REPLAY_HANDOFF_OFFSETS duration_us={duration} cases=32 parent_faults={parent_faults} evaluated={evaluated} projected_ok={passing} blocked={blocked} service_us=150 handoff_us=25");
        if duration >= 20_500 {
            assert!(
                passing < 32,
                "capture-only scheduling cannot qualify this complete envelope"
            );
        }
    }
}
