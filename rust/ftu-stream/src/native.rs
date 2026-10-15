//! C ABI for same-thread OS calls. See shared/ftu_native.h for ownership.
use crate::scheduler::ENTRIES;
use std::ffi::c_void;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Packet {
    pub status: i32,
    pub requested: u16,
    pub actual: u16,
    pub ticks: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Completion {
    pub status: i32,
    pub matched: u32,
    pub callback_ticks: u64,
    pub packets: [Packet; ENTRIES],
}
impl Default for Completion {
    fn default() -> Self {
        Self {
            status: 0,
            matched: 0,
            callback_ticks: 0,
            packets: [Packet::default(); ENTRIES],
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Rates {
    pub status: [i32; 2],
    pub hz: [u32; 2],
    pub actual: [u32; 2],
    pub raw: [u8; 6],
}
impl Rates {
    pub fn valid(&self) -> bool {
        (0..2).all(|i| self.status[i] == 0 && self.actual[i] == 3 && self.hz[i] == 48000)
    }
    pub fn json(&self) -> String {
        format!(
            "{{\"endpoints\":[1,129],\"status\":{:?},\"hz\":{:?},\"actual\":{:?},\"raw\":{:?}}}",
            self.status, self.hz, self.actual, self.raw
        )
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Clock {
    pub frame: u64,
    pub ticks: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Info {
    pub timebase_numer: u32,
    pub timebase_denom: u32,
    pub location: u32,
    pub vid: u32,
    pub pid: u32,
    pub stage: u32,
    pub original: [u32; 2],
    pub pipe: [u32; 2],
    pub restored_mask: u32,
    pub open_mask: u32,
    pub callback_fault: u32,
    pub restore_status: [i32; 2],
    pub cleanup_status: [i32; 2],
}
#[repr(C)]
pub struct Ops {
    pub config: unsafe extern "C" fn(*mut c_void, *mut u8, usize, *mut usize) -> i32,
    pub rates: unsafe extern "C" fn(*mut c_void, *mut Rates),
    pub prepare: unsafe extern "C" fn(*mut c_void, u32) -> i32,
    pub activate: unsafe extern "C" fn(*mut c_void) -> i32,
    pub clock: unsafe extern "C" fn(*mut c_void, *mut Clock) -> i32,
    pub buffer: unsafe extern "C" fn(*mut c_void, u32, u32) -> *mut u8,
    pub submit: unsafe extern "C" fn(*mut c_void, u32, u32, u64, *const u16) -> i32,
    pub completion: unsafe extern "C" fn(*mut c_void, u32, u32, *mut Completion) -> i32,
    pub pump: unsafe extern "C" fn(*mut c_void),
    pub abort: unsafe extern "C" fn(*mut c_void, u32) -> i32,
    pub cleanup: unsafe extern "C" fn(*mut c_void) -> i32,
    pub info: unsafe extern "C" fn(*mut c_void, *mut Info),
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_layout() {
        assert_eq!(std::mem::size_of::<Packet>(), 16);
        assert_eq!(std::mem::size_of::<Completion>(), 1040);
        assert_eq!(std::mem::size_of::<Rates>(), 32);
        assert_eq!(std::mem::size_of::<Info>(), 68);
        assert_eq!(std::mem::size_of::<Clock>(), 16);
    }
}
