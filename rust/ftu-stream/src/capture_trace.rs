//! Test-only bounded virtual-time trace. No driver ABI or runtime recording.
use std::cell::RefCell;
#[derive(Clone, Copy, Debug)]
pub(crate) struct Event {
    pub kind: &'static str,
    pub us: u64,
    pub seq: u64,
    pub a: u64,
    pub b: u64,
    pub c: u64,
}
struct Trace {
    begin: u64,
    end: u64,
    events: Vec<Event>,
}
thread_local! { static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) }; }
pub(crate) struct Scope;
impl Scope {
    pub fn new(begin: u64, end: u64) -> Self {
        Self::with_capacity(begin, end, 8192)
    }
    pub fn with_capacity(begin: u64, end: u64, capacity: usize) -> Self {
        TRACE.with(|v| {
            assert!(v.borrow().is_none());
            *v.borrow_mut() = Some(Trace {
                begin,
                end,
                events: Vec::with_capacity(capacity),
            });
        });
        Self
    }
    pub fn finish(self) -> Vec<Event> {
        TRACE.with(|v| v.borrow_mut().take().unwrap().events)
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        TRACE.with(|v| {
            v.borrow_mut().take();
        });
    }
}
pub(crate) fn record(kind: &'static str, us: u64, seq: u64, a: u64, b: u64, c: u64) {
    TRACE.with(|v| {
        if let Some(t) = v.borrow_mut().as_mut() {
            if (t.begin..=t.end).contains(&us) {
                assert!(t.events.len() < t.events.capacity(), "trace overflow");
                t.events.push(Event {
                    kind,
                    us,
                    seq,
                    a,
                    b,
                    c,
                });
            }
        }
    });
}
