//! Bounded standalone stream engine. OS mechanics are behind a C function table.
//! No HAL publication and no mixer/rate writes. Use only from a standalone probe.
mod clock;
mod continuous;
mod geometry;
mod live;
mod native;
mod pool;
mod scheduler;
pub mod timeline;
mod timing;
mod worker_timing;
use native::*;
use scheduler::*;
use std::{
    ffi::{c_char, c_void, CStr},
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
    time::{Duration, Instant},
};
const UNDERRUN: i32 = 0xe00002e7u32 as i32;
const PENDING_OWNERSHIP: i32 = -1000001;
fn ok(r: i32) -> bool {
    r == 0 || r == UNDERRUN
}
#[derive(Clone)]
struct Record {
    start: Option<u64>,
    submit: Option<i32>,
    completion: Option<Completion>,
    lengths: [u16; ENTRIES],
}
impl Default for Record {
    fn default() -> Self {
        Self {
            start: None,
            submit: None,
            completion: None,
            lengths: [0; ENTRIES],
        }
    }
}
struct Session {
    queues: [Queue; 2],
    records: [Vec<Record>; 2],
    next_sample: u64,
    reason: Option<&'static str>,
    events: Vec<(&'static str, i32, u32)>,
    clock: Clock,
    info: Info,
    rates: [Rates; 2],
    active: bool,
    cleanup: i32,
    drain_failed: bool,
}
impl Session {
    fn new(count: usize) -> Self {
        Self {
            queues: [Queue::new(count), Queue::new(count)],
            records: [
                vec![Record::default(); count],
                vec![Record::default(); count],
            ],
            next_sample: 0,
            reason: None,
            events: Vec::with_capacity(32),
            clock: Clock::default(),
            info: Info::default(),
            rates: [Rates::default(); 2],
            active: false,
            cleanup: 0,
            drain_failed: false,
        }
    }
    fn stop(&mut self, reason: &'static str) {
        if self.reason.is_none() {
            self.reason = Some(reason);
        }
        for q in &mut self.queues {
            q.stopped = true;
        }
    }
    fn pending(&self) -> usize {
        self.queues.iter().map(|q| q.pending).sum()
    }
    unsafe fn step(
        &mut self,
        ops: &Ops,
        ctx: *mut c_void,
        name: &'static str,
        status: i32,
    ) -> bool {
        (ops.info)(ctx, &mut self.info);
        self.events.push((name, status, self.info.stage));
        if status != 0 {
            self.stop(name);
            false
        } else {
            true
        }
    }
    unsafe fn poll(&mut self, ops: &Ops, ctx: *mut c_void) {
        for d in 0..2 {
            for i in 0..self.records[d].len() {
                if self.queues[d].states[i] != State::Pending {
                    continue;
                }
                let mut c = Completion::default();
                let found = (ops.completion)(ctx, d as u32, i as u32, &mut c);
                if found == 0 {
                    continue;
                }
                if found != 1 {
                    self.stop("native_completion_error");
                    continue;
                }
                if !self.queues[d].complete(i) {
                    self.stop("ownership_error");
                }
                self.records[d][i].completion = Some(c);
                if c.matched != 1 || !ok(c.status) {
                    self.stop("request_completion_error");
                    continue;
                }
                let valid = c.packets.iter().enumerate().all(|(slot, p)| {
                    ok(p.status)
                        && if d == 0 {
                            p.requested == 312 && valid_length(p.actual)
                        } else {
                            p.requested == self.records[d][i].lengths[slot]
                                && p.actual == p.requested
                        }
                });
                if !valid {
                    self.stop("packet_completion_error");
                    continue;
                }
                if d == 0 {
                    for (slot, p) in c.packets.iter().enumerate() {
                        self.records[1][i].lengths[slot] = p.actual;
                    }
                }
            }
        }
        (ops.info)(ctx, &mut self.info);
        if self.info.callback_fault != 0 {
            self.stop("native_callback_fault");
        }
    }
    unsafe fn run(
        &mut self,
        ops: &Ops,
        ctx: *mut c_void,
        duration: u32,
        tone: bool,
        stop: *const i32,
    ) {
        let status = (ops.clock)(ctx, &mut self.clock);
        if !self.step(ops, ctx, "clock", status) {
            return;
        }
        let Some(base) = self.clock.frame.checked_add(LEAD_MS) else {
            self.stop("clock_overflow");
            return;
        };
        let began = Instant::now();
        while self.reason.is_none() && self.queues[1].completed < self.records[1].len() {
            if std::ptr::read_volatile(stop) != 0 {
                self.stop("signal");
                break;
            }
            if began.elapsed()
                > Duration::from_millis(duration as u64 + LEAD_MS + FEEDBACK_DELAY_MS + 500)
            {
                self.stop("stream_deadline");
                break;
            }
            self.poll(ops, ctx);
            for d in 0..2 {
                while self.queues[d].ready() && self.reason.is_none() {
                    let i = self.queues[d].next;
                    if d == 1 && self.queues[0].states[i] != State::Complete {
                        break;
                    }
                    if std::ptr::read_volatile(stop) != 0 {
                        self.stop("signal");
                        break;
                    }
                    let Some(start) = start_frame(base, i, d == 1) else {
                        self.stop("schedule_overflow");
                        break;
                    };
                    let mut now = Clock::default();
                    let r = (ops.clock)(ctx, &mut now);
                    if r != 0 || !schedule_valid(now.frame, start) {
                        self.stop("stale_schedule");
                        break;
                    }
                    if d == 0 {
                        self.records[d][i].lengths = [312; ENTRIES];
                    } else {
                        if !self.records[d][i].lengths.iter().copied().all(valid_length) {
                            self.stop("missing_capture_feedback");
                            break;
                        }
                        let buffer = (ops.buffer)(ctx, d as u32, i as u32);
                        if buffer.is_null() {
                            self.stop("output_buffer_unavailable");
                            break;
                        }
                        // Unique, never-submitted slot; callback/kernel have no ownership yet.
                        let bytes = std::slice::from_raw_parts_mut(buffer, BLOCK_BYTES);
                        bytes.fill(0);
                        let frames = self.records[d][i]
                            .lengths
                            .iter()
                            .map(|&n| n as usize / 24)
                            .sum::<usize>();
                        if tone {
                            for f in 0..frames {
                                for ch in 0..2 {
                                    let p = ftu_protocol::encode_s24(tone_sample(
                                        self.next_sample + f as u64,
                                        ch,
                                    ));
                                    bytes[f * 24 + ch * 3..f * 24 + ch * 3 + 3].copy_from_slice(&p);
                                }
                            }
                        }
                        self.next_sample += frames as u64;
                    }
                    self.records[d][i].start = Some(start);
                    let r = (ops.submit)(
                        ctx,
                        d as u32,
                        i as u32,
                        start,
                        self.records[d][i].lengths.as_ptr(),
                    );
                    self.records[d][i].submit = Some(r);
                    if !self.queues[d].submit(r == 0) {
                        self.stop("submission_accounting_error");
                        break;
                    }
                    if r != 0 {
                        self.stop("submit_failed");
                        break;
                    }
                }
            }
            if self.pending() > 0 {
                (ops.pump)(ctx);
            }
        }
    }
    unsafe fn drain(&mut self, ops: &Ops, ctx: *mut c_void) {
        for q in &mut self.queues {
            q.stopped = true;
        }
        for d in 0..2 {
            if self.queues[d].pending > 0 {
                let r = (ops.abort)(ctx, d as u32);
                self.events.push((
                    if d == 0 {
                        "abort_input"
                    } else {
                        "abort_output"
                    },
                    r,
                    self.info.stage,
                ));
            }
        }
        let began = Instant::now();
        while self.pending() > 0 && began.elapsed() < Duration::from_millis(1000) {
            (ops.pump)(ctx);
            self.poll(ops, ctx);
        }
        self.drain_failed = self.pending() > 0;
        if self.drain_failed {
            self.stop("drain_deadline");
        }
    }
}
fn new_file(path: &Path, name: &str) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.join(name))
}
fn optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "null".into())
}
unsafe fn save_packets(
    session: &Session,
    ops: &Ops,
    ctx: *mut c_void,
    observations: &mut File,
    raw: &mut [File; 2],
) -> io::Result<()> {
    let mut offsets = [0usize; 2];
    for d in 0..2 {
        for (i, record) in session.records[d].iter().enumerate() {
            let owned = record.completion.as_ref().is_some_and(|c| c.matched == 1);
            let raw_offset = offsets[d];
            if owned {
                let ptr = (ops.buffer)(ctx, d as u32, i as u32);
                if ptr.is_null() {
                    return Err(io::Error::other("completed buffer unavailable"));
                }
                // The matching callback has returned ownership; never inspect pending slots.
                raw[d].write_all(std::slice::from_raw_parts(ptr, BLOCK_BYTES))?;
                offsets[d] += BLOCK_BYTES;
            }
            let mut packet_offset = 0;
            for slot in 0..ENTRIES {
                let c = record.completion.as_ref();
                let p = c.filter(|_| owned).map(|c| c.packets[slot]);
                let accepted = p.is_some_and(|p| {
                    ok(c.unwrap().status)
                        && ok(p.status)
                        && if d == 0 {
                            p.requested == 312 && valid_length(p.actual)
                        } else {
                            p.requested == record.lengths[slot] && p.actual == p.requested
                        }
                });
                writeln!(observations,"{{\"direction\":{},\"block\":{},\"slot\":{},\"frame_start\":{},\"state\":\"{:?}\",\"submit_status\":{},\"completion_status\":{},\"callback_matches\":{},\"callback_ticks\":{},\"planned_length\":{},\"requested\":{},\"actual\":{},\"packet_status\":{},\"usb_ticks\":{},\"accepted\":{},\"raw_offset\":{}}}",
                d,i,slot,optional(record.start),session.queues[d].states[i],optional(record.submit),optional(c.map(|v|v.status)),
                optional(c.map(|v|v.matched)),optional(c.map(|v|v.callback_ticks)),record.lengths[slot],optional(p.map(|v|v.requested)),optional(p.map(|v|v.actual)),optional(p.map(|v|v.status)),optional(p.map(|v|v.ticks)),accepted,
                optional(if owned{Some(raw_offset+packet_offset)}else{None}))?;
                packet_offset += if d == 0 {
                    312
                } else {
                    record.lengths[slot] as usize
                };
            }
        }
    }
    observations.flush()?;
    for f in raw {
        f.flush()?;
    }
    Ok(())
}
/// # Safety
/// All pointers/functions follow shared/ftu_native.h, remain live for this call,
/// and run on the same thread. stop is a live C sig_atomic_t. A non-draining
/// native backend causes process exit, retaining kernel-owned resources.
#[no_mangle]
pub unsafe extern "C" fn ftu_duplex_run(
    ops: *const Ops,
    ctx: *mut c_void,
    output: *const c_char,
    duration: u32,
    tone: u32,
    stop: *const i32,
) -> i32 {
    if ops.is_null()
        || ctx.is_null()
        || output.is_null()
        || stop.is_null()
        || !(8..=2048).contains(&duration)
        || !duration.is_multiple_of(8)
        || tone > 1
        || (tone == 1 && duration != 2048)
    {
        return 2;
    }
    let Ok(directory) = CStr::from_ptr(output).to_str() else {
        return 2;
    };
    let path = Path::new(directory);
    let ops = &*ops;
    let count = duration as usize / 8;
    let mut s = Session::new(count);
    // Reserve all evidence files and policy state before opening interfaces.
    let files = (|| -> io::Result<_> {
        Ok((
            new_file(path, "plan.json")?,
            new_file(path, "events.jsonl")?,
            new_file(path, "observations.jsonl")?,
            [
                new_file(path, "input-packets.bin")?,
                new_file(path, "output-packets.bin")?,
            ],
            new_file(path, "summary.json")?,
            new_file(path, "duplex-config.bin")?,
        ))
    })();
    let Ok((mut plan, mut events, mut observations, mut raw, mut summary, mut config_file)) = files
    else {
        return 1;
    };
    let mut file_error = false;
    let result = (|| -> io::Result<()> {
        writeln!(plan,"{{\"schema_version\":1,\"engine\":\"rust\",\"mode\":\"{}\",\"duration_ms\":{},\"rate_hz\":48000,\"channels\":8,\"packet_capacity\":312,\"entries_per_block\":64,\"block_ms\":8,\"queue_depth\":4,\"lead_ms\":16,\"feedback_delay_ms\":64,\"feedback_source\":\"IN81_actual_packet_counts\",\"mixer_writes\":false,\"tone_peak\":{}}}",if tone==1{"tone"}else{"silence"},duration,if tone==1{"0.01"}else{"0.0"})?;
        plan.flush()?;
        let mut bytes = vec![0u8; 65535];
        let mut length = 0;
        let r = (ops.config)(ctx, bytes.as_mut_ptr(), bytes.len(), &mut length);
        if !s.step(ops, ctx, "device_config", r) {
            return Ok(());
        }
        if length > bytes.len() {
            s.stop("configuration_length");
            return Ok(());
        }
        config_file.write_all(&bytes[..length])?;
        config_file.flush()?;
        if !ftu_protocol::validate_profile(&bytes[..length], false)
            || !ftu_protocol::validate_profile(&bytes[..length], true)
        {
            s.stop("profile_mismatch");
            return Ok(());
        }
        (ops.rates)(ctx, &mut s.rates[0]);
        if !s.rates[0].valid() {
            s.stop("rate_precondition");
            return Ok(());
        }
        let r = (ops.prepare)(ctx, count as u32);
        if !s.step(ops, ctx, "prepare", r) {
            return Ok(());
        }
        if std::ptr::read_volatile(stop) != 0 {
            s.stop("signal_before_activation");
            return Ok(());
        }
        let r = (ops.activate)(ctx);
        if !s.step(ops, ctx, "activate", r) {
            return Ok(());
        }
        s.active = true;
        // A flushed readiness marker precedes the clock sample and submissions.
        // The external interruption test can signal after activation without
        // requiring filesystem I/O in the timed transfer loop.
        writeln!(events, "{{\"event\":\"duplex_ready\"}}")?;
        events.flush()?;
        s.run(ops, ctx, duration, tone == 1, stop);
        Ok(())
    })();
    if result.is_err() {
        file_error = true;
        s.stop("evidence_write_failed");
    }
    s.drain(ops, ctx);
    if save_packets(&s, ops, ctx, &mut observations, &mut raw).is_err() {
        file_error = true;
        s.stop("packet_evidence_failed");
    }
    if !s.drain_failed {
        s.cleanup = (ops.cleanup)(ctx);
        s.events.push(("cleanup", s.cleanup, s.info.stage));
        if s.cleanup == PENDING_OWNERSHIP {
            s.drain_failed = true;
            s.stop("native_pending_on_cleanup");
        } else {
            (ops.rates)(ctx, &mut s.rates[1]);
        }
        (ops.info)(ctx, &mut s.info);
    }
    for (event, status, stage) in &s.events {
        if writeln!(
            events,
            "{{\"event\":\"{}\",\"status\":{},\"native_stage\":{}}}",
            event, status, stage
        )
        .is_err()
        {
            file_error = true;
        }
    }
    if events.flush().is_err() {
        file_error = true;
    }
    let code = if s.drain_failed {
        4
    } else if file_error || s.cleanup != 0 || s.info.callback_fault != 0 {
        1
    } else if s.reason.is_some()
        || !s.active
        || s.info.restored_mask != 3
        || !s.rates[1].valid()
        || s.queues.iter().any(|q| q.completed != count)
    {
        3
    } else {
        0
    };
    let text=format!("{{\"status\":\"{}\",\"exit_code\":{},\"reason\":{},\"engine\":\"rust\",\"tone\":{},\"duration_ms\":{},\"planned_blocks_per_direction\":{},\"submitted\":[{},{}],\"completed\":[{},{}],\"pending\":[{},{}],\"output_planned_frames\":{},\"file_error\":{},\"drain_failed\":{},\"cleanup_status\":{},\"restored_mask\":{},\"open_mask\":{},\"callback_fault\":{},\"pipes\":{:?},\"original_alternates\":{:?},\"restore_status\":{:?},\"native_cleanup_status\":{:?},\"native_stage\":{},\"vid\":{},\"pid\":{},\"location\":{},\"clock_frame\":{},\"clock_ticks\":{},\"timebase_numer\":{},\"timebase_denom\":{},\"rate_before\":{},\"rate_after\":{}}}\n",
        if code==0{"ok"}else if code==3{"incomplete"}else{"error"},code,s.reason.map(|r|format!("\"{}\"",r)).unwrap_or_else(||"null".into()),tone==1,duration,count,
        s.queues[0].submitted,s.queues[1].submitted,s.queues[0].completed,s.queues[1].completed,s.queues[0].pending,s.queues[1].pending,s.next_sample,
        file_error,s.drain_failed,s.cleanup,s.info.restored_mask,s.info.open_mask,s.info.callback_fault,s.info.pipe,s.info.original,s.info.restore_status,s.info.cleanup_status,s.info.stage,s.info.vid,s.info.pid,s.info.location,s.clock.frame,s.clock.ticks,s.info.timebase_numer,s.info.timebase_denom,s.rates[0].json(),s.rates[1].json());
    let saved = summary
        .write_all(text.as_bytes())
        .and_then(|_| summary.flush())
        .is_ok();
    let _ = io::stdout().write_all(text.as_bytes());
    let _ = io::stdout().flush();
    if s.drain_failed {
        std::process::exit(4);
    }
    if !saved {
        1
    } else {
        code
    }
}

#[cfg(test)]
mod simulation;

#[cfg(test)]
mod pool_simulation;
