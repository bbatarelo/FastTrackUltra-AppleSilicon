//! Standalone continuous-pool experiment harness. Diagnostic storage is reserved
//! before activation; the transport loop itself performs no allocation or I/O.
use crate::{
    clock::SampleClock, native::*, new_file, optional, pool::*, scheduler::*, PENDING_OWNERSHIP,
};
use std::{
    ffi::{c_char, c_void, CStr},
    io::{self, Write},
    path::Path,
};

unsafe fn save_trace(
    out: &mut impl Write,
    records: &[Vec<Trace>; 2],
    epoch: u64,
) -> io::Result<()> {
    for (d, direction) in records.iter().enumerate() {
        for r in direction {
            let Some(f) = r.flight else { continue };
            let completion = if let Some(c) = r.completion {
                format!("{{\"ticket\":{},\"status\":{},\"matched\":{},\"callback_ticks\":{},\"requested\":{:?},\"actual\":{:?},\"packet_status\":{:?},\"ticks\":{:?}}}",c.ticket,c.result.status,c.result.matched,c.result.callback_ticks,c.result.packets.map(|p|p.requested),c.result.packets.map(|p|p.actual),c.result.packets.map(|p|p.status),c.result.packets.map(|p|p.ticks))
            } else {
                "null".into()
            };
            writeln!(out,"{{\"epoch\":{},\"direction\":{},\"sequence\":{},\"slot\":{},\"ticket\":{},\"start\":{},\"submitted_at\":{},\"admission_ticks\":{},\"submit_status\":{},\"lengths\":{:?},\"first_sample\":{},\"crc_before\":{},\"crc_after\":{},\"peaks\":{:?},\"nonzero\":{:?},\"completion\":{}}}",epoch,d,f.seq,r.slot,f.ticket,f.start,r.submitted_at,r.admission_ticks,optional(r.submit),r.lengths,r.first_sample,r.crc_before,r.crc_after,r.peaks,r.nonzero,completion)?;
        }
    }
    out.flush()
}
/// # Safety
/// Same-thread native operations/pointers follow shared/ftu_native.h. The caller
/// must retain native resources and terminate its standalone process on code 4.
#[no_mangle]
pub unsafe extern "C" fn ftu_stream_run(
    ops: *const PoolOps,
    ctx: *mut c_void,
    output: *const c_char,
    duration: u32,
    tone: u32,
    epoch: u64,
    stop: *const i32,
) -> i32 {
    if ops.is_null()
        || ctx.is_null()
        || output.is_null()
        || stop.is_null()
        || epoch == 0
        || !(8..=60000).contains(&duration)
        || !duration.is_multiple_of(8)
        || tone > 1
        || (tone == 1 && duration != 10000)
    {
        return 2;
    }
    let Ok(directory) = CStr::from_ptr(output).to_str() else {
        return 2;
    };
    let path = Path::new(directory);
    let ops = &*ops;
    let files = (|| -> io::Result<_> {
        Ok((
            new_file(path, "plan.json")?,
            new_file(path, "events.jsonl")?,
            new_file(path, "trace.jsonl")?,
            new_file(path, "summary.json")?,
            new_file(path, "config.bin")?,
        ))
    })();
    let Ok((mut plan, mut events, mut trace, mut summary, mut config)) = files else {
        return 1;
    };
    let count = duration as usize / 8;
    let mut records = [vec![Trace::default(); count], vec![Trace::default(); count]];
    let waveform = if tone == 1 { tone_table() } else { Vec::new() };
    let mut session = PoolSession::new(epoch, Info::default());
    let mut rates = [Rates::default(); 2];
    let mut cleanup = 0;
    let mut active = false;
    let mut file_error = false;
    let result = (|| -> io::Result<()> {
        writeln!(plan,"{{\"schema_version\":3,\"engine\":\"rust_pool\",\"clock_group_anchor\":\"last_processing_tick\",\"epoch\":{},\"mode\":\"{}\",\"duration_ms\":{},\"rate_hz\":48000,\"channels\":8,\"entries\":64,\"block_ms\":8,\"packet_capacity\":312,\"pool_slots_per_direction\":4,\"feedback_slots\":32,\"feedback_delay_ms\":64,\"lead_ms\":16,\"native_audio_bytes\":{},\"journal_records\":{},\"journal_bytes\":{},\"tone_peak\":{},\"tone_period_frames\":96000,\"mixer_writes\":false,\"rate_writes\":false}}",epoch,if tone==1{"tone"}else{"silence"},duration,DEPTH*2*BLOCK_BYTES,count*2,count*2*std::mem::size_of::<Trace>(),if tone==1{"0.01"}else{"0.0"})?;
        plan.flush()?;
        let mut bytes = vec![0; 65535];
        let mut length = 0;
        let r = (ops.base.config)(ctx, bytes.as_mut_ptr(), bytes.len(), &mut length);
        writeln!(events, "{{\"event\":\"config\",\"status\":{}}}", r)?;
        if r != 0 || length > bytes.len() {
            session.stop("configuration_failed");
            return Ok(());
        }
        config.write_all(&bytes[..length])?;
        config.flush()?;
        if !ftu_protocol::validate_profile(&bytes[..length], false)
            || !ftu_protocol::validate_profile(&bytes[..length], true)
        {
            session.stop("profile_mismatch");
            return Ok(());
        }
        (ops.base.info)(ctx, &mut session.info);
        if session.info.callback_fault != 0 {
            session.stop("prior_native_callback_fault");
            return Ok(());
        }
        session.clock = SampleClock::new(
            epoch,
            session.info.timebase_numer,
            session.info.timebase_denom,
        );
        (ops.base.rates)(ctx, &mut rates[0]);
        if !rates[0].valid() {
            session.stop("rate_precondition");
            return Ok(());
        }
        let r = (ops.base.prepare)(ctx, DEPTH as u32);
        writeln!(events, "{{\"event\":\"prepare\",\"status\":{}}}", r)?;
        if r != 0 {
            session.stop("prepare_failed");
            return Ok(());
        }
        if std::ptr::read_volatile(stop) != 0 {
            session.stop("signal_before_activation");
            return Ok(());
        }
        let r = (ops.base.activate)(ctx);
        writeln!(events, "{{\"event\":\"activate\",\"status\":{}}}", r)?;
        if r != 0 {
            session.stop("activate_failed");
            return Ok(());
        }
        active = true;
        writeln!(events, "{{\"event\":\"stream_ready\",\"epoch\":{}}}", epoch)?;
        events.flush()?;
        session.run(ops, ctx, &mut records, &waveform, stop);
        Ok(())
    })();
    if result.is_err() {
        file_error = true;
        session.stop("evidence_write_failed");
    }
    session.drain(ops, ctx, &mut records);
    // Journal owns copied status/timestamp records, never native buffer references.
    // Restore hardware before potentially slow formatting and disk writes.
    if !session.drain_failed {
        cleanup = (ops.base.cleanup)(ctx);
        if cleanup == PENDING_OWNERSHIP {
            session.drain_failed = true;
            session.stop("native_pending_on_cleanup");
        } else {
            (ops.base.rates)(ctx, &mut rates[1]);
        }
    }
    (ops.base.info)(ctx, &mut session.info);
    if save_trace(&mut trace, &records, epoch).is_err() {
        file_error = true;
        session.stop("trace_write_failed");
    }
    if writeln!(
        events,
        "{{\"event\":\"cleanup\",\"status\":{},\"abort_input\":{},\"abort_output\":{}}}",
        cleanup,
        optional(session.abort_status[0]),
        optional(session.abort_status[1])
    )
    .and_then(|_| events.flush())
    .is_err()
    {
        file_error = true;
    }
    let code = if session.drain_failed {
        4
    } else if file_error || cleanup != 0 || session.info.callback_fault != 0 {
        1
    } else if session.reason.is_some()
        || !active
        || session.info.restored_mask != 3
        || session.info.open_mask != 0
        || !rates[1].valid()
        || session.completed != [count; 2]
    {
        3
    } else {
        0
    };
    let fit = session.clock.snapshot();
    let text=format!("{{\"engine\":\"rust_pool\",\"status\":\"{}\",\"exit_code\":{},\"reason\":{},\"epoch\":{},\"duration_ms\":{},\"tone\":{},\"submitted\":{:?},\"completed\":{:?},\"pending\":{:?},\"accepted_frames\":{:?},\"output_planned_frames\":{},\"max_pending\":{:?},\"reused\":{:?},\"drain_failed\":{},\"file_error\":{},\"cleanup_status\":{},\"restored_mask\":{},\"open_mask\":{},\"callback_fault\":{},\"restore_status\":{:?},\"native_cleanup_status\":{:?},\"rate_before\":{},\"rate_after\":{},\"timebase_numer\":{},\"timebase_denom\":{},\"bus_frame\":{},\"capture_fit\":{{\"epoch\":{},\"observations\":{},\"first_frame\":{},\"last_frame\":{},\"first_ticks\":{},\"last_ticks\":{},\"span_ns\":{},\"valid\":{},\"rate_hz\":{}}}}}\n",if code==0{"ok"}else if code==3{"incomplete"}else{"error"},code,session.reason.map(|r|format!("\"{}\"",r)).unwrap_or_else(||"null".into()),epoch,duration,tone==1,session.next,session.completed,session.pending(),session.frames,session.next_sample,session.max_pending,session.reused,session.drain_failed,file_error,cleanup,session.info.restored_mask,session.info.open_mask,session.info.callback_fault,session.info.restore_status,session.info.cleanup_status,rates[0].json(),rates[1].json(),session.info.timebase_numer,session.info.timebase_denom,session.anchor.frame,fit.epoch,fit.observations,fit.first_frame,fit.last_frame,fit.first_ticks,fit.last_ticks,fit.span_ns,fit.valid,optional(fit.rate_hz));
    let saved = summary
        .write_all(text.as_bytes())
        .and_then(|_| summary.flush())
        .is_ok();
    let _ = io::stdout().write_all(text.as_bytes());
    let _ = io::stdout().flush();
    if code == 4 {
        4
    } else if !saved {
        1
    } else {
        code
    }
}
