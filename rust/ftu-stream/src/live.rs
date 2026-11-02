//! Long-lived session: control calls own lifecycle; realtime calls only touch
//! fixed atomic timelines. Default USB/callbacks share one worker; the explicit
//! independent profile dispatches capture on a separate native thread.
use crate::{
    clock::ClockSnapshot,
    geometry::{
        playback_window, DEFAULT_BUFFER_FRAMES, INPUT_SAFETY, MIN_BUFFER_FRAMES, OUTPUT_SAFETY,
        PLAYBACK_OFFSET, RATE,
    },
    native::*,
    pool::*,
    recovery::{self, Decision, Recovery},
    runtime_fault::{RuntimeFault, Seq},
    scheduler::*,
    timeline::*,
    timing::{distance, Metric, Snapshot as TimingSnapshot, Timing},
    transport::{Transport, LIVE},
    worker_timing::{Event as WorkerEvent, Snapshot as WorkerSnapshot, WorkerTiming},
    PENDING_OWNERSHIP,
};
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst},
        Arc, Mutex, TryLockError,
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
// Keep the baseline 8/1/2 ms policy for the offline prototype comparison.
// Higher completion frequency still requires measured hardware cost before installation.
// This bring-up CPU allowance does not bound IOKit delivery or establish WCET.
const THREAD_SCHEDULE: ThreadSchedule = ThreadSchedule {
    period_ns: 8_000_000,
    computation_ns: 1_000_000,
    constraint_ns: 2_000_000,
};
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Factory {
    // (product, location, registry entry ID). Registry 0 is unpinned
    // (standalone tools); otherwise open must refuse any other USB enumeration.
    pub open: unsafe extern "C" fn(u32, u32, u64) -> *mut c_void,
    pub ops: unsafe extern "C" fn(*mut c_void) -> *const PoolOps,
    pub close: unsafe extern "C" fn(*mut c_void) -> i32,
    pub configure_thread: unsafe extern "C" fn(*mut c_void, *const ThreadSchedule) -> i32,
    pub set_rate: unsafe extern "C" fn(*mut c_void, u32, u32, *mut u32) -> i32,
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
    worker_timing: WorkerTiming,
    delivery: crate::worker_delivery::Delivery,
    pumps: crate::worker_pumps::Pumps,
    first_failure: crate::first_failure::FirstFailure,
    startup: crate::startup::Startup,
    retry: crate::output_retry::Published,
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
    // Monotonic origin for diagnostic/backoff milliseconds; never a sample clock.
    origin: Instant,
    // Last runtime stop (ftuE): never cleared by reset, Start or recovery.
    runtime: RuntimeFault,
    shutdown: crate::shutdown::Shutdown,
    // Published copy of the control-owned recovery state (ftuA).
    recovery: Seq<{ recovery::WORDS }>,
}
impl Shared {
    fn uptime_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            runtime: RuntimeFault::new(),
            shutdown: crate::shutdown::Shutdown::new(),
            recovery: Seq::new(Recovery::new(recovery::POLICY).words(0, 0, 0)),
            phase: AtomicU32::new(IDLE),
            stop: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            calls: AtomicUsize::new(0),
            timing: Timing::new(),
            worker_timing: WorkerTiming::new(),
            delivery: crate::worker_delivery::Delivery::new(),
            pumps: crate::worker_pumps::Pumps::new(),
            first_failure: crate::first_failure::FirstFailure::new(),
            startup: crate::startup::Startup::new(),
            retry: crate::output_retry::Published::new(),
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
        self.worker_timing.reset(epoch);
        self.delivery.reset(epoch);
        self.pumps.reset(epoch);
        self.startup.publish(&crate::startup::Snapshot::new(epoch));
        self.retry.publish(&crate::output_retry::Snapshot::new(
            epoch,
            crate::output_admission::LIVE_OUTPUT_POLICY,
            0,
            0,
        ));
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
// Active and retired client IDs share one fixed budget; neither Vec grows past it.
const CLIENT_SLOTS: usize = 64;
struct Control {
    worker: Option<JoinHandle<()>>,
    clients: Vec<u32>,
    // Clients that were active when their unit was detached and have not yet
    // acknowledged it with Stop or RemoveDeviceClient. A fresh Start reusing such
    // an ID is refused, so the delayed old Stop can never stop a new session.
    retired: Vec<u32>,
    // Automatic recovery is decided and performed only while holding this lock.
    recovery: Recovery,
}
impl Control {
    fn slots_full(&self) -> bool {
        self.clients.len() + self.retired.len() >= CLIENT_SLOTS
    }
    fn publish_recovery(&self, shared: &Shared) {
        shared.recovery.publish(&self.recovery.words(
            shared.uptime_ms(),
            self.clients.len(),
            self.retired.len(),
        ));
    }
}
// One resident handle per host process. The USB binding (attached/location/
// registry/binding) is written only while holding `control` with no live
// worker, so Start, Stop, detach and rebind are serialized; atomics only make
// status reads race-free. The product is fixed for the handle lifetime.
pub struct Live {
    shared: Arc<Shared>,
    control: Mutex<Control>,
    factory: Factory,
    product: u32,
    location: AtomicU32,
    // IORegistry entry ID the worker must open; 0 only for unpinned tools.
    registry: AtomicU64,
    // Publication generation, advanced by every rebind. Never a timestamp epoch.
    binding: AtomicU64,
    attached: AtomicBool,
}
impl Live {
    #[cfg(test)]
    fn new(shared: Arc<Shared>, factory: Factory, product: u32, location: u32) -> Self {
        Self::pinned(shared, factory, product, location, 0)
    }
    fn pinned(
        shared: Arc<Shared>,
        factory: Factory,
        product: u32,
        location: u32,
        registry: u64,
    ) -> Self {
        Self {
            shared,
            control: Mutex::new(Control {
                worker: None,
                clients: Vec::with_capacity(CLIENT_SLOTS),
                retired: Vec::with_capacity(CLIENT_SLOTS),
                recovery: Recovery::new(recovery::POLICY),
            }),
            factory,
            product,
            location: AtomicU32::new(location),
            registry: AtomicU64::new(registry),
            binding: AtomicU64::new(1),
            attached: AtomicBool::new(true),
        }
    }
}
struct AudioIo {
    shared: Arc<Shared>,
    epoch: u64,
    offset: u64,
    timebase: Info,
    stepper: TimestampStepper,
    input: Vec<u8>,
    tags: Vec<Option<usize>>,
    transport: Transport,
    scratch: Vec<f32>,
    staged_output: Option<(u64, usize)>,
}
impl AudioIo {
    #[cfg(any(test, not(feature = "independent-4ms")))]
    fn new(shared: Arc<Shared>, epoch: u64, info: Info) -> Self {
        Self::with_geometry(shared, epoch, info, LIVE)
    }
    fn with_geometry(shared: Arc<Shared>, epoch: u64, info: Info, transport: Transport) -> Self {
        Self {
            transport,
            shared,
            epoch,
            offset: epoch * EPOCH_FRAMES,
            timebase: info,
            stepper: TimestampStepper::new(epoch),
            input: vec![0; transport.history() * transport.bytes()],
            tags: vec![None; transport.history()],
            scratch: vec![0.; transport.max_frames() * CHANNELS],
            staged_output: None,
        }
    }
}
impl StreamIo for AudioIo {
    fn output_retry(&mut self, snapshot: &crate::output_retry::Snapshot) {
        self.shared.retry.publish(snapshot);
    }
    fn worker_timing(&self) -> bool {
        true
    }
    fn worker_event(&mut self, mut event: WorkerEvent, pump: Pump, feedback_pump: Pump) {
        event.captured = self.shared.captured.load(SeqCst);
        self.shared.worker_timing.observe(event);
        self.shared.delivery.admission(event, pump, feedback_pump);
        if event.captured >= crate::timing::STEADY_AFTER {
            if let Some(lead) = event.lead() {
                self.shared.pumps.link(
                    2 + event.direction as usize,
                    event.sequence,
                    event.ticket,
                    lead,
                    pump.id,
                );
            }
        }
    }
    fn worker_completion(
        &mut self,
        d: usize,
        slot: usize,
        flight: crate::pool::Flight,
        c: &crate::pool::PoolCompletion,
        consumed: u64,
        draining: bool,
    ) {
        self.shared.pumps.delivery(
            d,
            flight.seq as u64,
            flight.ticket,
            c,
            self.shared.captured.load(SeqCst),
            draining,
        );
        self.shared.delivery.completion(
            d,
            slot,
            flight,
            c,
            consumed,
            self.shared.captured.load(SeqCst),
            draining,
        );
    }
    fn worker_pump(&mut self, p: crate::native::PumpObservation, draining: bool) {
        self.shared
            .pumps
            .observe(p, self.shared.captured.load(SeqCst), draining);
    }
    fn worker_sample(&mut self, s: WorkerSample) {
        self.shared.delivery.sample(s);
    }
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
        let slot = seq % self.transport.history();
        if self.tags[slot].is_some() {
            return Err("capture_staging_overrun");
        }
        if bytes.len() != self.transport.bytes() {
            return Err("capture_geometry_mismatch");
        }
        self.input[slot * self.transport.bytes()..(slot + 1) * self.transport.bytes()]
            .copy_from_slice(bytes);
        self.tags[slot] = Some(seq);
        Ok(())
    }
    fn ordered(&mut self, seq: usize, first: u64, c: &Completion) -> Result<(), &'static str> {
        let slot = seq % self.transport.history();
        if self.tags[slot] != Some(seq) {
            return Err("capture_staging_generation");
        }
        self.staged_output = None;
        let mut frames = 0;
        for (packet, p) in c.packets[..self.transport.entries()].iter().enumerate() {
            for f in 0..usize::from(p.actual / 24) {
                for ch in 0..8 {
                    let at = slot * self.transport.bytes() + packet * 312 + f * 24 + ch * 3;
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
    fn stage_output(&mut self, first: u64, frames: usize) -> Result<bool, &'static str> {
        self.staged_output = None;
        let position = first
            .checked_add(PLAYBACK_OFFSET)
            .ok_or("timeline_exhausted")?;
        if frames == 0
            || frames > self.transport.max_frames()
            || position
                .checked_add(frames as u64)
                .is_none_or(|end| end >= EPOCH_FRAMES)
        {
            return Err("timeline_exhausted");
        }
        // This is the first PCM read, including unsuccessful attempts. Publish
        // its requested end before staging so HAL margin telemetry stays honest.
        self.shared
            .playback_requested_end
            .store(position + frames as u64, SeqCst);
        // Bounded seqlock reads copy all channels of every tagged frame. A later
        // HAL rewrite cannot invalidate this owned snapshot; high-water is unused.
        let missing = self.shared.playback.read(
            self.offset + position,
            &mut self.scratch[..frames * CHANNELS],
        );
        if missing == 0 {
            self.staged_output = Some((first, frames));
        }
        Ok(missing == 0)
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
        let missing = if self.staged_output.take() == Some((first, frames)) {
            0
        } else {
            self.shared
                .playback
                .read(self.offset + position, &mut self.scratch[..frames * 8])
        };
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
        Some("native_completion_geometry") => 52,
        Some("pool_abi_mismatch") => 53,
        Some("capture_geometry_mismatch") => 54,
        Some("output_lead_deadline") => 55,
        Some("capture_staging_overrun") => 33,
        Some("capture_staging_generation") => 34,
        Some("capture_publication_rejected") => 35,
        Some("timeline_exhausted") => 36,
        Some("drain_deadline") => 37,
        Some("rate_write_failed") => 38,
        Some("rate_readback_failed") => 39,
        _ => 99,
    }
}
#[cfg(not(feature = "independent-4ms"))]
unsafe fn worker(
    shared: Arc<Shared>,
    factory: Factory,
    product: u32,
    location: u32,
    registry: u64,
    epoch: u64,
) {
    let ctx = (factory.open)(product, location, registry);
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
    if !ops.compatible() {
        shared.fault.store(15, SeqCst);
        let closed = (factory.close)(ctx);
        shared.set_phase(if closed == 0 { FAULT } else { QUARANTINED });
        return;
    }
    let mut info = Info::default();
    let mut rates = Rates::default();
    let mut config = vec![0; 65535];
    let mut length = 0;
    let mut records = [
        vec![Trace::default(); LIVE.history()],
        vec![Trace::default(); LIVE.history()],
    ];
    let mut session = PoolSession::with_geometry(epoch, info, LIVE)
        .with_output_policy(crate::output_admission::LIVE_OUTPUT_POLICY)
        .with_submission_order(crate::submission_order::LIVE);
    let mut activated = false;
    let mut policy_attempted = false;
    let mut startup = crate::startup::Snapshot::new(epoch);
    startup.header[3] = 1;
    shared.startup.publish(&startup);
    let config_status = (ops.base.config)(ctx, config.as_mut_ptr(), config.len(), &mut length);
    (ops.base.info)(ctx, &mut info);
    (ops.base.rates)(ctx, &mut rates);
    let before_rates = rates;
    startup.header[5] = config_status as i64 as u64;
    startup.before(rates);
    shared.startup.publish(&startup);
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
        startup.header[4] = 2;
        shared.startup.publish(&startup);
        session.stop("setup_precondition");
        shared.fault.store(3, SeqCst);
    } else {
        shared
            .worker_timing
            .configure_geometry(info.timebase_numer, info.timebase_denom, LIVE);
        shared
            .delivery
            .configure_geometry(info.timebase_numer, info.timebase_denom, LIVE);
        shared
            .pumps
            .configure(info.timebase_numer, info.timebase_denom);
        session = PoolSession::with_geometry(epoch, info, LIVE)
            .with_output_policy(crate::output_admission::LIVE_OUTPUT_POLICY)
            .with_submission_order(crate::submission_order::LIVE);
        shared.retry.publish(&session.retry.snapshot);
        startup.header[3] = 2;
        startup.header[10] = 1;
        shared.startup.publish(&startup);
        let prepared = (ops.prepare_geometry)(ctx, &LIVE);
        startup.header[6] = prepared as i64 as u64;
        startup.header[10] = 2;
        if prepared != 0 {
            startup.header[4] = 2;
            session.stop("prepare_failed");
            shared.fault.store(4, SeqCst);
        } else if shared.stop.load(SeqCst) {
            startup.header[4] = 3;
            session.stop("signal");
        } else {
            startup.header[3] = 3;
            startup.header[11] = 1;
            shared.startup.publish(&startup);
            let activation = (ops.base.activate)(ctx);
            startup.header[7] = activation as i64 as u64;
            startup.header[11] = 2;
            if activation != 0 {
                startup.header[4] = 2;
                session.stop("activate_failed");
                shared.fault.store(5, SeqCst);
            } else {
                activated = true;
                match crate::startup::initialize(
                    ctx,
                    ops,
                    factory,
                    &shared.startup,
                    &mut startup,
                    || shared.stop.load(SeqCst),
                ) {
                    Err(reason) => {
                        session.stop(reason);
                        if reason != "signal" {
                            shared.fault.store(14, SeqCst);
                        }
                    }
                    Ok(()) => {
                        policy_attempted = true;
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
            }
        }
        shared.startup.publish(&startup);
    }
    shared.clock.publish(None);
    let mut failure_finish = crate::first_failure::Finish {
        epoch,
        activated,
        before_rates,
        after_rates: None,
        before_info: session.info,
        after_info: None,
        cleanup: 0,
        close: None,
    };
    let owns_failure_record = shared.first_failure.latch(&session, &failure_finish);
    if shared.phase.load(SeqCst) != QUARANTINED {
        shared.set_phase(STOPPING);
    }
    // Restore normal scheduling before abort/drain and native teardown.
    if policy_attempted && (factory.configure_thread)(ctx, std::ptr::null()) != 0 {
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
    failure_finish.cleanup = cleanup;

    if cleanup != 0 || session.drain_failed {
        shared
            .first_failure
            .finish(&session, &failure_finish, owns_failure_record);
        // Quarantine at most this one native context. Never _Exit the HAL host,
        // free uncertain USB memory, or retry over outstanding callback debt.
        shared.fault.store(7, SeqCst);
        shared.phase.store(QUARANTINED, SeqCst);
        return;
    }
    (ops.base.info)(ctx, &mut info);
    (ops.base.rates)(ctx, &mut rates);
    failure_finish.after_info = Some(info);
    failure_finish.after_rates = Some(rates);
    let bad_postcondition =
        activated && (info.restored_mask != 3 || info.open_mask != 0 || !rates.valid());
    if bad_postcondition {
        shared.fault.store(10, SeqCst);
    }
    let closed = (factory.close)(ctx);
    failure_finish.close = Some(closed);
    shared
        .first_failure
        .finish(&session, &failure_finish, owns_failure_record);
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
        #[cfg(not(feature = "independent-4ms"))]
        shared.clock.publish(None);
        shared.stop.store(true, SeqCst);
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
    create_live(factory, product, location, 0)
}
/// HAL constructor: every open of this handle is pinned to `registry`, the
/// IORegistry entry ID (USB enumeration) of the adopted unit. Zero is refused.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_create_pinned(
    factory: *const Factory,
    product: u32,
    location: u32,
    registry: u64,
) -> *mut Live {
    if registry == 0 {
        return std::ptr::null_mut();
    }
    create_live(factory, product, location, registry)
}
unsafe fn create_live(
    factory: *const Factory,
    product: u32,
    location: u32,
    registry: u64,
) -> *mut Live {
    if factory.is_null() || !matches!(product, 0x2080 | 0x2081) {
        return std::ptr::null_mut();
    }
    Box::into_raw(Box::new(Live::pinned(
        Arc::new(Shared::new()),
        *factory,
        product,
        location,
        registry,
    )))
}
/// Current publication generation (advanced by rebind). Lock-free; 0 for null.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_binding(handle: *mut Live) -> u64 {
    handle.as_ref().map_or(0, |h| h.binding.load(SeqCst))
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_start(handle: *mut Live, client: u32) -> i32 {
    start_session(handle, client, None)
}
/// Start only if the binding generation read by the caller before queueing on
/// the session lock is still current: a Start issued against the previous
/// publication must never wait out a detach/rebind and open the new unit.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_start_bound(handle: *mut Live, client: u32, binding: u64) -> i32 {
    start_session(handle, client, Some(binding))
}
unsafe fn start_session(handle: *mut Live, client: u32, binding: Option<u64>) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Ok(mut c) = h.control.lock() else {
        return -1;
    };
    // A removed unit admits no new start until a control-thread rebind, and a
    // Start from an older publication never adopts the newer binding.
    if !h.attached.load(SeqCst) || binding.is_some_and(|b| b != h.binding.load(SeqCst)) {
        return -6;
    }
    // A reused ID whose pre-detach session was never acknowledged: its delayed
    // Stop would be indistinguishable from a Stop of the new session.
    if c.retired.contains(&client) {
        return -9;
    }
    let s = &h.shared;
    // Clients retained across a recoverable runtime fault: a Start (new or
    // repeated ID) may run the same bounded recovery policy as the monitor,
    // never more. Anything not recovered is refused as before (-2).
    if c.clients.contains(&client) {
        return if s.phase.load(SeqCst) == RUNNING || recover_locked(h, &mut c, binding) == 0 {
            0
        } else {
            -2
        };
    }
    if !c.clients.is_empty() {
        if c.slots_full()
            || (s.phase.load(SeqCst) != RUNNING && recover_locked(h, &mut c, binding) != 0)
        {
            return -2;
        }
        c.clients.push(client);
        c.publish_recovery(s);
        return 0;
    }
    if s.phase.load(SeqCst) == QUARANTINED {
        return -3;
    }
    if c.slots_full() {
        return -2;
    }
    let rc = launch(h, &mut c).status;
    // A client-initiated session restores the consecutive recovery budget.
    c.recovery.fresh_start(rc == 0, s.uptime_ms());
    if rc == 0 {
        c.clients.push(client);
    }
    c.publish_recovery(s);
    rc
}
struct LaunchOutcome {
    status: i32,
    start_wait_expired: bool,
}
impl LaunchOutcome {
    fn early_failure(status: i32) -> Self {
        Self {
            status,
            start_wait_expired: false,
        }
    }
}
/// Open a fresh epoch on the current binding: join the previous worker and
/// drain admitted realtime calls first, then reset timelines and wait (bounded)
/// for the new clock. Never touches clients. Caller holds the control lock.
fn launch(h: &Live, c: &mut Control) -> LaunchOutcome {
    let s = &h.shared;
    s.stop.store(true, SeqCst);
    if !finish(s, c) {
        return LaunchOutcome::early_failure(-3);
    }
    let epoch = s.epoch.load(SeqCst) + 1;
    if epoch > 0xffffff {
        return LaunchOutcome::early_failure(-3);
    }
    s.reset(epoch);
    let cloned = s.clone();
    let factory = h.factory;
    let product = h.product;
    let location = h.location.load(SeqCst);
    let registry = h.registry.load(SeqCst);
    match thread::Builder::new()
        .name("ftu-usb".into())
        .spawn(move || {
            guarded_worker(cloned.clone(), || unsafe {
                worker(cloned, factory, product, location, registry, epoch)
            })
        }) {
        Ok(w) => c.worker = Some(w),
        Err(_) => {
            s.set_phase(FAULT);
            return LaunchOutcome::early_failure(-4);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while s.phase.load(SeqCst) == STARTING && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    // Preserve timeout provenance before requesting cancellation. A guard
    // reached during teardown must not turn an expired startup into a retry.
    let start_wait_expired = Instant::now() >= deadline;
    let status = if s.phase.load(SeqCst) == RUNNING {
        0
    } else {
        s.request_stop();
        finish(s, c);
        -5
    };
    LaunchOutcome {
        status,
        start_wait_expired,
    }
}
/// Bounded automatic recovery, control lock held. Returns 0 recovered (fresh
/// epoch RUNNING), 1 nothing to do, 2 backoff, 4 worker finishing / IO draining,
/// -3 quarantined, -5 attempt did not reach RUNNING (ftuA may show a pending
/// retry after a clean deadline failure), -6 detached
/// or stale binding, -10 refused, -11 consecutive budget exhausted.
fn recover_locked(h: &Live, c: &mut Control, binding: Option<u64>) -> i32 {
    let s = &h.shared;
    let current = h.binding.load(SeqCst);
    let code = 'decide: {
        // Observation only: terminal quarantine must replace stale Healthy even
        // after detach retired every client. This branch can never open USB.
        if s.phase.load(SeqCst) == QUARANTINED {
            c.recovery
                .quarantined(s.epoch.load(SeqCst), s.reason.load(SeqCst).into());
            break 'decide recovery::QUARANTINE_STATUS;
        }
        // Never admit or revive a detached generation or a stale publication.
        if !h.attached.load(SeqCst) || binding.is_some_and(|b| b != current) {
            break 'decide -6;
        }
        c.recovery.note_binding(current);
        if c.clients.is_empty() {
            c.recovery.cancel();
            break 'decide 1;
        }
        let epoch = s.epoch.load(SeqCst);
        match s.phase.load(SeqCst) {
            RUNNING => {
                c.recovery.running(s.uptime_ms());
                break 'decide 1;
            }
            FAULT => (),
            _ => break 'decide 1,
        }
        // Do not even classify until the faulted worker has returned and every
        // admitted realtime call has left; finish() below then joins at once.
        if c.worker.as_ref().is_some_and(|w| !w.is_finished()) || s.calls.load(SeqCst) != 0 {
            break 'decide 4;
        }
        if let Some(code) = c.recovery.settled(epoch) {
            break 'decide code;
        }
        let fault = if recovery::ENABLED {
            recovery::classify(&s.runtime.snapshot(), epoch, s.fault.load(SeqCst))
        } else {
            Err(recovery::R_PROFILE)
        };
        match c.recovery.fault(fault, epoch, s.uptime_ms()) {
            Decision::Wait => 2,
            Decision::Refuse(code) => code,
            Decision::Attempt => {
                // Same Live, same pinned registry/location: launch reads both
                // from this handle, which only rebind (clients empty) changes.
                let launch = launch(h, c);
                let rc = launch.status;
                let reopened_epoch = s.epoch.load(SeqCst);
                let retry = if recovery::ENABLED
                    && rc == -5
                    && !launch.start_wait_expired
                    && reopened_epoch > epoch
                    && s.phase.load(SeqCst) == FAULT
                    && c.worker.is_none()
                    && s.calls.load(SeqCst) == 0
                {
                    // launch joined this worker and drained HAL admission.
                    // Only its own finished, cleanly closed OUT deadline is
                    // eligible; a stale record or other startup failure is not.
                    recovery::classify(&s.runtime.snapshot(), reopened_epoch, s.fault.load(SeqCst))
                        .ok()
                } else {
                    None
                };
                c.recovery
                    .attempted(rc, reopened_epoch, s.uptime_ms(), retry);
                if rc == 0 {
                    0
                } else if s.phase.load(SeqCst) == QUARANTINED {
                    -3
                } else {
                    -5
                }
            }
        }
    };
    c.publish_recovery(s);
    code
}
#[no_mangle]
pub unsafe extern "C" fn ftu_live_stop(handle: *mut Live, client: u32) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Ok(mut c) = h.control.lock() else {
        return -1;
    };
    // The delayed Stop of a client whose unit was detached only acknowledges
    // its retirement; it never touches the current session or other clients.
    if let Some(i) = c.retired.iter().position(|&id| id == client) {
        c.retired.swap_remove(i);
        c.publish_recovery(&h.shared);
        return 0;
    }
    // An unknown ID changes nothing for the remaining clients. A retained
    // client's Stop still means stop, including during a pending recovery.
    c.clients.retain(|&id| id != client);
    if !c.clients.is_empty() {
        c.publish_recovery(&h.shared);
        return 0;
    }
    // The last client cancels any pending recovery; nothing is revived later.
    c.recovery.cancel();
    c.publish_recovery(&h.shared);
    // Idempotent Stop must preserve Idle/Fault and terminal quarantine even
    // when the worker already finished before the control call arrived.
    h.shared.request_stop();
    if finish(&h.shared, &mut c) {
        0
    } else {
        -3
    }
}
/// Acknowledge a retired client without Stop semantics. 0 = a retirement was
/// consumed, 1 = the ID was not retired. Never touches active clients or the
/// session. (The HAL routes RemoveDeviceClient through Stop, which consumes a
/// retirement first in the same locked call.)
#[no_mangle]
pub unsafe extern "C" fn ftu_live_remove_client(handle: *mut Live, client: u32) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Ok(mut c) = h.control.lock() else {
        return -1;
    };
    match c.retired.iter().position(|&id| id == client) {
        Some(i) => {
            c.retired.swap_remove(i);
            c.publish_recovery(&h.shared);
            0
        }
        None => 1,
    }
}
/// Unit removal. Closes start admission, retires the IO clients of the removed
/// unit and stops/drains the worker under the ordinary bounded shutdown
/// protocol. Each retired ID is refused by Start (-9) until its delayed Stop or
/// RemoveDeviceClient acknowledges it; that Stop then changes nothing else.
/// Timeout or uncertain native ownership quarantines exactly as Stop does (-3);
/// never auto-recovered.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_detach(handle: *mut Live) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Ok(mut c) = h.control.lock() else {
        return -1;
    };
    h.attached.store(false, SeqCst);
    // Start admits a client only while active + retired < CLIENT_SLOTS, and
    // never a retired ID, so this move stays within the fixed budget.
    let Control {
        clients, retired, ..
    } = &mut *c;
    retired.append(clients);
    debug_assert!(retired.len() <= CLIENT_SLOTS);
    c.recovery.cancel();
    c.publish_recovery(&h.shared);
    h.shared.request_stop();
    if finish(&h.shared, &mut c) {
        0
    } else {
        -3
    }
}
/// Bind a detached handle to the unit enumerated as `registry` at `location`;
/// the next Start opens exactly that enumeration. Requires a nonzero registry
/// (-1), the same product (-7), a prior detach (-8) and a joined worker with no
/// admitted IO; quarantine is terminal (-3). Never opens USB itself. Retired
/// clients stay retired; the binding generation advances.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_rebind(
    handle: *mut Live,
    product: u32,
    location: u32,
    registry: u64,
) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let Ok(mut c) = h.control.lock() else {
        return -1;
    };
    if registry == 0 {
        return -1;
    }
    if product != h.product {
        return -7;
    }
    if h.attached.load(SeqCst) || !c.clients.is_empty() {
        return -8;
    }
    // Never rebind over a worker or callback that may still own native storage.
    if h.shared.phase.load(SeqCst) == QUARANTINED || !finish(&h.shared, &mut c) {
        return -3;
    }
    h.location.store(location, SeqCst);
    h.registry.store(registry, SeqCst);
    h.binding.fetch_add(1, SeqCst);
    h.attached.store(true, SeqCst);
    0
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
        location: h.location.load(SeqCst),
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
pub unsafe extern "C" fn ftu_live_worker_timing(handle: *mut Live, out: *mut WorkerSnapshot) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.worker_timing.snapshot();
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

