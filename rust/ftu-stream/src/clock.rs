//! Diagnostic sample clock for the Fast Track Ultra continuous USB engine.
//!
//! Fits cumulative captured sample frames against host time (mach absolute
//! time converted through the numer/denom timebase) with an incremental,
//! numerically stable least-squares regression. One observation is fed per
//! 1 ms group of eight microframes, using its final processing timestamp.
//! Timestamps within a group may be equal or increasing. `sample_frame` is the
//! cumulative count of frames actually captured before the group.
//!
//! This is a diagnostic measurement only. It is not a HAL / zero-timestamp
//! reference, does not extrapolate, does not invent nominal timestamps and
//! makes no claim about the device oscillator calibration.
//!
//! Policy:
//! - Each stream run has its own non-zero epoch; a restarted stream gets a new
//!   `SampleClock`. Observations for another epoch are rejected without
//!   touching state.
//! - Any in-epoch clock error (zero/backwards/duplicate timestamps or frames,
//!   gaps, overflow, implausible rate) latches the clock invalid until a new
//!   `SampleClock` is created. There is no automatic re-anchoring.
//! - The fit becomes valid only with >= 64 observations spanning >= 100 ms and
//!   a rate within 48 kHz +/- 1 %.

const ERR_INVALID_TIMEBASE: &str = "invalid timebase";
const ERR_ZERO_EPOCH: &str = "zero epoch";
const ERR_STALE_EPOCH: &str = "stale epoch";
const ERR_EPOCH_MISMATCH: &str = "epoch mismatch";
const ERR_INVALIDATED: &str = "clock invalidated";
const ERR_ZERO_TIMESTAMP: &str = "zero timestamp";
const ERR_TICKS_NOT_INCREASING: &str = "timestamp not increasing";
const ERR_FRAMES_NOT_INCREASING: &str = "sample frame not increasing";
const ERR_TIMESTAMP_OVERFLOW: &str = "timestamp overflow";
const ERR_GAP_LONG: &str = "clock gap: interval too long";
const ERR_GAP_SHORT: &str = "clock gap: interval too short";
const ERR_IMPLAUSIBLE_RATE: &str = "implausible rate";
const ERR_GROUP_ORDER: &str = "USB timestamp group not monotonic";

const NOMINAL_RATE_HZ: f64 = 48_000.0;
const RATE_TOLERANCE: f64 = 0.01;
const MIN_OBSERVATIONS: u64 = 64;
const MIN_SPAN_NS: u64 = 100_000_000;
const MAX_INTERVAL_NS: u64 = 5_000_000;
const MIN_INTERVAL_NS: u64 = 500_000;

#[derive(Debug)]
pub struct SampleClock {
    epoch: u64,
    numer: u32,
    denom: u32,
    config_error: Option<&'static str>,
    faulted: bool,
    observations: u64,
    first_frame: u64,
    last_frame: u64,
    first_ticks: u64,
    last_ticks: u64,
    span_ns: u64,
    // Welford accumulators over (x = ns since first, y = frames since first).
    mean_x: f64,
    mean_y: f64,
    m2_x: f64,
    c_xy: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct ClockSnapshot {
    pub epoch: u64,
    pub observations: u64,
    pub first_frame: u64,
    pub last_frame: u64,
    pub first_ticks: u64,
    pub last_ticks: u64,
    pub valid: bool,
    pub rate_hz: Option<f64>,
    pub span_ns: u64,
}

impl SampleClock {
    pub fn new(epoch: u64, numer: u32, denom: u32) -> Self {
        let config_error = if numer == 0 || denom == 0 {
            Some(ERR_INVALID_TIMEBASE)
        } else if epoch == 0 {
            Some(ERR_ZERO_EPOCH)
        } else {
            None
        };
        SampleClock {
            epoch,
            numer,
            denom,
            config_error,
            faulted: false,
            observations: 0,
            first_frame: 0,
            last_frame: 0,
            first_ticks: 0,
            last_ticks: 0,
            span_ns: 0,
            mean_x: 0.0,
            mean_y: 0.0,
            m2_x: 0.0,
            c_xy: 0.0,
        }
    }

