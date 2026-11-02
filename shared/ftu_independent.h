/* Experimental live completion bridge. No changes to the stable HAL/factory ABI. */
#ifndef FTU_INDEPENDENT_H
#define FTU_INDEPENDENT_H
#include "ftu_native.h"
struct ftu_independent_event {
    uint64_t token, ticket, host_ticks;
    uint32_t direction, slot, entries, reserved;
    struct ftu_completion result;
    const uint8_t *data;
    uint64_t bytes;
};
struct ftu_independent_hooks {
    void *context;
    void (*complete)(void *, void *, const struct ftu_independent_event *);
    void (*poll)(void *, void *, uint64_t, uint32_t);
    uint32_t completion_qos; /* 1 = user-interactive; explicitly selected by Rust. */
};
/* Attach only after prepare/activate/rate readback, before any submit. Moves the
 * two async sources from the owner to the completion thread. On failure the
 * factory still owns everything. On success cleanup/close refuse until route
 * stop, join and destroy. Rust owns hooks.context until successful destruction. */
void *ftu_live_route_attach(void *, const struct ftu_independent_hooks *);
uint32_t ftu_live_route_fault(void *);
/* Shutdown ABI v1; owner-thread reads only, no USB calls.
 * Route: version, callbacks, faults, releases, ACKs, rejections, pending, held,
 * returned, stop_requested, joined, quarantined, poisoned, ever_timed_out,
 * signed abort status IN/OUT (16 words). Must read before successful destroy.
 * Cleanup: version, attempted-operation bitmask, removed endpoint mask,
 * restored/open masks, fatal status, then five signed statuses per endpoint:
 * SetAlternate, GetAlternate, destroy data, destroy frames, interface close.
 * A clear attempted bit means unavailable, not successful. The removal mask
 * never asserts an alternate was restored or that a rate was measured. */
void ftu_live_cleanup_snapshot(void *, uint64_t words[16]);
#endif