#[no_mangle]
pub unsafe extern "C" fn ftu_live_delivery(
    handle: *mut Live,
    out: *mut crate::worker_delivery::Snapshot,
) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.delivery.snapshot();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ftu_live_pumps(
    handle: *mut Live,
    out: *mut crate::worker_pumps::Snapshot,
) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.pumps.snapshot();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ftu_live_first_failure(
    handle: *mut Live,
    out: *mut crate::first_failure::Snapshot,
) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.first_failure.snapshot();
    }
}

#[no_mangle]
pub unsafe extern "C" fn ftu_live_startup(handle: *mut Live, out: *mut crate::startup::Snapshot) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.startup.snapshot();
    }
}

/// Monitor-thread recovery driver; never called from realtime IO. Applies the
/// bounded policy only while `binding` is still the current publication and
/// admitted clients remain. A busy session lock returns 3 without blocking.
/// Other codes are those of recover_locked.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_recover(handle: *mut Live, binding: u64) -> i32 {
    let Some(h) = handle.as_ref() else {
        return -1;
    };
    let mut c = match h.control.try_lock() {
        Ok(c) => c,
        Err(TryLockError::WouldBlock) => return 3,
        Err(TryLockError::Poisoned(_)) => return -1,
    };
    recover_locked(h, &mut c, Some(binding))
}

