//! Absolute-frame sample timeline, published clock anchor and zero-timestamp
//! stepper for the Fast Track Ultra continuous USB engine.
//!
//! - `Timeline` is a fixed-capacity ring of interleaved Float32 frames tagged
//!   with their absolute frame number. One writer (the Rust engine) publishes
//!   frames; readers (the HAL IO path) copy them out. Every payload word is an
//!   atomic, and each slot carries a seqlock-style version taken from a unique,
//!   never-reused counter, so a same-frame rewrite can never look unchanged and
//!   a wrapped slot can never be returned for the wrong frame.
//! - `PublishedClock` publishes one `ClockAnchor` coherently to readers.
//! - `TimestampStepper` turns measured clock observations into a zero
//!   timestamp pair that only moves when the 512-frame boundary advances.
//!
//! Policy:
//! - Nothing here waits, spins or retries unboundedly. Readers make at most two
//!   attempts per frame / anchor, then report it missing (zeros / `None`).
//! - Writers are single by contract, enforced by a fail-fast gate: a competing
//!   `Timeline::write` returns `false`; a competing `PublishedClock::publish`
//!   latches the clock invalid. Version exhaustion also fails closed.
//! - Session integration must use disjoint absolute frame ranges per epoch or
//!   fresh storage. Never free storage while an admitted IO call may hold it.
//! - No allocation after `Timeline::new`. Atomics are SeqCst throughout; this
//!   is lock-free only on targets with native 64-bit atomics (Apple arm64).
//! - Zero timestamps are a diagnostic HAL mapping derived from the measured
//!   sample clock, not a calibrated converter-presentation time.

use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

pub const CHANNELS: usize = 8;
pub const RING_FRAMES: usize = 16384;
pub const ZTS_PERIOD: u64 = 512;
pub const MAX_FRAMES_PER_CALL: usize = 1024;

const READ_ATTEMPTS: usize = 2;

const ERR_ZERO_EPOCH: &str = "zero epoch";
const ERR_EPOCH_MISMATCH: &str = "epoch mismatch";
const ERR_INVALIDATED: &str = "timestamp stepper invalidated";
const ERR_ZERO_TIMESTAMP: &str = "zero timestamp";
const ERR_INVALID_SLOPE: &str = "invalid ticks per frame";
const ERR_OBSERVATION_ORDER: &str = "observation not increasing";
const ERR_OVERFLOW: &str = "timestamp overflow";
const ERR_BOUNDARY_ORDER: &str = "zero timestamp host time not increasing";

/// Fail-fast single-owner gate; released on drop.
struct Gate<'a>(&'a AtomicBool);

impl<'a> Gate<'a> {
    fn try_acquire(flag: &'a AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, SeqCst, SeqCst)
            .ok()
            .map(|_| Gate(flag))
    }
}

impl Drop for Gate<'_> {
    fn drop(&mut self) {
        self.0.store(false, SeqCst);
    }
}

struct Slot {
    /// 0 = never written, odd = write in progress, even = stable.
    seq: AtomicU64,
    frame: AtomicU64,
    samples: [AtomicU32; CHANNELS],
}

