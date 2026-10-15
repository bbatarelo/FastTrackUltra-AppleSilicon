//! Portable Fast Track Ultra core, ported from the staged C modules.
//!
//! The `#[no_mangle]` functions keep the exact C ABI of `capture_model.h`,
//! `capture_profile.h`, `feedback_profile.h`, `pcm.h` and `controls.h`
//! (except `ctl_emit`, which stays in C), so the existing C tests link
//! against this crate unchanged. Safe Rust wrappers are provided for the
//! engine. Enum-typed C fields and parameters are carried as `u32` so
//! untrusted discriminants are never materialised as Rust enums.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod capture;
pub mod controls;
pub mod pcm;
pub mod profile;

pub use capture::{packet_check, CapModel, CapPacket};
pub use controls::{build_plan, mixer_request, CtlRequest, CtlResult, MixerRequest};
pub use pcm::{decode_frames, decode_s24, encode_frames, encode_s24};
pub use profile::validate_profile;
