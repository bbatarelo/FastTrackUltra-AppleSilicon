//! Offline event model, not a device emulator or a realtime implementation.
//! One tick is a 125 us USB service interval. Packet counts cycle 5/6/7 frames.
//! Callback delivery, owner-thread stalls and submission time are independent.
use std::array;

const FEEDBACK_SLOTS: usize = 32;
const REPLAY_MS: usize = 64;
const BASE: usize = 16 * 8;
const WARM_FRAMES: usize = 48_000;

#[derive(Clone, Copy, Debug)]
struct Geometry {
    block_ms: usize,
    depth: usize,
}
#[derive(Clone, Copy)]
struct Stress {
    name: &'static str,
    callback_ms: usize,
    stall_ms: usize,
    submit_ticks: usize,
}
#[derive(Clone, Copy)]
struct Flight {
    seq: usize,
    start: usize,
    counts: usize,
}
#[derive(Clone, Copy)]
struct Feedback {
    seq: usize,
    counts: usize,
}
#[derive(Debug)]
struct ResultRow {
    reason: Option<&'static str>,
    min_submit_ms: usize,
    min_output_capture: i64,
    max_output_capture: i64,
    max_capture_age_ticks: usize,
    max_feedback: usize,
    observed: usize,
    captured: usize,
    replayed: usize,
    reordered: bool,
}
fn packet_frames(seq: usize, block_ticks: usize) -> usize {
    (0..block_ticks)
        .map(|i| 5 + (seq * block_ticks + i) % 3)
        .sum()
}
fn run(g: Geometry, stress: Stress, phase: usize) -> ResultRow {
    assert!((1..=8).contains(&g.block_ms) && (2..=8).contains(&g.depth));
    let block = g.block_ms * 8;
    let count = 2200 / g.block_ms;
    let mut slots: [Vec<Option<Flight>>; 2] = array::from_fn(|_| vec![None; g.depth]);
    let mut feedback: [Option<Feedback>; FEEDBACK_SLOTS] = [None; FEEDBACK_SLOTS];
    let mut next = [0; 2];
    let mut complete = [0; 2];
    let mut last_completed = [None; 2];
    let mut clock_seq = 0;
    let mut captured = 0;
    let mut replayed = 0;
    let mut now = 0;
    let stall_start = BASE + 1200 * 8 + phase;
    let stall_end = stall_start + stress.stall_ms * 8;
    let mut r = ResultRow {
        reason: None,
        min_submit_ms: usize::MAX,
        min_output_capture: i64::MAX,
        max_output_capture: i64::MIN,
        max_capture_age_ticks: 0,
        max_feedback: 0,
        observed: 0,
        captured: 0,
        replayed: 0,
        reordered: false,
    };
    'stream: while complete[1] < count {
        if now > BASE + count * block + REPLAY_MS * 8 + 8000 {
            r.reason = Some("model_timeout");
            break;
        }
        if now >= BASE + 1000 * 8 && now < BASE + (count - g.depth) * block {
            let cursor_ticks = captured / 6;
            r.max_capture_age_ticks = r
                .max_capture_age_ticks
                .max(now.saturating_sub(BASE + cursor_ticks));
        }
        if now >= stall_start && now < stall_end {
            now += 1;
            continue;
        }
        for d in 0..2 {
            for flight_slot in &mut slots[d] {
                let Some(f) = *flight_slot else { continue };
                let delay = if f.seq.is_multiple_of(2) {
                    stress.callback_ms * 8
                } else {
                    0
                };
                if now < f.start + block + delay {
                    continue;
                }
                *flight_slot = None;
                if last_completed[d].is_some_and(|previous| previous > f.seq) {
                    r.reordered = true;
                }
                last_completed[d] = Some(f.seq);
                complete[d] += 1;
                if d == 0 {
                    let at = f.seq % FEEDBACK_SLOTS;
                    if feedback[at].is_some() {
                        r.reason = Some("feedback_overrun");
                        break 'stream;
                    }
                    feedback[at] = Some(Feedback {
                        seq: f.seq,
                        counts: f.counts,
                    });
                    r.max_feedback = r.max_feedback.max(feedback.iter().flatten().count());
                } else {
                    // A flight retains its original replayed count until completion.
                    assert_eq!(f.counts, packet_frames(f.seq, block));
                }
            }
        }
        while let Some(f) = feedback[clock_seq % FEEDBACK_SLOTS] {
            if f.seq != clock_seq {
                break;
            }
            captured += f.counts;
            clock_seq += 1;
        }
        for d in 0..2 {
            while next[d] < count {
                let seq = next[d];
                let slot = seq % g.depth;
                if slots[d][slot].is_some() {
                    break;
                }
                let counts = if d == 0 {
                    packet_frames(seq, block)
                } else {
                    let Some(f) = feedback[seq % FEEDBACK_SLOTS] else {
                        break;
                    };
                    if f.seq != seq || clock_seq <= seq {
                        break;
                    }
                    f.counts
                };
                let start = BASE + seq * block + if d == 1 { REPLAY_MS * 8 } else { 0 };
                // Same integer bus-frame rule as schedule_valid: more than 1 ms.
                if start / 8 <= now / 8 + 1 {
                    r.reason = Some("stale_schedule");
                    break 'stream;
                }
                if d == 1
                    && captured >= WARM_FRAMES
                    && seq + g.depth + REPLAY_MS / g.block_ms < count
                {
                    let lead = (replayed + REPLAY_MS * 48) as i64 - captured as i64;
                    r.min_output_capture = r.min_output_capture.min(lead);
                    r.max_output_capture = r.max_output_capture.max(lead);
                }
                // Account for time between admission and native submission return.
                now += stress.submit_ticks;
                let lead = (start / 8).saturating_sub(now / 8);
                if lead <= 1 {
                    r.reason = Some("post_submit_deadline");
                    break 'stream;
                }
                if captured >= WARM_FRAMES && seq + g.depth + REPLAY_MS / g.block_ms < count {
                    r.min_submit_ms = r.min_submit_ms.min(lead);
                    r.observed += 1;
                }
                slots[d][slot] = Some(Flight { seq, start, counts });
                next[d] += 1;
                if d == 1 {
                    replayed += counts;
                    feedback[seq % FEEDBACK_SLOTS] = None;
                }
            }
        }
        now += 1;
    }
    r.captured = captured;
    r.replayed = replayed;
    if r.reason.is_none() {
        assert_eq!(complete, [count; 2]);
        assert_eq!(captured, replayed);
        assert!(feedback.iter().all(Option::is_none));
        assert!(slots.iter().flatten().all(Option::is_none));
    }
    r
}
const CLEAN: Stress = Stress {
    name: "clean",
    callback_ms: 0,
    stall_ms: 0,
    submit_ticks: 0,
};
const STRESSES: [Stress; 4] = [
    CLEAN,
    Stress {
        name: "bounded",
        callback_ms: 2,
        stall_ms: 8,
        submit_ticks: 8,
    },
    Stress {
        name: "reordered",
        callback_ms: 10,
        stall_ms: 0,
        submit_ticks: 1,
    },
    Stress {
        name: "over_budget",
        callback_ms: 2,
        stall_ms: 32,
        submit_ticks: 8,
    },
];
fn main() {
    println!("{{\"schema\":1,\"hardware_qualification\":false,\"replay_ms\":64,\"feedback_slots\":32,\"rows\":[");
    let mut first = true;
    for block_ms in [1, 2, 4, 8] {
        for depth in [2, 3, 4, 6, 8] {
            for stress in STRESSES {
                let rows: Vec<_> = (0..block_ms * 16)
                    .map(|phase| run(Geometry { block_ms, depth }, stress, phase))
                    .collect();
                let accepted = rows.iter().filter(|r| r.reason.is_none()).count();
                let min_submit = rows
                    .iter()
                    .filter(|r| r.observed > 0)
                    .map(|r| r.min_submit_ms)
                    .min();
                let min_output = rows
                    .iter()
                    .filter(|r| r.min_output_capture != i64::MAX)
                    .map(|r| r.min_output_capture)
                    .min();
                let max_output = rows
                    .iter()
                    .filter(|r| r.max_output_capture != i64::MIN)
                    .map(|r| r.max_output_capture)
                    .max();
                let reasons: std::collections::BTreeSet<_> =
                    rows.iter().filter_map(|r| r.reason).collect();
                if !first {
                    println!(",");
                }
                first = false;
                print!("{{\"block_ms\":{block_ms},\"depth\":{depth},\"stress\":\"{}\",\"callback_delay_ms\":{},\"owner_stall_ms\":{},\"submit_us\":{},\"phases\":{},\"passed\":{accepted},\"min_submit_ms\":{},\"min_output_capture_frames\":{},\"max_output_capture_frames\":{},\"max_capture_age_ms\":{},\"max_feedback\":{},\"reasons\":[{}]}}",
            stress.name,stress.callback_ms,stress.stall_ms,stress.submit_ticks*125,rows.len(),
            min_submit.map_or("null".into(),|v|v.to_string()),min_output.map_or("null".into(),|v|v.to_string()),max_output.map_or("null".into(),|v|v.to_string()),
            rows.iter().map(|r|r.max_capture_age_ticks).max().unwrap() as f64/8.,
            rows.iter().map(|r|r.max_feedback).max().unwrap(),
            reasons.iter().map(|r|format!("\"{r}\"")).collect::<Vec<_>>().join(","));
            }
        }
    }
    println!("\n]}}");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn baseline_reproduces_measured_queue_lead_and_conserves_counts() {
        let r = run(
            Geometry {
                block_ms: 8,
                depth: 4,
            },
            CLEAN,
            0,
        );
        assert_eq!(r.reason, None);
        assert_eq!(r.min_submit_ms, 24);
        assert!((1151..=1153).contains(&r.min_output_capture));
        assert_eq!(r.captured, r.replayed);
    }
    #[test]
    fn smaller_queue_reduces_preparation_horizon_and_survives_chosen_budget() {
        for phase in 0..128 {
            let clean = run(
                Geometry {
                    block_ms: 8,
                    depth: 3,
                },
                CLEAN,
                phase,
            );
            assert_eq!(clean.min_submit_ms, 16);
            assert!((767..=769).contains(&clean.min_output_capture));
            for s in &STRESSES[..3] {
                assert_eq!(
                    run(
                        Geometry {
                            block_ms: 8,
                            depth: 3
                        },
                        *s,
                        phase
                    )
                    .reason,
                    None
                );
            }
        }
    }
    #[test]
    fn smaller_blocks_need_feedback_capacity_and_scheduling_budget() {
        assert_eq!(
            run(
                Geometry {
                    block_ms: 1,
                    depth: 4
                },
                CLEAN,
                0
            )
            .reason,
            Some("feedback_overrun")
        );
        assert!(run(
            Geometry {
                block_ms: 2,
                depth: 3
            },
            STRESSES[1],
            0
        )
        .reason
        .is_some());
        assert_eq!(
            run(
                Geometry {
                    block_ms: 4,
                    depth: 6
                },
                STRESSES[1],
                0
            )
            .reason,
            None
        );
    }
    #[test]
    fn reordered_delivery_is_exercised_and_long_stall_is_rejected() {
        let r = run(
            Geometry {
                block_ms: 8,
                depth: 3,
            },
            STRESSES[2],
            0,
        );
        assert_eq!(r.reason, None);
        assert!(r.reordered);
        assert!(run(
            Geometry {
                block_ms: 8,
                depth: 3
            },
            STRESSES[3],
            0
        )
        .reason
        .is_some());
    }
}
