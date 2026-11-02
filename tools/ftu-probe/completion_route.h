/* Experimental native routing only; not part of ftu_pool_ops or a driver build.
 * Rust retains PCM, scheduling, ordered publication and count/ACK policy.
 * All methods except request_stop/release run on the creating (submitter) thread.
 * No device discovery/open/activation is performed by this boundary. */
#ifndef FTU_COMPLETION_ROUTE_H
#define FTU_COMPLETION_ROUTE_H
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/usb/IOUSBLib.h>
#include <stdbool.h>
#include <stdint.h>
#define FTU_ROUTE_MAX_SLOTS 32u
#define FTU_ROUTE_PACKET_BYTES 312u
struct ftu_route;
/* These mean the old service is gone, not that its callback debt is retired. */
static inline bool ftu_route_device_gone(IOReturn status) {
    return status == kIOReturnNoDevice || status == kIOReturnNotAttached;
}
/* Owner-thread fixed-width diagnostic, valid even when stop/destroy fails.
 * Never call after successful destroy. See shared/ftu_independent.h. */
void ftu_route_snapshot(struct ftu_route *, uint64_t words[16]);
struct ftu_route_endpoint {
    IOUSBInterfaceInterface300 **interface;
    CFRunLoopSourceRef source;
    uint8_t *data;
    IOUSBLowLatencyIsocFrame *frames;
    uint8_t pipe;
    size_t data_bytes, frame_count;
};
struct ftu_route_event {
    uint64_t token, ticket, callback_ticks;
    uint32_t direction, slot, entries;
    IOReturn status;
    const uint8_t *data;
    const IOUSBLowLatencyIsocFrame *frames;
};
struct ftu_route_config {
    /* IN81/interface2 then OUT01/interface1. Distinct FTU async sources.
     * Data/frame buffers must have LowLatencyCreateBuffer-compatible storage;
     * the injected offline backend uses equivalent aligned allocations. */
    struct ftu_route_endpoint endpoint[2];
    uint32_t slots, entries;
    void *context;
    /* Completion-thread hooks: bounded, nonallocating, must not unwind.
     * The event struct is borrowed only for this call; copy its fields.
     * Its data/frame storage stays immutable until release(token). poll retries
     * publication/backpressure; it is also called during drain. Release means
     * no retained pointers. No native thread calls an IOUSBLib method. */
    void (*complete)(void *, struct ftu_route *, const struct ftu_route_event *);
    void (*poll)(void *, struct ftu_route *, bool stopping);
    /* Called only by destroy after join and every ACK. Must release backend
     * buffers/interfaces and consumer context, including original source refs.
     * Return true only after complete cleanup; false retains the route permanently
     * (any partly released backend must still preserve its remaining resources).
     * No access to context is allowed after successful disposal. */
    bool (*dispose)(void *);
    /* Optional setup only: native scheduling on the dispatcher before readiness.
     * Failure joins the new thread and leaves backend ownership with caller. */
    bool (*thread_start)(void *);
};
struct ftu_route_stats {
    uint64_t callbacks, faults, releases, acknowledgements, rejections;
    uint32_t pending, held, returned;
    bool stop_requested, joined, quarantined, poisoned, ever_timed_out;
    IOReturn abort_status[2];
};
/* Sources must be exclusive, valid, unscheduled. Success transfers backend and
 * consumer lifetime to the route. dispose owns the caller's original source
 * references; the route retains/releases an additional reference to each.
 * Failure leaves all resources with caller. Creation waits for thread readiness;
 * this startup handshake is not a real-time bounded operation. */
struct ftu_route *ftu_route_create(const struct ftu_route_config *);
/* Writable data only in FREE state; owner must finish filling before submit.
 * Packet j begins at sum(lengths[0..j]), NOT j * FTU_ROUTE_PACKET_BYTES.
 * The slot stride is capacity only. lengths must contain entries elements. */
uint8_t *ftu_route_buffer(struct ftu_route *, unsigned direction, unsigned slot);
/* Nonzero returned token always needs ACK, including synchronous rejection.
 * token=0 means admission failed before a native request was made. */
IOReturn ftu_route_submit(struct ftu_route *, unsigned direction, unsigned slot,
                         uint64_t ticket, uint64_t frame, const uint16_t *lengths,
                         uint64_t *token);
/* Completion thread only. Duplicate/stale release poisons the session; wrong
 * thread or out-of-range token is rejected without touching slot state. */
bool ftu_route_release(struct ftu_route *, uint64_t token);
/* Owner-only ACK; never reuses a slot before the synchronous submit returns. */
bool ftu_route_ack(struct ftu_route *, unsigned direction, unsigned slot, uint64_t token);
/* Cross-thread stop request closes admission; an already admitted call may finish.
 * Caller MUST quiesce all request_stop callers before destroy (Rust should hold
 * a shared lifetime guard until those callers retire). This API is not refcounted. */
void ftu_route_request_stop(struct ftu_route *);
/* Owner-only stop: abort, service callbacks/publication, ACK, detach on dispatcher,
 * join. Stop consumes returned ACKs, so Rust must retire its ACK queue on stop.
 * Timeout retains the live dispatcher and all storage; retry can finish a later
 * clean drain. Faults are permanent. Timeout bounds waiting, not blocking native
 * AbortPipe calls or OS scheduling. No destroy is permitted until joined. */
bool ftu_route_stop(struct ftu_route *, unsigned timeout_ms);
/* poisoned distinguishes permanent failure from an unfinished timed-out drain.
 * ever_timed_out is historical even after a successful join. */
bool ftu_route_destroy(struct ftu_route *);
void ftu_route_stats(struct ftu_route *, struct ftu_route_stats *);
#endif
