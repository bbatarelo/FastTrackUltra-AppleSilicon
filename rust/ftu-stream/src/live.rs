//! Long-lived session: control calls own lifecycle; realtime calls only touch
//! fixed atomic timelines. USB and callbacks remain on one dedicated thread.
use crate::{
    clock::ClockSnapshot,
    geometry::{
        playback_window, DEFAULT_BUFFER_FRAMES, INPUT_SAFETY, MIN_BUFFER_FRAMES, OUTPUT_SAFETY,
        PLAYBACK_OFFSET, RATE, USB_BLOCK_MAX_FRAMES,
    },
    native::*,
    pool::*,
    scheduler::*,
    timeline::*,
    timing::{distance, Metric, Snapshot as TimingSnapshot, Timing},
    PENDING_OWNERSHIP,
};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const IDLE: u32 = 0;
const STARTING: u32 = 1;
const RUNNING: u32 = 2;
const STOPPING: u32 = 3;
const FAULT: u32 = 4;
const QUARANTINED: u32 = 5;
const EPOCH_FRAMES: u64 = 1 << 40;
const _: () = assert!(BLOCK_BYTES == ENTRIES * 312 && ENTRIES.is_multiple_of(8));
#[repr(C)]
pub struct ThreadSchedule {
    pub period_ns: u32,
    pub computation_ns: u32,
    pub constraint_ns: u32,
}
// Both USB directions complete in 8 ms batches. Conservative bring-up CPU
// allowance; this does not bound IOKit delivery or establish measured WCET.
const THREAD_SCHEDULE: ThreadSchedule = ThreadSchedule {
    period_ns: 8_000_000,
    computation_ns: 1_000_000,
    constraint_ns: 2_000_000,
};
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Factory {
    pub open: unsafe extern "C" fn(u32, u32) -> *mut c_void,
    pub ops: unsafe extern "C" fn(*mut c_void) -> *const PoolOps,
    pub close: unsafe extern "C" fn(*mut c_void) -> i32,
    pub configure_thread: unsafe extern "C" fn(*mut c_void, *const ThreadSchedule) -> i32,
}
#[repr(C)]
pub struct Configuration {
    channels: u32,
    rate: u32,
    period: u32,
    min_buffer: u32,
    max_buffer: u32,
    default_buffer: u32,
    input_safety: u32,
    output_safety: u32,
    playback_offset: u32,
    ring_frames: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct Status {
    pub(crate) phase: u32,
    pub(crate) product: u32,
    pub(crate) location: u32,
    pub(crate) reason: u32,
    pub(crate) epoch: u64,
    pub(crate) captured: u64,
    pub(crate) prepared: u64,
    pub(crate) playback_missing: u64,
    pub(crate) capture_missing: u64,
    pub(crate) submitted: [u64; 2],
    pub(crate) completed: [u64; 2],
    pub(crate) pending: [u64; 2],
    pub(crate) cleanup_status: i32,
    pub(crate) fault: i32,
    pub(crate) submit_status: i32,
    pub(crate) submit_direction: u32,
    pub(crate) submit_sequence: u64,
    pub(crate) submit_start: u64,
    pub(crate) submit_checked: u64,
    pub(crate) submit_returned: u64,
}
struct Shared {
    phase: AtomicU32,
    stop: AtomicBool,
    epoch: AtomicU64,
    calls: AtomicUsize,
    timing: Timing,
    capture: Timeline,
    playback: Timeline,
    clock: PublishedClock,
    captured: AtomicU64,
    prepared: AtomicU64,
    playback_requested_end: AtomicU64,
    playback_missing: AtomicU64,
    capture_missing: AtomicU64,
    submitted: [AtomicU64; 2],
    completed: [AtomicU64; 2],
    pending: [AtomicU64; 2],
    cleanup: AtomicI32,
    fault: AtomicI32,
    reason: AtomicU32,
    submit_status: AtomicI32,
    submit_direction: AtomicU32,
    submit_detail: [AtomicU64; 4],
}
impl Shared {
    fn new() -> Self {
        Self {
            phase: AtomicU32::new(IDLE),
            stop: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            calls: AtomicUsize::new(0),
            timing: Timing::new(),
            capture: Timeline::new(),
            playback: Timeline::new(),
            clock: PublishedClock::new(),
            captured: AtomicU64::new(0),
            prepared: AtomicU64::new(0),
            playback_requested_end: AtomicU64::new(0),
            playback_missing: AtomicU64::new(0),
            capture_missing: AtomicU64::new(0),
            submitted: std::array::from_fn(|_| AtomicU64::new(0)),
            completed: std::array::from_fn(|_| AtomicU64::new(0)),
            pending: std::array::from_fn(|_| AtomicU64::new(0)),
            cleanup: AtomicI32::new(0),
            fault: AtomicI32::new(0),
            reason: AtomicU32::new(0),
            submit_status: AtomicI32::new(0),
            submit_direction: AtomicU32::new(0),
            submit_detail: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    // Quarantine is terminal for this handle, including races with a worker
    // finishing after the control thread's timeout.
    fn set_phase(&self, phase: u32) {
        let _ = self
            .phase
            .fetch_update(SeqCst, SeqCst, |old| (old != QUARANTINED).then_some(phase));
    }
    fn request_stop(&self) {
        // Close realtime admission before publishing cancellation. A late
        // initial clock fit must not turn an abandoned startup into RUNNING.
        let _ = self.phase.fetch_update(SeqCst, SeqCst, |phase| {
            matches!(phase, STARTING | RUNNING).then_some(STOPPING)
        });
        self.stop.store(true, SeqCst);
    }
    fn reset(&self, epoch: u64) {
        self.clock.publish(None);
        self.timing.reset(epoch);
        self.epoch.store(epoch, SeqCst);
        self.stop.store(false, SeqCst);
        for a in [
            &self.captured,
            &self.prepared,
            &self.playback_requested_end,
            &self.playback_missing,
            &self.capture_missing,
        ]
        .into_iter()
        .chain(self.submitted.iter())
        .chain(self.completed.iter())
        .chain(self.pending.iter())
        {
            a.store(0, SeqCst);
        }
        self.cleanup.store(0, SeqCst);
        self.fault.store(0, SeqCst);
        self.reason.store(0, SeqCst);
        self.submit_status.store(0, SeqCst);
        self.submit_direction.store(0, SeqCst);
        for a in &self.submit_detail {
            a.store(0, SeqCst);
        }
        self.set_phase(STARTING);
    }
    fn enter(&self) -> Option<Call<'_>> {
        let epoch = self.epoch.load(SeqCst);
        if self.phase.load(SeqCst) != RUNNING {
            return None;
        }
        self.calls.fetch_add(1, SeqCst);
        if self.phase.load(SeqCst) != RUNNING || self.epoch.load(SeqCst) != epoch {
            self.calls.fetch_sub(1, SeqCst);
            return None;
        }
        Some(Call {
            shared: self,
            epoch,
        })
    }
}
struct Call<'a> {
    shared: &'a Shared,
    epoch: u64,
}
impl Drop for Call<'_> {
    fn drop(&mut self) {
        self.shared.calls.fetch_sub(1, SeqCst);
    }
}
struct Control {
    worker: Option<JoinHandle<()>>,
    clients: Vec<u32>,
}
pub struct Live {
    shared: Arc<Shared>,
    control: Mutex<Control>,
    factory: Factory,
    product: u32,
    location: u32,
}
struct AudioIo {
    shared: Arc<Shared>,
    epoch: u64,
    offset: u64,
    timebase: Info,
    stepper: TimestampStepper,
    input: Vec<u8>,
    tags: [Option<usize>; FEEDBACK_SLOTS],
    scratch: Vec<f32>,
}
impl AudioIo {
    fn new(shared: Arc<Shared>, epoch: u64, info: Info) -> Self {
        Self {
            shared,
            epoch,
            offset: epoch * EPOCH_FRAMES,
            timebase: info,
            stepper: TimestampStepper::new(epoch),
            input: vec![0; FEEDBACK_SLOTS * BLOCK_BYTES],
            tags: [None; FEEDBACK_SLOTS],
            scratch: vec![0.; USB_BLOCK_MAX_FRAMES * CHANNELS],
        }
    }
}
impl StreamIo for AudioIo {
    fn submission_timing(&self) -> bool {
        true
    }
    fn submitted(&mut self, start: u64, checked: u64, returned: Option<u64>) {
        let captured = self.shared.captured.load(SeqCst);
        let timing = &self.shared.timing;
        timing.observe(
            Metric::SubmitCheckLeadMs,
            distance(start, checked),
            captured,
        );
        timing.observe(
            Metric::SubmitClockError,
            i64::from(returned.is_none()),
            captured,
        );
        if let Some(returned) = returned {
            timing.observe(
                Metric::SubmitReturnLeadMs,
                distance(start, returned),
                captured,
            );
        }
    }
    fn progress(&mut self, submitted: [usize; 2], completed: [usize; 2], pending: [usize; 2]) {
        for d in 0..2 {
            self.shared.submitted[d].store(submitted[d] as u64, SeqCst);
            self.shared.completed[d].store(completed[d] as u64, SeqCst);
            self.shared.pending[d].store(pending[d] as u64, SeqCst);
        }
    }
    fn capture(&mut self, seq: usize, _: &Completion, bytes: &[u8]) -> Result<(), &'static str> {
        let slot = seq % FEEDBACK_SLOTS;
        if self.tags[slot].is_some() {
            return Err("capture_staging_overrun");
        }
        self.input[slot * BLOCK_BYTES..(slot + 1) * BLOCK_BYTES].copy_from_slice(bytes);
        self.tags[slot] = Some(seq);
        Ok(())
    }
    fn ordered(&mut self, seq: usize, first: u64, c: &Completion) -> Result<(), &'static str> {
        let slot = seq % FEEDBACK_SLOTS;
        if self.tags[slot] != Some(seq) {
            return Err("capture_staging_generation");
        }
        let mut frames = 0;
        for (packet, p) in c.packets.iter().enumerate() {
            for f in 0..usize::from(p.actual / 24) {
                for ch in 0..8 {
                    let at = slot * BLOCK_BYTES + packet * 312 + f * 24 + ch * 3;
                    self.scratch[frames * 8 + ch] =
                        ftu_protocol::decode_s24(self.input[at..at + 3].try_into().unwrap()) as f32
                            / 8_388_608.0;
                }
                frames += 1;
            }
        }
        if first + frames as u64 >= EPOCH_FRAMES {
            return Err("timeline_exhausted");
        }
        if !self
            .shared
            .capture
            .write(self.offset + first, &self.scratch[..frames * 8])
        {
            return Err("capture_publication_rejected");
        }
        self.tags[slot] = None;
        self.shared.captured.store(first + frames as u64, SeqCst);
        Ok(())
    }
    fn clock(&mut self, fit: ClockSnapshot) -> Result<(), &'static str> {
        if !fit.valid {
            return Ok(());
        }
        let rate = fit.rate_hz.ok_or("clock_rate_missing")?;
        let ticks_per_frame = 1e9 * f64::from(self.timebase.timebase_denom)
            / f64::from(self.timebase.timebase_numer)
            / rate;
        let observed = ClockAnchor {
            epoch: self.epoch,
            frame: fit.last_frame,
            ticks: fit.last_ticks,
            ticks_per_frame,
        };
        if let Some(z) = self.stepper.advance(observed)? {
            self.shared.clock.publish(Some(ClockAnchor {
                epoch: z.epoch,
                frame: z.frame,
                ticks: z.ticks,
                ticks_per_frame,
            }));
            let _ = self
                .shared
                .phase
                .compare_exchange(STARTING, RUNNING, SeqCst, SeqCst);
        }
        Ok(())
    }
    fn output(&mut self, first: u64, frames: usize, bytes: &mut [u8]) -> Result<(), &'static str> {
        let position = first
            .checked_add(PLAYBACK_OFFSET)
            .ok_or("timeline_exhausted")?;
        if position + frames as u64 >= EPOCH_FRAMES {
            return Err("timeline_exhausted");
        }
        // Publish the full requested interval before its first sample is read.
        // HAL observation includes preparation in progress, not only blocks
        // whose packing has finished. This never gates or changes playback.
        self.shared
            .playback_requested_end
            .store(position + frames as u64, SeqCst);
        let captured = self.shared.captured.load(SeqCst);
        let head = self
            .shared
            .playback
            .high_water()
            .saturating_sub(self.offset);
        let timing = &self.shared.timing;
        timing.observe(
            Metric::OutputCaptureLead,
            distance(position, captured),
            captured,
        );
        timing.observe(
            Metric::OutputHighWaterMargin,
            distance(head, position + frames as u64),
            captured,
        );
        let missing = self
            .shared
            .playback
            .read(self.offset + position, &mut self.scratch[..frames * 8]);
        timing.observe(Metric::OutputMissing, missing as i64, captured);
        if self.shared.phase.load(SeqCst) == RUNNING {
            self.shared
                .playback_missing
                .fetch_add(missing as u64, SeqCst);
        }
        for (sample, out) in self.scratch[..frames * 8]
            .iter()
            .zip(bytes.chunks_exact_mut(3))
        {
            out.copy_from_slice(&ftu_protocol::encode_s24(*sample));
        }
        self.shared.prepared.store(first + frames as u64, SeqCst);
        Ok(())
    }
}
fn reason_code(reason: Option<&str>) -> u32 {
    match reason {
        None | Some("signal") => 0,
        Some("setup_precondition") => 1,
        Some("prepare_failed") => 2,
        Some("activate_failed") => 3,
        Some("thread_policy_failed") => 4,
        Some("thread_policy_restore_failed") => 5,
        Some("USB timestamp group not monotonic") => 10,
        Some("timestamp not increasing") => 11,
        Some("clock gap: interval too short") => 12,
        Some("clock gap: interval too long") => 13,
        Some("implausible rate") => 14,
        Some("zero timestamp") => 15,
        Some("stale_schedule") => 20,
        Some("completion_deadline") => 21,
        Some("packet_completion_error") => 22,
        Some("request_completion_error") => 23,
        Some("native_callback_fault") => 24,
        Some("native_completion_error") => 25,
        Some("submit_failed") => 26,
        Some("feedback_overrun") => 27,
        Some("stale_completion_ticket") => 28,
        Some("clock_query_failed") => 29,
        Some("zero timestamp host time not increasing") => 30,
        Some("observation not increasing") => 31,
        Some("output_payload_changed") => 32,
        Some("capture_staging_overrun") => 33,
        Some("capture_staging_generation") => 34,
        Some("capture_publication_rejected") => 35,
        Some("timeline_exhausted") => 36,
        Some("drain_deadline") => 37,
        _ => 99,
    }
}
unsafe fn worker(shared: Arc<Shared>, factory: Factory, product: u32, location: u32, epoch: u64) {
    let ctx = (factory.open)(product, location);
    if ctx.is_null() {
        shared.fault.store(1, SeqCst);
        shared.set_phase(FAULT);
        return;
    }
    let operations = (factory.ops)(ctx);
    if operations.is_null() {
        shared.fault.store(2, SeqCst);
        shared.phase.store(QUARANTINED, SeqCst);
        return;
    }
    let ops = &*operations;
    let mut info = Info::default();
    let mut rates = Rates::default();
    let mut config = vec![0; 65535];
    let mut length = 0;
    let mut records = [
        vec![Trace::default(); FEEDBACK_SLOTS],
        vec![Trace::default(); FEEDBACK_SLOTS],
    ];
    let mut session = PoolSession::new(epoch, info);
    let mut activated = false;
    let config_status = (ops.base.config)(ctx, config.as_mut_ptr(), config.len(), &mut length);
    (ops.base.info)(ctx, &mut info);
    (ops.base.rates)(ctx, &mut rates);
    let mut io = AudioIo::new(shared.clone(), epoch, info);
    let mut journal = Journal {
        records: &mut records,
        recycle: true,
    };
    if config_status != 0
        || length > config.len()
        || !ftu_protocol::validate_profile(&config[..length], false)
        || !ftu_protocol::validate_profile(&config[..length], true)
        || !rates.valid()
        || info.callback_fault != 0
        || info.timebase_numer == 0
        || info.timebase_denom == 0
    {
        session.stop("setup_precondition");
        shared.fault.store(3, SeqCst);
    } else {
        session = PoolSession::new(epoch, info);
        if (ops.base.prepare)(ctx, DEPTH as u32) != 0 {
            session.stop("prepare_failed");
            shared.fault.store(4, SeqCst);
        } else if shared.stop.load(SeqCst) {
            session.stop("signal");
        } else if (ops.base.activate)(ctx) != 0 {
            session.stop("activate_failed");
            shared.fault.store(5, SeqCst);
        } else {
            activated = true;
            if (factory.configure_thread)(ctx, &THREAD_SCHEDULE) != 0 {
                session.stop("thread_policy_failed");
                shared.fault.store(12, SeqCst);
            } else {
                session.run_with(ops, ctx, &mut journal, &mut io, None, &|| {
                    shared.stop.load(SeqCst)
                });
            }
        }
    }
    shared.clock.publish(None);
    if shared.phase.load(SeqCst) != QUARANTINED {
        shared.set_phase(STOPPING);
    }
    // Restore normal scheduling before abort/drain and native teardown.
    if activated && (factory.configure_thread)(ctx, std::ptr::null()) != 0 {
        // A normal stop's "signal" is not the cause of a restore failure.
        // Preserve any earlier transport error, but make the policy failure
        // independently distinguishable from an apply failure (fault 12).
        if session.reason == Some("signal") {
            session.reason = None;
        }
        session.stop("thread_policy_restore_failed");
        shared.fault.store(13, SeqCst);
    }
    session.drain_with(ops, ctx, &mut journal, &mut io);
    shared.reason.store(reason_code(session.reason), SeqCst);
    if let Some(f) = session.submission_failure {
        shared.submit_status.store(f.status, SeqCst);
        shared.submit_direction.store(f.direction, SeqCst);
        for (a, v) in shared
            .submit_detail
            .iter()
            .zip([f.sequence, f.start, f.checked, f.returned])
        {
            a.store(v, SeqCst);
        }
    }
    // No runtime callback can reference a timeline after the worker returns;
    // the permanent Shared object outlives admitted HAL calls independently.
    shared.clock.publish(None);
    for d in 0..2 {
        shared.submitted[d].store(session.next[d] as u64, SeqCst);
        shared.completed[d].store(session.completed[d] as u64, SeqCst);
        shared.pending[d].store(session.pending()[d] as u64, SeqCst);
    }
    let cleanup = if session.drain_failed {
        PENDING_OWNERSHIP
    } else {
        (ops.base.cleanup)(ctx)
    };
    shared.cleanup.store(cleanup, SeqCst);
    if cleanup != 0 || session.drain_failed {
        // Quarantine at most this one native context. Never _Exit the HAL host,
        // free uncertain USB memory, or retry over outstanding callback debt.
        shared.fault.store(7, SeqCst);
        shared.phase.store(QUARANTINED, SeqCst);
        return;
    }
    (ops.base.info)(ctx, &mut info);
    (ops.base.rates)(ctx, &mut rates);
    let bad_postcondition =
        activated && (info.restored_mask != 3 || info.open_mask != 0 || !rates.valid());
    if bad_postcondition {
        shared.fault.store(10, SeqCst);
    }
    let closed = (factory.close)(ctx);
    if closed != 0 {
        shared.cleanup.store(closed, SeqCst);
        shared.fault.store(8, SeqCst);
        shared.phase.store(QUARANTINED, SeqCst);
        return;
    }
    if shared.phase.load(SeqCst) == QUARANTINED {
        return;
    }
    if session.reason == Some("signal")
        && shared.stop.load(SeqCst)
        && !bad_postcondition
        && shared.fault.load(SeqCst) == 0
    {
        shared.set_phase(IDLE);
    } else {
        if shared.fault.load(SeqCst) == 0 {
            shared.fault.store(6, SeqCst);
        }
        shared.set_phase(FAULT);
    }
}
fn guarded_worker(shared: Arc<Shared>, body: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        shared.clock.publish(None);
        shared.fault.store(11, SeqCst);
        shared.phase.store(QUARANTINED, SeqCst);
    }
}
fn finish(shared: &Shared, control: &mut Control) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while control.worker.as_ref().is_some_and(|w| !w.is_finished())
        || shared.calls.load(SeqCst) != 0
    {
        if Instant::now() >= deadline {
            shared.phase.store(QUARANTINED, SeqCst);
            shared.fault.store(9, SeqCst);
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
    if let Some(worker) = control.worker.take() {
        if worker.join().is_err() {
            shared.phase.store(QUARANTINED, SeqCst);
            shared.fault.store(11, SeqCst);
            return false;
        }
    }
    shared.phase.load(SeqCst) != QUARANTINED
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_configuration(out: *mut Configuration) {
    if let Some(out) = out.as_mut() {
        *out = Configuration {
            channels: CHANNELS as u32,
            rate: RATE,
            period: ZTS_PERIOD as u32,
            min_buffer: MIN_BUFFER_FRAMES,
            max_buffer: MAX_FRAMES_PER_CALL as u32,
            default_buffer: DEFAULT_BUFFER_FRAMES,
            input_safety: INPUT_SAFETY,
            output_safety: OUTPUT_SAFETY,
            playback_offset: PLAYBACK_OFFSET as u32,
            ring_frames: RING_FRAMES as u32,
        };
    }
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_create(
    factory: *const Factory,
    product: u32,
    location: u32,
) -> *mut Live {
    if factory.is_null() || !matches!(product, 0x2080 | 0x2081) {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(Live {
        shared: Arc::new(Shared::new()),
        control: Mutex::new(Control {
            worker: None,
            clients: Vec::with_capacity(64),
        }),
        factory: *factory,
        product,
        location,
    }))
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_start(handle: *mut Live, client: u32) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Ok(mut c) = h.control.lock() else {
        return -1;
    };
    let s = &h.shared;
    if c.clients.contains(&client) {
        return if s.phase.load(SeqCst) == RUNNING {
            0
        } else {
            -2
        };
    }
    if !c.clients.is_empty() {
        if s.phase.load(SeqCst) != RUNNING || c.clients.len() == 64 {
            return -2;
        }
        c.clients.push(client);
        return 0;
    }
    if s.phase.load(SeqCst) == QUARANTINED {
        return -3;
    }
    s.stop.store(true, SeqCst);
    if !finish(s, &mut c) {
        return -3;
    }
    let epoch = s.epoch.load(SeqCst) + 1;
    if epoch > 0xffffff {
        return -3;
    }
    s.reset(epoch);
    let cloned = s.clone();
    let factory = h.factory;
    let product = h.product;
    let location = h.location;
    match thread::Builder::new()
        .name("ftu-usb".into())
        .spawn(move || {
            guarded_worker(cloned.clone(), || {
                worker(cloned, factory, product, location, epoch)
            })
        }) {
        Ok(w) => c.worker = Some(w),
        Err(_) => {
            s.set_phase(FAULT);
            return -4;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while s.phase.load(SeqCst) == STARTING && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    if s.phase.load(SeqCst) == RUNNING {
        c.clients.push(client);
        0
    } else {
        s.request_stop();
        finish(s, &mut c);
        -5
    }
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_stop(handle: *mut Live, client: u32) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Ok(mut c) = h.control.lock() else {
        return -1;
    };
    c.clients.retain(|&id| id != client);
    if !c.clients.is_empty() {
        return 0;
    }
    // Idempotent Stop must preserve Idle/Fault and terminal quarantine even
    // when the worker already finished before the control call arrived.
    h.shared.request_stop();
    if finish(&h.shared, &mut c) {
        0
    } else {
        -3
    }
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_timestamp(handle: *mut Live, out: *mut ZeroTimestamp) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Some(out) = out.as_mut() else {
        return -1;
    };
    let Some(call) = h.shared.enter() else {
        return -2;
    };
    let Some(a) = h.shared.clock.read() else {
        return -2;
    };
    if a.epoch != call.epoch {
        return -2;
    }
    *out = ZeroTimestamp {
        epoch: a.epoch,
        frame: a.frame,
        ticks: a.ticks,
    };
    0
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_write(
    handle: *mut Live,
    first: u64,
    frames: u32,
    data: *const f32,
) -> i32 {
    if frames == 0
        || frames as usize > MAX_FRAMES_PER_CALL
        || data.is_null()
        || first
            .checked_add(u64::from(frames))
            .is_none_or(|end| end >= EPOCH_FRAMES)
    {
        return -1;
    }
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Some(call) = h.shared.enter() else {
        return -2;
    };
    let Some(anchor) = h.shared.clock.read() else {
        return -2;
    };
    // ZTS is deliberately held stable within a long period. Capture publication
    // advances every completed USB batch and is the admission reference instead.
    let captured = h.shared.captured.load(SeqCst);
    if anchor.epoch != call.epoch || !playback_window(first, frames, captured) {
        return -3;
    }
    if h.shared.playback.write(
        call.epoch * EPOCH_FRAMES + first,
        std::slice::from_raw_parts(data, frames as usize * 8),
    ) {
        let captured = h.shared.captured.load(SeqCst);
        let prepared = h.shared.playback_requested_end.load(SeqCst);
        let timing = &h.shared.timing;
        timing.observe(Metric::WriteFrames, i64::from(frames), captured);
        timing.observe(
            Metric::WriteCaptureLead,
            distance(first, captured),
            captured,
        );
        timing.observe(
            Metric::WritePreparationLead,
            distance(first, prepared),
            captured,
        );
        0
    } else {
        -3
    }
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_read(
    handle: *mut Live,
    first: u64,
    frames: u32,
    data: *mut f32,
) -> i32 {
    if frames == 0
        || frames as usize > MAX_FRAMES_PER_CALL
        || data.is_null()
        || first
            .checked_add(u64::from(frames))
            .is_none_or(|end| end >= EPOCH_FRAMES)
    {
        return -1;
    }
    let output = std::slice::from_raw_parts_mut(data, frames as usize * 8);
    output.fill(0.);
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Some(call) = h.shared.enter() else {
        return -2;
    };
    let captured = h.shared.captured.load(SeqCst);
    let timing = &h.shared.timing;
    timing.observe(Metric::ReadFrames, i64::from(frames), captured);
    timing.observe(
        Metric::ReadCaptureMargin,
        distance(captured, first + u64::from(frames)),
        captured,
    );
    let missing = h
        .shared
        .capture
        .read(call.epoch * EPOCH_FRAMES + first, output);
    h.shared.capture_missing.fetch_add(missing as u64, SeqCst);
    timing.observe(Metric::ReadMissing, missing as i64, captured);
    0
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_status(handle: *mut Live, out: *mut Status) {
    let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) else {
        return;
    };
    let s = &h.shared;
    *out = Status {
        phase: s.phase.load(SeqCst),
        product: h.product,
        location: h.location,
        reason: s.reason.load(SeqCst),
        epoch: s.epoch.load(SeqCst),
        captured: s.captured.load(SeqCst),
        prepared: s.prepared.load(SeqCst),
        playback_missing: s.playback_missing.load(SeqCst),
        capture_missing: s.capture_missing.load(SeqCst),
        submitted: s.submitted.each_ref().map(|a| a.load(SeqCst)),
        completed: s.completed.each_ref().map(|a| a.load(SeqCst)),
        pending: s.pending.each_ref().map(|a| a.load(SeqCst)),
        cleanup_status: s.cleanup.load(SeqCst),
        fault: s.fault.load(SeqCst),
        submit_status: s.submit_status.load(SeqCst),
        submit_direction: s.submit_direction.load(SeqCst),
        submit_sequence: s.submit_detail[0].load(SeqCst),
        submit_start: s.submit_detail[1].load(SeqCst),
        submit_checked: s.submit_detail[2].load(SeqCst),
        submit_returned: s.submit_detail[3].load(SeqCst),
    };
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_timing(handle: *mut Live, out: *mut TimingSnapshot) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.timing.snapshot();
    }
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_destroy(handle: *mut Live) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    {
        let Ok(c) = h.control.lock() else {
            return -1;
        };
        if c.worker.is_some()
            || !c.clients.is_empty()
            || h.shared.calls.load(SeqCst) != 0
            || !matches!(h.shared.phase.load(SeqCst), IDLE | FAULT | STOPPING)
        {
            return -2;
        }
    }
    drop(Box::from_raw(handle));
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" fn unused_open(_: u32, _: u32) -> *mut c_void {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn unused_ops(_: *mut c_void) -> *const PoolOps {
        std::ptr::null()
    }
    unsafe extern "C" fn unused_close(_: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn unused_schedule(_: *mut c_void, _: *const ThreadSchedule) -> i32 {
        0
    }

    #[test]
    fn full_size_io_uses_capture_progress_between_stable_zero_timestamps() {
        let shared = Arc::new(Shared::new());
        shared.reset(1);
        shared.clock.publish(Some(ClockAnchor {
            epoch: 1,
            frame: 0,
            ticks: 1_000_000,
            ticks_per_frame: 500.0,
        }));
        shared.set_phase(RUNNING);
        let mut live = Live {
            shared: shared.clone(),
            control: Mutex::new(Control {
                worker: None,
                clients: Vec::new(),
            }),
            factory: Factory {
                open: unused_open,
                ops: unused_ops,
                close: unused_close,
                configure_thread: unused_schedule,
            },
            product: 0x2080,
            location: 123,
        };
        let samples = vec![0.25; MAX_FRAMES_PER_CALL * CHANNELS];
        let mut readback = vec![0.; samples.len()];
        for captured in [
            MAX_FRAMES_PER_CALL as u64,
            ZTS_PERIOD - 48,
            2 * ZTS_PERIOD - 1,
        ] {
            shared.captured.store(captured, SeqCst);
            let first = captured + u64::from(OUTPUT_SAFETY);
            unsafe {
                assert_eq!(
                    ftu_live_write(
                        &mut live,
                        first,
                        MAX_FRAMES_PER_CALL as u32,
                        samples.as_ptr()
                    ),
                    0
                );
                assert_eq!(
                    ftu_live_write(
                        &mut live,
                        first,
                        MAX_FRAMES_PER_CALL as u32 + 1,
                        samples.as_ptr()
                    ),
                    -1
                );
                assert_eq!(ftu_live_write(&mut live, first, 0, samples.as_ptr()), -1);
                assert_eq!(
                    ftu_live_write(
                        &mut live,
                        captured + crate::geometry::WRITE_AHEAD_FRAMES,
                        1,
                        samples.as_ptr()
                    ),
                    -3
                );
            }
            assert_eq!(shared.playback.read(EPOCH_FRAMES + first, &mut readback), 0);
            assert_eq!(readback, samples);
            let input_first = captured - MAX_FRAMES_PER_CALL as u64;
            assert!(shared.capture.write(EPOCH_FRAMES + input_first, &samples));
            unsafe {
                assert_eq!(
                    ftu_live_read(
                        &mut live,
                        input_first,
                        MAX_FRAMES_PER_CALL as u32,
                        readback.as_mut_ptr()
                    ),
                    0
                );
                assert_eq!(
                    ftu_live_read(
                        &mut live,
                        input_first,
                        MAX_FRAMES_PER_CALL as u32 + 1,
                        readback.as_mut_ptr()
                    ),
                    -1
                );
                assert_eq!(
                    ftu_live_read(&mut live, input_first, 0, readback.as_mut_ptr()),
                    -1
                );
            }
            assert_eq!(readback, samples);
            assert_eq!(shared.clock.read().unwrap().frame, 0);
        }
        let snapshot = shared.timing.snapshot();
        assert_eq!(snapshot.metrics[0][Metric::WriteFrames as usize].count, 3);
        assert_eq!(
            snapshot.metrics[0][Metric::WriteCaptureLead as usize].min,
            i64::from(OUTPUT_SAFETY)
        );
        assert_eq!(
            snapshot.metrics[0][Metric::ReadCaptureMargin as usize].max,
            0
        );
        assert_eq!(snapshot.metrics[0][Metric::ReadMissing as usize].max, 0);
        assert_eq!(snapshot.metrics[1][Metric::WriteFrames as usize].count, 0);
        // A clock belonging to another epoch still prevents write admission.
        shared.epoch.store(2, SeqCst);
        unsafe {
            assert_eq!(
                ftu_live_write(&mut live, 2 * ZTS_PERIOD, 1, samples.as_ptr()),
                -3
            );
        }
    }

    #[test]
    fn positive_high_water_is_not_mistaken_for_complete_payload_coverage() {
        let shared = Arc::new(Shared::new());
        shared.reset(1);
        shared.set_phase(RUNNING);
        shared.captured.store(50_000, SeqCst);
        let mut io = AudioIo::new(shared.clone(), 1, Info::default());
        let position = 50_000 + PLAYBACK_OFFSET;
        let tail = vec![0.25; 64 * 8];
        assert!(shared.playback.write(EPOCH_FRAMES + position + 64, &tail));
        let mut bytes = vec![0xff; 64 * 24];
        io.output(50_000, 64, &mut bytes).unwrap();
        assert!(bytes.iter().all(|&x| x == 0));
        let snapshot = shared.timing.snapshot();
        assert_eq!(
            snapshot.metrics[1][Metric::OutputHighWaterMargin as usize].min,
            64
        );
        assert_eq!(snapshot.metrics[1][Metric::OutputMissing as usize].max, 64);
        assert_eq!(
            snapshot.metrics[1][Metric::OutputCaptureLead as usize].min,
            PLAYBACK_OFFSET as i64
        );
        assert!(shared.playback.write(EPOCH_FRAMES + position, &tail));
        io.output(50_000, 64, &mut bytes).unwrap();
        assert!(bytes
            .chunks_exact(3)
            .all(|b| ftu_protocol::decode_s24(b.try_into().unwrap()) == 2_097_152));
        io.submitted(100, 75, Some(77));
        io.submitted(100, 75, None);
        let snapshot = shared.timing.snapshot();
        assert_eq!(
            snapshot.metrics[1][Metric::SubmitReturnLeadMs as usize].min,
            23
        );
        assert_eq!(
            snapshot.metrics[1][Metric::SubmitClockError as usize].max,
            1
        );
        assert_eq!(snapshot.metrics[1][Metric::OutputMissing as usize].min, 0);
    }

    #[test]
    fn worker_panic_invalidates_clock_without_terminating_host() {
        let shared = Arc::new(Shared::new());
        shared.phase.store(RUNNING, SeqCst);
        shared.clock.publish(Some(ClockAnchor {
            epoch: 1,
            frame: 512,
            ticks: 1000000,
            ticks_per_frame: 500.,
        }));
        guarded_worker(shared.clone(), || panic!("injected Rust worker fault"));
        assert_eq!(shared.phase.load(SeqCst), QUARANTINED);
        assert_eq!(shared.fault.load(SeqCst), 11);
        assert!(shared.clock.read().is_none());
        assert!(shared.enter().is_none());
    }
    #[test]
    fn late_worker_cannot_revoke_quarantine() {
        let shared = Shared::new();
        shared.phase.store(QUARANTINED, SeqCst);
        for phase in [IDLE, STARTING, RUNNING, STOPPING, FAULT] {
            shared.set_phase(phase);
            assert_eq!(shared.phase.load(SeqCst), QUARANTINED);
        }
    }
    #[test]
    fn cancelled_startup_rejects_late_initial_clock_and_preserves_terminal_states() {
        let shared = Arc::new(Shared::new());
        shared.reset(1);
        let worker = shared.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let late_fit = thread::spawn(move || {
            let mut audio = AudioIo::new(
                worker.clone(),
                1,
                Info {
                    timebase_numer: 125,
                    timebase_denom: 3,
                    ..Info::default()
                },
            );
            receive.recv().unwrap();
            audio
                .clock(ClockSnapshot {
                    epoch: 1,
                    observations: 32,
                    first_frame: 0,
                    last_frame: 2048,
                    first_ticks: 1_000_000,
                    last_ticks: 2_024_000,
                    valid: true,
                    rate_hz: Some(48_000.),
                    span_ns: 42_666_667,
                })
                .unwrap();
            worker.phase.load(SeqCst)
        });
        shared.request_stop();
        send.send(()).unwrap();
        assert_eq!(late_fit.join().unwrap(), STOPPING);
        assert!(
            shared.clock.read().is_some(),
            "exercise a genuinely published late clock"
        );
        assert!(shared.stop.load(SeqCst));
        assert!(shared.enter().is_none());
        for phase in [IDLE, FAULT, QUARANTINED] {
            shared.phase.store(phase, SeqCst);
            shared.request_stop();
            assert_eq!(shared.phase.load(SeqCst), phase);
        }
    }
}
