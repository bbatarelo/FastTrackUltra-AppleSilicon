//! Fixed-memory transport state; the caller supplies a bounded diagnostic journal.
//! A slot is reusable only after its matching generation's completion is consumed.
use crate::{
    clock::SampleClock,
    native::*,
    ok,
    scheduler::*,
    worker_timing::{self as wt, Event as WorkerEvent},
};
use std::{
    ffi::c_void,
    time::{Duration, Instant},
};
pub const FEEDBACK_SLOTS: usize = 32;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct PoolCompletion {
    pub ticket: u64,
    pub result: Completion,
}
#[repr(C)]
pub struct PoolOps {
    pub base: Ops,
    pub submit: unsafe extern "C" fn(*mut c_void, u32, u32, u64, u64, *const u16) -> i32,
    pub completion: unsafe extern "C" fn(*mut c_void, u32, u32, *mut PoolCompletion) -> i32,
    pub now_ticks: unsafe extern "C" fn(*mut c_void) -> u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Flight {
    pub seq: usize,
    pub ticket: u64,
    pub start: u64,
}
#[derive(Clone, Debug)]
pub struct Trace {
    pub flight: Option<Flight>,
    pub slot: usize,
    pub submitted_at: u64,
    pub admission_ticks: u64,
    pub submit: Option<i32>,
    pub submit_return_frame: u64,
    pub completion: Option<PoolCompletion>,
    pub lengths: [u16; ENTRIES],
    pub first_sample: u64,
    pub crc_before: u32,
    pub crc_after: u32,
    pub peaks: [u32; 8],
    pub nonzero: [u32; 8],
}
impl Default for Trace {
    fn default() -> Self {
        Self {
            flight: None,
            slot: 0,
            submitted_at: 0,
            admission_ticks: 0,
            submit: None,
            submit_return_frame: 0,
            completion: None,
            lengths: [0; ENTRIES],
            first_sample: 0,
            crc_before: 0,
            crc_after: 0,
            peaks: [0; 8],
            nonzero: [0; 8],
        }
    }
}
#[derive(Clone, Copy)]
struct Feedback {
    seq: usize,
    completion: Completion,
    consumed_ticks: u64,
}
const fn crc_table() -> [u32; 256] {
    let mut t = [0; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 {
                0xedb88320 ^ (c >> 1)
            } else {
                c >> 1
            };
            j += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
}
const CRC_TABLE: [u32; 256] = crc_table();
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = u32::MAX;
    for &b in data {
        c = CRC_TABLE[((c as u8) ^ b) as usize] ^ (c >> 8);
    }
    !c
}
/// Precomputed outside activation; copying samples performs no trig or allocation.
pub fn tone_table() -> Vec<[u8; 6]> {
    (0..96000)
        .map(|f| {
            let mut p = [0; 6];
            p[..3].copy_from_slice(&ftu_protocol::encode_s24(tone_sample(f, 0)));
            p[3..].copy_from_slice(&ftu_protocol::encode_s24(tone_sample(f, 1)));
            p
        })
        .collect()
}
/// The diagnostic keeps every record; a live session recycles a fixed journal.
pub struct Journal<'a> {
    pub records: &'a mut [Vec<Trace>; 2],
    pub recycle: bool,
}
impl Journal<'_> {
    fn record(&mut self, direction: usize, seq: usize) -> &mut Trace {
        let at = if self.recycle {
            seq % self.records[direction].len()
        } else {
            seq
        };
        &mut self.records[direction][at]
    }
}
/// Hooks run only on the transport owner thread, never after buffer submission.
/// Capture must be copied here before the native slot is reused; ordered delivery
/// later supplies its absolute sample cursor. Implementations preallocate storage.
pub trait StreamIo {
    fn worker_timing(&self) -> bool {
        false
    }
    fn worker_event(&mut self, _event: WorkerEvent) {}
    /// Optional observation on the USB owner thread. It never changes scheduling
    /// or turns a diagnostic clock-read error into a transport failure.
    fn submission_timing(&self) -> bool {
        false
    }
    fn submitted(&mut self, _start: u64, _checked: u64, _returned: Option<u64>) {}
    fn progress(&mut self, _submitted: [usize; 2], _completed: [usize; 2], _pending: [usize; 2]) {}
    fn capture(
        &mut self,
        _seq: usize,
        _completion: &Completion,
        _bytes: &[u8],
    ) -> Result<(), &'static str> {
        Ok(())
    }
    fn ordered(
        &mut self,
        _seq: usize,
        _first: u64,
        _completion: &Completion,
    ) -> Result<(), &'static str> {
        Ok(())
    }
    fn clock(&mut self, _clock: crate::clock::ClockSnapshot) -> Result<(), &'static str> {
        Ok(())
    }
    fn output(
        &mut self,
        _first: u64,
        _frames: usize,
        _bytes: &mut [u8],
    ) -> Result<(), &'static str> {
        Ok(())
    }
}
struct ToneOutput<'a>(&'a [[u8; 6]]);
impl StreamIo for ToneOutput<'_> {
    fn output(&mut self, first: u64, frames: usize, bytes: &mut [u8]) -> Result<(), &'static str> {
        if !self.0.is_empty() {
            for frame in 0..frames {
                let sample = self.0[(first as usize + frame) % self.0.len()];
                bytes[frame * 24..frame * 24 + 6].copy_from_slice(&sample);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
pub struct SubmissionFailure {
    pub status: i32,
    pub direction: u32,
    pub sequence: u64,
    pub start: u64,
    pub checked: u64,
    pub returned: u64,
}
#[derive(Clone, Copy)]
struct PreviousCompletion {
    sequence: u64,
    start: u64,
    last_packet: u64,
    callback: u64,
    consumed: u64,
}
pub struct PoolSession {
    previous: [[Option<PreviousCompletion>; DEPTH]; 2],
    // Immutable session policy; storage capacity and the legacy probe stay at DEPTH.
    depth: usize,
    pub submission_failure: Option<SubmissionFailure>,
    slots: [[Option<Flight>; DEPTH]; 2],
    feedback: [Option<Feedback>; FEEDBACK_SLOTS],
    pub next: [usize; 2],
    pub completed: [usize; 2],
    pub frames: [u64; 2],
    pub max_pending: [usize; 2],
    pub reused: [usize; 2],
    pub reason: Option<&'static str>,
    pub clock: SampleClock,
    pub anchor: Clock,
    pub info: Info,
    clock_seq: usize,
    clock_frames: u64,
    pub next_sample: u64,
    pub drain_failed: bool,
    pub abort_status: [Option<i32>; 2],
    epoch: u64,
}
impl PoolSession {
    pub fn new(epoch: u64, info: Info) -> Self {
        Self::with_depth(epoch, info, DEPTH)
    }
    pub fn with_depth(epoch: u64, info: Info, depth: usize) -> Self {
        assert!((2..=DEPTH).contains(&depth), "invalid queue depth");
        Self {
            depth,
            previous: [[None; DEPTH]; 2],
            submission_failure: None,
            slots: [[None; DEPTH]; 2],
            feedback: [None; FEEDBACK_SLOTS],
            next: [0; 2],
            completed: [0; 2],
            frames: [0; 2],
            max_pending: [0; 2],
            reused: [0; 2],
            reason: None,
            clock: SampleClock::new(epoch, info.timebase_numer, info.timebase_denom),
            anchor: Clock::default(),
            info,
            clock_seq: 0,
            clock_frames: 0,
            next_sample: 0,
            drain_failed: false,
            abort_status: [None; 2],
            epoch,
        }
    }
    pub fn queue_depth(&self) -> usize {
        self.depth
    }
    pub fn stop(&mut self, r: &'static str) {
        if self.reason.is_none() {
            self.reason = Some(r);
        }
    }
    pub fn pending(&self) -> [usize; 2] {
        self.slots.map(|a| a.iter().filter(|x| x.is_some()).count())
    }
    unsafe fn poll(
        &mut self,
        ops: &PoolOps,
        ctx: *mut c_void,
        records: &mut Journal<'_>,
        io: &mut impl StreamIo,
    ) {
        for d in 0..2 {
            for slot in 0..self.depth {
                let Some(flight) = self.slots[d][slot] else {
                    continue;
                };
                let mut completion = PoolCompletion::default();
                let got = (ops.completion)(ctx, d as u32, slot as u32, &mut completion);
                let consumed_ticks = if got == 1 && io.worker_timing() {
                    (ops.now_ticks)(ctx)
                } else {
                    0
                };
                if got == 0 {
                    continue;
                }
                if got != 1 {
                    self.stop("native_completion_error");
                    continue;
                }
                if completion.ticket != flight.ticket {
                    self.stop("stale_completion_ticket");
                    continue;
                }
                if io.worker_timing() {
                    self.previous[d][slot] = Some(PreviousCompletion {
                        sequence: flight.seq as u64,
                        start: flight.start,
                        last_packet: completion.result.packets[ENTRIES - 1].ticks,
                        callback: completion.result.callback_ticks,
                        consumed: consumed_ticks,
                    });
                }
                self.slots[d][slot] = None;
                self.completed[d] += 1;
                let record = records.record(d, flight.seq);
                record.completion = Some(completion);
                let c = completion.result;
                if c.matched != 1 || !ok(c.status) {
                    self.stop("request_completion_error");
                    continue;
                }
                if !c.packets.iter().enumerate().all(|(i, p)| {
                    ok(p.status)
                        && p.requested == record.lengths[i]
                        && if d == 0 {
                            valid_length(p.actual)
                        } else {
                            p.actual == p.requested
                        }
                }) {
                    self.stop("packet_completion_error");
                    continue;
                }
                self.frames[d] += c
                    .packets
                    .iter()
                    .map(|p| u64::from(p.actual / 24))
                    .sum::<u64>();
                if d == 1 {
                    let ptr = (ops.base.buffer)(ctx, 1, slot as u32);
                    if ptr.is_null() {
                        self.stop("completed_buffer_unavailable");
                        continue;
                    }
                    record.crc_after = crc32(std::slice::from_raw_parts(ptr, BLOCK_BYTES));
                    if record.crc_after != record.crc_before {
                        self.stop("output_payload_changed");
                    }
                } else if self.reason.is_none() {
                    let at = flight.seq % FEEDBACK_SLOTS;
                    if self.feedback[at].is_some() {
                        self.stop("feedback_overrun");
                        continue;
                    }
                    let ptr = (ops.base.buffer)(ctx, 0, slot as u32);
                    if ptr.is_null() {
                        self.stop("capture_buffer_unavailable");
                        continue;
                    }
                    if let Err(r) =
                        io.capture(flight.seq, &c, std::slice::from_raw_parts(ptr, BLOCK_BYTES))
                    {
                        self.stop(r);
                        continue;
                    }
                    self.feedback[at] = Some(Feedback {
                        seq: flight.seq,
                        completion: c,
                        consumed_ticks,
                    });
                }
            }
        }
        (ops.base.info)(ctx, &mut self.info);
        if self.info.callback_fault != 0 {
            self.stop("native_callback_fault");
        }
        while self.reason.is_none() {
            let Some(f) = self.feedback[self.clock_seq % FEEDBACK_SLOTS] else {
                break;
            };
            if f.seq != self.clock_seq {
                break;
            }
            records.record(0, f.seq).first_sample = self.clock_frames;
            if let Err(r) = io.ordered(f.seq, self.clock_frames, &f.completion) {
                self.stop(r);
                break;
            }
            for group in f.completion.packets.chunks_exact(8) {
                let ticks = std::array::from_fn(|i| group[i].ticks);
                if let Err(r) = self
                    .clock
                    .observe_usb_group(self.epoch, self.clock_frames, &ticks)
                {
                    self.stop(r);
                    break;
                }
                if let Err(r) = io.clock(self.clock.snapshot()) {
                    self.stop(r);
                    break;
                }
                self.clock_frames += group.iter().map(|p| u64::from(p.actual / 24)).sum::<u64>();
            }
            self.clock_seq += 1;
        }
        io.progress(self.next, self.completed, self.pending());
    }
    pub unsafe fn run(
        &mut self,
        ops: &PoolOps,
        ctx: *mut c_void,
        records: &mut [Vec<Trace>; 2],
        tone: &[[u8; 6]],
        stop: *const i32,
    ) {
        let count = records[0].len();
        if records[1].len() != count || count == 0 {
            self.stop("invalid_journal");
            return;
        }
        self.run_with(
            ops,
            ctx,
            &mut Journal {
                records,
                recycle: false,
            },
            &mut ToneOutput(tone),
            Some(count),
            &|| std::ptr::read_volatile(stop) != 0,
        );
    }
    pub unsafe fn run_with(
        &mut self,
        ops: &PoolOps,
        ctx: *mut c_void,
        records: &mut Journal<'_>,
        io: &mut impl StreamIo,
        limit: Option<usize>,
        stopped: &impl Fn() -> bool,
    ) {
        if records.records.iter().any(|r| r.is_empty())
            || (records.recycle && records.records.iter().any(|r| r.len() < FEEDBACK_SLOTS))
        {
            self.stop("invalid_journal");
            return;
        }
        let r = (ops.base.clock)(ctx, &mut self.anchor);
        if r != 0 {
            self.stop("clock_query_failed");
            return;
        }
        let Some(base) = self.anchor.frame.checked_add(LEAD_MS) else {
            self.stop("clock_overflow");
            return;
        };
        let observe_worker = io.worker_timing();
        let mut pump_times = None;
        let began = Instant::now();
        let mut last_progress = began;
        let mut last_completed = self.completed;
        while self.reason.is_none() && limit.is_none_or(|count| self.completed[1] < count) {
            if stopped() {
                self.stop("signal");
                break;
            }
            if limit.is_some_and(|count| {
                began.elapsed()
                    > Duration::from_millis(count as u64 * 8 + LEAD_MS + FEEDBACK_DELAY_MS + 1000)
            }) {
                self.stop("stream_deadline");
                break;
            }
            let loop_enter_ticks = if observe_worker {
                (ops.now_ticks)(ctx)
            } else {
                0
            };
            self.poll(ops, ctx, records, io);
            let poll_end_ticks = if observe_worker {
                (ops.now_ticks)(ctx)
            } else {
                0
            };
            if self.completed != last_completed {
                last_completed = self.completed;
                last_progress = Instant::now();
            } else if last_progress.elapsed() > Duration::from_millis(250) {
                self.stop("completion_deadline");
            }
            for d in 0..2 {
                while limit.is_none_or(|count| self.next[d] < count) && self.reason.is_none() {
                    let seq = self.next[d];
                    let slot = seq % self.depth;
                    if self.slots[d][slot].is_some() {
                        break;
                    }
                    let feedback = if d == 1 {
                        let Some(f) = self.feedback[seq % FEEDBACK_SLOTS] else {
                            break;
                        };
                        if f.seq != seq || self.clock_seq <= seq {
                            break;
                        }
                        Some(f)
                    } else {
                        None
                    };
                    let Some(start) = start_frame(base, seq, d == 1) else {
                        self.stop("schedule_overflow");
                        break;
                    };
                    let mut event = WorkerEvent {
                        direction: d as u64,
                        sequence: seq as u64,
                        slot: slot as u64,
                        ticket: (seq as u64).saturating_add(1),
                        start_frame: start,
                        loop_enter_ticks,
                        poll_end_ticks,
                        ..WorkerEvent::default()
                    };
                    if observe_worker {
                        if let Some(p) = self.previous[d][slot] {
                            event.flags |= wt::PREVIOUS;
                            event.previous_sequence = p.sequence;
                            event.previous_start_frame = p.start;
                            event.previous_last_packet_ticks = p.last_packet;
                            event.previous_callback_ticks = p.callback;
                            event.previous_consumed_ticks = p.consumed;
                        }
                        if let Some(f) = feedback {
                            event.flags |= wt::FEEDBACK;
                            event.feedback_sequence = f.seq as u64;
                            event.feedback_last_packet_ticks =
                                f.completion.packets[ENTRIES - 1].ticks;
                            event.feedback_callback_ticks = f.completion.callback_ticks;
                            event.feedback_consumed_ticks = f.consumed_ticks;
                        }
                        if let Some((begin, end)) = pump_times {
                            event.flags |= wt::PUMP;
                            event.pump_enter_ticks = begin;
                            event.pump_exit_ticks = end;
                        }
                        event.check_begin_ticks = (ops.now_ticks)(ctx);
                    }
                    let mut now = Clock::default();
                    let clock_result = (ops.base.clock)(ctx, &mut now);
                    if observe_worker {
                        event.check_end_ticks = (ops.now_ticks)(ctx);
                        event.check_status = clock_result as i64 as u64;
                        if clock_result == 0 {
                            event.flags |= wt::CHECKED;
                            event.checked_frame = now.frame;
                        }
                    }
                    if clock_result != 0 || !schedule_valid(now.frame, start) {
                        if observe_worker {
                            io.worker_event(event);
                        }
                        self.submission_failure = Some(SubmissionFailure {
                            status: clock_result,
                            direction: d as u32,
                            sequence: seq as u64,
                            start,
                            checked: now.frame,
                            returned: now.frame,
                        });
                        self.stop(if clock_result != 0 {
                            "clock_query_failed"
                        } else {
                            "stale_schedule"
                        });
                        break;
                    }
                    if stopped() {
                        self.stop("signal");
                        break;
                    }
                    let record = records.record(d, seq);
                    *record = Trace::default();
                    record.slot = slot;
                    record.submitted_at = now.frame;
                    record.lengths = if let Some(f) = feedback {
                        f.completion.packets.map(|p| p.actual)
                    } else {
                        [312; ENTRIES]
                    };
                    record.first_sample = if d == 1 { self.next_sample } else { 0 };
                    if d == 1 {
                        let ptr = (ops.base.buffer)(ctx, 1, slot as u32);
                        if ptr.is_null() {
                            self.stop("output_buffer_unavailable");
                            break;
                        }
                        let bytes = std::slice::from_raw_parts_mut(ptr, BLOCK_BYTES);
                        bytes.fill(0);
                        let frames = record
                            .lengths
                            .iter()
                            .map(|&n| usize::from(n / 24))
                            .sum::<usize>();
                        if let Err(r) = io.output(self.next_sample, frames, bytes) {
                            self.stop(r);
                            break;
                        }
                        for frame in 0..frames {
                            for ch in 0..8 {
                                let at = frame * 24 + ch * 3;
                                let v =
                                    ftu_protocol::decode_s24(bytes[at..at + 3].try_into().unwrap())
                                        .unsigned_abs();
                                record.peaks[ch] = record.peaks[ch].max(v);
                                record.nonzero[ch] += u32::from(v != 0);
                            }
                        }
                        record.crc_before = crc32(bytes);
                        self.next_sample += frames as u64;
                    }
                    if observe_worker {
                        event.flags |= wt::PREPARED;
                        event.prepare_end_ticks = (ops.now_ticks)(ctx);
                    }
                    // Tickets identify sequence within this epoch; native callback tokens
                    // additionally remain unique across context lifetimes on
                    // their owning callback/run-loop thread.
                    let Some(ticket) = (seq as u64).checked_add(1) else {
                        self.stop("sequence_overflow");
                        break;
                    };
                    let flight = Flight { seq, ticket, start };
                    record.flight = Some(flight);
                    record.admission_ticks = (ops.now_ticks)(ctx);
                    if observe_worker {
                        event.flags |= wt::SUBMITTED;
                        event.submit_begin_ticks = record.admission_ticks;
                    }
                    let r = (ops.submit)(
                        ctx,
                        d as u32,
                        slot as u32,
                        ticket,
                        start,
                        record.lengths.as_ptr(),
                    );
                    if observe_worker {
                        event.submit_end_ticks = (ops.now_ticks)(ctx);
                        event.submit_status = r as i64 as u64;
                    }
                    record.submit = Some(r);
                    if r != 0 {
                        let mut returned = Clock::default();
                        let query = (ops.base.clock)(ctx, &mut returned);
                        if query == 0 {
                            record.submit_return_frame = returned.frame;
                        }
                        if observe_worker {
                            event.flags |= wt::QUERIED;
                            event.query_end_ticks = (ops.now_ticks)(ctx);
                            event.return_clock_status = query as i64 as u64;
                            if query == 0 {
                                event.flags |= wt::RETURNED;
                                event.returned_frame = returned.frame;
                            }
                            io.worker_event(event);
                        }
                        self.submission_failure = Some(SubmissionFailure {
                            status: r,
                            direction: d as u32,
                            sequence: seq as u64,
                            start,
                            checked: now.frame,
                            returned: record.submit_return_frame,
                        });
                        self.stop("submit_failed");
                        break;
                    }
                    self.slots[d][slot] = Some(flight);
                    self.next[d] += 1;
                    if seq >= self.depth {
                        self.reused[d] += 1;
                    }
                    self.max_pending[d] = self.max_pending[d].max(self.pending()[d]);
                    if d == 1 {
                        self.feedback[seq % FEEDBACK_SLOTS] = None;
                        if io.submission_timing() || observe_worker {
                            let mut returned = Clock::default();
                            let query = (ops.base.clock)(ctx, &mut returned);
                            if query == 0 {
                                record.submit_return_frame = returned.frame;
                            }
                            if observe_worker {
                                event.flags |= wt::QUERIED;
                                event.query_end_ticks = (ops.now_ticks)(ctx);
                                event.return_clock_status = query as i64 as u64;
                                if query == 0 {
                                    event.flags |= wt::RETURNED;
                                    event.returned_frame = returned.frame;
                                }
                            }
                            if io.submission_timing() {
                                io.submitted(
                                    start,
                                    now.frame,
                                    (query == 0).then_some(returned.frame),
                                );
                            }
                        }
                    }
                    if observe_worker {
                        io.worker_event(event);
                    }
                }
            }
            if self.pending().iter().any(|&n| n > 0) {
                let begin = if observe_worker {
                    (ops.now_ticks)(ctx)
                } else {
                    0
                };
                (ops.base.pump)(ctx);
                if observe_worker {
                    pump_times = Some((begin, (ops.now_ticks)(ctx)));
                }
            } else if limit.is_none_or(|count| self.completed[1] < count) && self.reason.is_none() {
                self.stop("pipeline_stalled");
            }
        }
    }
    pub unsafe fn drain(&mut self, ops: &PoolOps, ctx: *mut c_void, records: &mut [Vec<Trace>; 2]) {
        self.drain_with(
            ops,
            ctx,
            &mut Journal {
                records,
                recycle: false,
            },
            &mut ToneOutput(&[]),
        );
    }
    pub unsafe fn drain_with(
        &mut self,
        ops: &PoolOps,
        ctx: *mut c_void,
        records: &mut Journal<'_>,
        io: &mut impl StreamIo,
    ) {
        for d in 0..2 {
            if self.pending()[d] > 0 {
                self.abort_status[d] = Some((ops.base.abort)(ctx, d as u32));
            }
        }
        let began = Instant::now();
        while self.pending().iter().any(|&n| n > 0) && began.elapsed() < Duration::from_millis(1000)
        {
            (ops.base.pump)(ctx);
            self.poll(ops, ctx, records, io);
        }
        self.drain_failed = self.pending().iter().any(|&n| n > 0);
        if self.drain_failed {
            self.stop("drain_deadline");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
    #[test]
    fn queue_depth_rejects_unsupported_native_counts() {
        for depth in [0, 1, DEPTH + 1, usize::MAX] {
            assert!(std::panic::catch_unwind(|| PoolSession::with_depth(
                1,
                Info::default(),
                depth
            ))
            .is_err());
        }
        assert_eq!(PoolSession::new(1, Info::default()).queue_depth(), DEPTH);
    }
    #[test]
    fn callback_layout() {
        assert_eq!(std::mem::size_of::<PoolCompletion>(), 1048);
    }
}
