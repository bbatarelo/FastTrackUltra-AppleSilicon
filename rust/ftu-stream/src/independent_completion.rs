//! Generation-fenced USB ownership and bounded count handoff.
//! The experimental independent runtime connects these to native routing. The callback context must be serviced
//! independently; a second consumer of an owner-pumped source is insufficient.
use std::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    sync::{
        atomic::{
            AtomicU64, AtomicUsize,
            Ordering::{AcqRel, Acquire, Release},
        },
        Arc,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub(crate) enum State {
    Free,
    Preparing,
    Submitted,
    Completing,
    Complete,
    Reading,
    Claimed,
    Published,
    Released,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ticket {
    pub key: u32,
    pub epoch: u64,
    pub generation: u64,
}
fn word(generation: u64, state: State) -> Option<u64> {
    (generation <= u64::MAX >> 4).then_some((generation << 4) | state as u64)
}
/// All payload accesses hold one exclusive transient state. Publication and
/// acknowledgement are distinct; neither Completed nor Published grants reuse.
/// Panicking in a payload closure strands ownership, requiring quarantine.
/// No method returns a reference into the shared payload.
pub(crate) struct Slot<T: Copy + Send> {
    key: u32,
    epoch: u64,
    state: AtomicU64,
    data: UnsafeCell<T>,
}
// SAFETY: CAS grants exclusive payload access; release publishes it and acquire
// observes it. No transition can steal Preparing/Completing/Reading ownership.
unsafe impl<T: Copy + Send> Sync for Slot<T> {}
impl<T: Copy + Send> Slot<T> {
    pub fn new(epoch: u64, key: u32, data: T) -> Self {
        assert_ne!(epoch, 0);
        Self {
            key,
            epoch,
            state: AtomicU64::new(0),
            data: UnsafeCell::new(data),
        }
    }
    fn change(&self, t: Ticket, from: State, to: State) -> bool {
        if t.key != self.key || t.epoch != self.epoch || t.generation == 0 {
            return false;
        }
        let (Some(from), Some(to)) = (word(t.generation, from), word(t.generation, to)) else {
            return false;
        };
        self.state
            .compare_exchange(from, to, AcqRel, Acquire)
            .is_ok()
    }
    pub fn prepare(&self, previous: u64, t: Ticket, fill: impl FnOnce(&mut T)) -> bool {
        if t.key != self.key || t.epoch != self.epoch || t.generation <= previous {
            return false;
        }
        let (Some(old), Some(new)) = (
            word(previous, State::Free),
            word(t.generation, State::Preparing),
        ) else {
            return false;
        };
        if self
            .state
            .compare_exchange(old, new, AcqRel, Acquire)
            .is_err()
        {
            return false;
        }
        // SAFETY: Preparing cannot be transitioned by another method.
        fill(unsafe { &mut *self.data.get() });
        self.state
            .store(word(t.generation, State::Submitted).unwrap(), Release);
        true
    }
    /// Called only once native ownership has actually returned. A stale or
    /// duplicate callback cannot touch the current generation's payload.
    pub fn complete(&self, t: Ticket, fill: impl FnOnce(&mut T)) -> bool {
        if !self.change(t, State::Submitted, State::Completing) {
            return false;
        }
        fill(unsafe { &mut *self.data.get() });
        self.state
            .store(word(t.generation, State::Complete).unwrap(), Release);
        true
    }
    pub fn claim(&self, t: Ticket) -> Option<T> {
        if !self.change(t, State::Complete, State::Reading) {
            return None;
        }
        let copy = unsafe { *self.data.get() };
        self.state
            .store(word(t.generation, State::Claimed).unwrap(), Release);
        Some(copy)
    }
    pub fn publish(&self, t: Ticket) -> bool {
        self.change(t, State::Claimed, State::Published)
    }
    pub fn release(&self, t: Ticket) -> bool {
        self.change(t, State::Published, State::Released)
    }
    /// Submission owner consumes the generation-specific return ACK.
    pub fn acknowledge(&self, t: Ticket) -> bool {
        self.change(t, State::Released, State::Free)
    }
    pub fn state(&self) -> (u64, State) {
        let w = self.state.load(Acquire);
        let state = match w & 15 {
            0 => State::Free,
            1 => State::Preparing,
            2 => State::Submitted,
            3 => State::Completing,
            4 => State::Complete,
            5 => State::Reading,
            6 => State::Claimed,
            7 => State::Published,
            8 => State::Released,
            _ => unreachable!(),
        };
        (w >> 4, state)
    }
}

struct Queue<T: Copy + Send, const N: usize> {
    cells: [UnsafeCell<MaybeUninit<T>>; N],
    head: AtomicUsize,
    tail: AtomicUsize,
}
// SAFETY: construction produces exactly one non-clonable producer and consumer.
// Release/acquire cursors protect initialized cells and consumed-slot reuse.
unsafe impl<T: Copy + Send, const N: usize> Sync for Queue<T, N> {}
pub(crate) struct Producer<T: Copy + Send, const N: usize> {
    q: Arc<Queue<T, N>>,
    not_sync: PhantomData<Cell<()>>,
}
pub(crate) struct Consumer<T: Copy + Send, const N: usize> {
    q: Arc<Queue<T, N>>,
    not_sync: PhantomData<Cell<()>>,
}
pub(crate) fn channel<T: Copy + Send, const N: usize>() -> (Producer<T, N>, Consumer<T, N>) {
    assert!(N > 0);
    let q = Arc::new(Queue {
        cells: std::array::from_fn(|_| UnsafeCell::new(MaybeUninit::uninit())),
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
    });
    (
        Producer {
            q: q.clone(),
            not_sync: PhantomData,
        },
        Consumer {
            q,
            not_sync: PhantomData,
        },
    )
}
impl<T: Copy + Send, const N: usize> Producer<T, N> {
    /// Full or exhausted cursor: caller keeps the record and applies backpressure.
    pub fn push(&mut self, value: T) -> Result<(), T> {
        let tail = self.q.tail.load(Acquire);
        if tail - self.q.head.load(Acquire) == N {
            return Err(value);
        }
        let Some(next) = tail.checked_add(1) else {
            return Err(value);
        };
        unsafe {
            (*self.q.cells[tail % N].get()).write(value);
        }
        self.q.tail.store(next, Release);
        Ok(())
    }
}
impl<T: Copy + Send, const N: usize> Consumer<T, N> {
    pub fn pop(&mut self) -> Option<T> {
        let head = self.q.head.load(Acquire);
        if head == self.q.tail.load(Acquire) {
            return None;
        }
        let value = unsafe { (*self.q.cells[head % N].get()).assume_init_read() };
        self.q.head.store(head.checked_add(1).unwrap(), Release);
        Some(value)
    }
}
/// Call only after admissions stop, synchronous submits return, abort callbacks
/// finish and the completion context joins. Unknown ownership retains storage.
#[cfg(test)]
pub(crate) fn reclaimable<T: Copy + Send>(slots: &[Slot<T>], joined: bool) -> bool {
    joined && slots.iter().all(|s| s.state().1 == State::Free)
}

/// Stable storage with fail-closed destruction. The native boundary must call
/// finish only after its submit and completion contexts have actually joined.
/// Unknown ownership retains the arena rather than freeing callback/USB memory.
#[cfg(test)]
pub(crate) struct Arena<T: Copy + Send> {
    slots: Option<Box<[Slot<T>]>>,
    joined: bool,
}
#[cfg(test)]
impl<T: Copy + Send> Arena<T> {
    pub fn new(slots: Vec<Slot<T>>) -> Self {
        Self {
            slots: Some(slots.into_boxed_slice()),
            joined: false,
        }
    }
    pub fn finish(&mut self, joined: bool) -> bool {
        self.joined = joined;
        reclaimable(self.slots.as_ref().unwrap(), joined)
    }
}
#[cfg(test)]
impl<T: Copy + Send> std::ops::Deref for Arena<T> {
    type Target = [Slot<T>];
    fn deref(&self) -> &Self::Target {
        self.slots.as_ref().unwrap()
    }
}
#[cfg(test)]
impl<T: Copy + Send> Drop for Arena<T> {
    fn drop(&mut self) {
        if !reclaimable(self.slots.as_ref().unwrap(), self.joined) {
            std::mem::forget(self.slots.take().unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ticket(generation: u64) -> Ticket {
        Ticket {
            key: 0,
            epoch: 7,
            generation,
        }
    }
    #[test]
    fn independent_slot_requires_publication_release_and_matching_ack() {
        let slot = Slot::new(7, 0, [0; 8]);
        assert!(slot.prepare(0, ticket(1), |v| *v = [42; 8]));
        assert!(!slot.acknowledge(ticket(1)));
        assert!(!slot.complete(
            Ticket {
                key: 99,
                ..ticket(1)
            },
            |_| panic!("wrong slot touched")
        ));
        assert!(!slot.prepare(1, ticket(2), |_| panic!("pending touched")));
        assert!(!slot.complete(
            Ticket {
                key: 0,
                epoch: 6,
                generation: 1
            },
            |_| panic!("stale touched")
        ));
        assert!(slot.complete(ticket(1), |_| {}));
        assert!(!slot.complete(ticket(1), |_| panic!("duplicate touched")));
        assert_eq!(slot.claim(ticket(1)), Some([42; 8]));
        assert!(slot.claim(ticket(1)).is_none());
        assert!(!slot.release(ticket(1)));
        assert!(slot.publish(ticket(1)));
        assert!(!slot.acknowledge(ticket(1)));
        assert!(slot.release(ticket(1)));
        assert!(!slot.acknowledge(ticket(2)));
        assert!(slot.acknowledge(ticket(1)));
        assert!(slot.prepare(1, ticket(2), |v| *v = [99; 8]));
        assert!(!slot.complete(ticket(1), |_| panic!("ABA touched")));
        assert!(!slot.prepare(2, ticket(u64::MAX), |_| {}));
    }
    #[test]
    fn independent_count_queue_backpressure_preserves_order() {
        let (mut p, mut c) = channel::<u64, 2>();
        assert_eq!(p.push(1), Ok(()));
        assert_eq!(p.push(2), Ok(()));
        assert_eq!(p.push(3), Err(3));
        assert_eq!(c.pop(), Some(1));
        assert_eq!(p.push(3), Ok(()));
        assert_eq!(c.pop(), Some(2));
        assert_eq!(c.pop(), Some(3));
        assert_eq!(c.pop(), None);
    }
    #[test]
    fn independent_shutdown_retains_unknown_ownership_until_both_contexts_finish() {
        let slots = [Slot::new(7, 0, 0)];
        assert!(!reclaimable(&slots, false));
        assert!(slots[0].prepare(0, ticket(1), |_| {}));
        assert!(!reclaimable(&slots, true)); // lost abort callback: quarantine
        assert!(slots[0].complete(ticket(1), |_| {}));
        assert_eq!(slots[0].claim(ticket(1)), Some(0));
        assert!(!reclaimable(&slots, true));
        assert!(slots[0].publish(ticket(1)));
        assert!(slots[0].release(ticket(1)));
        assert!(slots[0].acknowledge(ticket(1)));
        assert!(!reclaimable(&slots, false));
        assert!(reclaimable(&slots, true));
    }
    #[test]
    fn independent_submit_rejection_and_callback_have_one_terminal_owner() {
        for callback_first in [false, true] {
            let s = Slot::new(7, 0, 0);
            assert!(s.prepare(0, ticket(1), |_| {}));
            // A synchronous rejection and a callback use the same CAS fence.
            // The loser must retain the winner's terminal ownership and data.
            assert!(s.complete(ticket(1), |v| *v = if callback_first { 1 } else { -1 }));
            assert!(!s.complete(ticket(1), |_| panic!("terminal owner overwritten")));
            assert_eq!(
                s.claim(ticket(1)),
                Some(if callback_first { 1 } else { -1 })
            );
            assert!(s.publish(ticket(1)));
            assert!(s.release(ticket(1)));
            assert!(s.acknowledge(ticket(1)));
            assert!(s.prepare(1, ticket(2), |v| *v = 2));
            assert!(!s.complete(ticket(1), |_| panic!("late callback inherited new request")));
        }
    }
    #[test]
    fn independent_count_cursor_exhaustion_never_wraps_into_old_cells() {
        let (mut p, mut c) = channel::<u64, 2>();
        p.q.head.store(usize::MAX, Release);
        p.q.tail.store(usize::MAX, Release);
        assert_eq!(p.push(99), Err(99));
        assert_eq!(c.pop(), None);
    }
    #[test]
    fn independent_real_threads_exchange_payload_and_counts_without_allocation() {
        let slots = Arc::new(std::array::from_fn::<_, 10, _>(|i| {
            Slot::new(7, i as u32, [0u64; 32])
        }));
        let (mut p, mut c) = channel::<(u64, u64), 16>();
        let other = slots.clone();
        let worker = std::thread::spawn(move || {
            crate::pool_simulation::without_allocations(|| {
                let began = std::time::Instant::now();
                for n in 1..=20_000u64 {
                    let s = &other[(n as usize - 1) % 10];
                    let t = Ticket {
                        key: ((n - 1) % 10) as u32,
                        ..ticket(n)
                    };
                    while s.state() != (n, State::Submitted) {
                        assert!(
                            began.elapsed().as_secs() < 10,
                            "submission peer failed to progress"
                        );
                        std::hint::spin_loop();
                    }
                    assert!(s.complete(t, |v| {
                        assert_eq!(*v, [n; 32]);
                        v[31] = !n;
                    }));
                    let data = s.claim(t).unwrap();
                    assert_eq!(data[31], !n);
                    assert!(s.publish(t));
                    while p.push((n, data[31])).is_err() {
                        assert!(
                            began.elapsed().as_secs() < 10,
                            "count consumer failed to progress"
                        );
                        std::hint::spin_loop();
                    }
                    assert!(s.release(t));
                }
            })
        });
        crate::pool_simulation::without_allocations(|| {
            let began = std::time::Instant::now();
            let mut spins = 0u64;
            let mut previous = [0; 10];
            let mut next = 1;
            let mut received = 0;
            while received < 20_000 {
                spins += 1;
                if spins.is_multiple_of(65536) {
                    assert!(
                        began.elapsed().as_secs() < 10,
                        "protocol peer failed to progress"
                    );
                }
                for (i, s) in slots.iter().enumerate() {
                    let (g, state) = s.state();
                    if state == State::Released {
                        assert!(s.acknowledge(Ticket {
                            key: i as u32,
                            ..ticket(g)
                        }));
                    }
                }
                if next <= 20_000 {
                    let i = (next as usize - 1) % 10;
                    if slots[i].prepare(
                        previous[i],
                        Ticket {
                            key: i as u32,
                            ..ticket(next)
                        },
                        |v| *v = [next; 32],
                    ) {
                        previous[i] = next;
                        next += 1;
                    }
                }
                if let Some((n, v)) = c.pop() {
                    received += 1;
                    assert_eq!((n, v), (received, !received));
                }
            }
        });
        worker.join().unwrap();
        for (i, s) in slots.iter().enumerate() {
            let (g, st) = s.state();
            if st == State::Released {
                assert!(s.acknowledge(Ticket {
                    key: i as u32,
                    ..ticket(g)
                }));
            }
        }
        assert!(reclaimable(slots.as_slice(), true));
    }
}