impl Slot {
    fn new() -> Self {
        Slot {
            seq: AtomicU64::new(0),
            frame: AtomicU64::new(0),
            samples: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}

pub struct Timeline {
    slots: Box<[Slot]>,
    writing: AtomicBool,
    // Writer-owned; only touched while holding `writing` (or via `&mut`).
    next_version: AtomicU64,
    /// One past the newest frame ever written.
    head: AtomicU64,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    /// The only allocation: `RING_FRAMES` slots.
    pub fn new() -> Self {
        Timeline {
            slots: (0..RING_FRAMES).map(|_| Slot::new()).collect(),
            writing: AtomicBool::new(false),
            next_version: AtomicU64::new(0),
            head: AtomicU64::new(0),
        }
    }

    /// Publishes `samples.len() / CHANNELS` interleaved frames starting at
    /// absolute frame `first`. All-or-nothing: returns `false` without touching
    /// the ring for an invalid length (empty, partial frame, > 1024 frames),
    /// frame overflow, a competing writer, a late write that would clobber a
    /// newer frame in the same slot, or version exhaustion.
    pub fn write(&self, first: u64, samples: &[f32]) -> bool {
        let Some(frames) = frame_count(samples.len()) else {
            return false;
        };
        let Some(end) = first.checked_add(frames as u64) else {
            return false;
        };
        let Some(_gate) = Gate::try_acquire(&self.writing) else {
            return false;
        };
        // Frames >= head - RING_FRAMES can only share a slot with older frames.
        let head = self.head.load(SeqCst);
        if first < head.saturating_sub(RING_FRAMES as u64) {
            return false;
        }
        let version = self.next_version.load(SeqCst);
        let Some(next) = version.checked_add(2 * frames as u64) else {
            return false;
        };
        self.next_version.store(next, SeqCst);

        for (i, frame) in samples.chunks_exact(CHANNELS).enumerate() {
            let tag = first + i as u64;
            let slot = &self.slots[slot_index(tag)];
            let v = version + 2 * i as u64;
            slot.seq.store(v + 1, SeqCst);
            slot.frame.store(tag, SeqCst);
            for (dst, &x) in slot.samples.iter().zip(frame) {
                dst.store(sanitize(x).to_bits(), SeqCst);
            }
            slot.seq.store(v + 2, SeqCst);
        }
        self.head.store(head.max(end), SeqCst);
        true
    }

    /// Copies frames starting at `first` into `samples` and returns the number
    /// of missing frames, which are zeroed. A frame is missing if it was never
    /// written, has been overwritten by a later wrap, or stayed unstable for
    /// two attempts. Invalid requests are zeroed entirely and every touched
    /// frame is reported missing.
    pub fn read(&self, first: u64, samples: &mut [f32]) -> usize {
        let valid =
            frame_count(samples.len()).filter(|&frames| first.checked_add(frames as u64).is_some());
        if valid.is_none() {
            samples.fill(0.0);
            return samples.len().div_ceil(CHANNELS);
        }
        let mut missing = 0;
        for (i, out) in samples.chunks_exact_mut(CHANNELS).enumerate() {
            if !self.read_frame(first + i as u64, out) {
                out.fill(0.0);
                missing += 1;
            }
        }
        missing
    }

    fn read_frame(&self, tag: u64, out: &mut [f32]) -> bool {
        let slot = &self.slots[slot_index(tag)];
        for _ in 0..READ_ATTEMPTS {
            let before = slot.seq.load(SeqCst);
            if before == 0 {
                return false;
            }
            if before % 2 == 1 {
                continue;
            }
            let stored = slot.frame.load(SeqCst);
            for (o, s) in out.iter_mut().zip(&slot.samples) {
                *o = f32::from_bits(s.load(SeqCst));
            }
            if slot.seq.load(SeqCst) == before {
                return stored == tag;
            }
        }
        false
    }

    /// Empties the ring. Requires exclusive access, so only usable while
    /// quiescent; prefer a fresh `Timeline` per session. The version counter
    /// is deliberately kept so versions stay unique for the object's lifetime.
    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot.seq.get_mut() = 0;
            *slot.frame.get_mut() = 0;
            for s in slot.samples.iter_mut() {
                *s.get_mut() = 0;
            }
        }
        *self.head.get_mut() = 0;
    }
}

fn frame_count(len: usize) -> Option<usize> {
    let frames = len / CHANNELS;
    if len == 0 || !len.is_multiple_of(CHANNELS) || frames > MAX_FRAMES_PER_CALL {
        None
    } else {
        Some(frames)
    }
}

fn slot_index(frame: u64) -> usize {
    (frame % RING_FRAMES as u64) as usize
}

fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockAnchor {
    pub epoch: u64,
    pub frame: u64,
    pub ticks: u64,
    pub ticks_per_frame: f64,
}

fn anchor_valid(a: &ClockAnchor) -> bool {
    a.epoch != 0 && a.ticks != 0 && a.ticks_per_frame.is_finite() && a.ticks_per_frame > 0.0
}

/// Single-writer, coherent publication of one `ClockAnchor`.
pub struct PublishedClock {
    /// Even = stable, odd = publish in progress.
    seq: AtomicU64,
    publishing: AtomicBool,
    /// Latched by a competing publisher or version exhaustion.
    poisoned: AtomicBool,
    present: AtomicBool,
    epoch: AtomicU64,
    frame: AtomicU64,
    ticks: AtomicU64,
    ticks_per_frame: AtomicU64,
}

