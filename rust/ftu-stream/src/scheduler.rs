//! Bounded duplex policy. Each OUT packet uses the corresponding completed IN
//! packet count 64 USB milliseconds earlier; no nominal-count fallback.
pub const ENTRIES: usize = 64;
pub const BLOCK_BYTES: usize = ENTRIES * 312;
pub const DEPTH: usize = 4;
pub const LEAD_MS: u64 = 16;
pub const FEEDBACK_DELAY_MS: u64 = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    New,
    Pending,
    Complete,
    Failed,
}
#[derive(Debug)]
pub struct Queue {
    pub states: Vec<State>,
    pub next: usize,
    pub pending: usize,
    pub submitted: usize,
    pub completed: usize,
    pub stopped: bool,
}
impl Queue {
    pub fn new(count: usize) -> Self {
        Self {
            states: vec![State::New; count],
            next: 0,
            pending: 0,
            submitted: 0,
            completed: 0,
            stopped: false,
        }
    }
    pub fn ready(&self) -> bool {
        !self.stopped && self.next < self.states.len() && self.pending < DEPTH
    }
    pub fn submit(&mut self, ok: bool) -> bool {
        if !self.ready() {
            return false;
        }
        self.states[self.next] = if ok { State::Pending } else { State::Failed };
        self.next += 1;
        if ok {
            self.pending += 1;
            self.submitted += 1;
        } else {
            self.stopped = true;
        }
        true
    }
    pub fn complete(&mut self, index: usize) -> bool {
        if self.states.get(index) != Some(&State::Pending) {
            return false;
        }
        self.states[index] = State::Complete;
        self.pending -= 1;
        self.completed += 1;
        true
    }
}
pub fn start_frame(base: u64, index: usize, output: bool) -> Option<u64> {
    base.checked_add((index as u64).checked_mul(8)?)?
        .checked_add(if output { FEEDBACK_DELAY_MS } else { 0 })
}
pub fn schedule_valid(now: u64, start: u64) -> bool {
    now.checked_add(1).is_some_and(|x| x < start)
}
pub fn valid_length(bytes: u16) -> bool {
    bytes > 0 && bytes <= 312 && bytes.is_multiple_of(24)
}
/// On a full eight-channel stream, only channel1 then channel2 carry test audio.
/// Hard amplitude ceiling 0.01 (-40 dBFS), with 20ms raised-cosine ramps.
pub fn tone_sample(frame: u64, channel: usize) -> f32 {
    let (begin, end, hz) = match channel {
        0 => (4800, 33600, 440.0),
        1 => (43200, 72000, 660.0),
        _ => return 0.0,
    };
    if frame < begin || frame >= end {
        return 0.0;
    }
    let distance = (frame - begin).min(end - 1 - frame).min(960) as f64;
    let envelope = 0.5 - 0.5 * (std::f64::consts::PI * distance / 960.0).cos();
    (0.01 * envelope * (std::f64::consts::TAU * hz * (frame - begin) as f64 / 48000.0).sin()) as f32
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn debt_failed_submit_and_stop() {
        let mut q = Queue::new(8);
        for _ in 0..4 {
            assert!(q.submit(true));
        }
        assert!(!q.ready());
        assert!(q.complete(2));
        assert!(!q.complete(2));
        assert!(q.submit(false));
        assert!(!q.ready());
        for i in [3, 0, 1] {
            assert!(q.complete(i));
        }
        assert_eq!(q.pending, 0);
        assert_eq!(q.submitted, q.completed);
        let mut q = Queue::new(2);
        q.submit(true);
        q.stopped = true;
        assert!(!q.submit(true));
        assert!(q.complete(0));
        assert_eq!(q.pending, 0);
    }
    #[test]
    fn scheduling_is_fixed_and_bounded() {
        assert_eq!(start_frame(100, 7, true), Some(220));
        assert!(!schedule_valid(219, 220));
        assert!(schedule_valid(218, 220));
        assert_eq!(start_frame(u64::MAX, 1, false), None);
        assert!(!schedule_valid(u64::MAX, 3));
    }
    #[test]
    fn captured_lengths_conserve_frames_without_nominal_fallback() {
        let lengths = [120, 144, 168, 144];
        assert!(lengths.into_iter().all(valid_length));
        assert_eq!(lengths.into_iter().map(|n| n / 24).sum::<u16>(), 24);
        for n in [0, 1, 23, 145, 313, u16::MAX] {
            assert!(!valid_length(n));
        }
    }
    #[test]
    fn tone_is_bounded_ramped_and_confined() {
        let mut nonzero = [0; 8];
        for frame in 0..100000 {
            for (ch, count) in nonzero.iter_mut().enumerate() {
                let x = tone_sample(frame, ch);
                assert!(x.is_finite() && x.abs() <= 0.01);
                if x != 0.0 {
                    *count += 1;
                }
            }
        }
        assert!(nonzero[0] > 20000 && nonzero[1] > 20000);
        assert!(nonzero[2..].iter().all(|&n| n == 0));
        for frame in [0, 4799, 4800, 33599, 33600, 43200, 71999, 72000, 98304] {
            assert!(
                tone_sample(frame, 0).abs() < 0.000001 && tone_sample(frame, 1).abs() < 0.000001
            );
        }
    }
}
