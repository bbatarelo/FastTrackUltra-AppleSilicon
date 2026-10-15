/* Standalone bounded IN experiment. No output pipe or mixer/rate writes.
 * See docs/USB-CAPTURE-CONTRACT.md for scheduling and lifetime contracts. */
#include "capture.h"
#include "capture_model.h"
#include "capture_profile.h"
#include "feedback_profile.h"
#include "pcm.h"
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOCFPlugIn.h>
#include <mach/mach_time.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define ENTRIES 64u
#define BLOCK_MS 8u
#define LEAD_MS 16u
#define DRAIN_MS 1000u
#define NOT_DONE ((IOReturn)kUSBLowLatencyIsochTransferKey)

struct capture;
struct block {
    struct capture *owner;
    size_t index;
    UInt64 frame_start;
    IOReturn submit_status, completion_status;
    uint64_t callback_ticks;
    bool callback_pointer_matches;
};
struct rate_read {
    bool attempted, accepted;
    IOReturn status;
    uint32_t actual, hz;
    uint8_t bytes[3];
};
struct capture {
    IOUSBDeviceInterface500 **dev;
    IOUSBInterfaceInterface300 **intf;
    CFRunLoopSourceRef source;
    uint8_t *data;
    IOUSBLowLatencyIsocFrame *frames;
    struct cap_model model;
    struct block blocks[CAP_MAX_BLOCKS];
    struct rate_read rates[2][2];
    mach_timebase_info_data_t timebase;
    uint64_t started, finished, anchor_ticks, current_ticks;
    UInt64 anchor_frame, current_frame, first_frame;
    bool anchor_ok, schedule_known, opened, alt_attempted, original_known;
    bool drain_failed, file_error, cleanup_error, callback_error;
    bool cleanup_attempted, restore_attempted, restore_verified;
    UInt8 original_alt, pipe;
    UInt16 vid, pid;
    UInt32 location;
    unsigned duration_ms;
    bool feedback;
    UInt8 interface_number, endpoint_number, endpoint_count;
    UInt16 packet_bytes;
    const char *reason;
    volatile sig_atomic_t *stop_signal;
    FILE *events, *observations, *raw, *audio, *summary;
    size_t accepted_packets, empty_packets, rejected_packets, unobserved_packets, accepted_frames;
    size_t short_packets, underrun_packets, raw_bytes, audio_bytes;
    double sumsq[8], peak[8];
    size_t nonzero[8];
};

static uint64_t ticks(AbsoluteTime t)
{
    uint64_t out;
    _Static_assert(sizeof t == sizeof out, "AbsoluteTime must be 64 bits");
    memcpy(&out, &t, sizeof out);
    return out;
}

static double ms_since(struct capture *c, uint64_t start)
{
    return (double)(mach_absolute_time() - start) * c->timebase.numer /
           c->timebase.denom / 1e6;
}

static void stop_capture(struct capture *c, const char *reason)
{
    if (!c->reason) c->reason = reason;
    cap_model_stop(&c->model);
}

static void event(struct capture *c, const char *name, IOReturn status)
{
    /* name is always a fixed internal string, never user or device text. */
    if (c->events && (fprintf(c->events,
            "{\"event\":\"%s\",\"ioreturn\":\"0x%08x\",\"host_ticks\":%" PRIu64 "}\n",
            name, (unsigned)status, mach_absolute_time()) < 0 || fflush(c->events))) {
        c->file_error = true;
        stop_capture(c, "event_write_failed");
    }
}

static FILE *new_file(const char *dir, const char *name)
{
    char path[PATH_MAX];
    int n = snprintf(path, sizeof path, "%s/%s", dir, name);
    if (n < 0 || (size_t)n >= sizeof path) { errno = ENAMETOOLONG; return NULL; }
    int fd = open(path, O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW, 0644);
    if (fd < 0) return NULL;
    FILE *f = fdopen(fd, "wb");
    if (!f) close(fd);
    return f;
}

static bool step(struct capture *c, const char *name, IOReturn r)
{
    event(c, name, r);
    if (r != kIOReturnSuccess) stop_capture(c, name);
    return r == kIOReturnSuccess && !c->file_error;
}