impl Default for PublishedClock {
    fn default() -> Self {
        Self::new()
    }
}

impl PublishedClock {
    pub fn new() -> Self {
        PublishedClock {
            seq: AtomicU64::new(0),
            publishing: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            present: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            frame: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
            ticks_per_frame: AtomicU64::new(0),
        }
    }

    /// `None`, or an anchor with zero epoch/ticks or a non-finite/non-positive
    /// slope, invalidates. A competing publisher or version exhaustion latches
    /// the clock invalid for the rest of its life.
    pub fn publish(&self, anchor: Option<ClockAnchor>) {
        let Some(_gate) = Gate::try_acquire(&self.publishing) else {
            self.poisoned.store(true, SeqCst);
            return;
        };
        let seq = self.seq.load(SeqCst);
        let Some(next) = seq.checked_add(2) else {
            self.poisoned.store(true, SeqCst);
            return;
        };
        let anchor = anchor.filter(anchor_valid);
        let a = anchor.unwrap_or(ClockAnchor {
            epoch: 0,
            frame: 0,
            ticks: 0,
            ticks_per_frame: 0.0,
        });
        self.seq.store(seq + 1, SeqCst);
        self.present.store(anchor.is_some(), SeqCst);
        self.epoch.store(a.epoch, SeqCst);
        self.frame.store(a.frame, SeqCst);
        self.ticks.store(a.ticks, SeqCst);
        self.ticks_per_frame
            .store(a.ticks_per_frame.to_bits(), SeqCst);
        self.seq.store(next, SeqCst);
    }

