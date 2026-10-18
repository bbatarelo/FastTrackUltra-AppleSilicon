//! One static 48 kHz configuration for the HAL and the live Rust timelines.
//! Live queue depth is explicit; IN-derived packet counts are still replayed
//! 64 ms later, with immutable submitted payloads. Standalone probes retain
//! their original four-slot policy.
use crate::scheduler::{DEPTH, ENTRIES, FEEDBACK_DELAY_MS};

pub const CHANNELS: usize = 8;
pub const RATE: u32 = 48_000;
// AudioServerPlugIn.h requires at least 10923 frames. A power-of-two period
// above that floor also allows the empirically observed 4096-frame HAL ceiling.
// This is a clock publication interval, NOT a USB batch or buffering delay.
pub const ZTS_PERIOD: u64 = 16_384;
pub const MIN_BUFFER_FRAMES: u32 = 64;
pub const MAX_BUFFER_FRAMES: usize = 4096;
pub const DEFAULT_BUFFER_FRAMES: u32 = 256;
pub const INPUT_SAFETY: u32 = 1024;
// Single-variable output-safety candidate; USB ownership/queue stay qualified.
// Prediction, evidence limits and gates: docs/PLACEMENT-MODEL.md.
pub const LIVE_QUEUE_DEPTH: usize = 4;
pub const OUTPUT_SAFETY: u32 = 2560;
pub const PLAYBACK_OFFSET: u64 = FEEDBACK_DELAY_MS * RATE as u64 / 1000;
pub const USB_BLOCK_MAX_FRAMES: usize = ENTRIES * 13;
pub const RING_FRAMES: usize = 32_768;
pub const WRITE_AHEAD_FRAMES: u64 = (RING_FRAMES / 2) as u64;

// Keep enough space for both the accepted history and the future horizon.
// Include two maximum client buffers and a full USB completion batch in the
// future budget; actual HAL geometry must still be measured on each OS.
const _: () = assert!(ZTS_PERIOD >= 10_923);
const _: () = assert!(LIVE_QUEUE_DEPTH >= 2 && LIVE_QUEUE_DEPTH <= DEPTH);
const _: () = assert!(MIN_BUFFER_FRAMES <= DEFAULT_BUFFER_FRAMES);
const _: () = assert!(DEFAULT_BUFFER_FRAMES as usize <= MAX_BUFFER_FRAMES);
const _: () = assert!(USB_BLOCK_MAX_FRAMES <= MAX_BUFFER_FRAMES);
const _: () = assert!(
    2 * MAX_BUFFER_FRAMES + OUTPUT_SAFETY as usize + USB_BLOCK_MAX_FRAMES
        <= WRITE_AHEAD_FRAMES as usize
);
const _: () = assert!(MAX_BUFFER_FRAMES + (WRITE_AHEAD_FRAMES as usize) < RING_FRAMES);

/// Bound playback admission using the freshly published capture cursor.
/// The HAL zero timestamp may remain constant for an entire ZTS_PERIOD and
/// must not be used as the current timeline head. Absolute tags independently
/// protect against wrap/overwrite and epochs are checked by the session.
pub fn playback_window(first: u64, frames: u32, captured: u64) -> bool {
    frames > 0
        && frames as usize <= MAX_BUFFER_FRAMES
        && first >= captured.saturating_sub(MAX_BUFFER_FRAMES as u64)
        && first
            .checked_add(u64::from(frames))
            .zip(captured.checked_add(WRITE_AHEAD_FRAMES))
            .is_some_and(|(end, limit)| end <= limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_count_schedule_and_pcm_coordinates_agree() {
        assert_eq!(
            PLAYBACK_OFFSET,
            crate::scheduler::FEEDBACK_DELAY_MS * u64::from(RATE) / 1000
        );
        assert_eq!(ENTRIES / 8, 8);
        for seq in [0, 1, 8, 1000] {
            let input = crate::scheduler::start_frame(100, seq, false).unwrap();
            let output = crate::scheduler::start_frame(100, seq, true).unwrap();
            assert_eq!(output - input, crate::scheduler::FEEDBACK_DELAY_MS);
        }
    }
    #[test]
    fn write_window_covers_whole_hal_period_and_maximum_buffers() {
        for captured in [0, ZTS_PERIOD - 48, ZTS_PERIOD, 2 * ZTS_PERIOD - 1] {
            for buffer in [64, 128, 192, 256, 512, 1024, 2048, 4096] {
                let first = captured + u64::from(OUTPUT_SAFETY) + buffer;
                assert!(playback_window(first, buffer as u32, captured));
            }
        }
    }

    #[test]
    fn write_window_rejects_stale_future_empty_oversize_and_overflow() {
        let captured = 100_000;
        assert!(playback_window(
            captured - MAX_BUFFER_FRAMES as u64,
            1,
            captured
        ));
        assert!(!playback_window(
            captured - MAX_BUFFER_FRAMES as u64 - 1,
            1,
            captured
        ));
        assert!(playback_window(
            captured + WRITE_AHEAD_FRAMES - 1,
            1,
            captured
        ));
        assert!(!playback_window(captured + WRITE_AHEAD_FRAMES, 1, captured));
        assert!(!playback_window(captured, 0, captured));
        assert!(!playback_window(
            captured,
            MAX_BUFFER_FRAMES as u32 + 1,
            captured
        ));
        assert!(!playback_window(u64::MAX, 1, u64::MAX));
    }
}