static void rate_reads(struct capture *c, unsigned phase)
{
    for (unsigned i = 0; i < 2; i++) {
        struct rate_read *r = &c->rates[phase][i];
        IOUSBDevRequestTO q = {0};
        q.bmRequestType = 0xa2; q.bRequest = 0x81; q.wValue = 0x0100;
        q.wIndex = i ? 0x81 : 0x01; q.wLength = 3; q.pData = r->bytes;
        q.noDataTimeout = q.completionTimeout = 500;
        q.wLenDone = UINT32_MAX;
        r->attempted = true;
        r->status = (*c->dev)->DeviceRequestTO(c->dev, &q);
        r->actual = q.wLenDone;
        r->accepted = r->status == kIOReturnSuccess && q.wLenDone == 3;
        if (r->accepted)
            r->hz = (uint32_t)r->bytes[0] | ((uint32_t)r->bytes[1] << 8) |
                    ((uint32_t)r->bytes[2] << 16);
        event(c, phase ? (i ? "rate_after_81" : "rate_after_01") :
                         (i ? "rate_before_81" : "rate_before_01"), r->status);
    }
}

static bool rates_48k(struct capture *c, unsigned phase)
{
    return c->rates[phase][0].accepted && c->rates[phase][0].hz == 48000 &&
           c->rates[phase][1].accepted && c->rates[phase][1].hz == 48000;
}

static bool profile(struct capture *c, const char *dir)
{
    UInt8 speed = 0, config = 0, nconfigs = 0;
    if (!step(c, "get_device_speed", (*c->dev)->GetDeviceSpeed(c->dev, &speed)) ||
        !step(c, "get_vendor", (*c->dev)->GetDeviceVendor(c->dev, &c->vid)) ||
        !step(c, "get_product", (*c->dev)->GetDeviceProduct(c->dev, &c->pid)) ||
        !step(c, "get_location", (*c->dev)->GetLocationID(c->dev, &c->location)) ||
        !step(c, "get_configuration", (*c->dev)->GetConfiguration(c->dev, &config)) ||
        !step(c, "get_configuration_count", (*c->dev)->GetNumberOfConfigurations(c->dev, &nconfigs)))
        return false;
    if (speed != kUSBDeviceSpeedHigh || c->vid != 0x0763 ||
        (c->pid != 0x2080 && c->pid != 0x2081) || config == 0) {
        stop_capture(c, "device_profile_mismatch"); return false;
    }
    for (unsigned i = 0; i < nconfigs; i++) {
        IOUSBConfigurationDescriptorPtr desc = NULL;
        if (!step(c, "get_cached_configuration", (*c->dev)->GetConfigurationDescriptorPtr(c->dev, (UInt8)i, &desc)))
            return false;
        if (!desc || desc->bConfigurationValue != config) continue;
        const uint8_t *p = (const uint8_t *)desc;
        size_t n = (size_t)p[2] | ((size_t)p[3] << 8);
        FILE *f = new_file(dir, c->feedback ? "feedback-config.bin" : "capture-config.bin");
        if (!f) { c->file_error = true; stop_capture(c, "configuration_file_failed"); return false; }
        bool saved = fwrite(p, 1, n, f) == n;
        if (fclose(f)) saved = false;
        if (!saved) { c->file_error = true; stop_capture(c, "configuration_file_failed"); return false; }
        if (!(c->feedback ? feedback_profile_validate(p, n) : cap_profile_validate(p, n))) { stop_capture(c, "capture_descriptor_mismatch"); return false; }
        return true;
    }
    stop_capture(c, "current_configuration_not_found"); return false;
}

