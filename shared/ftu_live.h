#ifndef FTU_LIVE_H
#define FTU_LIVE_H
#include "ftu_native.h"
/* Rust owns session lifetime, audio timelines and resolved HAL geometry. Native
 * factory calls execute exclusively on the USB worker; no device-wide open. */
struct ftu_thread_schedule { uint32_t period_ns, computation_ns, constraint_ns; };
struct ftu_live_factory {
    void *(*open)(uint32_t product, uint32_t location);
    const struct ftu_pool_ops *(*ops)(void *);
    int32_t (*close)(void *);
    /* Null schedule restores normal scheduling before drain/teardown. */
    int32_t (*configure_thread)(void *, const struct ftu_thread_schedule *);
};
struct ftu_live;
struct ftu_live_config {
    uint32_t channels, rate, period, min_buffer, max_buffer, default_buffer;
    uint32_t input_safety, output_safety, playback_offset, ring_frames;
};
struct ftu_live_timestamp { uint64_t epoch, frame, ticks; };
struct ftu_live_status {
    uint32_t phase, product, location, reason;
    uint64_t epoch, captured, prepared, playback_missing, capture_missing;
    uint64_t submitted[2], completed[2], pending[2];
    int32_t cleanup_status, fault;
    int32_t submit_status; uint32_t submit_direction;
    uint64_t submit_sequence, submit_start, submit_checked, submit_returned;
};
/* phase: 0 idle, 1 starting, 2 running, 3 stopping, 4 fault, 5 quarantined.
 * Quarantine forbids restart and retains uncertain native resources in-process.
 * Startup/stop are control-thread calls; IO/timestamp calls are bounded, with
 * no mutex, allocation, filesystem access or USB call. Samples: interleaved f32. */
void ftu_live_configuration(struct ftu_live_config *);
struct ftu_live *ftu_live_create(const struct ftu_live_factory *, uint32_t product, uint32_t location);
int32_t ftu_live_start(struct ftu_live *, uint32_t client);
int32_t ftu_live_stop(struct ftu_live *, uint32_t client);
int32_t ftu_live_timestamp(struct ftu_live *, struct ftu_live_timestamp *);
int32_t ftu_live_write(struct ftu_live *, uint64_t first, uint32_t frames, const float *);
int32_t ftu_live_read(struct ftu_live *, uint64_t first, uint32_t frames, float *);
void ftu_live_status(struct ftu_live *, struct ftu_live_status *);
/* Control-owner-only destruction: caller MUST first prevent every future API
 * entry and join/quiesce all external callers. The internal IO counter does
 * not own the raw handle lifetime. Never called by the resident HAL plug-in.
 * Only succeeds when stopped, quiescent and with no live worker. */
int32_t ftu_live_destroy(struct ftu_live *);
extern const struct ftu_live_factory ftu_usb_factory;
/* Read-only discovery, exactly one supported unit; zero = success. */
int32_t ftu_usb_find(uint32_t *product, uint32_t *location);
#endif
