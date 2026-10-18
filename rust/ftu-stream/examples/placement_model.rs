//! Offline nominal-48-kHz sample-placement model, independent of production.
//! One tick = one sample frame. USB packets contain six frames; the separate
//! timing_model and production tests cover variable captured packet counts.
//! HAL and hardware progress independently while the USB owner is delayed.
use std::array;

const RATE: usize = 48_000;
const BASE: usize = 16 * 48;
const REPLAY: usize = 64 * 48;
const RING: usize = 32_768;
const HISTORY: usize = 32;
const ANALOG: usize = 58; // Chosen integer fixture, NOT converter calibration.
const DURATION: usize = 2 * RATE;
const MARKERS: [usize; 2] = [48_000, 60_000];

#[derive(Clone, Copy)]
struct Geometry {
    block_ms: usize,
    depth: usize,
    input: usize,
    output: usize,
    buffer: usize,
    // Only altered by the negative control: production equals REPLAY.
    pcm_offset: usize,
    anchor_bias: isize, // Positive means HAL coordinate lags nominal device time.
}
#[derive(Clone, Copy)]
struct Stress {
    name: &'static str,
    callback_delay: usize,
    owner_stall: usize,
    hal_delay: usize,
    prepare: usize,
    submit: usize,
    hole: bool,
}
const CLEAN: Stress = Stress {
    name: "clean",
    callback_delay: 0,
    owner_stall: 0,
    hal_delay: 0,
    prepare: 0,
    submit: 0,
    hole: false,
};
#[derive(Clone, Copy, Default)]
struct Sample {
    tag: Option<usize>,
    value: Option<usize>,
}
struct Ring(Vec<Sample>);
impl Ring {
    fn new() -> Self {
        Self(vec![Sample::default(); RING])
    }
    fn put(&mut self, frame: usize, value: Option<usize>) {
        self.0[frame % RING] = Sample {
            tag: Some(frame),
            value,
        };
    }
    fn get(&self, frame: usize) -> (bool, Option<usize>) {
        let sample = self.0[frame % RING];
        (
            sample.tag == Some(frame),
            if sample.tag == Some(frame) {
                sample.value
            } else {
                None
            },
        )
    }
}
struct Flight {
    seq: usize,
    start: usize,
    callback: usize,
    serviced: bool,
    payload: Vec<Option<usize>>,
}
struct Admission {
    direction: usize,
    seq: usize,
    start: usize,
    finish: usize,
    payload: Vec<Option<usize>>,
}
#[derive(Default, Debug)]
struct ResultRow {
    reason: Option<&'static str>,
    output_missing: usize,
    input_missing: usize,
    missing_with_positive_high_water: bool,
    min_lead_ms: Option<usize>,
    min_high_water_margin: Option<i64>,
    max_feedback: usize,
    captured: usize,
    replayed: usize,
    completed: [usize; 2],
    rtl: [Option<usize>; 2],
}
fn minimum<T: Ord + Copy>(out: &mut Option<T>, value: T) {
    *out = Some(out.map_or(value, |old| old.min(value)));
}
fn delayed(due: usize, start: usize, duration: usize) -> usize {
    if duration > 0 && due >= start && due < start + duration {
        start + duration
    } else {
        due
    }
}
fn run(g: Geometry, stress: Stress, phase: usize) -> ResultRow {
    let block = g.block_ms * 48;
    assert!([4, 8].contains(&g.block_ms) && (2..=8).contains(&g.depth));
    let count = DURATION / block;
    let mut slots: [Vec<Option<Flight>>; 2] =
        array::from_fn(|_| (0..g.depth).map(|_| None).collect());
    let mut feedback: [Option<usize>; HISTORY] = [None; HISTORY];
    let mut capture_ready: [Option<usize>; HISTORY] = [None; HISTORY];
    let mut playback = Ring::new();
    let mut capture = Ring::new();
    let mut physical = vec![None; DURATION + REPLAY + ANALOG + 2 * block];
    let mut next = [0; 2];
    let mut publish = 0;
    let mut admission: Option<Admission> = None;
    let mut high_water = 0;
    let mut hal_cycle = 0;
    // Exclude bring-up from loss gates, while still modelling startup data.
    let hal_base = BASE + 4096 + phase;
    let disturbance = BASE + RATE + phase;
    // Start each delivery pause at an actual scheduled callback, so a phase
    // cannot accidentally turn the requested delay into a no-op.
    let hal_pause = hal_base + (disturbance - hal_base).div_ceil(g.buffer) * g.buffer;
    let callback_pause = BASE + (disturbance - BASE).div_ceil(block) * block;
    let mut r = ResultRow::default();
    for now in 0..BASE + DURATION + REPLAY + 10 * RATE / 100 {
        // Hardware operates even when the USB worker or HAL client is delayed.
        for flight in slots[1].iter_mut().flatten() {
            if !flight.serviced && now >= flight.start {
                for (f, value) in flight.payload.iter().enumerate() {
                    physical[flight.seq * block + REPLAY + ANALOG + f] = *value;
                }
                flight.serviced = true;
            }
        }
        // HAL timestamps remain at their scheduled coordinates under delivery
        // delay. This explicit test assumption is not an OS timing guarantee.
        loop {
            let nominal = hal_base + hal_cycle * g.buffer;
            let due = delayed(nominal, hal_pause, stress.hal_delay);
            if now < due {
                break;
            }
            let coordinate = (nominal as isize - BASE as isize - g.anchor_bias) as usize;
            let output_first = coordinate + g.buffer + g.output;
            if coordinate >= g.buffer + g.input {
                let input_first = coordinate - g.buffer - g.input;
                for f in 0..g.buffer {
                    let (valid, value) = capture.get(input_first + f);
                    if now >= BASE + RATE && now < BASE + DURATION - 2 * g.buffer - block {
                        r.input_missing += usize::from(!valid);
                    }
                    if let Some(raw) = value {
                        if let Some(i) = MARKERS.iter().position(|&m| m == raw) {
                            let rtl = hal_cycle * g.buffer + f - raw;
                            assert!(r.rtl[i].is_none() || r.rtl[i] == Some(rtl));
                            r.rtl[i] = Some(rtl);
                        }
                    }
                }
            }
            for f in 0..g.buffer {
                let raw = hal_cycle * g.buffer + f;
                // A deliberately missing middle publication with future writes
                // retained: high-water alone must not pass the coverage gate.
                if !(stress.hole && (48_064..48_128).contains(&raw)) {
                    playback.put(output_first + f, Some(raw));
                }
            }
            high_water = high_water.max(output_first + g.buffer);
            hal_cycle += 1;
        }
        if r.completed == [count; 2] {
            break;
        }
        // A stalled owner cannot dispatch callbacks or complete submission work.
        if delayed(now, disturbance, stress.owner_stall) != now {
            continue;
        }
        if let Some(a) = admission.take() {
            if now < a.finish {
                admission = Some(a);
                continue;
            }
            let lead = (a.start / 48).saturating_sub(now / 48);
            if lead <= 1 {
                r.reason = Some("submission_deadline");
                break;
            }
            if now >= BASE + RATE && a.seq + g.depth + REPLAY / block < count {
                minimum(&mut r.min_lead_ms, lead);
            }
            let direction = a.direction;
            let seq = a.seq;
            let end = a.start + block;
            let callback = delayed(end, callback_pause, stress.callback_delay);
            slots[direction][seq % g.depth] = Some(Flight {
                seq,
                start: a.start,
                callback,
                serviced: false,
                payload: a.payload,
            });
            next[direction] += 1;
            if direction == 1 {
                feedback[seq % HISTORY] = None;
                r.replayed += block;
            }
        }
        // Poll matching completions; publish input strictly in sequence.
        for (direction, direction_slots) in slots.iter_mut().enumerate() {
            for slot in direction_slots {
                if slot.as_ref().is_some_and(|f| now >= f.callback) {
                    let flight = slot.take().unwrap();
                    if direction == 0 {
                        let at = flight.seq % HISTORY;
                        if feedback[at].is_some() || capture_ready[at].is_some() {
                            r.reason = Some("feedback_overrun");
                            break;
                        }
                        feedback[at] = Some(flight.seq);
                        capture_ready[at] = Some(flight.seq);
                        r.max_feedback = r.max_feedback.max(feedback.iter().flatten().count());
                    } else {
                        assert!(flight.serviced);
                    }
                    r.completed[direction] += 1;
                }
            }
        }
        if r.reason.is_some() {
            break;
        }
        while capture_ready[publish % HISTORY] == Some(publish) {
            for f in 0..block {
                let frame = publish * block + f;
                capture.put(frame, physical[frame]);
            }
            capture_ready[publish % HISTORY] = None;
            publish += 1;
            r.captured += block;
        }
        // Same IN-before-OUT admission order. The payload read happens before
        // packing/native-submit cost; concurrent later publication cannot repair
        // an already copied missing sample. Real atomics/races have separate tests.
        for direction in 0..2 {
            let seq = next[direction];
            if seq == count || slots[direction][seq % g.depth].is_some() {
                continue;
            }
            if direction == 1 && (feedback[seq % HISTORY] != Some(seq) || publish <= seq) {
                continue;
            }
            let start = BASE + seq * block + if direction == 1 { REPLAY } else { 0 };
            if start / 48 <= now / 48 + 1 {
                r.reason = Some("admission_deadline");
                break;
            }
            let mut payload = Vec::new();
            if direction == 1 {
                let first = seq * block + g.pcm_offset;
                let mut missing = 0;
                for f in 0..block {
                    let (valid, value) = playback.get(first + f);
                    missing += usize::from(!valid);
                    payload.push(value);
                }
                if now >= BASE + RATE && seq + g.depth + REPLAY / block < count {
                    r.output_missing += missing;
                    let coverage = high_water as i64 - (first + block) as i64;
                    minimum(&mut r.min_high_water_margin, coverage);
                    r.missing_with_positive_high_water |= missing > 0 && coverage > 0;
                }
            }
            admission = Some(Admission {
                direction,
                seq,
                start,
                finish: now + stress.submit + if direction == 1 { stress.prepare } else { 0 },
                payload,
            });
            break;
        }
        if r.reason.is_some() {
            break;
        }
    }
    if r.reason.is_none() && r.completed != [count; 2] {
        r.reason = Some("model_timeout");
    }
    if r.reason.is_none() {
        assert!(slots.iter().flatten().all(Option::is_none));
        assert!(feedback.iter().all(Option::is_none));
        assert!(capture_ready.iter().all(Option::is_none));
        assert_eq!(r.captured, r.replayed);
    }
    r
}
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn phases(g: Geometry) -> Vec<usize> {
    let n = gcd(g.buffer, g.block_ms * 48);
    let mut v = vec![0, 1, n / 4 - 1, n / 4, n / 2 - 1, n / 2, n - 2, n - 1];
    v.sort();
    v.dedup();
    v
}
fn stresses(buffer: usize) -> [Stress; 6] {
    [
        CLEAN,
        Stress {
            name: "delivery_6ms",
            callback_delay: 6 * 48,
            prepare: 12,
            submit: 12,
            ..CLEAN
        },
        Stress {
            name: "owner8_callback2",
            owner_stall: 8 * 48,
            callback_delay: 2 * 48,
            prepare: 12,
            submit: 12,
            ..CLEAN
        },
        Stress {
            name: "hal_extra_buffer_and_max_block",
            hal_delay: buffer + 832,
            prepare: 12,
            submit: 12,
            ..CLEAN
        },
        Stress {
            name: "beyond_budget",
            owner_stall: 32 * 48,
            prepare: 12,
            submit: 12,
            ..CLEAN
        },
        Stress {
            name: "owner12_5ms",
            owner_stall: 600,
            prepare: 12,
            submit: 12,
            ..CLEAN
        },
    ]
}
fn main() {
    println!("{{\"schema\":1,\"hardware_qualification\":false,\"nominal_packet_frames\":6,\"analog_fixture_frames\":{ANALOG},\"rows\":[");
    let mut comma = "";
    for (name, block_ms, depth, output) in [
        ("baseline", 8, 4, 3072),
        ("safety2560", 8, 4, 2560),
        ("safety2048", 8, 4, 2048),
        ("short4_depth6", 4, 6, 2560),
        ("short4_depth4", 4, 4, 2048),
    ] {
        for buffer in [64, 128, 256, 512, 1024, 4096] {
            let g = Geometry {
                block_ms,
                depth,
                input: 1024,
                output,
                buffer,
                pcm_offset: REPLAY,
                anchor_bias: 48,
            };
            for stress in stresses(buffer) {
                let results: Vec<_> = phases(g)
                    .iter()
                    .map(|&phase| run(g, stress, phase))
                    .collect();
                let passed = results
                    .iter()
                    .filter(|r| {
                        r.reason.is_none()
                            && r.output_missing == 0
                            && r.input_missing == 0
                            && r.rtl.iter().all(Option::is_some)
                    })
                    .count();
                let leads: Vec<_> = results.iter().filter_map(|r| r.min_lead_ms).collect();
                let reasons: std::collections::BTreeSet<_> =
                    results.iter().filter_map(|r| r.reason).collect();
                let rtls: Vec<_> = results.iter().flat_map(|r| r.rtl).flatten().collect();
                print!("{comma}{{\"profile\":\"{name}\",\"block_ms\":{block_ms},\"depth\":{depth},\"output_safety\":{output},\"buffer\":{buffer},\"stress\":\"{}\",\"phases\":{},\"passed\":{passed},\"max_output_missing\":{},\"max_input_missing\":{},\"min_submit_ms\":{},\"raw_rtl_frames\":{:?},\"reasons\":{:?}}}",stress.name,results.len(),results.iter().map(|r|r.output_missing).max().unwrap(),results.iter().map(|r|r.input_missing).max().unwrap(),leads.iter().min().map_or("null".into(),|v|v.to_string()),rtls,reasons.into_iter().collect::<Vec<_>>());
                comma = ",\n";
            }
        }
    }
    println!("\n]}}");
}
#[cfg(test)]
mod tests {
    use super::*;
    fn geometry(output: usize) -> Geometry {
        Geometry {
            block_ms: 8,
            depth: 4,
            input: 1024,
            output,
            buffer: 64,
            pcm_offset: REPLAY,
            anchor_bias: 48,
        }
    }
    #[test]
    fn physical_marker_round_trip_and_replay_cancellation() {
        for output in [3072, 2560] {
            for buffer in [64, 128, 256, 512, 1024, 4096] {
                let g = Geometry {
                    buffer,
                    ..geometry(output)
                };
                let r = run(g, CLEAN, 0);
                assert_eq!(r.reason, None);
                assert_eq!((r.output_missing, r.input_missing), (0, 0));
                assert_eq!(r.rtl, [Some(2 * buffer + 1024 + output + ANALOG); 2]);
                assert_eq!(r.captured, r.replayed);
            }
        }
    }
    #[test]
    fn missing_middle_write_is_not_proved_by_high_water() {
        let r = run(
            geometry(2560),
            Stress {
                hole: true,
                ..CLEAN
            },
            0,
        );
        assert_eq!(r.reason, None);
        assert!(r.output_missing > 0);
        assert!(r.missing_with_positive_high_water);
    }
    #[test]
    fn wrong_pcm_offset_adds_replay_latency_or_starves() {
        let r = run(
            Geometry {
                pcm_offset: 0,
                ..geometry(3072)
            },
            CLEAN,
            0,
        );
        assert_eq!(r.rtl, [Some(2 * 64 + 1024 + 3072 + REPLAY + ANALOG); 2]);
    }
    #[test]
    fn chosen_publication_budget_distinguishes_candidates() {
        let mut aggressive_failures = 0;
        for buffer in [64, 128, 256, 512, 1024, 4096] {
            let good = Geometry {
                buffer,
                ..geometry(2560)
            };
            for phase in phases(good) {
                let r = run(good, stresses(buffer)[3], phase);
                assert_eq!(r.reason, None);
                assert_eq!((r.output_missing, r.input_missing), (0, 0));
                assert_eq!(r.rtl, [Some(2 * buffer + 1024 + 2560 + ANALOG); 2]);
                let r = run(
                    Geometry {
                        output: 2048,
                        ..good
                    },
                    stresses(buffer)[3],
                    phase,
                );
                aggressive_failures += usize::from(r.output_missing > 0);
            }
        }
        assert!(
            aggressive_failures > 0,
            "too-aggressive safety must fail at some phase"
        );
    }
    #[test]
    fn held_anchor_bias_changes_margin_but_not_round_trip() {
        let mut margins = Vec::new();
        for anchor_bias in [-48, 0, 48] {
            let g = Geometry {
                anchor_bias,
                ..geometry(2560)
            };
            let r = run(g, stresses(64)[3], 0);
            assert_eq!((r.output_missing, r.input_missing), (0, 0));
            assert_eq!(r.rtl, [Some(2 * 64 + 1024 + 2560 + ANALOG); 2]);
            margins.push(r.min_high_water_margin.unwrap());
        }
        assert!(margins.windows(2).all(|pair| pair[0] > pair[1]));
        let mut ring = Ring::new();
        ring.put(7, Some(123));
        ring.put(7 + RING, Some(456));
        assert_eq!(ring.get(7), (false, None));
        assert_eq!(ring.get(7 + RING), (true, Some(456)));
    }
    #[test]
    fn capture_and_transport_failures_are_independent() {
        let r = run(geometry(2560), stresses(64)[4], 0);
        assert!(matches!(
            r.reason,
            Some("admission_deadline" | "submission_deadline")
        ));
        let r = run(
            Geometry {
                input: 128,
                ..geometry(2560)
            },
            stresses(64)[1],
            0,
        );
        assert!(r.input_missing > 0); // Output coverage alone cannot qualify capture.
        assert_eq!(r.reason, None);
        assert_eq!(r.output_missing, 0);
    }
}