static bool find_interface(struct capture *c)
{
    IOUSBFindInterfaceRequest request = { kIOUSBFindInterfaceDontCare,
        kIOUSBFindInterfaceDontCare, kIOUSBFindInterfaceDontCare, kIOUSBFindInterfaceDontCare };
    io_iterator_t iter = IO_OBJECT_NULL;
    if (!step(c, "create_interface_iterator", (*c->dev)->CreateInterfaceIterator(c->dev, &request, &iter)))
        return false;
    io_service_t chosen = IO_OBJECT_NULL, s;
    unsigned count = 0;
    while ((s = IOIteratorNext(iter))) {
        CFTypeRef prop = IORegistryEntryCreateCFProperty(s, CFSTR("bInterfaceNumber"), kCFAllocatorDefault, 0);
        int number = -1;
        bool match = prop && CFGetTypeID(prop) == CFNumberGetTypeID() &&
            CFNumberGetValue((CFNumberRef)prop, kCFNumberIntType, &number) && number == c->interface_number;
        if (prop) CFRelease(prop);
        if (match) { count++; if (!chosen) chosen = s; else IOObjectRelease(s); }
        else IOObjectRelease(s);
    }
    bool iterator_valid = IOIteratorIsValid(iter);
    IOObjectRelease(iter);
    if (count != 1 || !iterator_valid) {
        if (chosen) IOObjectRelease(chosen);
        stop_capture(c, "capture_interface_selection_failed"); return false;
    }
    IOCFPlugInInterface **plugin = NULL;
    SInt32 score = 0;
    IOReturn r = IOCreatePlugInInterfaceForService(chosen, kIOUSBInterfaceUserClientTypeID,
                         kIOCFPlugInInterfaceID, &plugin, &score);
    IOObjectRelease(chosen);
    if (!step(c, "create_capture_user_client", r) || !plugin) {
        if (plugin) (*plugin)->Release(plugin);
        if (!plugin) stop_capture(c, "null_capture_plugin");
        return false;
    }
    HRESULT h = (*plugin)->QueryInterface(plugin, CFUUIDGetUUIDBytes(kIOUSBInterfaceInterfaceID300),
                                         (LPVOID *)&c->intf);
    (*plugin)->Release(plugin);
    if (h || !c->intf) { stop_capture(c, "capture_interface_query_failed"); return false; }
    UInt8 number = 0; UInt16 vid = 0, pid = 0; UInt32 location = 0, interval_us = 0;
    if (!step(c, "interface_number", (*c->intf)->GetInterfaceNumber(c->intf, &number)) ||
        !step(c, "interface_vendor", (*c->intf)->GetDeviceVendor(c->intf, &vid)) ||
        !step(c, "interface_product", (*c->intf)->GetDeviceProduct(c->intf, &pid)) ||
        !step(c, "interface_location", (*c->intf)->GetLocationID(c->intf, &location)) ||
        !step(c, "interface_interval", (*c->intf)->GetFrameListTime(c->intf, &interval_us))) return false;
    if (number != c->interface_number || vid != c->vid || pid != c->pid || location != c->location || interval_us != 125) {
        stop_capture(c, "capture_interface_identity_mismatch"); return false;
    }
    return true;
}

static void completed(void *refcon, IOReturn result, void *arg0)
{
    struct block *b = refcon;
    struct capture *c = b->owner;
    b->callback_ticks = mach_absolute_time();
    b->completion_status = result;
    b->callback_pointer_matches = arg0 == c->frames + b->index * ENTRIES;
    if (!cap_model_complete(&c->model, b->index) || !b->callback_pointer_matches) {
        c->callback_error = true;
        stop_capture(c, "callback_ownership_error");
    }
    if (result != kIOReturnSuccess && result != kIOReturnUnderrun)
        stop_capture(c, "transfer_completion_error");
}

static bool prepare(struct capture *c)
{
    IOReturn open_result = (*c->intf)->USBInterfaceOpen(c->intf);
    c->opened = open_result == kIOReturnSuccess;
    if (!step(c, "open_capture_interface", open_result)) return false;
    if (!step(c, "get_original_alternate", (*c->intf)->GetAlternateSetting(c->intf, &c->original_alt))) return false;
    c->original_known = true;
    if (c->original_alt != 0) { stop_capture(c, "capture_interface_not_idle"); return false; }
    UInt8 type = 0, interval = 0; UInt16 max = 0;
    if (!step(c, "get_capture_endpoint", (*c->intf)->GetEndpointProperties(c->intf, 1, c->endpoint_number, kUSBIn, &type, &max, &interval)))
        return false;
    if (type != kUSBIsoc || max != c->packet_bytes || interval != 1) {
        stop_capture(c, "capture_endpoint_mismatch"); return false;
    }
    size_t entries = c->model.count * ENTRIES;
    if (!step(c, "allocate_payload", (*c->intf)->LowLatencyCreateBuffer(c->intf,
              (void **)&c->data, entries * c->packet_bytes, kUSBLowLatencyReadBuffer)) ||
        !step(c, "allocate_frame_list", (*c->intf)->LowLatencyCreateBuffer(c->intf,
              (void **)&c->frames, entries * sizeof *c->frames, kUSBLowLatencyFrameListBuffer))) return false;
    if (!c->data || !c->frames) { stop_capture(c, "null_low_latency_buffer"); return false; }
    memset(c->data, 0, entries * c->packet_bytes);
    memset(c->frames, 0, entries * sizeof *c->frames);
    for (size_t i = 0; i < entries; i++) {
        c->frames[i].frReqCount = c->packet_bytes;
        c->frames[i].frStatus = NOT_DONE;
    }
    if (!step(c, "create_async_source", (*c->intf)->CreateInterfaceAsyncEventSource(c->intf, &c->source))) return false;
    if (!c->source) { stop_capture(c, "null_async_source"); return false; }
    CFRunLoopAddSource(CFRunLoopGetCurrent(), c->source, kCFRunLoopDefaultMode);
    if (*c->stop_signal) { stop_capture(c, "signal_before_activation"); return false; }
    c->alt_attempted = true;
    if (!step(c, "select_capture_alternate_1", (*c->intf)->SetAlternateInterface(c->intf, 1))) return false;
    UInt8 n = 0, alt = 0;
    if (!step(c, "verify_active_alternate", (*c->intf)->GetAlternateSetting(c->intf, &alt)) ||
        !step(c, "get_active_endpoint_count", (*c->intf)->GetNumEndpoints(c->intf, &n))) return false;
    if (alt != 1 || n != c->endpoint_count) { stop_capture(c, "active_capture_layout_mismatch"); return false; }
    /* Pipe references are not endpoint addresses. Resolve the actual IN pipe;
     * on interface 1 verify the companion OUT descriptor but never submit it. */
    unsigned found = 0, out = 0;
    for (UInt8 pipe = 1; pipe <= n; pipe++) {
        UInt8 direction = 0, number = 0;
        if (!step(c, "get_active_pipe", (*c->intf)->GetPipeProperties(c->intf, pipe,
            &direction, &number, &type, &max, &interval))) return false;
        if (direction == kUSBIn && number == c->endpoint_number && type == kUSBIsoc &&
            max == c->packet_bytes && interval == 1) {
            c->pipe = pipe; found++;
        } else if (c->feedback && direction == kUSBOut && number == 1 &&
                   type == kUSBIsoc && max == CAP_PACKET_BYTES && interval == 1) out++;
        else { stop_capture(c, "capture_pipe_mismatch"); return false; }
    }
    if (found != 1 || out != (unsigned)c->feedback) {
        stop_capture(c, "capture_pipe_selection_failed"); return false;
    }
    event(c, c->feedback ? "feedback_pipe_ready" : "capture_pipe_ready", kIOReturnSuccess);
    return true;
}

