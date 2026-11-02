//! Last independent native shutdown (ftuQ v1), retained across new sessions.
//! One worker writer, bounded lock-free readers; no IO-thread calls or USB reads.
use crate::runtime_fault::Seq;
pub const WORDS: usize = 44;
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub words: [u64; WORDS],
}
impl Default for Snapshot {
    fn default() -> Self {
        let mut words = [0; WORDS];
        words[0] = 1;
        words[1] = 1;
        Self { words }
    }
}
pub struct Shutdown {
    seq: Seq<WORDS>,
}
impl Shutdown {
    pub fn new() -> Self {
        Self {
            seq: Seq::new(Snapshot::default().words),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.seq
            .read()
            .map(|words| Snapshot { words })
            .unwrap_or_else(|| {
                let mut s = Snapshot::default();
                s.words[1] = 0;
                s
            })
    }
    #[cfg(feature = "independent-4ms")]
    pub fn begin(&self, epoch: u64) -> Snapshot {
        let mut s = Snapshot::default();
        s.words[2] = 1;
        s.words[3] = epoch;
        s.words[11] = self.seq.current()[11].saturating_add(1);
        s
    }
    #[cfg(feature = "independent-4ms")]
    pub fn publish(&self, s: &Snapshot) {
        self.seq.publish(&s.words);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_snapshot_is_absent_and_has_the_ffi_width() {
        let s = Shutdown::new().snapshot();
        assert_eq!(std::mem::size_of::<Snapshot>(), WORDS * 8);
        assert_eq!(
            (s.words[0], s.words[1], s.words[2], s.words[11]),
            (1, 1, 0, 0)
        );
    }
    #[cfg(feature = "independent-4ms")]
    #[test]
    fn counters_and_evidence_survive_between_native_shutdowns() {
        let state = Shutdown::new();
        let mut a = state.begin(14);
        a.words[18] = 24;
        state.publish(&a);
        assert_eq!(state.snapshot().words, a.words);
        let b = state.begin(15);
        assert_eq!((b.words[3], b.words[11], b.words[18]), (15, 2, 0));
        assert_eq!(
            state.snapshot().words,
            a.words,
            "begin alone never clears published evidence"
        );
        state.publish(&b);
        assert_eq!(state.snapshot().words, b.words);
    }
}
