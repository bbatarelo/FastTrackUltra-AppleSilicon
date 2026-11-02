//! Offline nominal-48-kHz sample-placement model, independent of production.
//! One tick = one sample frame. USB packets contain six frames; the separate
//! timing_model and production tests cover variable captured packet counts.
//! HAL and hardware progress independently while the USB owner is delayed.
use std::array;

const RATE: usize = 48_000;
const BASE: usize = 16 * 48;
const REPLAY: usize = 64 * 48;
const RING: usize = 32_768;
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
    cost_after: usize,
    hole: bool,
}
const CLEAN: Stress = Stress {
    name: "clean",
    callback_delay: 0,
    owner_stall: 0,
    hal_delay: 0,
    prepare: 0,
    submit: 0,
    cost_after: 0,
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
    delivered: bool,
    payload: Vec<Option<usize>>,
}
struct Admission {
    direction: usize,
    seq: usize,
    start: usize,
    finish: usize,
    payload: Vec<Option<usize>>,
}
// Separate event service from idle waiting. These units are nominal 48 kHz
// frames; three frames = 62.5 us, conservatively rounding the fake's 50 us.
#[derive(Clone, Copy)]
struct Dispatch {
    ready_first: bool,
    callback_cost: usize,
    entry_cost: usize,
    batch: usize,
}
const DISPATCH: Dispatch = Dispatch {
    ready_first: true,
    callback_cost: 3,
    entry_cost: 0,
    batch: 10,
};
#[derive(Default, Debug)]
struct DispatchStats {
    calls: usize,
    waits: usize,
    idle_frames: usize,
    callbacks: usize,
    callback_frames: usize,
    entry_frames: usize,
    max_batch: usize,
    max_delivery_delay: usize,
}
#[derive(Default)]
enum Pump {
    #[default]
    Start,
    Enter {
        until: usize,
    },
    Wait {
        until: usize,
    },
    Ready {
        began: usize,
        count: usize,
    },
    Working {
        until: usize,
        began: usize,
        count: usize,
        direction: usize,
        slot: usize,
    },
}
fn ready_callback(slots: &[Vec<Option<Flight>>; 2], now: usize) -> Option<(usize, usize, usize)> {
    slots
        .iter()
        .enumerate()
        .flat_map(|(d, slots)| {
            slots.iter().enumerate().filter_map(move |(i, f)| {
                f.as_ref()
                    .filter(|f| !f.delivered && f.callback <= now)
                    .map(|f| (f.callback, d, i))
            })
        })
        .min()
}
impl Pump {
    fn step(
        &mut self,
        now: usize,
        idle: usize,
        cfg: Dispatch,
        slots: &mut [Vec<Option<Flight>>; 2],
        stats: &mut DispatchStats,
    ) -> bool {
        assert!(idle > 0 && (1..=10).contains(&cfg.batch));
        loop {
            match *self {
                Self::Start => {
                    stats.calls += 1;
                    stats.entry_frames += cfg.entry_cost;
                    *self = Self::Enter {
                        until: now + cfg.entry_cost,
                    };
                }
                Self::Enter { until } => {
                    if now < until {
                        return false;
                    }
                    if !cfg.ready_first || ready_callback(slots, now).is_none() {
                        stats.waits += 1;
                        stats.idle_frames += idle;
                        *self = Self::Wait { until: now + idle };
                    } else {
                        *self = Self::Ready {
                            began: now,
                            count: 0,
                        };
                    }
                }
                Self::Wait { until } => {
                    if now < until {
                        return false;
                    }
                    *self = Self::Ready {
                        began: now,
                        count: 0,
                    };
                }
                Self::Ready { began, count } => {
                    stats.max_batch = stats.max_batch.max(count);
                    let ready = ready_callback(slots, now);
                    if count >= cfg.batch || (count > 0 && now - began >= 2 * 48) || ready.is_none()
                    {
                        *self = Self::Start;
                        return true;
                    }
                    let (_, direction, slot) = ready.unwrap();
                    *self = Self::Working {
                        until: now + cfg.callback_cost,
                        began,
                        count,
                        direction,
                        slot,
                    };
                }
                Self::Working {
                    until,
                    began,
                    count,
                    direction,
                    slot,
                } => {
                    if now < until {
                        return false;
                    }
                    let f = slots[direction][slot].as_mut().unwrap();
                    assert!(!f.delivered);
                    f.delivered = true;
                    stats.callbacks += 1;
                    stats.callback_frames += cfg.callback_cost;
                    stats.max_delivery_delay = stats.max_delivery_delay.max(now - f.callback);
                    *self = Self::Ready {
                        began,
                        count: count + 1,
                    };
                }
            }
        }
    }
}
#[derive(Default, Debug)]
struct ResultRow {
    dispatch: DispatchStats,
    reason: Option<&'static str>,
    output_missing: usize,
    input_missing: usize,
    missing_with_positive_high_water: bool,
    min_lead_ms: Option<usize>,
    min_output_lead_ms: Option<usize>,
    max_burst: usize,
    min_high_water_margin: Option<i64>,
    max_feedback: usize,
    captured: usize,
    replayed: usize,
    completed: [usize; 2],
    deferred: usize,
    deadline_fallbacks: usize,
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
#[cfg(test)]
fn run(g: Geometry, stress: Stress, phase: usize) -> ResultRow {
    run_with_policy(g, stress, phase, false)
}
#[cfg(test)]
fn run_with_policy(g: Geometry, stress: Stress, phase: usize, ready_policy: bool) -> ResultRow {
    run_with_cadence(g, stress, phase, ready_policy, 1)
}
fn run_with_cadence(
    g: Geometry,
    stress: Stress,
    phase: usize,
    ready_policy: bool,
    owner_step: usize,
) -> ResultRow {
    run_with_order(g, stress, phase, ready_policy, owner_step, false)
}
fn run_with_order(
    g: Geometry,
    stress: Stress,
    phase: usize,
    ready_policy: bool,
    owner_step: usize,
    deadline_order: bool,
) -> ResultRow {
    run_with_dispatch(
        g,
        stress,
        phase,
        ready_policy,
        owner_step,
        deadline_order,
        None,
    )
}
fn run_with_dispatch(
    g: Geometry,
    stress: Stress,
    phase: usize,
    ready_policy: bool,
    owner_step: usize,
    deadline_order: bool,
    dispatch: Option<Dispatch>,
) -> ResultRow {
    assert!(dispatch.is_none() || deadline_order);
    let block = g.block_ms * 48;
    assert!([4, 8].contains(&g.block_ms) && (2..=10).contains(&g.depth));
    let count = DURATION / block;
    let history = (64 / g.block_ms + 2 * g.depth + 1).next_power_of_two();
    let mut slots: [Vec<Option<Flight>>; 2] =
        array::from_fn(|_| (0..g.depth).map(|_| None).collect());
    let mut feedback = vec![None; history];
    let mut capture_ready = vec![None; history];
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
    let mut burst = dispatch.is_some();
    let mut pump = Pump::default();
    let mut burst_attempts = 0;
    let mut dispatched_at = 0;
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
        // No PCM retry until the next owner wake; HAL/hardware above still run.
        if admission.is_none() && (!deadline_order || !burst) {
            if let Some(cfg) = dispatch {
                if !pump.step(now, owner_step, cfg, &mut slots, &mut r.dispatch) {
                    continue;
                }
            } else if !now.is_multiple_of(owner_step) {
                continue;
            }
            dispatched_at = now;
            burst_attempts = 0;
            burst = true;
        }
        if let Some(a) = admission.take() {
            if now < a.finish {
                admission = Some(a);
                continue;
            }
            let lead = (a.start / 48).saturating_sub(now / 48);
            if lead <= 1 || (ready_policy && a.direction == 1 && lead < 16) {
                r.reason = Some("submission_deadline");
                break;
            }
            if now >= BASE + RATE && a.seq + g.depth + REPLAY / block < count {
                minimum(&mut r.min_lead_ms, lead);
                if a.direction == 1 {
                    minimum(&mut r.min_output_lead_ms, lead);
                }
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
                delivered: false,
                payload: a.payload,
            });
            next[direction] += 1;
            if direction == 1 {
                feedback[seq % history] = None;
                r.replayed += block;
            }
        }
        // Poll matching completions; publish input strictly in sequence.
        for (direction, direction_slots) in slots.iter_mut().enumerate() {
            for slot in direction_slots {
                if slot.as_ref().is_some_and(|f| {
                    if dispatch.is_some() {
                        f.delivered
                    } else {
                        (if deadline_order { dispatched_at } else { now }) >= f.callback
                    }
                }) {
                    let flight = slot.take().unwrap();
                    if direction == 0 {
                        let at = flight.seq % history;
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
        while capture_ready[publish % history] == Some(publish) {
            for f in 0..block {
                let frame = publish * block + f;
                capture.put(frame, physical[frame]);
            }
            capture_ready[publish % history] = None;
            publish += 1;
            r.captured += block;
        }
        // Independent nominal scheduler: periodic callback dispatch remains
        // distinct from work finishing. Candidate admits at most ten requests
        // per wake, with OUT winning equal effective deadlines (start-16/2).
        if deadline_order && burst_attempts >= 10 {
            burst = false;
            continue;
        }
        let eligible: [bool; 2] = array::from_fn(|d| {
            next[d] < count
                && slots[d][next[d] % g.depth].is_none()
                && (d == 0 || (feedback[next[d] % history] == Some(next[d]) && publish > next[d]))
        });
        let starts: [usize; 2] =
            array::from_fn(|d| BASE + next[d] * block + if d == 1 { REPLAY } else { 0 });
        let output_first = deadline_order
            && eligible[1]
            && (!eligible[0]
                || starts[1].saturating_sub(16 * 48) <= starts[0].saturating_sub(2 * 48));
        for direction in if output_first { [1, 0] } else { [0, 1] } {
            if !eligible[direction] {
                continue;
            }
            let seq = next[direction];
            let start = starts[direction];
            burst_attempts += 1;
            if deadline_order {
                r.max_burst = r.max_burst.max(burst_attempts);
            }
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
                if ready_policy {
                    let lead = (start / 48).saturating_sub(now / 48);
                    if lead < 16 {
                        r.reason = Some("output_lead_deadline");
                        break;
                    }
                    if missing > 0 && lead > 26 {
                        r.deferred += 1;
                        if deadline_order {
                            break;
                        }
                        continue;
                    }
                    r.deadline_fallbacks += usize::from(missing > 0);
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
                finish: now
                    + if now >= stress.cost_after {
                        stress.submit + if direction == 1 { stress.prepare } else { 0 }
                    } else {
                        0
                    },
                payload,
            });
            break;
        }
        if admission.is_none() {
            burst = false;
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
    if std::env::args().any(|s| s == "--dispatch-study") {
        dispatch_study();
        return;
    }
    if std::env::args().any(|s| s == "--deadline-study") {
        deadline_cost_study();
        return;
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--retry-cost-study"] {
        retry_cost_study();
        return;
    }
    let preparation = args == ["--preparation-study"];
    let study = args == ["--completion-study"] || preparation;
    if !args.is_empty() && !study {
        eprintln!("usage: placement_model [--completion-study | --preparation-study | --retry-cost-study | --deadline-study | --dispatch-study]");
        std::process::exit(2);
    }
    println!("{{\"schema\":1,\"hardware_qualification\":false,\"nominal_packet_frames\":6,\"analog_fixture_frames\":{ANALOG},\"rows\":[");
    let mut comma = "";
    let profiles = if study {
        vec![
            ("qualified_geometry", 8, 4, 2560),
            ("short4_depth6", 4, 6, 2560),
            ("short4_depth7", 4, 7, 2560),
            ("short4_depth8", 4, 8, 2560),
            ("short4_depth10", 4, 10, 2560),
            ("short4_depth8_safety3072", 4, 8, 3072),
        ]
    } else {
        vec![
            ("baseline", 8, 4, 3072),
            ("safety2560", 8, 4, 2560),
            ("safety2048", 8, 4, 2048),
            ("short4_depth6", 4, 6, 2560),
            ("short4_depth4", 4, 4, 2048),
        ]
    };
    let buffers = if study {
        vec![64, 512, 4096]
    } else {
        vec![64, 128, 256, 512, 1024, 4096]
    };
    for (name, block_ms, depth, output) in profiles {
        for ready_policy in if preparation && depth == 10 {
            vec![false, true]
        } else {
            vec![false]
        } {
            for &buffer in &buffers {
                let g = Geometry {
                    block_ms,
                    depth,
                    input: 1024,
                    output,
                    buffer,
                    pcm_offset: REPLAY,
                    anchor_bias: 48,
                };
                let mut scenarios = stresses(buffer).to_vec();
                if study {
                    scenarios.push(Stress {
                        name: "delivery_pause_8ms",
                        callback_delay: 8 * 48,
                        prepare: 12,
                        submit: 12,
                        ..CLEAN
                    });
                }
                if preparation {
                    scenarios.push(Stress {
                        name: "permanent_pcm_hole",
                        hole: true,
                        prepare: 12,
                        submit: 12,
                        ..CLEAN
                    });
                    scenarios.push(Stress {
                        name: "hal_delay_plus_owner8",
                        hal_delay: buffer + 832,
                        owner_stall: 8 * 48,
                        prepare: 12,
                        submit: 12,
                        ..CLEAN
                    });
                }
                for owner_step in if preparation && depth == 10 {
                    vec![1, 4 * 48, 8 * 48]
                } else {
                    vec![1]
                } {
                    for &stress in &scenarios {
                        let results: Vec<_> = phases(g)
                            .iter()
                            .map(|&phase| {
                                run_with_cadence(g, stress, phase, ready_policy, owner_step)
                            })
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
                        let accepted = results
                            .iter()
                            .filter(|r| {
                                r.reason.is_none()
                                    && r.output_missing == 0
                                    && r.input_missing == 0
                                    && r.rtl.iter().all(Option::is_some)
                                    && r.min_lead_ms.is_some_and(|lead| lead >= 16)
                            })
                            .count();
                        let deferred: usize = results.iter().map(|r| r.deferred).sum();
                        let leads: Vec<_> = results.iter().filter_map(|r| r.min_lead_ms).collect();
                        let reasons: std::collections::BTreeSet<_> =
                            results.iter().filter_map(|r| r.reason).collect();
                        let rtls: Vec<_> = results.iter().flat_map(|r| r.rtl).flatten().collect();
                        print!("{comma}{{\"profile\":\"{name}\",\"ready_policy\":{ready_policy},\"owner_step_frames\":{owner_step},\"block_ms\":{block_ms},\"depth\":{depth},\"output_safety\":{output},\"buffer\":{buffer},\"stress\":\"{}\",\"phases\":{},\"passed\":{passed},\"accepted\":{accepted},\"deferrals\":{deferred},\"max_output_missing\":{},\"max_input_missing\":{},\"min_submit_ms\":{},\"raw_rtl_frames\":{:?},\"reasons\":{:?}}}",stress.name,results.len(),results.iter().map(|r|r.output_missing).max().unwrap(),results.iter().map(|r|r.input_missing).max().unwrap(),leads.iter().min().map_or("null".into(),|v|v.to_string()),rtls,reasons.into_iter().collect::<Vec<_>>());
                        comma = ",\n";
                    }
                }
            }
        }
    }
    println!("\n]}}");
}
fn retry_cost_study() {
    println!("{{\"schema\":1,\"hardware_qualification\":false,\"rows\":[");
    let mut comma = "";
    for buffer in [64, 512, 4096] {
        let g = Geometry {
            block_ms: 4,
            depth: 10,
            input: 1024,
            output: 2560,
            buffer,
            pcm_offset: REPLAY,
            anchor_bias: 48,
        };
        for ready in [false, true] {
            for cadence in [4 * 48, 8 * 48] {
                for cost in [0, 48, 96] {
                    for stall in [0, 384, 600] {
                        let stress = Stress {
                            submit: cost,
                            owner_stall: stall,
                            cost_after: BASE + RATE,
                            ..CLEAN
                        };
                        let mut accepted = 0;
                        let mut faults = 0;
                        let mut missing_in = 0;
                        let mut missing_out = 0;
                        let mut lead = usize::MAX;
                        for phase in phases(g) {
                            let r = run_with_cadence(g, stress, phase, ready, cadence);
                            accepted += usize::from(
                                r.reason.is_none()
                                    && r.input_missing == 0
                                    && r.output_missing == 0
                                    && r.min_lead_ms.is_some_and(|n| n >= 16)
                                    && r.rtl.iter().all(Option::is_some),
                            );
                            faults += usize::from(r.reason.is_some());
                            missing_in = missing_in.max(r.input_missing);
                            missing_out = missing_out.max(r.output_missing);
                            if let Some(n) = r.min_lead_ms {
                                lead = lead.min(n);
                            }
                        }
                        print!("{comma}{{\"buffer\":{buffer},\"ready_policy\":{ready},\"owner_step_frames\":{cadence},\"submit_frames_each_direction\":{cost},\"stall_frames\":{stall},\"phases\":8,\"accepted\":{accepted},\"faults\":{faults},\"max_input_missing\":{missing_in},\"max_output_missing\":{missing_out},\"min_returned_lead_bus_frames\":{lead}}}");
                        comma = ",\n";
                    }
                }
            }
        }
    }
    println!("\n]}}");
}
fn deadline_cost_study() {
    println!("{{\"schema\":1,\"hardware_qualification\":false,\"rows\":[");
    let mut comma = "";
    for buffer in [64, 512, 4096] {
        let g = Geometry {
            block_ms: 4,
            depth: 10,
            input: 1024,
            output: 2560,
            buffer,
            pcm_offset: REPLAY,
            anchor_bias: 48,
        };
        for deadline in [false, true] {
            for cadence in [4 * 48, 8 * 48] {
                for cost in [0, 48, 96] {
                    for stall in [0, 384, 600] {
                        let stress = Stress {
                            submit: cost,
                            owner_stall: stall,
                            cost_after: BASE + RATE,
                            ..CLEAN
                        };
                        let mut accepted = 0;
                        let mut faults = 0;
                        let mut missing_in = 0;
                        let mut missing_out = 0;
                        let mut lead = usize::MAX;
                        for phase in phases(g) {
                            let r = run_with_order(g, stress, phase, true, cadence, deadline);
                            assert!(!deadline || r.max_burst <= 10);
                            accepted += usize::from(
                                r.reason.is_none()
                                    && r.input_missing == 0
                                    && r.output_missing == 0
                                    && r.min_output_lead_ms.is_some_and(|n| n >= 16)
                                    && r.rtl.iter().all(Option::is_some),
                            );
                            faults += usize::from(r.reason.is_some());
                            missing_in = missing_in.max(r.input_missing);
                            missing_out = missing_out.max(r.output_missing);
                            if let Some(n) = r.min_output_lead_ms {
                                lead = lead.min(n);
                            }
                        }
                        print!("{comma}{{\"buffer\":{buffer},\"deadline_order\":{deadline},\"ready_policy\":true,\"owner_step_frames\":{cadence},\"submit_frames_each_direction\":{cost},\"stall_frames\":{stall},\"phases\":8,\"accepted\":{accepted},\"faults\":{faults},\"max_input_missing\":{missing_in},\"max_output_missing\":{missing_out},\"min_returned_lead_bus_frames\":{lead}}}");
                        comma = ",\n";
                    }
                }
            }
        }
    }
    println!("\n]}}");
}
fn dispatch_study() {
    println!(
        "{{\"schema\":1,\"hardware_qualification\":false,\"callback_cost_frames\":3,\"rows\":["
    );
    let mut comma = "";
    for buffer in [64, 512, 4096] {
        let g = Geometry {
            block_ms: 4,
            depth: 10,
            input: 1024,
            output: 2560,
            buffer,
            pcm_offset: REPLAY,
            anchor_bias: 48,
        };
        for ready_first in [false, true] {
            for cadence in [4 * 48, 8 * 48] {
                for cost in [0, 48, 96] {
                    for stall in [0, 384, 600] {
                        let stress = Stress {
                            submit: cost,
                            owner_stall: stall,
                            cost_after: BASE + RATE,
                            ..CLEAN
                        };
                        let cfg = Dispatch {
                            ready_first,
                            ..DISPATCH
                        };
                        let mut accepted = 0;
                        let mut faults = 0;
                        let mut missing_in = 0;
                        let mut missing_out = 0;
                        let mut lead = usize::MAX;
                        let (mut waits, mut callbacks, mut max_batch) = (0, 0, 0);
                        let mut coordinates_unchanged = true;
                        for phase in phases(g) {
                            let r =
                                run_with_dispatch(g, stress, phase, true, cadence, true, Some(cfg));
                            assert!(r.max_burst <= 10 && r.dispatch.max_batch <= 10);
                            assert_eq!(
                                r.dispatch.callback_frames,
                                r.dispatch.callbacks * cfg.callback_cost
                            );
                            assert_eq!(r.dispatch.idle_frames, r.dispatch.waits * cadence);
                            let expected_rtl = [Some(2 * buffer + 1024 + 2560 + ANALOG); 2];
                            let passed = r.reason.is_none()
                                && r.input_missing == 0
                                && r.output_missing == 0
                                && r.min_output_lead_ms.is_some_and(|n| n >= 16)
                                && r.rtl == expected_rtl;
                            accepted += usize::from(passed);
                            coordinates_unchanged &= r
                                .rtl
                                .iter()
                                .flatten()
                                .all(|&n| n == expected_rtl[0].unwrap());
                            faults += usize::from(r.reason.is_some());
                            missing_in = missing_in.max(r.input_missing);
                            missing_out = missing_out.max(r.output_missing);
                            if let Some(n) = r.min_output_lead_ms {
                                lead = lead.min(n);
                            }
                            waits += r.dispatch.waits;
                            callbacks += r.dispatch.callbacks;
                            max_batch = max_batch.max(r.dispatch.max_batch);
                        }
                        assert!(coordinates_unchanged);
                        print!("{comma}{{\"buffer\":{buffer},\"ready_first\":{ready_first},\"idle_frames\":{cadence},\"submit_frames_each_direction\":{cost},\"stall_frames\":{stall},\"phases\":8,\"accepted\":{accepted},\"faults\":{faults},\"max_input_missing\":{missing_in},\"max_output_missing\":{missing_out},\"min_returned_lead_bus_frames\":{lead},\"idle_calls\":{waits},\"callbacks\":{callbacks},\"max_batch\":{max_batch},\"coordinates_unchanged\":{coordinates_unchanged}}}");
                        comma = ",\n";
                    }
                }
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
    fn separate_dispatch_retains_marker_coordinates_and_negative_controls() {
        for buffer in [64, 512, 4096] {
            let g = Geometry {
                block_ms: 4,
                depth: 10,
                buffer,
                ..geometry(2560)
            };
            for phase in phases(g) {
                for cadence in [192, 384] {
                    let clean =
                        run_with_dispatch(g, CLEAN, phase, true, cadence, true, Some(DISPATCH));
                    assert_eq!(clean.reason, None);
                    assert_eq!((clean.input_missing, clean.output_missing), (0, 0));
                    assert_eq!(clean.rtl, [Some(2 * buffer + 1024 + 2560 + ANALOG); 2]);
                    assert!(clean.min_output_lead_ms.unwrap() >= 16);
                    assert!(clean.max_burst <= 10 && clean.dispatch.max_batch <= 10);
                    assert_eq!(clean.dispatch.callback_frames, clean.dispatch.callbacks * 3);
                    let late_hal = run_with_dispatch(
                        g,
                        Stress {
                            hal_delay: buffer + 832,
                            ..CLEAN
                        },
                        phase,
                        true,
                        cadence,
                        true,
                        Some(DISPATCH),
                    );
                    assert_eq!(late_hal.reason, None);
                    assert_eq!((late_hal.input_missing, late_hal.output_missing), (0, 0));
                    assert_eq!(late_hal.rtl, clean.rtl);
                    assert!(late_hal.min_output_lead_ms.unwrap() >= 16);
                    for stress in [
                        Stress {
                            hole: true,
                            ..CLEAN
                        },
                        Stress {
                            owner_stall: 32 * 48,
                            ..CLEAN
                        },
                        Stress {
                            submit: 96,
                            cost_after: BASE + RATE,
                            ..CLEAN
                        },
                    ] {
                        let r = run_with_dispatch(
                            g,
                            stress,
                            phase,
                            true,
                            cadence,
                            true,
                            Some(DISPATCH),
                        );
                        assert!(
                            r.reason.is_some()
                                || r.output_missing > 0
                                || r.input_missing > 0
                                || r.min_output_lead_ms.is_none_or(|n| n < 16)
                        );
                        if stress.hole {
                            assert_eq!(r.output_missing, 64);
                        }
                    }
                }
            }
        }
    }
    fn ready_slots() -> [Vec<Option<Flight>>; 2] {
        array::from_fn(|_| {
            (0..10)
                .map(|seq| {
                    Some(Flight {
                        seq,
                        start: 0,
                        callback: 10,
                        delivered: false,
                        serviced: true,
                        payload: vec![],
                    })
                })
                .collect()
        })
    }
    #[test]
    fn model_pump_separates_readiness_delivery_and_idle_without_spin() {
        for ready_first in [false, true] {
            let mut slots = ready_slots();
            let mut p = Pump::default();
            let mut stats = DispatchStats::default();
            let cfg = Dispatch {
                ready_first,
                ..DISPATCH
            };
            // Ready packets stay invisible until individual service finishes.
            assert!(!p.step(10, 384, cfg, &mut slots, &mut stats));
            assert!(slots.iter().flatten().flatten().all(|f| !f.delivered));
            let finish = 10 + if ready_first { 0 } else { 384 } + 30;
            for now in 11..=finish {
                let done = p.step(now, 384, cfg, &mut slots, &mut stats);
                assert_eq!(done, now == finish);
            }
            assert_eq!(stats.callbacks, 10);
            assert_eq!(stats.waits, usize::from(!ready_first));
            assert_eq!(stats.max_batch, 10);
        }
        let mut slots: [Vec<Option<Flight>>; 2] = array::from_fn(|_| vec![]);
        let mut p = Pump::default();
        let mut stats = DispatchStats::default();
        assert!(!p.step(0, 384, DISPATCH, &mut slots, &mut stats));
        assert!(!p.step(383, 384, DISPATCH, &mut slots, &mut stats));
        assert!(p.step(384, 384, DISPATCH, &mut slots, &mut stats));
        assert!(!p.step(384, 384, DISPATCH, &mut slots, &mut stats));
        assert_eq!(stats.waits, 2);
        assert_eq!(stats.callbacks, 0);
    }
    #[test]
    fn model_callback_time_budget_is_cooperative_and_entry_cost_is_independent() {
        for (cost, count) in [(0, 10), (3, 10), (48, 2), (144, 1)] {
            let mut slots = ready_slots();
            let mut p = Pump::default();
            let mut stats = DispatchStats::default();
            let cfg = Dispatch {
                callback_cost: cost,
                entry_cost: 48,
                ..DISPATCH
            };
            let finish = 10 + 48 + cost * count;
            for now in 10..=finish {
                assert_eq!(p.step(now, 384, cfg, &mut slots, &mut stats), now == finish);
            }
            assert_eq!(stats.callbacks, count);
            assert_eq!(stats.entry_frames, 48);
            assert_eq!(stats.waits, 0);
        }
    }
    #[test]
    fn deadline_order_preserves_coordinates_and_negative_controls() {
        for buffer in [64, 512, 4096] {
            let g = Geometry {
                block_ms: 4,
                depth: 10,
                buffer,
                ..geometry(2560)
            };
            for phase in phases(g) {
                for cadence in [192, 384] {
                    let clean = run_with_order(g, CLEAN, phase, true, cadence, true);
                    assert_eq!(clean.reason, None);
                    assert_eq!((clean.input_missing, clean.output_missing), (0, 0));
                    assert_eq!(clean.rtl, [Some(2 * buffer + 1024 + 2560 + ANALOG); 2]);
                    assert!(clean.min_output_lead_ms.unwrap() >= 16);
                    assert!(clean.max_burst <= 10);
                    let hole = run_with_order(
                        g,
                        Stress {
                            hole: true,
                            ..CLEAN
                        },
                        phase,
                        true,
                        cadence,
                        true,
                    );
                    assert_eq!(hole.reason, None);
                    assert_eq!(hole.output_missing, 64);
                    let stall = run_with_order(
                        g,
                        Stress {
                            owner_stall: 32 * 48,
                            ..CLEAN
                        },
                        phase,
                        true,
                        cadence,
                        true,
                    );
                    assert!(
                        stall.reason.is_some()
                            || stall.output_missing > 0
                            || stall.input_missing > 0
                            || stall.min_output_lead_ms.is_none_or(|n| n < 16)
                    );
                }
            }
        }
    }
    #[test]
    fn readiness_preserves_latency_and_does_not_mask_a_permanent_hole() {
        for buffer in [64, 512, 4096] {
            let g = Geometry {
                block_ms: 4,
                depth: 10,
                buffer,
                ..geometry(2560)
            };
            for phase in phases(g) {
                let before = run(g, stresses(buffer)[3], phase);
                assert!(before.output_missing > 0);
                for cadence in [1, 192, 384] {
                    let after = run_with_cadence(g, stresses(buffer)[3], phase, true, cadence);
                    assert_eq!(after.reason, None);
                    assert_eq!((after.input_missing, after.output_missing), (0, 0));
                    assert!(after.min_lead_ms.unwrap() >= 16);
                    assert_eq!(after.rtl, [Some(2 * buffer + 1024 + 2560 + ANALOG); 2]);
                    assert!(after.deferred > 0);
                    let hole = run_with_cadence(
                        g,
                        Stress {
                            hole: true,
                            ..CLEAN
                        },
                        phase,
                        true,
                        cadence,
                    );
                    assert_eq!(hole.reason, None);
                    assert_eq!(hole.output_missing, 64);
                    assert!(hole.deadline_fallbacks > 0);
                }
            }
        }
    }
    #[test]
    fn coarse_owner_wakes_do_not_hide_a_combined_stall_failure() {
        // A 12.5 ms stall plus an 8 ms polling cadence exceeds the retry
        // allowance. Keep this observed boundary a rejection, not a new gate.
        for buffer in [64, 512, 4096] {
            let g = Geometry {
                block_ms: 4,
                depth: 10,
                buffer,
                ..geometry(2560)
            };
            let mut rejected = 0;
            for phase in phases(g) {
                let r = run_with_cadence(g, stresses(buffer)[5], phase, true, 384);
                if r.reason.is_some() {
                    rejected += 1;
                    assert_eq!(r.reason, Some("output_lead_deadline"));
                } else {
                    assert!(r.min_lead_ms.unwrap() >= 16);
                    assert_eq!((r.input_missing, r.output_missing), (0, 0));
                }
            }
            assert_eq!(rejected, 1);
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
    fn shorter_batches_do_not_reduce_rtl_without_changing_reference_offsets() {
        for depth in [6, 7, 8, 10] {
            for buffer in [64, 512, 4096] {
                let g = Geometry {
                    block_ms: 4,
                    depth,
                    buffer,
                    ..geometry(2560)
                };
                for phase in phases(g) {
                    let r = run(g, CLEAN, phase);
                    assert_eq!(r.reason, None);
                    assert_eq!((r.input_missing, r.output_missing), (0, 0));
                    assert_eq!(r.rtl, [Some(2 * buffer + 1024 + 2560 + ANALOG); 2]);
                    assert_eq!(r.captured, r.replayed);
                }
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