static void stream(struct capture *c)
{
    AbsoluteTime t;
    IOReturn r = (*c->intf)->GetBusFrameNumberWithTime(c->intf, &c->anchor_frame, &t);
    c->anchor_ok = r == kIOReturnSuccess;
    if (c->anchor_ok) c->anchor_ticks = ticks(t);
    event(c, "bus_frame_anchor", r); /* Optional; lack of anchor is recorded. */
    if (!step(c, "bus_frame_now", (*c->intf)->GetBusFrameNumber(c->intf, &c->current_frame, &t))) return;
    c->current_ticks = ticks(t);
    if (c->current_frame > UINT64_MAX - LEAD_MS - c->duration_ms) {
        stop_capture(c, "schedule_overflow"); return;
    }
    c->first_frame = c->current_frame + LEAD_MS;
    c->schedule_known = true;
    for (size_t i = 0; i < c->model.count; i++)
        c->blocks[i].frame_start = c->first_frame + i * BLOCK_MS;
    uint64_t start = mach_absolute_time();
    while (!c->model.stopped && (c->model.next < c->model.count || c->model.pending)) {
        if (*c->stop_signal) { stop_capture(c, "signal"); break; }
        if (ms_since(c, start) > c->duration_ms + LEAD_MS + 500u) {
            stop_capture(c, "capture_deadline"); break;
        }
        while (cap_model_can_submit(&c->model, c->model.next)) {
            if (*c->stop_signal) { stop_capture(c, "signal"); break; }
            size_t i = c->model.next;
            struct block *b = &c->blocks[i];
            UInt64 now = 0;
            r = (*c->intf)->GetBusFrameNumber(c->intf, &now, &t);
            if (r != kIOReturnSuccess || now >= b->frame_start - 1) {
                stop_capture(c, r ? "schedule_clock_failed" : "schedule_too_late"); break;
            }
            b->submit_status = (*c->intf)->LowLatencyReadIsochPipeAsync(c->intf, c->pipe,
                c->data + i * ENTRIES * c->packet_bytes, b->frame_start, ENTRIES, 0,
                c->frames + i * ENTRIES, completed, b);
            /* Callbacks run only when this thread dispatches the source below. */
            if (!cap_model_submit(&c->model, i, b->submit_status == kIOReturnSuccess)) {
                c->callback_error = true; stop_capture(c, "submission_accounting_error"); break;
            }
            if (b->submit_status != kIOReturnSuccess) {
                stop_capture(c, "isochronous_submit_failed"); break;
            }
        }
        if (!c->model.stopped && c->model.pending)
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.004, true);
    }
    cap_model_stop(&c->model);
}