/// ftuE: last runtime stop, retained across restarts and recovery. Read-only.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_runtime_fault(
    handle: *mut Live,
    out: *mut crate::runtime_fault::Snapshot,
) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.runtime.snapshot();
    }
}

/// ftuQ: exact native shutdown ownership and cleanup, retained across epochs.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_shutdown(handle: *mut Live, out: *mut crate::shutdown::Snapshot) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.shutdown.snapshot();
    }
}

/// ftuA: published recovery policy state and counters. Read-only, lock-free.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_recovery(handle: *mut Live, out: *mut RecoverySnapshot) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = RecoverySnapshot {
            words: h.shared.recovery.read().unwrap_or_else(|| {
                let mut w = [0; recovery::WORDS];
                w[0] = recovery::VERSION;
                w
            }),
        };
    }
}
#[repr(C)]
pub struct RecoverySnapshot {
    pub words: [u64; recovery::WORDS],
}

/// Read-only, bounded coherent retry diagnostics; lifecycle retains the last epoch.
#[no_mangle]
pub unsafe extern "C" fn ftu_live_retry(
    handle: *mut Live,
    out: *mut crate::output_retry::Snapshot,
) {
    if let (Some(h), Some(out)) = (handle.as_ref(), out.as_mut()) {
        *out = h.shared.retry.snapshot();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" fn unused_open(_: u32, _: u32, _: u64) -> *mut c_void {
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

    unsafe extern "C" fn unused_rate(_: *mut c_void, _: u32, _: u32, _: *mut u32) -> i32 {
        -1
    }
    fn unused_factory() -> Factory {
        Factory {
            open: unused_open,
            ops: unused_ops,
            close: unused_close,
            configure_thread: unused_schedule,
            set_rate: unused_rate,
        }
    }

    fn placement_fixture() -> (Live, AudioIo) {
        placement_fixture_for(crate::transport::BASELINE)
    }
    fn placement_fixture_for(transport: Transport) -> (Live, AudioIo) {
        let shared = Arc::new(Shared::new());
        shared.reset(1);
        shared.clock.publish(Some(ClockAnchor {
            epoch: 1,
            frame: 0,
            ticks: 1_000_000,
            ticks_per_frame: 500.0,
        }));
        shared.set_phase(RUNNING);
        let io = AudioIo::with_geometry(
            shared.clone(),
            1,
            Info {
                timebase_numer: 125,
                timebase_denom: 3,
                ..Info::default()
            },
            transport,
        );
        let live = Live::new(shared, unused_factory(), 0x2080, 123);
        (live, io)
    }
    fn frame_tags(first: u64, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|f| {
                (0..CHANNELS).map(move |ch| {
                    let frame = first + f as u64;
                    let value = ((frame % 10_000) * 8 + ch as u64 + 1) as i32;
                    let word = match ch {
                        6 => -value,
                        7 if frame.is_multiple_of(17) => 8_388_607,
                        7 if frame.is_multiple_of(19) => -8_388_608,
                        _ => value,
                    };
                    word as f32 / 8_388_608.0
                })
            })
            .collect()
    }
    #[test]
    #[cfg(not(feature = "independent-4ms"))]
    fn baseline_profile_never_retries_even_a_clean_deadline() {
        use crate::runtime_fault as rf;
        let (mut h, _io) = placement_fixture();
        h.shared
            .runtime
            .publish(&rf::Snapshot::primary(&rf::Primary {
                epoch: 1,
                reason: 55,
                site: Some(rf::Site {
                    stage: rf::STAGE_READINESS,
                    direction: 1,
                    required: 16,
                    ..rf::Site::default()
                }),
                ..rf::Primary::default()
            }));
        h.shared.runtime.finish(
            1,
            rf::Terminal {
                drain_failed: false,
                cleanup: 0,
                close: Some(0),
                phase: FAULT,
                fault: 6,
            },
        );
        h.shared.fault.store(6, SeqCst);
        h.shared.set_phase(FAULT);
        h.control.lock().unwrap().clients.push(7);
        assert!(recovery::classify(&h.shared.runtime.snapshot(), 1, 6).is_ok());
        let binding = h.binding.load(SeqCst);
        for _ in 0..3 {
            assert_eq!(unsafe { ftu_live_recover(&mut h, binding) }, -10);
        }
        let c = h.control.lock().unwrap();
        let w = c.recovery.words(0, c.clients.len(), c.retired.len());
        assert_eq!(
            (w[recovery::REFUSAL], w[recovery::ATTEMPTS]),
            (recovery::R_PROFILE, 0)
        );
        assert!(c.worker.is_none());
        assert_eq!(h.shared.epoch.load(SeqCst), 1);
    }

    #[test]
    fn real_hal_pcm_capture_path_preserves_sample_indices_across_ring_wrap() {
        for (ms, slots) in [(8, 4), (4, 8), (4, 10), (2, 16), (1, 32)] {
            let transport = Transport::new(ms, slots).unwrap();
            for buffer in [64, 128, 256, 512, 1024, 4096] {
                for maximum_packet in [false, true] {
                    let (mut live, mut io) = placement_fixture_for(transport);
                    // Every sample phase against each HAL buffer size. Reuse
                    // allocated storage; each case publishes exact absolute tags.
                    for phase in 0..buffer {
                        let position = 3 * RING_FRAMES as u64 - 96 + phase as u64;
                        live.shared.captured.store(position - 3 * 384, SeqCst);
                        let mut completion = Completion::default();
                        for (i, p) in completion.packets[..transport.entries()]
                            .iter_mut()
                            .enumerate()
                        {
                            p.requested = 312;
                            p.actual = if maximum_packet {
                                312
                            } else {
                                ((5 + i % 3) * 24) as u16
                            };
                            p.ticks = 1_000_000 + i as u64 * 3000;
                        }
                        let frames: usize = completion
                            .packets
                            .iter()
                            .map(|p| usize::from(p.actual / 24))
                            .sum();
                        let mut first = position / buffer as u64 * buffer as u64;
                        while first < position + frames as u64 {
                            let samples = frame_tags(first, buffer);
                            assert_eq!(
                                unsafe {
                                    ftu_live_write(
                                        &mut live,
                                        first,
                                        buffer as u32,
                                        samples.as_ptr(),
                                    )
                                },
                                0
                            );
                            first += buffer as u64;
                        }
                        let mut packed = vec![0; transport.bytes()];
                        io.output(position - PLAYBACK_OFFSET, frames, &mut packed)
                            .unwrap();
                        assert_eq!(live.shared.playback_missing.load(SeqCst), 0);
                        // A nominal analogue delay is a fixture only; preserve it in
                        // the input coordinate while exercising actual strided decode.
                        let input_first = position + 58;
                        let mut native_capture = vec![0; transport.bytes()];
                        let mut source = 0;
                        for (i, p) in completion.packets[..transport.entries()].iter().enumerate() {
                            let n = usize::from(p.actual);
                            native_capture[i * 312..i * 312 + n]
                                .copy_from_slice(&packed[source..source + n]);
                            source += n;
                        }
                        io.capture(0, &completion, &native_capture).unwrap();
                        io.ordered(0, input_first, &completion).unwrap();
                        let mut returned = vec![0.; frames * CHANNELS];
                        assert_eq!(
                            unsafe {
                                ftu_live_read(
                                    &mut live,
                                    input_first,
                                    frames as u32,
                                    returned.as_mut_ptr(),
                                )
                            },
                            0
                        );
                        assert_eq!(returned, frame_tags(position, frames));
                        assert_eq!(live.shared.capture_missing.load(SeqCst), 0);
                    }
                }
            }
        }
    }

    #[cfg(all(feature = "native-integration", not(feature = "independent-4ms")))]
    mod integrated_tests {
        use super::*;
        include!("integrated_tests.rs");
    }

    mod independent_tests {
        use super::*;
        include!("independent_tests.rs");
    }

    mod replay_tests {
        use super::*;
        include!("fixed_replay_tests.rs");
    }

    mod reconnect {
        use super::*;
        include!("reconnect_tests.rs");
    }

    struct PcmHal {
        live: Live,
        io: AudioIo,
        wall: Arc<AtomicU64>,
        buffer: usize,
        next: u64,
        write: Vec<f32>,
        read: Vec<f32>,
        steady_callbacks: usize,
        pause_start: u64,
        pause_frames: u64,
        steady_input_missing: u64,
        steady_output_missing: u64,
        min_lead_ms: u64,
        min_steady_lead_ms: u64,
        work: crate::pool_simulation::PcmWorkload,
        stage_paid: bool,
    }
    impl PcmHal {
        fn cost(&mut self) {
            if self.wall.load(SeqCst) / 1000 >= self.work.cost_start_frame {
                let end = crate::fixed_replay::work(
                    self.wall.load(SeqCst),
                    self.work.prepare_us,
                    self.work.fixed_absence,
                );
                self.wall.store(end, SeqCst);
            }
        }
        fn service(&mut self) {
            // Fake USB anchor is frame116. HAL progresses on nominal device
            // time, even while the USB owner cannot deliver a completion.
            let now = self
                .wall
                .load(SeqCst)
                .saturating_sub(self.work.hal_service_delay_us)
                .saturating_sub(116_000)
                * 48
                / 1000;
            loop {
                let due = if (self.pause_start..self.pause_start + self.pause_frames)
                    .contains(&self.next)
                {
                    self.pause_start + self.pause_frames
                } else {
                    self.next
                };
                if due > now {
                    break;
                }
                let out = self.next + u64::from(OUTPUT_SAFETY) + self.buffer as u64;
                for (i, s) in self.write.iter_mut().enumerate() {
                    *s = ((out + i as u64 / 8) % 10000 + 1) as f32 / 8_388_608.0;
                }
                assert_eq!(
                    unsafe {
                        ftu_live_write(&mut self.live, out, self.buffer as u32, self.write.as_ptr())
                    },
                    0
                );
                if self.next >= u64::from(INPUT_SAFETY) + self.buffer as u64 {
                    let before = self.live.shared.capture_missing.load(SeqCst);
                    let input = self.next - u64::from(INPUT_SAFETY) - self.buffer as u64;
                    assert_eq!(
                        unsafe {
                            ftu_live_read(
                                &mut self.live,
                                input,
                                self.buffer as u32,
                                self.read.as_mut_ptr(),
                            )
                        },
                        0
                    );
                    crate::capture_trace::record(
                        "hal_read",
                        116_000 + (due * 1000).div_ceil(48),
                        input,
                        self.buffer as u64,
                        self.live.shared.capture_missing.load(SeqCst) - before,
                        self.next,
                    );
                    // IN stops before the final OUT replay drains; those
                    // later callbacks are outside the capture stimulus.
                    if (48000..self.work.steady_end_frames).contains(&self.next) {
                        let missing = self.live.shared.capture_missing.load(SeqCst) - before;
                        self.steady_input_missing += missing;
                        if missing > 0 {
                            let first_missing = self
                                .read
                                .chunks_exact(8)
                                .position(|f| f.iter().all(|&v| v == 0.))
                                .unwrap();
                            crate::capture_trace::record(
                                "hal_missing",
                                116_000 + (due * 1000).div_ceil(48),
                                input + first_missing as u64,
                                missing,
                                self.live.shared.captured.load(SeqCst),
                                self.wall.load(SeqCst),
                            );
                        }
                        assert_eq!(
                            self.read
                                .chunks_exact(8)
                                .filter(|frame| frame.iter().all(|&x| x == 0.))
                                .count() as u64,
                            missing
                        );
                        assert!(self.read.iter().all(|&x| x == 0.25 || x == 0.));
                        self.steady_callbacks += 1;
                    }
                }
                self.next += self.buffer as u64;
            }
        }
    }
    impl StreamIo for PcmHal {
        fn output_retry(&mut self, s: &crate::output_retry::Snapshot) {
            self.io.output_retry(s);
        }
        fn submission_timing(&self) -> bool {
            true
        }
        fn submitted(&mut self, start: u64, _checked: u64, returned: Option<u64>) {
            self.service();
            let lead = start.saturating_sub(returned.unwrap());
            self.min_lead_ms = self.min_lead_ms.min(lead);
            if self.io.shared.captured.load(SeqCst) >= 48000 {
                self.min_steady_lead_ms = self.min_steady_lead_ms.min(lead);
            }
        }
        fn stage_output(&mut self, first: u64, frames: usize) -> Result<bool, &'static str> {
            self.service();
            let ready = self.io.stage_output(first, frames)?;
            // Charge work after the snapshot; later HAL publication cannot
            // repair or rewrite that owned PCM payload.
            self.cost();
            self.service();
            self.stage_paid = true;
            Ok(ready)
        }
        fn capture(&mut self, seq: usize, c: &Completion, b: &[u8]) -> Result<(), &'static str> {
            self.service();
            let end = crate::fixed_replay::work(
                self.wall.load(SeqCst),
                self.work.capture_us,
                self.work.fixed_absence,
            );
            self.wall.store(end, SeqCst);
            self.service();
            crate::capture_trace::record(
                "stage",
                self.wall.load(SeqCst),
                seq as u64,
                0,
                self.live.shared.captured.load(SeqCst),
                0,
            );
            self.io.capture(seq, c, b)
        }
        fn ordered(&mut self, seq: usize, first: u64, c: &Completion) -> Result<(), &'static str> {
            let result = self.io.ordered(seq, first, c);
            crate::capture_trace::record(
                "publish",
                self.wall.load(SeqCst),
                seq as u64,
                first,
                self.live.shared.captured.load(SeqCst),
                u64::from(result.is_ok()),
            );
            result
        }
        fn clock(&mut self, c: ClockSnapshot) -> Result<(), &'static str> {
            self.io.clock(c)
        }
        fn output(&mut self, first: u64, frames: usize, b: &mut [u8]) -> Result<(), &'static str> {
            let charge = !self.stage_paid;
            self.stage_paid = false;
            self.service();
            let before = self.live.shared.playback_missing.load(SeqCst);
            self.io.output(first, frames, b)?;
            if charge {
                self.cost();
                self.service();
            }
            if first >= 48000 {
                let missing = self.live.shared.playback_missing.load(SeqCst) - before;
                self.steady_output_missing += missing;
                let mut silent_frames = 0;
                for (i, frame) in b[..frames * 24].chunks_exact(24).enumerate() {
                    if frame.iter().all(|&x| x == 0) {
                        silent_frames += 1;
                    } else {
                        for chunk in frame.chunks_exact(3) {
                            assert_eq!(
                                ftu_protocol::decode_s24(chunk.try_into().unwrap()),
                                ((first + PLAYBACK_OFFSET + i as u64) % 10000 + 1) as i32
                            );
                        }
                    }
                }
                assert_eq!(silent_frames, missing);
            }
            Ok(())
        }
    }
    #[test]
    fn production_pool_services_real_pcm_rings_at_every_hal_phase() {
        pcm_phase_screen(crate::pool_simulation::Dispatch::Legacy);
    }
    #[test]
    fn separate_dispatch_preserves_original_pcm_phase_screen() {
        pcm_phase_screen(crate::pool_simulation::READY_DISPATCH);
    }
    fn pcm_phase_screen(dispatch: crate::pool_simulation::Dispatch) {
        use crate::output_admission::OutputPolicy::{Eager, WhenReady};
        for (geometry, policy, order) in [
            (
                crate::transport::SHORTER,
                Eager,
                crate::submission_order::SubmissionOrder::InputFirst,
            ),
            (
                crate::transport::SHORTER_TEN,
                Eager,
                crate::submission_order::SubmissionOrder::InputFirst,
            ),
            (
                crate::transport::SHORTER_TEN,
                WhenReady,
                crate::submission_order::SubmissionOrder::InputFirst,
            ),
            (
                crate::transport::SHORTER_TEN,
                WhenReady,
                crate::submission_order::SubmissionOrder::Deadline,
            ),
        ] {
            if dispatch != crate::pool_simulation::Dispatch::Legacy
                && order != crate::submission_order::SubmissionOrder::Deadline
            {
                continue;
            }
            for pump_step in if policy == WhenReady {
                vec![1, 4, 8]
            } else {
                vec![1]
            } {
                for buffer in [64, 128, 256, 512, 1024, 4096] {
                    // Keep the original delayed-publication disturbance: buffer +
                    // 832 frames, NOT a smaller block derived from the new geometry.
                    for (callback_delay, pause_frames) in [(5, 0), (0, buffer as u64 + 832)] {
                        let mut rejected = 0;
                        let mut maximum_missing = 0;
                        let mut minimum_lead = u64::MAX;
                        let mut steady_lead = u64::MAX;
                        let mut deferrals = 0;
                        let mut fallbacks = 0;
                        // gcd(buffer,192)=64: all distinct relative HAL/USB sample phases.
                        for phase in 0..64 {
                            let (live, io) = placement_fixture_for(geometry);
                            let wall = Arc::new(AtomicU64::new(100_000));
                            let work = crate::pool_simulation::PcmWorkload {
                                dispatch,
                                order,
                                callback_delay_ms: callback_delay,
                                pump_step_ms: pump_step,
                                ..Default::default()
                            };
                            let mut hal = PcmHal {
                                live,
                                io,
                                wall: wall.clone(),
                                buffer,
                                next: phase,
                                write: vec![0.; buffer * 8],
                                read: vec![0.; buffer * 8],
                                steady_callbacks: 0,
                                pause_start: phase
                                    + (72000_u64 - phase).div_ceil(buffer as u64) * buffer as u64,
                                pause_frames,
                                steady_input_missing: 0,
                                steady_output_missing: 0,
                                min_lead_ms: u64::MAX,
                                min_steady_lead_ms: u64::MAX,
                                work,
                                stage_paid: false,
                            };
                            let session = crate::pool_simulation::run_pcm_fixture(
                                geometry, wall, work, policy, &mut hal,
                            );
                            assert_eq!(session.reason, None, "buffer={buffer}, phase={phase}");
                            assert_eq!(session.frames[0], session.frames[1]);
                            assert_eq!(session.pending(), [0; 2]);
                            assert!(!session.drain_failed);
                            assert!(hal.steady_callbacks > 0);
                            assert_eq!(hal.steady_input_missing, 0);
                            rejected += usize::from(hal.steady_output_missing > 0);
                            maximum_missing = maximum_missing.max(hal.steady_output_missing);
                            minimum_lead = minimum_lead.min(hal.min_lead_ms);
                            steady_lead = steady_lead.min(hal.min_steady_lead_ms);
                            deferrals += session.output_deferrals;
                            fallbacks += session.output_deadline_fallbacks;
                        }
                        let tag = if dispatch == crate::pool_simulation::Dispatch::Legacy {
                            "PCM"
                        } else {
                            "PCM_DISPATCH"
                        };
                        println!("{tag} slots={} order={} ready_policy={} pump_ms={pump_step} buffer={buffer} callback_delay_ms={callback_delay} hal_pause_frames={pause_frames} phases=64 rejected={rejected} max_missing_out={maximum_missing} min_lead_ms={minimum_lead} min_steady_lead_ms={steady_lead} deferrals={deferrals} deadline_fallbacks={fallbacks}", geometry.slots(), order as u32, u8::from(policy == WhenReady));
                        if pause_frames == 0 || geometry.slots() == 8 || policy == WhenReady {
                            assert_eq!(rejected, 0);
                            assert!(minimum_lead >= 16);
                        } else {
                            // A passing regression test preserves this failed acceptance
                            // screen: extra queue coverage prepares PCM too early.
                            assert!(
                                rejected > 0,
                                "retain delayed-HAL rejection, do not hide missing samples"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn capture_drain_yields_to_ready_submissions_after_catch_up() {
        use crate::pool_simulation::{Dispatch, PcmWorkload};
        let geometry = crate::transport::SHORTER_TEN;
        let (live, io) = placement_fixture_for(geometry);
        let wall = Arc::new(AtomicU64::new(100_000));
        let work = PcmWorkload {
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
            prepare_us: 500,
            submit_us: [500, 1500],
            ..Default::default()
        };
        let mut hal = PcmHal {
            live,
            io,
            wall: wall.clone(),
            buffer: 4096,
            next: 1,
            write: vec![0.; 4096 * 8],
            read: vec![0.; 4096 * 8],
            steady_callbacks: 0,
            pause_start: 1 + (72000_u64 - 1).div_ceil(4096) * 4096,
            pause_frames: 4096 + 832,
            steady_input_missing: 0,
            steady_output_missing: 0,
            min_lead_ms: u64::MAX,
            min_steady_lead_ms: u64::MAX,
            work,
            stage_paid: false,
        };
        let trace = crate::capture_trace::Scope::new(1_920_000, 1_960_000);
        let session = crate::pool_simulation::run_pcm_fixture(
            geometry,
            wall,
            work,
            crate::output_admission::OutputPolicy::WhenReady,
            &mut hal,
        );
        assert_eq!(session.reason, None);
        assert_eq!(session.completed, [600; 2]);
        assert_eq!(session.frames[0], session.frames[1]);
        assert_eq!(session.pending(), [0; 2]);
        assert!(!session.drain_failed);
        assert!(hal.min_lead_ms >= 16);
        assert_eq!(hal.steady_input_missing, 0);
        assert_eq!(hal.steady_output_missing, 0);
        println!(
            "DRAIN_LATE_HAL reason={:?} failure={:?} lead={} missing_in={} missing_out={}",
            session.reason,
            session.submission_failure.as_ref().map(|f| (
                f.direction,
                f.sequence,
                f.start,
                f.checked,
                f.returned
            )),
            hal.min_lead_ms,
            hal.steady_input_missing,
            hal.steady_output_missing
        );
        for e in trace.finish() {
            println!(
                "DRAIN_EVENT kind={} us={} seq={} a={} b={} c={}",
                e.kind, e.us, e.seq, e.a, e.b, e.c
            );
        }
    }
    #[test]
    fn trace_capture_delivery_failures() {
        capture_delivery_trace_screen(false);
    }
    #[test]
    fn bounded_capture_dispatch_trace_screen() {
        capture_delivery_trace_screen(true);
    }
    fn capture_delivery_trace_screen(service_ready: bool) {
        use crate::output_admission::OutputPolicy::WhenReady;
        use crate::pool_simulation::{PcmWorkload, READY_DISPATCH};
        use crate::submission_order::SubmissionOrder::Deadline;
        let geometry = crate::transport::SHORTER_TEN;
        for buffer in [64, 512, 4096] {
            for cost in [0, 1000] {
                let mut failures = 0;
                for phase in 0..64 {
                    let (live, io) = placement_fixture_for(geometry);
                    let wall = Arc::new(AtomicU64::new(100_000));
                    let work = PcmWorkload {
                        dispatch: if service_ready {
                            crate::pool_simulation::Dispatch::Separate {
                                ready_first: true,
                                callback_us: 50,
                                entry_us: 0,
                                batch: 1,
                            }
                        } else {
                            READY_DISPATCH
                        },
                        service_ready,
                        order: Deadline,
                        pump_step_ms: 8,
                        cost_start_frame: 1616,
                        submit_us: [cost; 2],
                        stall_at_frame: 1616 + phase % 8,
                        stall_us: 12500,
                        ..Default::default()
                    };
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
                    let trace = crate::capture_trace::Scope::new(1_590_000, 1_680_000);
                    let session = crate::pool_simulation::run_pcm_fixture(
                        geometry, wall, work, WhenReady, &mut hal,
                    );
                    let events = trace.finish();
                    assert_eq!(session.reason, None);
                    assert_eq!(session.completed, [600; 2]);
                    assert_eq!(session.frames[0], session.frames[1]);
                    assert_eq!(session.pending(), [0; 2]);
                    assert!(!session.drain_failed);
                    assert_eq!(hal.steady_output_missing, 0);
                    assert!(hal.min_lead_ms >= 16);
                    if hal.steady_input_missing > 0 || session.reason.is_some() {
                        failures += 1;
                        println!("CAPTURE_FAILURE interleaved={} buffer={buffer} cost_us={cost} phase={phase} reason={:?} missing={} lead={}",u8::from(service_ready),session.reason,hal.steady_input_missing,hal.min_lead_ms);
                        if failures == 1 {
                            let missing = events.iter().find(|e| e.kind == "hal_missing").unwrap();
                            let published = events
                                .iter()
                                .find(|e| {
                                    e.kind == "publish" && e.a <= missing.seq && missing.seq < e.b
                                })
                                .unwrap();
                            let delivered = events
                                .iter()
                                .find(|e| e.kind == "deliver" && e.a == 0 && e.seq == published.seq)
                                .unwrap();
                            if service_ready {
                                let idle = events
                                    .iter()
                                    .rfind(|e| e.kind == "idle_begin" && e.us <= delivered.b)
                                    .unwrap();
                                let stall_end = events
                                    .iter()
                                    .find(|e| e.kind == "stall_end" && e.us > delivered.b)
                                    .unwrap();
                                // Request was not ready when this idle began;
                                // the HAL deadline expires before the owner returns.
                                assert!(idle.us < delivered.b);
                                assert!(delivered.b < missing.us);
                                assert!(missing.us < stall_end.us);
                                assert!(stall_end.us < delivered.us);
                                assert_eq!(published.us, delivered.us);
                                println!("CAPTURE_UNAVAILABLE buffer={buffer} phase={phase} idle_begin_us={} physically_ready_us={} hal_deadline_us={} owner_return_us={} delivered_us={} published_us={}", idle.us,delivered.b,missing.us,stall_end.us,delivered.us,published.us);
                            } else if buffer == 64 && cost == 0 {
                                // The original failure is publication batching,
                                // not a physically late callback.
                                assert!(delivered.us < missing.us);
                                assert!(missing.us < published.us);
                            }
                            for e in events {
                                println!("CAPTURE_EVENT buffer={buffer} cost_us={cost} phase={phase} kind={} us={} seq={} a={} b={} c={}",e.kind,e.us,e.seq,e.a,e.b,e.c);
                            }
                        }
                    }
                }
                let expected = match (service_ready, buffer, cost) {
                    (true, 64 | 512, 1000) => 8,
                    (false, 64, 0) => 17,
                    (false, 64, 1000) => 10,
                    (false, 512, 1000) => 8,
                    _ => 0,
                };
                assert_eq!(failures, expected, "retain the failed acceptance cases");
                println!("CAPTURE_TRACED interleaved={} buffer={buffer} cost_us={cost} failures={failures}",u8::from(service_ready));
            }
        }
    }
    #[test]
    fn combined_stall_and_submission_costs_keep_the_same_acceptance_gates() {
        use crate::output_admission::OutputPolicy::{Eager, WhenReady};
        use crate::submission_order::SubmissionOrder::InputFirst;
        cost_screen(
            &[(Eager, InputFirst), (WhenReady, InputFirst)],
            crate::pool_simulation::Dispatch::Legacy,
        );
    }
    #[test]
    fn deadline_order_combined_cost_screen() {
        cost_screen(
            &[(
                crate::output_admission::OutputPolicy::WhenReady,
                crate::submission_order::SubmissionOrder::Deadline,
            )],
            crate::pool_simulation::Dispatch::Legacy,
        );
    }
    #[test]
    fn separate_callback_dispatch_cost_screen() {
        cost_screen(
            &[(
                crate::output_admission::OutputPolicy::WhenReady,
                crate::submission_order::SubmissionOrder::Deadline,
            )],
            crate::pool_simulation::READY_DISPATCH,
        );
    }
    #[test]
    fn wait_first_callback_dispatch_cost_screen() {
        cost_screen(
            &[(
                crate::output_admission::OutputPolicy::WhenReady,
                crate::submission_order::SubmissionOrder::Deadline,
            )],
            crate::pool_simulation::Dispatch::Separate {
                ready_first: false,
                callback_us: 50,
                entry_us: 0,
                batch: 10,
            },
        );
    }
    #[test]
    fn dispatch_cost_and_entry_delay_sensitivity() {
        use crate::pool_simulation::Dispatch::Separate;
        // Eight representative phases per row, distinct from the full 64-phase
        // comparison. Charge entry delay even when a ready callback avoids idle.
        for (callback_us, entry_us, batch) in [
            (0, 0, 10),
            (50, 0, 10),
            (50, 250, 10),
            (50, 1000, 10),
            (50, 8000, 10),
            (250, 0, 10),
            (1000, 0, 10),
            (50, 0, 1),
            (50, 0, 2),
        ] {
            cost_screen_phases(
                &[(
                    crate::output_admission::OutputPolicy::WhenReady,
                    crate::submission_order::SubmissionOrder::Deadline,
                )],
                Separate {
                    ready_first: true,
                    callback_us,
                    entry_us,
                    batch,
                },
                true,
                false,
            );
        }
    }
    #[test]
    fn bounded_capture_dispatch_cost_screen() {
        cost_screen_phases(
            &[(
                crate::output_admission::OutputPolicy::WhenReady,
                crate::submission_order::SubmissionOrder::Deadline,
            )],
            crate::pool_simulation::Dispatch::Separate {
                ready_first: true,
                callback_us: 50,
                entry_us: 0,
                batch: 1,
            },
            false,
            true,
        );
    }
    #[test]
    fn bounded_capture_dispatch_sensitivity() {
        for (callback_us, entry_us, batch) in [
            (0, 0, 1),
            (50, 0, 1),
            (50, 250, 1),
            (50, 1000, 1),
            (50, 8000, 1),
            (250, 0, 1),
            (1000, 0, 1),
            (50, 0, 2),
            (50, 0, 10),
        ] {
            cost_screen_phases(
                &[(
                    crate::output_admission::OutputPolicy::WhenReady,
                    crate::submission_order::SubmissionOrder::Deadline,
                )],
                crate::pool_simulation::Dispatch::Separate {
                    ready_first: true,
                    callback_us,
                    entry_us,
                    batch,
                },
                true,
                true,
            );
        }
    }
    fn cost_screen(
        policies: &[(
            crate::output_admission::OutputPolicy,
            crate::submission_order::SubmissionOrder,
        )],
        dispatch: crate::pool_simulation::Dispatch,
    ) {
        cost_screen_phases(policies, dispatch, false, false);
    }
    fn cost_screen_phases(
        policies: &[(
            crate::output_admission::OutputPolicy,
            crate::submission_order::SubmissionOrder,
        )],
        dispatch: crate::pool_simulation::Dispatch,
        sensitivity: bool,
        service_ready: bool,
    ) {
        use crate::output_admission::OutputPolicy::WhenReady;
        use crate::output_retry as retry;
        use crate::pool_simulation::PcmWorkload;
        let geometry = crate::transport::SHORTER_TEN;
        // Cost is recurring, on each indicated native call after the steady
        // boundary. Startup-cost is separate so priming failures stay visible.
        let base = PcmWorkload {
            dispatch,
            service_ready,
            pump_step_ms: 8,
            cost_start_frame: 1616,
            ..Default::default()
        };
        let cases = [
            ("control", base, false),
            (
                "submit1",
                PcmWorkload {
                    submit_us: [1000; 2],
                    ..base
                },
                false,
            ),
            (
                "submit2",
                PcmWorkload {
                    submit_us: [2000; 2],
                    ..base
                },
                false,
            ),
            (
                "out1",
                PcmWorkload {
                    submit_us: [0, 1000],
                    ..base
                },
                false,
            ),
            (
                "out2",
                PcmWorkload {
                    submit_us: [0, 2000],
                    ..base
                },
                false,
            ),
            (
                "stall8_cost1",
                PcmWorkload {
                    stall_at_frame: 1616,
                    stall_us: 8000,
                    submit_us: [1000; 2],
                    ..base
                },
                false,
            ),
            (
                "stall12_5",
                PcmWorkload {
                    stall_at_frame: 1616,
                    stall_us: 12500,
                    ..base
                },
                false,
            ),
            (
                "stall12_5_cost1",
                PcmWorkload {
                    stall_at_frame: 1616,
                    stall_us: 12500,
                    submit_us: [1000; 2],
                    ..base
                },
                false,
            ),
            (
                "stall12_5_cost2",
                PcmWorkload {
                    stall_at_frame: 1616,
                    stall_us: 12500,
                    submit_us: [2000; 2],
                    ..base
                },
                false,
            ),
            (
                "uniform5_cost1",
                PcmWorkload {
                    uniform_delay: true,
                    callback_delay_ms: 5,
                    submit_us: [1000; 2],
                    ..base
                },
                false,
            ),
            (
                "uniform5_cost2",
                PcmWorkload {
                    uniform_delay: true,
                    callback_delay_ms: 5,
                    submit_us: [2000; 2],
                    ..base
                },
                false,
            ),
            ("late_hal", base, true),
            (
                "late_hal_cost1",
                PcmWorkload {
                    submit_us: [1000; 2],
                    ..base
                },
                true,
            ),
            (
                "late_hal_cost2",
                PcmWorkload {
                    submit_us: [2000; 2],
                    ..base
                },
                true,
            ),
            (
                "prepare_submit2",
                PcmWorkload {
                    prepare_us: 500,
                    submit_us: [500, 1500],
                    ..base
                },
                true,
            ),
            (
                "startup_cost2",
                PcmWorkload {
                    cost_start_frame: 0,
                    submit_us: [2000; 2],
                    ..base
                },
                false,
            ),
            (
                "beyond32",
                PcmWorkload {
                    stall_at_frame: 1616,
                    stall_us: 32000,
                    ..base
                },
                false,
            ),
        ];
        for &(policy, order) in policies {
            for buffer in [64, 512, 4096] {
                for (name, work, late_hal) in cases {
                    if sensitivity
                        && !matches!(
                            name,
                            "control"
                                | "submit1"
                                | "submit2"
                                | "stall8_cost1"
                                | "stall12_5_cost1"
                                | "uniform5_cost1"
                                | "prepare_submit2"
                                | "late_hal_cost1"
                        )
                    {
                        continue;
                    }
                    let phases: Vec<u64> = if sensitivity {
                        vec![0, 1, 15, 16, 31, 32, 62, 63]
                    } else {
                        (0..64).collect()
                    };
                    let phase_count = phases.len();
                    let (mut accepted, mut faults, mut out_loss, mut in_loss) = (0, 0, 0, 0);
                    let (mut min_lead, mut max_retry, mut over_budget) = (u64::MAX, 0, 0);
                    let (
                        mut idle,
                        mut callbacks,
                        mut callback_work,
                        mut entry_work,
                        mut stalls,
                        mut max_batch,
                    ) = (0, 0, 0, 0, 0, 0);
                    let mut reasons = std::collections::BTreeSet::new();
                    for phase in phases {
                        let work = PcmWorkload {
                            order,
                            stall_at_frame: if work.stall_at_frame == 0 {
                                0
                            } else {
                                work.stall_at_frame + phase % 8
                            },
                            ..work
                        };
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
                            pause_start: phase
                                + (72000_u64 - phase).div_ceil(buffer as u64) * buffer as u64,
                            pause_frames: if late_hal { buffer as u64 + 832 } else { 0 },
                            steady_input_missing: 0,
                            steady_output_missing: 0,
                            min_lead_ms: u64::MAX,
                            min_steady_lead_ms: u64::MAX,
                            work,
                            stage_paid: false,
                        };
                        let (session, ds) = crate::pool_simulation::run_pcm_dispatch_fixture(
                            geometry, wall, work, policy, &mut hal,
                        );
                        idle += ds.idle_calls;
                        callbacks += ds.callbacks;
                        callback_work += ds.callback_us;
                        entry_work += ds.entry_us;
                        stalls += ds.stalls;
                        max_batch = max_batch.max(ds.max_batch);
                        if let crate::pool_simulation::Dispatch::Separate {
                            callback_us,
                            entry_us,
                            batch,
                            ..
                        } = dispatch
                        {
                            assert!(ds.max_batch <= batch);
                            assert_eq!(ds.callback_us, ds.callbacks * callback_us);
                            assert_eq!(ds.entry_us, ds.calls * entry_us);
                            assert_eq!(ds.idle_us, ds.idle_calls * work.pump_step_ms * 1000);
                            assert_eq!(ds.stall_us, ds.stalls * work.stall_us);
                            if session.reason.is_none() && work.stall_at_frame > 0 {
                                assert_eq!(ds.stalls, 1);
                            }
                        }
                        assert_eq!(session.pending(), [0; 2]);
                        assert!(!session.drain_failed);
                        let pass = session.reason.is_none()
                            && session.completed == [600; 2]
                            && session.frames[0] == session.frames[1]
                            && hal.steady_callbacks > 0
                            && hal.steady_input_missing == 0
                            && hal.steady_output_missing == 0
                            && hal.min_lead_ms >= 16;
                        accepted += usize::from(pass);
                        faults += usize::from(session.reason.is_some());
                        if let Some(reason) = session.reason {
                            reasons.insert(reason);
                        }
                        out_loss = out_loss.max(hal.steady_output_missing);
                        in_loss = in_loss.max(hal.steady_input_missing);
                        min_lead = min_lead.min(hal.min_lead_ms);
                        if policy == WhenReady {
                            let snapshot = hal.io.shared.retry.snapshot();
                            assert_eq!(snapshot, session.retry.snapshot);
                            let all = snapshot.counts[0];
                            assert_eq!(
                                all[retry::ATTEMPTS],
                                all[retry::READY] + all[retry::DEFERRALS] + all[retry::FALLBACKS]
                            );
                            assert_eq!(
                                all[retry::DEFERRALS] - all[retry::RETRIES],
                                snapshot.header[11]
                            );
                            assert_eq!(all[retry::DEFERRALS], session.output_deferrals);
                            assert_eq!(all[retry::FALLBACKS], session.output_deadline_fallbacks);
                            assert!(
                                all[retry::OVER_BUDGET] + all[retry::INVALID_TIME]
                                    <= all[retry::RETRIES]
                            );
                            max_retry = max_retry.max(all[retry::MAX_NS]);
                            over_budget += all[retry::OVER_BUDGET];
                        }
                    }
                    if dispatch == crate::pool_simulation::Dispatch::Legacy {
                        println!("COST case={name} order={} ready_policy={} buffer={buffer} phases=64 accepted={accepted} faults={faults} max_missing_in={in_loss} max_missing_out={out_loss} min_lead_bus_frames={min_lead} max_retry_ns={max_retry} over_budget={over_budget} reasons={reasons:?}",order as u32,u8::from(policy==WhenReady));
                        if name == "control" || (name == "late_hal" && policy == WhenReady) {
                            assert_eq!(accepted, 64);
                        }
                        if matches!(name, "beyond32" | "startup_cost2") {
                            assert_eq!(accepted, 0);
                        }
                        if order == crate::submission_order::SubmissionOrder::Deadline {
                            let expected = match name {
                                "control" | "submit1" | "out1" | "out2" | "stall8_cost1"
                                | "stall12_5" | "late_hal" | "late_hal_cost1" => 64,
                                "stall12_5_cost1" => 40,
                                "uniform5_cost1" => {
                                    if buffer == 64 {
                                        31
                                    } else {
                                        64
                                    }
                                }
                                _ => 0,
                            };
                            assert_eq!(
                                accepted, expected,
                                "preserve measured gate result: {name}, {buffer}"
                            );
                        }
                    } else {
                        let crate::pool_simulation::Dispatch::Separate {
                            ready_first,
                            callback_us,
                            entry_us,
                            batch,
                        } = dispatch
                        else {
                            unreachable!()
                        };
                        let tag = if service_ready && sensitivity {
                            "CAPTURE_SENSITIVITY"
                        } else if service_ready {
                            "CAPTURE_DISPATCH"
                        } else if sensitivity {
                            "SENSITIVITY"
                        } else {
                            "DISPATCH"
                        };
                        println!("{tag} case={name} ready_first={} callback_cost_us={callback_us} entry_cost_us={entry_us} batch_limit={batch} buffer={buffer} phases={phase_count} accepted={accepted} faults={faults} max_missing_in={in_loss} max_missing_out={out_loss} min_lead_bus_frames={min_lead} max_retry_ns={max_retry} over_budget={over_budget} idle_calls={idle} callbacks={callbacks} callback_us={callback_work} entry_us={entry_work} stalls={stalls} max_batch={max_batch} reasons={reasons:?}", u8::from(ready_first));
                        if service_ready && !sensitivity {
                            let expected = match name {
                                "submit2" | "stall12_5_cost2" | "uniform5_cost2"
                                | "late_hal_cost2" | "startup_cost2" | "beyond32" => 0,
                                "stall12_5_cost1" if buffer < 4096 => 56,
                                _ => 64,
                            };
                            assert_eq!(
                                accepted, expected,
                                "capture drain acceptance: {name}, {buffer}"
                            );
                            if expected > 0 {
                                assert_eq!(faults, 0);
                                assert_eq!(out_loss, 0);
                                assert!(min_lead >= 16);
                            }
                        }
                        // Saturation is a negative control with paid callback work.
                        // The explicitly optimistic zero-cost sensitivity control
                        // may fit exactly; do not invent a rejection or change its gates.
                        if matches!(name, "beyond32" | "startup_cost2")
                            || (name == "submit2" && callback_us > 0)
                        {
                            assert_eq!(accepted, 0);
                        }
                    }
                    // A passing regression test includes correct rejection of
                    // the failed rows; it does not qualify the whole matrix.
                }
            }
        }
    }

    #[test]
    fn retry_property_retains_terminal_epoch_and_resets_on_new_start() {
        use crate::output_admission::OutputPolicy::WhenReady;
        use crate::output_retry::{Snapshot, Tracker, DEFERRALS};
        let (mut live, mut io) = placement_fixture_for(crate::transport::SHORTER_TEN);
        let mut tracker = Tracker::new(23, WhenReady, 125, 3);
        tracker.begin(48000, 1);
        tracker.decision(48000, false, true, 1);
        io.output_retry(&tracker.snapshot);
        let mut snapshot = Snapshot::new(0, WhenReady, 0, 0);
        unsafe {
            ftu_live_retry(&mut live, &mut snapshot);
        }
        assert_eq!(snapshot, tracker.snapshot);
        live.shared.set_phase(FAULT);
        unsafe {
            ftu_live_retry(&mut live, &mut snapshot);
        }
        assert_eq!(snapshot.header[2], 23);
        assert_eq!(snapshot.header[11], 1);
        assert_eq!(snapshot.counts[0][DEFERRALS], 1);
        live.shared.reset(24);
        unsafe {
            ftu_live_retry(&mut live, &mut snapshot);
        }
        assert_eq!(snapshot.header[2], 24);
        assert_eq!(snapshot.header[11], 0);
        assert_eq!(snapshot.counts, [[0; crate::output_retry::COUNTS]; 2]);
    }

    #[test]
    fn staged_pcm_survives_hal_rewrite_and_holes_remain_missing() {
        let (mut live, mut io) = placement_fixture_for(crate::transport::SHORTER_TEN);
        let position = 3 * RING_FRAMES as u64 - 96;
        live.shared.captured.store(position - 3 * 384, SeqCst);
        let original = frame_tags(position, 192);
        assert_eq!(
            unsafe { ftu_live_write(&mut live, position, 192, original.as_ptr()) },
            0
        );
        assert!(io.stage_output(position - PLAYBACK_OFFSET, 192).unwrap());
        assert_eq!(
            live.shared.playback_requested_end.load(SeqCst),
            position + 192
        );
        let replacement = vec![0.75; 192 * 8];
        assert_eq!(
            unsafe { ftu_live_write(&mut live, position, 192, replacement.as_ptr()) },
            0
        );
        let mut packed = vec![0; io.transport.bytes()];
        io.output(position - PLAYBACK_OFFSET, 192, &mut packed)
            .unwrap();
        for (sample, chunk) in original.iter().zip(packed.chunks_exact(3)) {
            assert_eq!(
                ftu_protocol::decode_s24(chunk.try_into().unwrap()),
                (*sample * 8_388_608.0) as i32
            );
        }
        let hole = position + 192;
        let future = frame_tags(hole + 1, 192);
        assert_eq!(
            unsafe { ftu_live_write(&mut live, hole + 1, 192, future.as_ptr()) },
            0
        );
        assert!(live.shared.playback.high_water() > io.offset + hole + 192);
        let before = live.shared.playback_missing.load(SeqCst);
        assert!(!io.stage_output(hole - PLAYBACK_OFFSET, 192).unwrap());
        assert_eq!(live.shared.playback_missing.load(SeqCst), before);
        io.output(hole - PLAYBACK_OFFSET, 192, &mut packed).unwrap();
        assert_eq!(live.shared.playback_missing.load(SeqCst), before + 1);
        assert_eq!(&packed[..24], &[0; 24]);
    }

    #[test]
    fn real_packing_reports_a_hole_despite_future_publication_and_late_repair() {
        let (mut live, mut io) = placement_fixture();
        let position = 3 * RING_FRAMES as u64 - 96;
        live.shared.captured.store(position - 3 * 384, SeqCst);
        for (first, frames) in [(position, 9), (position + 10, 758)] {
            let samples = frame_tags(first, frames);
            assert_eq!(
                unsafe { ftu_live_write(&mut live, first, frames as u32, samples.as_ptr()) },
                0
            );
        }
        assert!(live.shared.playback.high_water() > EPOCH_FRAMES + position + 384);
        let mut packed = vec![0; BLOCK_BYTES];
        io.output(position - PLAYBACK_OFFSET, 384, &mut packed)
            .unwrap();
        assert_eq!(live.shared.playback_missing.load(SeqCst), 1);
        assert_eq!(&packed[9 * 24..10 * 24], &[0; 24]);
        let repaired = frame_tags(position + 9, 1);
        assert_eq!(
            unsafe { ftu_live_write(&mut live, position + 9, 1, repaired.as_ptr()) },
            0
        );
        let mut newer = vec![0; BLOCK_BYTES];
        io.output(position - PLAYBACK_OFFSET, 384, &mut newer)
            .unwrap();
        assert_ne!(&newer[9 * 24..10 * 24], &[0; 24]);
        assert_eq!(live.shared.playback_missing.load(SeqCst), 1);
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
        let mut live = Live::new(shared.clone(), unused_factory(), 0x2080, 123);
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

#[cfg(feature = "independent-4ms")]
#[path = "independent_live.rs"]
mod independent_live;
#[cfg(feature = "independent-4ms")]
use independent_live::worker;
