/* Offline injected backend for the REAL completion router. No device discovery. */
#ifndef FTU_INTEGRATION_NATIVE_H
#define FTU_INTEGRATION_NATIVE_H
#include "../../shared/ftu_native.h"
struct ftu_integrated_event {
    uint64_t token, ticket, logical_us;
    uint32_t direction, slot, entries, reserved;
    struct ftu_completion result;
    const uint8_t *data;
    uint64_t bytes;
};
struct ftu_integrated_hooks {
    void *context;
    void (*complete)(void *, void *, const struct ftu_integrated_event *);
    void (*poll)(void *, void *, uint64_t, uint32_t);
};
void *ftu_fixture_create(unsigned slots,const struct ftu_integrated_hooks *);
uint8_t *ftu_fixture_buffer(void *,unsigned,unsigned);
int32_t ftu_fixture_submit(void *,unsigned,unsigned,uint64_t,uint64_t,const uint16_t *,uint64_t *);
/* command: kind 0 = logical service tick; 1 = deliver one native completion.
 * One Mach message, one command. Waits only for its dispatch barrier, not audio. */
int32_t ftu_fixture_command(void *,uint64_t now,unsigned kind,unsigned direction,unsigned slot);
int32_t ftu_fixture_ack(void *,unsigned,unsigned,uint64_t);
int32_t ftu_fixture_release(void *route,uint64_t);
/* Fault selection before submit/stop: 1 missing abort, 2 reject next submit,
 * 3 malformed packet, 4 bad frame identity, 5 early callback before return, 6 device removal callback,
 * 7 pre-admission rejection (invalid request length), 8 Rust ticket mismatch,
 * 9 no-device abort with complete drain, 10 no-device abort with missing callbacks, 11 removal callback and no-device abort,
 * 12 IN-only no-device abort, 13 OUT-only no-device abort. */
void ftu_fixture_fault(void *,unsigned);
void ftu_fixture_clock(void *,int ppm);
void ftu_fixture_real_time(void *,uint64_t base);
uint64_t ftu_integrated_allocations(void);
int32_t ftu_fixture_stop(void *,unsigned timeout_ms);
int32_t ftu_fixture_destroy(void *);
uint64_t ftu_fixture_stat(void *,unsigned);
#endif