static void drain(struct capture *c)
{
    cap_model_stop(&c->model);
    if (!c->model.pending) return;
    event(c, "abort_capture_pipe", (*c->intf)->AbortPipe(c->intf, c->pipe));
    uint64_t start = mach_absolute_time();
    while (c->model.pending && ms_since(c, start) < DRAIN_MS)
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.004, true);
    if (c->model.pending) { c->drain_failed = true; stop_capture(c, "drain_deadline"); }
}

static void cleanup(struct capture *c)
{
    /* Never call with kernel/callback ownership outstanding. */
    if (!cap_model_can_release(&c->model)) abort();
    c->cleanup_attempted = true;
    if (c->opened && c->alt_attempted && c->original_known) {
        c->restore_attempted = true;
        IOReturn r = (*c->intf)->SetAlternateInterface(c->intf, c->original_alt);
        event(c, "restore_alternate", r);
        if (r != kIOReturnSuccess) c->cleanup_error = true;
        UInt8 actual = 255;
        r = (*c->intf)->GetAlternateSetting(c->intf, &actual);
        event(c, "verify_restored_alternate", r);
        c->restore_verified = r == kIOReturnSuccess && actual == c->original_alt;
        if (r != kIOReturnSuccess || actual != c->original_alt) c->cleanup_error = true;
    }
    if (c->source) {
        CFRunLoopRemoveSource(CFRunLoopGetCurrent(), c->source, kCFRunLoopDefaultMode);
        CFRelease(c->source); c->source = NULL;
    }
    if (c->data) {
        IOReturn r = (*c->intf)->LowLatencyDestroyBuffer(c->intf, c->data);
        event(c, "destroy_payload", r); if (r != kIOReturnSuccess) c->cleanup_error = true;
        c->data = NULL;
    }
    if (c->frames) {
        IOReturn r = (*c->intf)->LowLatencyDestroyBuffer(c->intf, c->frames);
        event(c, "destroy_frame_list", r); if (r != kIOReturnSuccess) c->cleanup_error = true;
        c->frames = NULL;
    }
    if (c->opened) {
        IOReturn r = (*c->intf)->USBInterfaceClose(c->intf);
        event(c, "close_capture_interface", r); if (r != kIOReturnSuccess) c->cleanup_error = true;
        c->opened = false;
    }
    if (c->intf) { (*c->intf)->Release(c->intf); c->intf = NULL; }
}

static bool status_ok(IOReturn r) { return r == kIOReturnSuccess || r == kIOReturnUnderrun; }