    pub fn observe(
        &mut self,
        epoch: u64,
        sample_frame: u64,
        ticks: u64,
    ) -> Result<(), &'static str> {
        self.check_epoch(epoch)?;
        match self.accept(sample_frame, ticks) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.faulted = true;
                Err(err)
            }
        }
    }

    /// USB timestamps describe processing time, not a promised shared timestamp
    /// for all eight microframes. Use the final timestamp consistently, retaining
    /// the group's actual starting sample cursor as a diagnostic convention.
    /// This does not establish a converter-presentation anchor.
    pub fn observe_usb_group(
        &mut self,
        epoch: u64,
        sample_frame: u64,
        ticks: &[u64; 8],
    ) -> Result<(), &'static str> {
        self.check_epoch(epoch)?;
        let error = if ticks.contains(&0) {
            Some(ERR_ZERO_TIMESTAMP)
        } else if ticks.windows(2).any(|p| p[1] < p[0]) {
            Some(ERR_GROUP_ORDER)
        } else if self.observations > 0 && ticks[0] < self.last_ticks {
            Some(ERR_TICKS_NOT_INCREASING)
        } else {
            match self.ticks_to_ns(ticks[7] - ticks[0]) {
                Some(span) if span <= MAX_INTERVAL_NS => None,
                Some(_) => Some(ERR_GAP_LONG),
                None => Some(ERR_TIMESTAMP_OVERFLOW),
            }
        };
        if let Some(error) = error {
            self.faulted = true;
            return Err(error);
        }
        self.observe(epoch, sample_frame, ticks[7])
    }

    fn check_epoch(&self, epoch: u64) -> Result<(), &'static str> {
        if let Some(err) = self.config_error {
            return Err(err);
        }
        // Foreign-epoch observations never touch state.
        if epoch == 0 {
            return Err(ERR_ZERO_EPOCH);
        }
        if epoch < self.epoch {
            return Err(ERR_STALE_EPOCH);
        }
        if epoch != self.epoch {
            return Err(ERR_EPOCH_MISMATCH);
        }
        if self.faulted {
            return Err(ERR_INVALIDATED);
        }
        Ok(())
    }

    pub fn snapshot(&self) -> ClockSnapshot {
        let rate_hz = if self.config_error.is_none() && !self.faulted && self.is_mature() {
            self.fit_rate_hz().filter(|r| rate_plausible(*r))
        } else {
            None
        };
        ClockSnapshot {
            epoch: self.epoch,
            observations: self.observations,
            first_frame: self.first_frame,
            last_frame: self.last_frame,
            first_ticks: self.first_ticks,
            last_ticks: self.last_ticks,
            valid: rate_hz.is_some(),
            rate_hz,
            span_ns: self.span_ns,
        }
    }

    /// Validates and folds one in-epoch observation. State is only committed
    /// once every check has passed; the caller latches the fault on error.
    fn accept(&mut self, sample_frame: u64, ticks: u64) -> Result<(), &'static str> {
        if ticks == 0 {
            return Err(ERR_ZERO_TIMESTAMP);
        }

        if self.observations == 0 {
            self.observations = 1;
            self.first_frame = sample_frame;
            self.last_frame = sample_frame;
            self.first_ticks = ticks;
            self.last_ticks = ticks;
            self.span_ns = 0;
            self.mean_x = 0.0;
            self.mean_y = 0.0;
            self.m2_x = 0.0;
            self.c_xy = 0.0;
            return Ok(());
        }

        let interval_ticks = match ticks.checked_sub(self.last_ticks) {
            Some(d) if d > 0 => d,
            _ => return Err(ERR_TICKS_NOT_INCREASING),
        };
        match sample_frame.checked_sub(self.last_frame) {
            Some(d) if d > 0 => {}
            _ => return Err(ERR_FRAMES_NOT_INCREASING),
        }

        let interval_ns = self
            .ticks_to_ns(interval_ticks)
            .ok_or(ERR_TIMESTAMP_OVERFLOW)?;
        if interval_ns > MAX_INTERVAL_NS {
            return Err(ERR_GAP_LONG);
        }
        if interval_ns < MIN_INTERVAL_NS {
            return Err(ERR_GAP_SHORT);
        }

        let rel_ticks = ticks
            .checked_sub(self.first_ticks)
            .ok_or(ERR_TICKS_NOT_INCREASING)?;
        let rel_ns = self.ticks_to_ns(rel_ticks).ok_or(ERR_TIMESTAMP_OVERFLOW)?;
        let rel_frames = sample_frame
            .checked_sub(self.first_frame)
            .ok_or(ERR_FRAMES_NOT_INCREASING)?;
        let n = self
            .observations
            .checked_add(1)
            .ok_or(ERR_TIMESTAMP_OVERFLOW)?;

        let x = rel_ns as f64;
        let y = rel_frames as f64;
        let nf = n as f64;
        let dx = x - self.mean_x;
        let dy = y - self.mean_y;
        let mean_x = self.mean_x + dx / nf;
        let mean_y = self.mean_y + dy / nf;
        let m2_x = self.m2_x + dx * (x - mean_x);
        let c_xy = self.c_xy + dx * (y - mean_y);

        let mature = n >= MIN_OBSERVATIONS && rel_ns >= MIN_SPAN_NS;
        if mature {
            match slope_to_hz(c_xy, m2_x) {
                Some(rate) if rate_plausible(rate) => {}
                _ => return Err(ERR_IMPLAUSIBLE_RATE),
            }
        }

        self.observations = n;
        self.last_frame = sample_frame;
        self.last_ticks = ticks;
        self.span_ns = rel_ns;
        self.mean_x = mean_x;
        self.mean_y = mean_y;
        self.m2_x = m2_x;
        self.c_xy = c_xy;
        Ok(())
    }

    fn ticks_to_ns(&self, ticks: u64) -> Option<u64> {
        let ns = (ticks as u128)
            .checked_mul(self.numer as u128)?
            .checked_div(self.denom as u128)?;
        u64::try_from(ns).ok()
    }

    fn is_mature(&self) -> bool {
        self.observations >= MIN_OBSERVATIONS && self.span_ns >= MIN_SPAN_NS
    }

    fn fit_rate_hz(&self) -> Option<f64> {
        slope_to_hz(self.c_xy, self.m2_x)
    }
}