    /// At most two attempts; `None` if unpublished, invalidated or unstable.
    pub fn read(&self) -> Option<ClockAnchor> {
        for _ in 0..READ_ATTEMPTS {
            let before = self.seq.load(SeqCst);
            if before % 2 == 1 {
                continue;
            }
            let present = self.present.load(SeqCst);
            let anchor = ClockAnchor {
                epoch: self.epoch.load(SeqCst),
                frame: self.frame.load(SeqCst),
                ticks: self.ticks.load(SeqCst),
                ticks_per_frame: f64::from_bits(self.ticks_per_frame.load(SeqCst)),
            };
            if self.seq.load(SeqCst) != before {
                continue;
            }
            if !present || self.poisoned.load(SeqCst) {
                return None;
            }
            return Some(anchor);
        }
        None
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroTimestamp {
    pub epoch: u64,
    pub frame: u64,
    pub ticks: u64,
}

/// Maps measured clock observations onto `ZTS_PERIOD` boundaries.
///
/// Each observation is the cumulative captured frame count before a USB group
/// plus that group's final processing timestamp, with `ticks_per_frame` from
/// a mature measured fit (the caller must not feed nominal or callback-arrival
/// values). The boundary `frame / 512 * 512` is projected back by
/// `ticks_per_frame`; a new pair is published only when the boundary strictly
/// advances, so the pair is stable within each period. Foreign epochs are
/// rejected without touching state; any other error latches the stepper
/// invalid, which must stop the engine.
#[derive(Debug)]
pub struct TimestampStepper {
    epoch: u64,
    faulted: bool,
    last_observed: Option<(u64, u64)>,
    published: Option<ZeroTimestamp>,
}

impl TimestampStepper {
    pub fn new(epoch: u64) -> Self {
        TimestampStepper {
            epoch,
            faulted: false,
            last_observed: None,
            published: None,
        }
    }

    pub fn current(&self) -> Option<ZeroTimestamp> {
        if self.faulted {
            None
        } else {
            self.published
        }
    }

    pub fn advance(
        &mut self,
        observed: ClockAnchor,
    ) -> Result<Option<ZeroTimestamp>, &'static str> {
        if self.epoch == 0 {
            return Err(ERR_ZERO_EPOCH);
        }
        if observed.epoch != self.epoch {
            return Err(ERR_EPOCH_MISMATCH);
        }
        if self.faulted {
            return Err(ERR_INVALIDATED);
        }
        let result = self.step(observed);
        if result.is_err() {
            self.faulted = true;
        }
        result
    }

    /// Validates fully before committing any state.
    fn step(&mut self, o: ClockAnchor) -> Result<Option<ZeroTimestamp>, &'static str> {
        if o.ticks == 0 {
            return Err(ERR_ZERO_TIMESTAMP);
        }
        if !(o.ticks_per_frame.is_finite() && o.ticks_per_frame > 0.0) {
            return Err(ERR_INVALID_SLOPE);
        }
        if let Some((frame, ticks)) = self.last_observed {
            if o.frame <= frame || o.ticks <= ticks {
                return Err(ERR_OBSERVATION_ORDER);
            }
        }
        let boundary = o.frame / ZTS_PERIOD * ZTS_PERIOD;
        let back = (o.frame - boundary) as f64 * o.ticks_per_frame;
        // u64::MAX as f64 is 2^64; also rejects infinity.
        if !back.is_finite() || back >= u64::MAX as f64 {
            return Err(ERR_OVERFLOW);
        }
        let ticks = o
            .ticks
            .checked_sub(back.round() as u64)
            .ok_or(ERR_OVERFLOW)?;
        if ticks == 0 {
            return Err(ERR_ZERO_TIMESTAMP);
        }
        // Observed frames strictly increase, so the boundary never regresses.
        let publish = match self.published {
            None => true,
            Some(p) if boundary > p.frame => {
                if ticks <= p.ticks {
                    return Err(ERR_BOUNDARY_ORDER);
                }
                true
            }
            Some(_) => false,
        };
        self.last_observed = Some((o.frame, o.ticks));
        if !publish {
            return Ok(None);
        }
        let zts = ZeroTimestamp {
            epoch: self.epoch,
            frame: boundary,
            ticks,
        };
        self.published = Some(zts);
        Ok(Some(zts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    const SCALE: f32 = (1u32 << 23) as f32;

    /// Exactly representable, all channels distinct, encodes frame (mod 2^18,
    /// so wrapped frames differ) and gen.
    fn pattern(frame: u64, gen: u64, out: &mut [f32]) {
        for (ch, v) in out.iter_mut().enumerate() {
            *v = ((frame % (1 << 18)) * 32 + (gen % 4) * 8 + ch as u64) as f32 / SCALE;
        }
    }

    fn frames(first: u64, count: usize, gen: u64) -> Vec<f32> {
        let mut v = vec![0.0; count * CHANNELS];
        for (i, f) in v.chunks_exact_mut(CHANNELS).enumerate() {
            pattern(first + i as u64, gen, f);
        }
        v
    }

    /// A frame is either missing (all zero) or one whole, untorn write of it.
    fn check_frame(frame: u64, got: &[f32]) {
        if got.iter().all(|&v| v == 0.0) {
            return;
        }
        let gen = ((got[0] * SCALE) as u64 / 8) % 4;
        let mut want = [0.0; CHANNELS];
        pattern(frame, gen, &mut want);
        assert_eq!(got, &want[..], "torn or stale frame {frame}");
    }

    #[test]
    fn roundtrip_and_holes() {
        let tl = Timeline::new();
        assert!(tl.write(10, &frames(10, 4, 0)));
        let mut buf = vec![9.0; 8 * CHANNELS];
        assert_eq!(tl.read(8, &mut buf), 4);
        for (i, f) in buf.chunks_exact(CHANNELS).enumerate() {
            let frame = 8 + i as u64;
            if (10..14).contains(&frame) {
                let mut want = [0.0; CHANNELS];
                pattern(frame, 0, &mut want);
                assert_eq!(f, &want[..]);
            } else {
                assert!(f.iter().all(|&v| v == 0.0));
            }
        }
    }

    #[test]
    fn wrap_never_returns_stale_frames() {
        let tl = Timeline::new();
        let total = RING_FRAMES as u64 + 1024;
        for first in (0..total).step_by(1024) {
            assert!(tl.write(first, &frames(first, 1024, 1)));
        }
        let mut buf = vec![0.0; 1024 * CHANNELS];
        // Slots 0..1024 now hold frames RING_FRAMES.. .
        assert_eq!(tl.read(0, &mut buf), 1024);
        assert!(buf.iter().all(|&v| v == 0.0));
        assert_eq!(tl.read(RING_FRAMES as u64, &mut buf), 0);
        assert_eq!(buf, frames(RING_FRAMES as u64, 1024, 1));
        assert_eq!(tl.read(1024, &mut buf), 0);
        // Beyond the head: never written at that tag.
        assert_eq!(tl.read(total, &mut buf), 1024);
    }

    #[test]
    fn late_and_out_of_order_writes() {
        let tl = Timeline::new();
        assert!(tl.write(2000, &frames(2000, 10, 0)));
        assert!(tl.write(1990, &frames(1990, 10, 0)));
        let mut buf = vec![0.0; 20 * CHANNELS];
        assert_eq!(tl.read(1990, &mut buf), 0);
        assert_eq!(buf, frames(1990, 20, 0));

        let head = RING_FRAMES as u64 + 1024;
        assert!(tl.write(head - 8, &frames(head - 8, 8, 0)));
        // Older than head - RING_FRAMES: its slot may hold a newer frame.
        assert!(!tl.write(1000, &frames(1000, 8, 0)));
        // Oldest frame that cannot collide with anything newer.
        assert!(tl.write(head - RING_FRAMES as u64, &frames(1024, 8, 0)));
        let mut one = vec![0.0; 8 * CHANNELS];
        assert_eq!(tl.read(1024, &mut one), 0);
        assert_eq!(tl.read(1000, &mut one), 8);
    }

    #[test]
    fn same_frame_overwrite_uses_fresh_version() {
        let tl = Timeline::new();
        assert!(tl.write(5, &frames(5, 1, 0)));
        let v1 = tl.slots[5].seq.load(SeqCst);
        assert!(tl.write(5, &frames(5, 1, 3)));
        let v2 = tl.slots[5].seq.load(SeqCst);
        assert!(v2 > v1 && v2.is_multiple_of(2));
        let mut buf = [0.0; CHANNELS];
        assert_eq!(tl.read(5, &mut buf), 0);
        assert_eq!(&buf[..], &frames(5, 1, 3)[..]);
    }

    #[test]
    fn samples_are_sanitized() {
        let tl = Timeline::new();
        let input = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            2.0,
            -3.0,
            0.5,
            -1.0,
            1.0,
        ];
        assert!(tl.write(0, &input));
        let mut buf = [9.0; CHANNELS];
        assert_eq!(tl.read(0, &mut buf), 0);
        assert_eq!(buf, [0.0, 0.0, 0.0, 1.0, -1.0, 0.5, -1.0, 1.0]);
    }

    #[test]
    fn invalid_lengths_and_overflow() {
        let tl = Timeline::new();
        assert!(!tl.write(0, &[]));
        assert!(!tl.write(0, &[0.0; 7]));
        assert!(!tl.write(0, &vec![0.0; (MAX_FRAMES_PER_CALL + 1) * CHANNELS]));
        assert!(tl.write(0, &vec![0.5; MAX_FRAMES_PER_CALL * CHANNELS]));

        assert_eq!(tl.read(0, &mut []), 0);
        let mut partial = [9.0; 7];
        assert_eq!(tl.read(0, &mut partial), 1);
        assert!(partial.iter().all(|&v| v == 0.0));
        let mut big = vec![9.0; (MAX_FRAMES_PER_CALL + 1) * CHANNELS];
        assert_eq!(tl.read(0, &mut big), MAX_FRAMES_PER_CALL + 1);
        assert!(big.iter().all(|&v| v == 0.0));

        let tl = Timeline::new();
        let one = [0.25; CHANNELS];
        assert!(!tl.write(u64::MAX, &one));
        assert!(tl.write(u64::MAX - 1, &one));
        let mut buf = [9.0; CHANNELS];
        assert_eq!(tl.read(u64::MAX, &mut buf), 1);
        assert_eq!(buf, [0.0; CHANNELS]);
        assert_eq!(tl.read(u64::MAX - 1, &mut buf), 0);
        assert_eq!(buf, one);
    }

    #[test]
    fn version_exhaustion_fails_closed() {
        let tl = Timeline::new();
        tl.next_version.store(u64::MAX - 3, SeqCst);
        assert!(!tl.write(0, &frames(0, 2, 0)));
        assert!(tl.write(0, &frames(0, 1, 0)));
        assert!(!tl.write(1, &frames(1, 1, 0)));
        assert!(!tl.write(0, &frames(0, 1, 1)));
        let mut buf = vec![0.0; 2 * CHANNELS];
        assert_eq!(tl.read(0, &mut buf), 1);
        assert_eq!(&buf[..CHANNELS], &frames(0, 1, 0)[..]);
    }

    #[test]
    fn held_gate_rejects_writer() {
        let tl = Timeline::new();
        tl.writing.store(true, SeqCst);
        assert!(!tl.write(0, &frames(0, 1, 0)));
        tl.writing.store(false, SeqCst);
        assert!(tl.write(0, &frames(0, 1, 0)));
        assert!(!tl.writing.load(SeqCst));
    }

    #[test]
    fn clear_empties_ring() {
        let mut tl = Timeline::new();
        assert!(tl.write(RING_FRAMES as u64 * 3, &frames(0, 4, 0)));
        tl.clear();
        let mut buf = vec![0.0; 4 * CHANNELS];
        assert_eq!(tl.read(RING_FRAMES as u64 * 3, &mut buf), 4);
        // Head reset: early frames are writable again.
        assert!(tl.write(0, &frames(0, 4, 2)));
        assert_eq!(tl.read(0, &mut buf), 0);
        assert_eq!(buf, frames(0, 4, 2));
    }

    #[test]
    fn concurrent_writer_reader_with_overwrites_and_wrap() {
        let tl = Arc::new(Timeline::new());
        let head = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let reader = {
            let (tl, head, done) = (tl.clone(), head.clone(), done.clone());
            thread::spawn(move || {
                let mut buf = vec![0.0; 512 * CHANNELS];
                let mut present = 0usize;
                while !done.load(SeqCst) {
                    let h = head.load(SeqCst);
                    if h < 512 {
                        continue;
                    }
                    present += 512 - tl.read(h - 512, &mut buf);
                    for (i, f) in buf.chunks_exact(CHANNELS).enumerate() {
                        check_frame(h - 512 + i as u64, f);
                    }
                }
                present
            })
        };
        // Overlapping 256-frame windows advancing by 128: every frame is
        // rewritten with a new gen, and the ring wraps several times.
        let mut data = vec![0.0; 256 * CHANNELS];
        for round in 0..1000u64 {
            let first = round * 128;
            for (i, f) in data.chunks_exact_mut(CHANNELS).enumerate() {
                pattern(first + i as u64, round, f);
            }
            assert!(tl.write(first, &data));
            head.store(first + 256, SeqCst);
        }
        done.store(true, SeqCst);
        reader.join().unwrap();
        let h = head.load(SeqCst);
        let mut buf = vec![0.0; 512 * CHANNELS];
        assert_eq!(tl.read(h - 512, &mut buf), 0);
        for (i, f) in buf.chunks_exact(CHANNELS).enumerate() {
            check_frame(h - 512 + i as u64, f);
        }
    }

    #[test]
    fn competing_writers_are_rejected_not_raced() {
        let tl = Arc::new(Timeline::new());
        let done = Arc::new(AtomicBool::new(false));
        let writers: Vec<_> = [1u64, 2]
            .into_iter()
            .map(|gen| {
                let tl = tl.clone();
                thread::spawn(move || {
                    let data = frames(0, 64, gen);
                    (0..3000).filter(|_| tl.write(0, &data)).count()
                })
            })
            .collect();
        let reader = {
            let (tl, done) = (tl.clone(), done.clone());
            thread::spawn(move || {
                let mut buf = vec![0.0; 64 * CHANNELS];
                while !done.load(SeqCst) {
                    tl.read(0, &mut buf);
                    for (i, f) in buf.chunks_exact(CHANNELS).enumerate() {
                        check_frame(i as u64, f);
                    }
                }
            })
        };
        let accepted: usize = writers.into_iter().map(|w| w.join().unwrap()).sum();
        done.store(true, SeqCst);
        reader.join().unwrap();
        assert!(accepted > 0 && accepted <= 6000);
        assert!(!tl.writing.load(SeqCst));
        let mut buf = vec![0.0; 64 * CHANNELS];
        assert_eq!(tl.read(0, &mut buf), 0);
        for (i, f) in buf.chunks_exact(CHANNELS).enumerate() {
            check_frame(i as u64, f);
        }
    }

    fn anchor(i: u64) -> ClockAnchor {
        ClockAnchor {
            epoch: i,
            frame: i * 512,
            ticks: i * 1000 + 1,
            ticks_per_frame: i as f64 + 0.5,
        }
    }

    #[test]
    fn published_clock_roundtrip_and_invalidation() {
        let c = PublishedClock::new();
        assert_eq!(c.read(), None);
        c.publish(Some(anchor(3)));
        assert_eq!(c.read(), Some(anchor(3)));
        c.publish(None);
        assert_eq!(c.read(), None);
        c.publish(Some(anchor(4)));
        assert_eq!(c.read(), Some(anchor(4)));
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            c.publish(Some(ClockAnchor {
                ticks_per_frame: bad,
                ..anchor(5)
            }));
            assert_eq!(c.read(), None);
        }
        c.publish(Some(ClockAnchor {
            epoch: 0,
            ..anchor(5)
        }));
        assert_eq!(c.read(), None);
        c.publish(Some(ClockAnchor {
            ticks: 0,
            ..anchor(5)
        }));
        assert_eq!(c.read(), None);
    }

    #[test]
    fn published_clock_fails_closed() {
        let c = PublishedClock::new();
        c.publish(Some(anchor(1)));
        c.publishing.store(true, SeqCst);
        c.publish(Some(anchor(2)));
        c.publishing.store(false, SeqCst);
        assert_eq!(c.read(), None);
        c.publish(Some(anchor(3)));
        assert_eq!(c.read(), None);

        let c = PublishedClock::new();
        c.publish(Some(anchor(1)));
        c.seq.store(u64::MAX - 1, SeqCst);
        assert_eq!(c.read(), Some(anchor(1)));
        c.publish(Some(anchor(2)));
        assert_eq!(c.read(), None);
    }

    #[test]
    fn published_clock_concurrent_fields_stay_correlated() {
        let c = Arc::new(PublishedClock::new());
        let done = Arc::new(AtomicBool::new(false));
        let reader = {
            let (c, done) = (c.clone(), done.clone());
            thread::spawn(move || {
                let mut last = 0;
                while !done.load(SeqCst) {
                    if let Some(a) = c.read() {
                        assert_eq!(a, anchor(a.epoch));
                        assert!(a.epoch >= last);
                        last = a.epoch;
                    }
                }
            })
        };
        for i in 1..=50_000u64 {
            c.publish(if i % 7 == 0 { None } else { Some(anchor(i)) });
        }
        done.store(true, SeqCst);
        reader.join().unwrap();
        assert_eq!(c.read(), Some(anchor(50_000)));
    }

    fn obs(frame: u64, ticks: u64, tpf: f64) -> ClockAnchor {
        ClockAnchor {
            epoch: 1,
            frame,
            ticks,
            ticks_per_frame: tpf,
        }
    }

    #[test]
    fn stepper_pair_stable_within_period() {
        let mut s = TimestampStepper::new(1);
        let base = 6_490_423_617_548u64;
        let first = s.advance(obs(0, base, 500.0)).unwrap().unwrap();
        assert_eq!(
            first,
            ZeroTimestamp {
                epoch: 1,
                frame: 0,
                ticks: base
            }
        );
        for k in 1..=10u64 {
            assert_eq!(
                s.advance(obs(k * 48, base + k * 24_000 + 7, 500.0)),
                Ok(None)
            );
            assert_eq!(s.current(), Some(first));
        }
        let next = s
            .advance(obs(528, base + 528 * 500, 500.0))
            .unwrap()
            .unwrap();
        assert_eq!(
            next,
            ZeroTimestamp {
                epoch: 1,
                frame: 512,
                ticks: base + 512 * 500
            }
        );
    }

    #[test]
    fn stepper_microjitter_publishes_each_boundary_once() {
        let mut s = TimestampStepper::new(1);
        let base = 6_490_423_617_548u64;
        let mut lcg: u64 = 0x1234_5678_9abc_def0;
        let mut published = Vec::new();
        for i in 0..400u64 {
            lcg = lcg
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let jitter = (lcg >> 33) % 7; // 0..=6 ticks
            let tpf = 500.0 + ((lcg >> 20) % 21) as f64 / 1000.0 - 0.01;
            let frame = i * 48;
            if let Some(z) = s
                .advance(obs(frame, base + 1000 + frame * 500 + jitter, tpf))
                .unwrap()
            {
                published.push(z);
            }
        }
        assert_eq!(published.len() as u64, 399 * 48 / 512 + 1);
        for (k, z) in published.iter().enumerate() {
            assert_eq!(z.frame, k as u64 * 512);
            let ideal = base + 1000 + z.frame * 500;
            assert!(
                z.ticks.abs_diff(ideal) <= 12,
                "boundary {} off by {}",
                z.frame,
                z.ticks.abs_diff(ideal)
            );
        }
        assert!(published.windows(2).all(|p| p[1].ticks > p[0].ticks));
    }

    #[test]
    fn stepper_boundary_jump() {
        let mut s = TimestampStepper::new(1);
        assert_eq!(
            s.advance(obs(100, 1_000_000, 500.0))
                .unwrap()
                .unwrap()
                .frame,
            0
        );
        let z = s.advance(obs(2000, 2_000_000, 500.0)).unwrap().unwrap();
        assert_eq!(
            z,
            ZeroTimestamp {
                epoch: 1,
                frame: 1536,
                ticks: 2_000_000 - 464 * 500
            }
        );
    }

    #[test]
    fn stepper_epochs() {
        let mut s = TimestampStepper::new(3);
        assert_eq!(s.advance(obs(0, 1000, 500.0)), Err(ERR_EPOCH_MISMATCH));
        assert_eq!(s.current(), None);
        let z = s
            .advance(ClockAnchor {
                epoch: 3,
                ..obs(0, 1000, 500.0)
            })
            .unwrap();
        assert_eq!(
            z,
            Some(ZeroTimestamp {
                epoch: 3,
                frame: 0,
                ticks: 1000
            })
        );
        assert_eq!(s.advance(obs(48, 25_000, 500.0)), Err(ERR_EPOCH_MISMATCH));
        assert_eq!(s.current(), z);

        let mut s = TimestampStepper::new(0);
        assert_eq!(
            s.advance(ClockAnchor {
                epoch: 0,
                ..obs(0, 1000, 500.0)
            }),
            Err(ERR_ZERO_EPOCH)
        );
    }

    fn assert_latched(s: &mut TimestampStepper) {
        assert_eq!(s.current(), None);
        assert_eq!(
            s.advance(obs(1 << 40, u64::MAX / 2, 500.0)),
            Err(ERR_INVALIDATED)
        );
    }

    #[test]
    fn stepper_overflow_and_zero_host_time() {
        for (o, err) in [
            (obs(511, 1000, 500.0), ERR_OVERFLOW),
            (obs(1, u64::MAX, 1e300), ERR_OVERFLOW),
            (obs(511, u64::MAX, f64::MAX), ERR_OVERFLOW),
            (obs(2, 1000, 500.0), ERR_ZERO_TIMESTAMP),
            (obs(0, 0, 500.0), ERR_ZERO_TIMESTAMP),
        ] {
            let mut s = TimestampStepper::new(1);
            assert_eq!(s.advance(o), Err(err));
            assert_latched(&mut s);
        }
    }

    #[test]
    fn stepper_rejects_bad_slope() {
        for tpf in [0.0, -500.0, f64::NAN, f64::INFINITY] {
            let mut s = TimestampStepper::new(1);
            assert_eq!(s.advance(obs(0, 1000, tpf)), Err(ERR_INVALID_SLOPE));
            assert_latched(&mut s);
        }
    }

    #[test]
    fn stepper_rejects_nonmonotonic_input() {
        let mut s = TimestampStepper::new(1);
        s.advance(obs(500, 1_000_000, 500.0)).unwrap();
        assert_eq!(
            s.advance(obs(500, 1_000_100, 500.0)),
            Err(ERR_OBSERVATION_ORDER)
        );
        assert_latched(&mut s);

        let mut s = TimestampStepper::new(1);
        s.advance(obs(500, 1_000_000, 500.0)).unwrap();
        assert_eq!(
            s.advance(obs(548, 1_000_000, 500.0)),
            Err(ERR_OBSERVATION_ORDER)
        );
        assert_latched(&mut s);

        // Boundary 0 at 750_000; projecting 512 back with a steep slope lands
        // before it.
        let mut s = TimestampStepper::new(1);
        let z = s.advance(obs(500, 1_000_000, 500.0)).unwrap().unwrap();
        assert_eq!(z.ticks, 750_000);
        assert_eq!(
            s.advance(obs(520, 1_000_001, 100_000.0)),
            Err(ERR_BOUNDARY_ORDER)
        );
        assert_latched(&mut s);
    }
}