static void save_packets(struct capture *c)
{
    size_t segment = 0;
    for (size_t b = 0; b < c->model.count; b++) {
        const struct block *block = &c->blocks[b];
        bool complete = c->model.state[b] == CAP_COMPLETE && block->callback_pointer_matches;
        for (size_t slot = 0; slot < ENTRIES; slot++) {
            size_t idx = b * ENTRIES + slot;
            const IOUSBLowLatencyIsocFrame *f = complete ? c->frames + idx : NULL;
            bool observed = f && f->frStatus != NOT_DONE;
            struct cap_packet p = (c->feedback ? feedback_packet_check : cap_packet_check)(observed,
                observed && status_ok(block->completion_status) &&
                status_ok(f->frStatus) && f->frReqCount == c->packet_bytes,
                observed ? f->frActCount : 0);
            if (p.accepted && !f) {
                c->callback_error = true;
                p = (struct cap_packet){ CAP_PACKET_ERROR, 0, false };
            }
            size_t raw_offset = c->raw_bytes, audio_offset = c->audio_bytes;
            if (complete) {
                /* Keep the reserved slot, including padding. Valid lengths are in JSON. */
                if (fwrite(c->data + idx * c->packet_bytes, 1, c->packet_bytes, c->raw) != c->packet_bytes)
                    c->file_error = true;
                c->raw_bytes += c->packet_bytes;
            }
            if (p.accepted && f) {
                c->accepted_packets++; c->accepted_frames += p.audio_frames;
                if (p.kind == CAP_PACKET_EMPTY) c->empty_packets++;
                if (f->frActCount < c->packet_bytes) c->short_packets++;
                if (f->frStatus == kIOReturnUnderrun) c->underrun_packets++;
                const uint8_t *bytes = c->data + idx * c->packet_bytes;
                if (!c->feedback) {
                    if (fwrite(bytes, 1, f->frActCount, c->audio) != f->frActCount) c->file_error = true;
                    c->audio_bytes += f->frActCount;
                }
                for (size_t frame = 0; !c->feedback && frame < p.audio_frames; frame++)
                    for (size_t ch = 0; ch < 8; ch++) {
                        float sample = pcm_s24_float(bytes + frame * 24 + ch * 3);
                        c->sumsq[ch] += (double)sample * sample;
                        if (fabs((double)sample) > c->peak[ch]) c->peak[ch] = fabs((double)sample);
                        if (sample != 0) c->nonzero[ch]++;
                    }
            } else if (!observed) c->unobserved_packets++;
            else c->rejected_packets++;
            FILE *o = c->observations;
            fprintf(o, "{\"packet\":%zu,\"block\":%zu,\"slot\":%zu,\"frame_start\":", idx, b, slot);
            if (c->schedule_known) fprintf(o, "%" PRIu64, (uint64_t)block->frame_start); else fputs("null", o);
            fprintf(o, ",\"microframe_offset\":%zu,\"requested_length\":%u,\"submitted\":%s,\"submit_status\":",
                    slot, c->packet_bytes, c->model.state[b] == CAP_PENDING || c->model.state[b] == CAP_COMPLETE ? "true" : "false");
            if (c->model.state[b] != CAP_NEW) fprintf(o, "\"0x%08x\"", (unsigned)block->submit_status); else fputs("null", o);
            fputs(",\"completion_status\":", o);
            if (c->model.state[b] == CAP_COMPLETE) fprintf(o, "\"0x%08x\"", (unsigned)block->completion_status); else fputs("null", o);
            fputs(",\"callback_ticks\":", o);
            if (c->model.state[b] == CAP_COMPLETE) fprintf(o, "%" PRIu64, block->callback_ticks); else fputs("null", o);
            fputs(",\"callback_pointer_matches\":", o);
            if (c->model.state[b] == CAP_COMPLETE) fputs(block->callback_pointer_matches ? "true" : "false", o);
            else fputs("null", o);
            fputs(",\"frame_requested_length\":", o);
            if (f) fprintf(o, "%u", f->frReqCount); else fputs("null", o);
            fputs(",\"packet_status\":", o);
            if (f) fprintf(o, "\"0x%08x\"", (unsigned)f->frStatus); else fputs("null", o);
            fputs(",\"actual_length\":", o);
            if (observed) fprintf(o, "%u", f->frActCount); else fputs("null", o);
            fputs(",\"usb_timestamp_ticks\":", o);
            if (observed) fprintf(o, "%" PRIu64, ticks(f->frTimeStamp)); else fputs("null", o);
            fputs(",\"raw_file_offset\":", o);
            if (complete) fprintf(o, "%zu", raw_offset); else fputs("null", o);
            fprintf(o, ",\"accepted\":%s,\"classification\":\"%s\",\"audio_frames\":%zu,\"audio_file_offset\":",
                    p.accepted ? "true" : "false", cap_packet_name(p.kind), p.audio_frames);
            if (p.accepted && !c->feedback) fprintf(o, "%zu", audio_offset); else fputs("null", o);
            fprintf(o, ",\"segment\":%zu}\n", segment);
            if (!p.accepted || p.kind == CAP_PACKET_EMPTY) segment++;
        }
    }
    if (fflush(c->raw) || ferror(c->raw) || (c->audio && (fflush(c->audio) || ferror(c->audio))) ||
        fflush(c->observations) || ferror(c->observations)) c->file_error = true;
}

static void emit_rate(FILE *f, const struct rate_read *r)
{
    if (!r->attempted) { fputs("null", f); return; }
    fprintf(f, "{\"status\":\"0x%08x\",\"actual_length\":", (unsigned)r->status);
    if (r->actual != UINT32_MAX) fprintf(f, "%u", r->actual); else fputs("null", f);
    fprintf(f, ",\"accepted\":%s,\"hz\":", r->accepted ? "true" : "false");
    if (r->accepted) fprintf(f, "%u", r->hz); else fputs("null", f);
    fputs(",\"raw_hex\":", f);
    if (r->actual != UINT32_MAX && r->actual <= 3) {
        fputc('"', f); for (uint32_t i = 0; i < r->actual; i++) fprintf(f, "%02x", r->bytes[i]); fputc('"', f);
    } else fputs("null", f);
    fputc('}', f);
}

static int result_code(struct capture *c)
{
    if (c->drain_failed) return 4;
    if (c->file_error || c->cleanup_error || c->callback_error) return 1;
    if (c->reason || c->rejected_packets || c->unobserved_packets || !rates_48k(c, 1) ||
        c->model.completed != c->model.count) return 3;
    return 0;
}