fn slope_to_hz(c_xy: f64, m2_x: f64) -> Option<f64> {
    if !(m2_x.is_finite() && m2_x > 0.0 && c_xy.is_finite()) {
        return None;
    }
    let rate = c_xy / m2_x * 1e9;
    if rate.is_finite() {
        Some(rate)
    } else {
        None
    }
}

fn rate_plausible(rate: f64) -> bool {
    let lo = NOMINAL_RATE_HZ * (1.0 - RATE_TOLERANCE);
    let hi = NOMINAL_RATE_HZ * (1.0 + RATE_TOLERANCE);
    rate >= lo && rate <= hi
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn real_split_processing_group_uses_final_timestamp() {
        let mut c = SampleClock::new(1, 125, 3);
        c.observe_usb_group(1, 330937, &[6490423617548; 8]).unwrap();
        c.observe_usb_group(
            1,
            330985,
            &[
                6490423620602,
                6490423623791,
                6490423641411,
                6490423641411,
                6490423641411,
                6490423641411,
                6490423641411,
                6490423641411,
            ],
        )
        .unwrap();
        assert_eq!(c.snapshot().observations, 2);
        assert_eq!(c.snapshot().last_ticks, 6490423641411);
        assert_eq!(c.snapshot().last_frame, 330985);
    }

    #[test]
    fn invalid_groups_latch_fault_but_foreign_epoch_does_not() {
        for mode in 0..5 {
            let mut c = SampleClock::new(1, 1, 1);
            feed_unit(&mut c, 1, MS, &[48], 150);
            let before = c.snapshot();
            assert!(before.valid);
            assert_eq!(
                c.observe_usb_group(2, 7200, &[0; 8]),
                Err(ERR_EPOCH_MISMATCH)
            );
            assert!(same(&before, &c.snapshot()));
            let mut ticks = [before.last_ticks + MS; 8];
            match mode {
                0 => ticks[0] = 0,
                1 => ticks[1] += 1,
                2 => ticks[0] = before.last_ticks - 1,
                3 => ticks[7] += 6 * MS,
                _ => ticks.fill(before.last_ticks),
            }
            assert!(c.observe_usb_group(1, 7200, &ticks).is_err());
            assert_latched(&mut c, 1);
            assert_eq!(c.snapshot().observations, before.observations);
        }
    }

    /// Feeds `count` observations at 1 ms spacing with a 1/1 timebase and the
    /// given per-group frame counts (cycled).
    fn feed_unit(
        clock: &mut SampleClock,
        epoch: u64,
        start_ticks: u64,
        counts: &[u64],
        count: u64,
    ) {
        let mut frame = 0u64;
        for i in 0..count {
            clock.observe(epoch, frame, start_ticks + i * MS).unwrap();
            frame += counts[(i as usize) % counts.len()];
        }
    }

    fn same(a: &ClockSnapshot, b: &ClockSnapshot) -> bool {
        a.epoch == b.epoch
            && a.observations == b.observations
            && a.first_frame == b.first_frame
            && a.last_frame == b.last_frame
            && a.first_ticks == b.first_ticks
            && a.last_ticks == b.last_ticks
            && a.valid == b.valid
            && a.rate_hz.map(f64::to_bits) == b.rate_hz.map(f64::to_bits)
            && a.span_ns == b.span_ns
    }

    #[test]
    fn alternating_47_49_fits_48k() {
        let mut c = SampleClock::new(1, 1, 1);
        feed_unit(&mut c, 1, 1_000, &[47, 49], 400);
        let s = c.snapshot();
        assert!(s.valid);
        let rate = s.rate_hz.unwrap();
        assert!((rate - 48_000.0).abs() < 1.0, "rate {rate}");
        assert_eq!(s.observations, 400);
        assert_eq!(s.span_ns, 399 * MS);
    }

    #[test]
    fn drift_80ppm_with_jitter_large_ticks_and_125_3_timebase() {
        // 48 kHz * (1 + 80 ppm) = 48003.84 Hz -> 4800384 frames per 100 s.
        let base_ticks: u64 = u64::MAX / 4;
        let mut c = SampleClock::new(7, 125, 3);
        let mut lcg: u64 = 0x1234_5678_9abc_def0;
        let count = 2_000u64;
        for i in 0..count {
            lcg = lcg
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let jitter_ns = ((lcg >> 33) % 32_001) as i64 - 16_000;
            let t_ns = (i * MS + 100_000) as i64 + jitter_ns;
            let ticks = base_ticks + (t_ns as u64) * 3 / 125;
            let frame = i * 4_800_384 / 100_000;
            c.observe(7, frame, ticks).unwrap();
        }
        let s = c.snapshot();
        assert!(s.valid);
        assert_eq!(s.first_frame, 0);
        let rate = s.rate_hz.unwrap();
        assert!((rate - 48_003.84).abs() < 1.0, "rate {rate}");
    }

    #[test]
    fn stale_epoch_preserves_state() {
        let mut c = SampleClock::new(5, 1, 1);
        feed_unit(&mut c, 5, 10_000, &[48], 150);
        let before = c.snapshot();
        assert!(before.valid);
        assert_eq!(
            c.observe(4, before.last_frame + 48, before.last_ticks + MS),
            Err("stale epoch")
        );
        assert_eq!(
            c.observe(0, before.last_frame + 48, before.last_ticks + MS),
            Err("zero epoch")
        );
        assert_eq!(
            c.observe(6, before.last_frame + 48, before.last_ticks + MS),
            Err("epoch mismatch")
        );
        assert!(same(&before, &c.snapshot()));
        // Still accepts in-epoch observations afterwards.
        c.observe(5, before.last_frame + 48, before.last_ticks + MS)
            .unwrap();
        assert_eq!(c.snapshot().observations, before.observations + 1);
    }

    fn assert_latched(c: &mut SampleClock, epoch: u64) {
        let s = c.snapshot();
        assert!(!s.valid);
        assert!(s.rate_hz.is_none());
        assert_eq!(
            c.observe(epoch, s.last_frame + 48, s.last_ticks + MS),
            Err("clock invalidated")
        );
        assert!(!c.snapshot().valid);
    }

    #[test]
    fn duplicate_observation_invalidates() {
        let mut c = SampleClock::new(1, 1, 1);
        feed_unit(&mut c, 1, 1_000, &[48], 150);
        let s = c.snapshot();
        assert_eq!(
            c.observe(1, s.last_frame, s.last_ticks),
            Err("timestamp not increasing")
        );
        assert_latched(&mut c, 1);
    }

    #[test]
    fn backwards_ticks_invalidate() {
        let mut c = SampleClock::new(1, 1, 1);
        feed_unit(&mut c, 1, 10 * MS, &[48], 150);
        let s = c.snapshot();
        assert_eq!(
            c.observe(1, s.last_frame + 48, s.last_ticks - MS),
            Err("timestamp not increasing")
        );
        assert_latched(&mut c, 1);
    }

    #[test]
    fn non_increasing_frames_invalidate() {
        let mut c = SampleClock::new(1, 1, 1);
        feed_unit(&mut c, 1, 1_000, &[48], 20);
        let s = c.snapshot();
        assert_eq!(
            c.observe(1, s.last_frame, s.last_ticks + MS),
            Err("sample frame not increasing")
        );
        assert_latched(&mut c, 1);
    }

    #[test]
    fn long_gap_invalidates() {
        let mut c = SampleClock::new(1, 1, 1);
        feed_unit(&mut c, 1, 1_000, &[48], 150);
        let s = c.snapshot();
        assert_eq!(
            c.observe(1, s.last_frame + 6 * 48, s.last_ticks + 6 * MS),
            Err("clock gap: interval too long")
        );
        assert_latched(&mut c, 1);
    }

    #[test]
    fn short_interval_invalidates() {
        let mut c = SampleClock::new(1, 1, 1);
        feed_unit(&mut c, 1, 1_000, &[48], 10);
        let s = c.snapshot();
        assert_eq!(
            c.observe(1, s.last_frame + 19, s.last_ticks + 400_000),
            Err("clock gap: interval too short")
        );
        assert_latched(&mut c, 1);
    }

    #[test]
    fn warmup_requires_span_and_count() {
        let mut c = SampleClock::new(1, 1, 1);
        c.observe(1, 0, 5_000).unwrap();
        let s = c.snapshot();
        assert_eq!(
            (s.first_frame, s.last_frame, s.first_ticks, s.last_ticks),
            (0, 0, 5_000, 5_000)
        );
        assert!(!s.valid && s.rate_hz.is_none());

        // 100 observations span 99 ms: enough count, not enough span.
        let mut c = SampleClock::new(1, 1, 1);
        feed_unit(&mut c, 1, 5_000, &[48], 100);
        let s = c.snapshot();
        assert_eq!(s.span_ns, 99 * MS);
        assert_eq!(s.last_frame, 99 * 48);
        assert!(!s.valid && s.rate_hz.is_none());
        c.observe(1, 100 * 48, 5_000 + 100 * MS).unwrap();
        assert!(c.snapshot().valid);

        // 30 observations at 4 ms span 116 ms: enough span, not enough count.
        let mut c = SampleClock::new(1, 1, 1);
        for i in 0..30u64 {
            c.observe(1, i * 192, 5_000 + i * 4 * MS).unwrap();
        }
        let s = c.snapshot();
        assert!(s.span_ns >= MIN_SPAN_NS);
        assert!(!s.valid && s.rate_hz.is_none());
    }

    #[test]
    fn invalid_timebase_and_zero_ticks() {
        let mut c = SampleClock::new(1, 0, 1);
        assert_eq!(c.observe(1, 0, 1_000), Err("invalid timebase"));
        let mut c = SampleClock::new(1, 1, 0);
        assert_eq!(c.observe(1, 0, 1_000), Err("invalid timebase"));
        assert!(!c.snapshot().valid);
        let mut c = SampleClock::new(0, 1, 1);
        assert_eq!(c.observe(0, 0, 1_000), Err("zero epoch"));

        let mut c = SampleClock::new(1, 1, 1);
        assert_eq!(c.observe(1, 0, 0), Err("zero timestamp"));
        assert_eq!(c.snapshot().observations, 0);
        assert_eq!(c.observe(1, 0, 1_000), Err("clock invalidated"));
    }

    #[test]
    fn timestamp_overflow_invalidates() {
        let mut c = SampleClock::new(1, u32::MAX, 1);
        c.observe(1, 0, 1).unwrap();
        assert_eq!(c.observe(1, 48, u64::MAX), Err("timestamp overflow"));
        assert_latched(&mut c, 1);
    }

    #[test]
    fn restart_uses_fresh_epoch_from_frame_zero() {
        let mut old = SampleClock::new(1, 1, 1);
        feed_unit(&mut old, 1, 1_000, &[48], 150);
        let mut fresh = SampleClock::new(2, 1, 1);
        assert_eq!(fresh.observe(1, 0, 900 * MS), Err("stale epoch"));
        assert_eq!(fresh.snapshot().observations, 0);
        fresh.observe(2, 0, 900 * MS).unwrap();
        let s = fresh.snapshot();
        assert_eq!(
            (s.epoch, s.observations, s.first_frame, s.span_ns),
            (2, 1, 0, 0)
        );
        assert!(!s.valid);
        assert!(old.snapshot().valid);
    }

    #[test]
    fn implausible_rate_rejected_when_mature() {
        let mut c = SampleClock::new(1, 1, 1);
        let mut result = Ok(());
        for i in 0..200u64 {
            result = c.observe(1, i * 50, 1_000 + i * MS);
            if result.is_err() {
                break;
            }
        }
        assert_eq!(result, Err("implausible rate"));
        assert_latched(&mut c, 1);
    }
}
