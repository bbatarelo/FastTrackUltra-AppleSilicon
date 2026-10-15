#ifndef FTU_NATIVE_H
#define FTU_NATIVE_H
#include <stdint.h>
#include <stddef.h>
/* Version 1, same-thread run-loop API. Direction 0 = capture IN81/interface2;
 * direction 1 = playback OUT01/interface1. No arbitrary USB access is exposed.
 * Buffers belong to the native adapter; Rust owns submission/lifecycle policy.
 * Writable only before submit; readable only after matching completion. */
#define FTU_ENTRIES 64u
#define FTU_BLOCK_BYTES (64u * 312u)
/* Private ownership status, distinct from IOKit teardown errors. */
#define FTU_PENDING_OWNERSHIP ((int32_t)-1000001)
struct ftu_packet { int32_t status; uint16_t requested, actual; uint64_t ticks; };
struct ftu_completion {
    int32_t status;
    uint32_t matched;
    uint64_t callback_ticks;
    struct ftu_packet packets[FTU_ENTRIES];
};
/* Rate array order is endpoint OUT01, IN81, independent of transfer direction. */
struct ftu_rates { int32_t status[2]; uint32_t hz[2], actual[2]; uint8_t raw[6]; };
struct ftu_clock { uint64_t frame, ticks; };
struct ftu_info {
    uint32_t timebase_numer, timebase_denom, location, vid, pid, stage;
    uint32_t original[2], pipe[2], restored_mask, open_mask, callback_fault;
    int32_t restore_status[2], cleanup_status[2];
};
struct ftu_native_ops {
    int32_t (*config)(void *, uint8_t *, size_t, size_t *);
    void (*rates)(void *, struct ftu_rates *);
    int32_t (*prepare)(void *, uint32_t);
    int32_t (*activate)(void *);
    int32_t (*clock)(void *, struct ftu_clock *);
    uint8_t *(*buffer)(void *, uint32_t, uint32_t);
    /* Copies lengths synchronously; never retains this caller-owned pointer. */
    int32_t (*submit)(void *, uint32_t, uint32_t, uint64_t, const uint16_t *);
    int32_t (*completion)(void *, uint32_t, uint32_t, struct ftu_completion *);
    void (*pump)(void *);
    int32_t (*abort)(void *, uint32_t);
    int32_t (*cleanup)(void *);
    void (*info)(void *, struct ftu_info *);
};
/* Rust entry point. Output directory must be new and owned by caller.
 * stop points to a live C sig_atomic_t, accessed volatile on this same thread.
 * On undrained callback debt exits the standalone process without releasing
 * live USB resources. Not callable from a long-lived HAL host. */
int32_t ftu_duplex_run(const struct ftu_native_ops *, void *, const char *,
                       uint32_t duration_ms, uint32_t tone, const volatile int32_t *stop);
/* Reusable-pool extension. The original one-shot ABI remains unchanged.
 * Each successful submit owes exactly one completion with the same ticket.
 * Reuse only after consuming a matched completion; never reuse a pending slot.
 * All calls and callback dispatch are on the same thread. */
struct ftu_pool_completion { uint64_t ticket; struct ftu_completion result; };
struct ftu_pool_ops {
    struct ftu_native_ops base;
    int32_t (*submit)(void *, uint32_t, uint32_t, uint64_t ticket, uint64_t frame, const uint16_t *);
    int32_t (*completion)(void *, uint32_t, uint32_t, struct ftu_pool_completion *);
    uint64_t (*now_ticks)(void *);
};
int32_t ftu_stream_run(const struct ftu_pool_ops *, void *, const char *,
                      uint32_t duration_ms, uint32_t tone, uint64_t epoch, const volatile int32_t *stop);
#endif