static void emit_summary(struct capture *c, FILE *f)
{
    int code = result_code(c);
    fprintf(f, "{\n  \"status\":\"%s\",\"reason\":", code == 0 ? "ok" : code == 3 ? "incomplete" : "error");
    if (c->reason) fprintf(f, "\"%s\"", c->reason); else fputs("null", f);
    fprintf(f, ",\"exit_code\":%d,\"duration_ms\":%u,\"interface\":%u,\"alternate\":1,\"endpoint\":\"0x%02x\",\n",
            result_code(c), c->duration_ms, c->interface_number, 0x80u | c->endpoint_number);
    fprintf(f, "  \"mode\":\"%s\",\"packet_capacity\":%u,\"pipe_ref\":%u,\n",
            c->feedback ? "feedback" : "capture", c->packet_bytes, c->pipe);
    fprintf(f, "  \"vid\":%u,\"pid\":%u,\"location\":%u,\"original_alternate\":", c->vid, c->pid, c->location);
    if (c->original_known) fprintf(f, "%u", c->original_alt); else fputs("null", f);
    fprintf(f, ",\"planned_blocks\":%zu,\"submitted_blocks\":%zu,\"completed_blocks\":%zu,\"pending_blocks\":%zu,\"failed_submissions\":%zu,\n",
            c->model.count, c->model.submitted, c->model.completed, c->model.pending, c->model.failed);
    fprintf(f, "  \"accepted_packets\":%zu,\"empty_packets\":%zu,\"rejected_packets\":%zu,\"short_packets\":%zu,\"underrun_packets\":%zu,\"accepted_audio_frames\":%zu,\"raw_bytes\":%zu,\"audio_bytes\":%zu,\n",
            c->accepted_packets,c->empty_packets,c->rejected_packets,c->short_packets,c->underrun_packets,c->accepted_frames,c->raw_bytes,c->audio_bytes);
    fprintf(f, "  \"unobserved_packets\":%zu,\n", c->unobserved_packets);
    fprintf(f, "  \"file_error\":%s,\"cleanup_error\":%s,\"callback_error\":%s,\"drain_failed\":%s,\"signal\":%d,\n",
            c->file_error?"true":"false",c->cleanup_error?"true":"false",c->callback_error?"true":"false",c->drain_failed?"true":"false",(int)*c->stop_signal);
    fprintf(f, "  \"cleanup_attempted\":%s,\"alternate_restored\":",
            c->cleanup_attempted ? "true" : "false");
    fputs(c->restore_attempted ? (c->restore_verified ? "true" : "false") : "null", f);
    fputs(",\n", f);
    fprintf(f, "  \"host_start_ticks\":%" PRIu64 ",\"host_end_ticks\":%" PRIu64 ",\"timebase_numer\":%u,\"timebase_denom\":%u,\"bus_anchor\":",
            c->started,c->finished,c->timebase.numer,c->timebase.denom);
    if (c->anchor_ok) fprintf(f,"{\"usb_frame\":%" PRIu64 ",\"host_ticks\":%" PRIu64 "}",(uint64_t)c->anchor_frame,c->anchor_ticks); else fputs("null",f);
    fputs(",\"scheduling_clock\":",f);
    if (c->schedule_known) fprintf(f,"{\"usb_frame\":%" PRIu64 ",\"host_ticks\":%" PRIu64 ",\"first_frame\":%" PRIu64 "}",(uint64_t)c->current_frame,c->current_ticks,(uint64_t)c->first_frame); else fputs("null",f);
    fputs(",\n  \"rate_before\":[",f); emit_rate(f,&c->rates[0][0]); fputc(',',f); emit_rate(f,&c->rates[0][1]);
    fputs("],\"rate_after\":[",f); emit_rate(f,&c->rates[1][0]); fputc(',',f); emit_rate(f,&c->rates[1][1]);
    fputs("],\n  \"channels\":[",f);
    for (size_t ch=0;!c->feedback && ch<8;ch++) fprintf(f,"%s{\"channel\":%zu,\"nonzero_samples\":%zu,\"peak\":%.9g,\"rms\":%.9g}",ch?",":"",ch+1,c->nonzero[ch],c->peak[ch],c->accepted_frames?sqrt(c->sumsq[ch]/c->accepted_frames):0.0);
    fputs("]\n}\n",f);
}

static void close_file(struct capture *c, FILE **f)
{
    if (*f) {
        if (ferror(*f)) c->file_error = true;
        if (fclose(*f)) c->file_error = true;
    }
    *f = NULL;
}

static int input_run(IOUSBDeviceInterface500 **dev, const char *output,
                     unsigned duration_ms, volatile sig_atomic_t *stop_signal, bool feedback)
{
    if (!dev || !stop_signal || !duration_ms || duration_ms > 2048 || duration_ms % BLOCK_MS) return 2;
    struct capture *c = calloc(1, sizeof *c);
    if (!c) return 1;
    c->dev = dev; c->duration_ms = duration_ms; c->stop_signal = stop_signal;
    c->feedback = feedback;
    c->interface_number = feedback ? 1 : 2;
    c->endpoint_number = feedback ? 2 : 1;
    c->endpoint_count = feedback ? 2 : 1;
    c->packet_bytes = feedback ? 3 : CAP_PACKET_BYTES;
    if (mach_timebase_info(&c->timebase) != KERN_SUCCESS || !c->timebase.denom ||
        !cap_model_init(&c->model, duration_ms / BLOCK_MS)) { free(c); return 1; }
    c->started = mach_absolute_time();
    for (size_t i=0;i<c->model.count;i++) { c->blocks[i].owner=c; c->blocks[i].index=i; }
    c->events = new_file(output,"events.jsonl");
    c->observations = new_file(output,"observations.jsonl");
    c->raw = new_file(output,"packets.bin");
    if (!feedback) c->audio = new_file(output,"audio.s24le");
    c->summary = new_file(output,"summary.json");
    if (!c->events || !c->observations || !c->raw || (!feedback && !c->audio) || !c->summary) {
        c->file_error = true; stop_capture(c,"evidence_file_creation_failed"); goto finish;
    }
    FILE *plan = new_file(output, "plan.json");
    if (!plan) { c->file_error = true; stop_capture(c,"plan_file_creation_failed"); goto finish; }
    bool plan_ok = fprintf(plan,
        "{\"schema_version\":1,\"duration_ms\":%u,\"required_rate_hz\":48000,"
        "\"mode\":\"%s\",\"interface\":%u,\"alternate\":1,\"endpoint\":%u,\"packet_capacity\":%u,"
        "\"service_interval_us\":125,\"block_ms\":8,\"entries_per_block\":64,"
        "\"queue_depth\":4,\"schedule_lead_ms\":16,\"drain_timeout_ms\":1000,"
        "\"timebase_numer\":%u,\"timebase_denom\":%u}\n",
        duration_ms,feedback ? "feedback" : "capture",c->interface_number,
        0x80u | c->endpoint_number,c->packet_bytes,c->timebase.numer,c->timebase.denom) >= 0;
    if (fclose(plan)) plan_ok = false;
    if (!plan_ok) { c->file_error = true; stop_capture(c,"plan_file_write_failed"); goto finish; }
    event(c,feedback ? "feedback_begin" : "capture_begin",kIOReturnSuccess);
    if (!profile(c,output)) goto finish;
    rate_reads(c,0);
    if (!rates_48k(c,0)) { stop_capture(c,"rate_precondition_failed"); goto finish; }
    if (c->file_error || !find_interface(c) || !prepare(c)) goto finish;
    stream(c);
finish:
    drain(c);
    /* Pending blocks are not inspected, even on drain failure. */
    if (c->observations && c->raw && (feedback || c->audio)) save_packets(c);
    if (!c->drain_failed) {
        cleanup(c);
        if (c->rates[0][0].attempted) rate_reads(c,1);
        if (c->rates[0][0].attempted && !rates_48k(c,1)) stop_capture(c,"rate_postcondition_failed");
    }
    c->finished = mach_absolute_time();
    close_file(c,&c->events); close_file(c,&c->observations);
    close_file(c,&c->raw); close_file(c,&c->audio);
    if (c->summary) { emit_summary(c,c->summary); close_file(c,&c->summary); }
    emit_summary(c,stdout); fflush(stdout);
    int rc = result_code(c);
    if (c->drain_failed) {
        fprintf(stderr,"ftu-probe: callbacks did not drain; retaining all USB resources until process exit\n");
        _Exit(4);
    }
    free(c);
    return rc;
}

int capture_run(IOUSBDeviceInterface500 **dev, const char *output,
                unsigned duration_ms, volatile sig_atomic_t *stop_signal)
{
    return input_run(dev, output, duration_ms, stop_signal, false);
}

int feedback_run(IOUSBDeviceInterface500 **dev, const char *output,
                 unsigned duration_ms, volatile sig_atomic_t *stop_signal)
{
    return input_run(dev, output, duration_ms, stop_signal, true);
}
