#ifndef FTU_LIVE_H
#define FTU_LIVE_H
#include "ftu_native.h"
/* Rust owns session lifetime, audio timelines and resolved HAL geometry. Native
 * factory calls execute exclusively on the USB worker; no device-wide open. */
struct ftu_thread_schedule { uint32_t period_ns, computation_ns, constraint_ns; };
struct ftu_live_factory {
    /* registry: IORegistry entry ID (USB enumeration) the open must match on the
     * retained service before creating a user client; 0 = unpinned (tools). */
    void *(*open)(uint32_t product, uint32_t location, uint64_t registry);
    const struct ftu_pool_ops *(*ops)(void *);
    int32_t (*close)(void *);
    /* Null schedule restores normal scheduling before drain/teardown. */
    int32_t (*configure_thread)(void *, const struct ftu_thread_schedule *);
    /* Internal live factory v2; worker-only, direction IN81 then OUT01.
     * Returns native status and actual byte count, including failed requests. */
    int32_t (*set_rate)(void *, uint32_t direction, uint32_t hz, uint32_t *actual);
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
/* Timing v1 is observation only. Metrics are individually coherent snapshots;
 * valid=0 means a bounded read/reset race. Empty min/max/last are not measured.
 * Banks: 0 all observations, 1 captured cursor >= steady_after (48000 frames).
 * Buckets use exclusive upper bounds in limits, then an unbounded final bucket.
 * Indices/units are specified in docs/TIMING-OBSERVATION.md. */
#define FTU_TIMING_METRICS 12
#define FTU_TIMING_BUCKETS 17
struct ftu_timing_metric {
    uint64_t valid, count;
    int64_t min, max, last;
    uint64_t dropped, buckets[FTU_TIMING_BUCKETS];
};
struct ftu_timing_snapshot {
    uint32_t version, metric_count;
    uint64_t epoch, steady_after, valid;
    int64_t limits[FTU_TIMING_BUCKETS-1];
    struct ftu_timing_metric metrics[2][FTU_TIMING_METRICS];
};
void ftu_live_timing(struct ftu_live *, struct ftu_timing_snapshot *);
/* Worker observation v2 adds explicit transport geometry; separate from timing v1. Flags and field order are
 * specified in docs/WORKER-TIMING.md; no samples or scheduling policy here. */
#define FTU_WORKER_WORDS 32
#define FTU_WORKER_RECENT 16
#define FTU_WORKER_LATE 8
struct ftu_worker_event { uint64_t words[FTU_WORKER_WORDS]; };
struct ftu_worker_snapshot {
    uint64_t version, valid, epoch, timebase_numer, timebase_denom, queue_depth;
    uint64_t block_ms, entries, feedback_slots, replay_ms;
    uint64_t threshold_ms, observed, dropped, recent_overwritten, late_seen, late_retained, late_overflow;
    struct ftu_worker_event worst[2], recent[FTU_WORKER_RECENT], late[FTU_WORKER_LATE];
};
void ftu_live_worker_timing(struct ftu_live *, struct ftu_worker_snapshot *);

/* ftuD delivery v2 adds queue depth, block ms and active entries. Fixed word layouts are documented in DELIVERY-OBSERVATION.md.
 * ns histogram limits are exclusive; threshold counters use >=. No samples. */
#define FTU_DELIVERY_HEADER 31
#define FTU_DELIVERY_METRICS 8
#define FTU_DELIVERY_BUCKETS 18
#define FTU_DELIVERY_HIST 29
#define FTU_DELIVERY_RECORD 18
#define FTU_DELIVERY_RECENT 8
#define FTU_DELIVERY_LINK (FTU_WORKER_WORDS+12)
#define FTU_DELIVERY_SAMPLE 12
struct ftu_delivery_snapshot {
    uint64_t header[FTU_DELIVERY_HEADER];
    uint64_t metrics[2][FTU_DELIVERY_METRICS][FTU_DELIVERY_HIST];
    uint64_t worst[2][FTU_DELIVERY_RECORD], recent[FTU_DELIVERY_RECENT][FTU_DELIVERY_RECORD];
    uint64_t lead[2][FTU_DELIVERY_LINK], samples[2][FTU_DELIVERY_SAMPLE];
};
void ftu_live_delivery(struct ftu_live *, struct ftu_delivery_snapshot *);

/* ftuF v1: first failed completed request, immutable across later starts.
 * Primary precedes teardown; finished=1 adds terminal outcomes. No audio samples. Layout: STARTUP-RESTART.md. */
#define FTU_FAILURE_HEADER 35
#define FTU_FAILURE_RATE_WORDS 12
#define FTU_FAILURE_INFO_WORDS 17
struct ftu_failure_snapshot {
    uint64_t header[FTU_FAILURE_HEADER];
    uint64_t rates[2][FTU_FAILURE_RATE_WORDS], info[2][FTU_FAILURE_INFO_WORDS];
    uint64_t requested[64], packets[64][4], pump[6];
};
void ftu_live_first_failure(struct ftu_live *, struct ftu_failure_snapshot *);

/* ftuI v1: latest initializer, retained through teardown, reset on new epoch.
 * Outcome is initializer-only, not audio health. Layout: STARTUP-48K.md. */
struct ftu_startup_snapshot {
    uint64_t header[12], writes[2][4], rates[2][12];
};
void ftu_live_startup(struct ftu_live *, struct ftu_startup_snapshot *);

/* ftuP v1: every native pump; fixed layouts in ALL-PUMP-OBSERVATION.md. */
#define FTU_PUMPS_HEADER 13
#define FTU_PUMPS_PHASES 3
#define FTU_PUMPS_COUNTS 12
#define FTU_PUMPS_METRICS 6
#define FTU_PUMPS_HIST 21
#define FTU_PUMPS_RECORD 13
#define FTU_PUMPS_PAIR 26
#define FTU_PUMPS_RECENT 32
#define FTU_PUMPS_LINK 30
struct ftu_pumps_snapshot {
    uint64_t header[FTU_PUMPS_HEADER], counts[3][FTU_PUMPS_COUNTS];
    uint64_t metrics[3][FTU_PUMPS_METRICS][FTU_PUMPS_HIST];
    uint64_t worst[3][FTU_PUMPS_METRICS][FTU_PUMPS_PAIR];
    uint64_t recent[FTU_PUMPS_RECENT][FTU_PUMPS_RECORD], links[4][FTU_PUMPS_LINK];
};
void ftu_live_pumps(struct ftu_live *, struct ftu_pumps_snapshot *);

/* ftuR v1: PCM retry observations, coherent per epoch; see RETRY-COST-OBSERVATION.md.
 * Integer bus-frame gates differ from elapsed ns. waiting is not USB ownership. */
#define FTU_RETRY_HEADER 12
#define FTU_RETRY_COUNTS 10
struct ftu_retry_snapshot { uint64_t header[FTU_RETRY_HEADER], counts[2][FTU_RETRY_COUNTS]; };
void ftu_live_retry(struct ftu_live *, struct ftu_retry_snapshot *);

/* ftuE v1: last runtime stop of the streaming owner. Independent of ftuF (first
 * failed completion, lifetime latch). Retained across reset, Start and recovery;
 * a newer runtime fault replaces it and increments overwritten. Primary words are
 * published before teardown; finished=1 adds drain/cleanup/close outcomes.
 * Fields guarded by flags are "not measured" when the flag is clear, never zero.
 * Word layout: docs/RUNTIME-RECOVERY.md. No audio samples. */
#define FTU_RUNTIME_WORDS 46
struct ftu_runtime_snapshot { uint64_t words[FTU_RUNTIME_WORDS]; };
void ftu_live_runtime_fault(struct ftu_live *, struct ftu_runtime_snapshot *);

/* ftuQ v1: native route stop/dispose/cleanup only; excludes subsequent factory
 * close and Rust postconditions (see ftuS and ftuE for terminal outcome).
 * Retained across new starts. Word layout: docs/DISCONNECT-RECOVERY.md. */
#define FTU_SHUTDOWN_WORDS 44u
struct ftu_shutdown_snapshot { uint64_t words[FTU_SHUTDOWN_WORDS]; };
void ftu_live_shutdown(struct ftu_live *, struct ftu_shutdown_snapshot *);
/* ftuA v1: automatic recovery policy state and counters (control-side). Counters
 * persist across epochs. Word layout: docs/RUNTIME-RECOVERY.md. */
#define FTU_RECOVERY_WORDS 30
struct ftu_recovery_snapshot { uint64_t words[FTU_RECOVERY_WORDS]; };
void ftu_live_recovery(struct ftu_live *, struct ftu_recovery_snapshot *);
/* Monitor/control thread only; never from IO. Recovers only a cleanly drained
 * OUT lead-deadline fault (reason 55) of the independent profile, while admitted
 * clients remain and `binding` is the current publication; quarantine, uncertain
 * cleanup/close and other faults are refused. Clients are retained; the reopened
 * session uses a fresh epoch, i.e. a new GetZeroTimeStamp seed.
 * 0 recovered (running), 1 nothing to do, 2 backoff,
 * 3 session lock busy (not blocked), 4 worker finishing / IO draining, -1 null, -3 quarantined, -5 attempt did not
 * reach running, -6 detached/stale binding, -10 refused, -11 budget exhausted. */
int32_t ftu_live_recover(struct ftu_live *, uint64_t binding);

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
/* Reconnect (control thread only; serialized with Start/Stop by the session lock).
 * The resident HAL handle is never freed or replaced; only its USB binding moves.
 * create_pinned: HAL constructor; every worker open demands this registry ID
 *   (nonzero, else NULL). ftu_live_create stays unpinned for standalone tools.
 * detach: forbid new starts (-6 from Start), RETIRE the active IO clients, stop
 *   and drain the worker under the bounded Stop protocol. 0 = quiescent;
 *   -3 = quarantined.
 * Retired clients: Start of a retired ID returns -9 until its delayed Stop (which
 *   then only consumes the retirement, never touching the session or other
 *   clients) or remove_client acknowledges it. Distinct IDs start normally.
 *   Active + retired IDs share a fixed 64-slot budget; when full, Start of a new
 *   ID returns -2 without USB access. An app whose old client was never stopped
 *   or removed may need to stop and reopen the device.
 * remove_client: acknowledgement only (0 consumed, 1 not retired), never a stop.
 *   The plug-in's RemoveDeviceClient calls ftu_live_stop, which consumes a
 *   retirement first in the same locked call and otherwise stops as before.
 *   Duplicate or reordered old callbacks after an acknowledgement are not
 *   distinguishable from the reused ID's new session.
 * rebind: re-arm a detached handle for the unit enumerated as registry at
 *   location. -1 registry 0, -7 different product (the handle's model is fixed),
 *   -8 not detached, -3 quarantined or not quiescent. Advances the binding
 *   generation. Never opens USB: the next successful Start opens on the worker
 *   and begins a new epoch, so timestamps and IO from earlier epochs stay invalid.
 * binding/start_bound: read the generation before queueing on the session lock;
 *   start_bound returns -6 if a detach/rebind intervened, never opening the new
 *   unit for a Start issued against the previous publication.
 * Status.location reports the current binding. Quarantine is never recovered.
 * Runtime fault with admitted clients (independent profile): clients are retained;
 *   ftu_live_recover (monitor) or a Start from any ID applies the same bounded
 *   policy (see ftu_live_recover). Unrecovered, Start of a new or retained ID
 *   still returns -2. Stop of the last client or detach cancels a pending
 *   recovery; an unknown ID's Stop never affects other clients. */
struct ftu_live *ftu_live_create_pinned(const struct ftu_live_factory *, uint32_t product,
                                        uint32_t location, uint64_t registry);
int32_t ftu_live_detach(struct ftu_live *);
int32_t ftu_live_rebind(struct ftu_live *, uint32_t product, uint32_t location, uint64_t registry);
int32_t ftu_live_remove_client(struct ftu_live *, uint32_t client);
uint64_t ftu_live_binding(struct ftu_live *);
int32_t ftu_live_start_bound(struct ftu_live *, uint32_t client, uint64_t binding);
/* Control-owner-only destruction: caller MUST first prevent every future API
 * entry and join/quiesce all external callers. The internal IO counter does
 * not own the raw handle lifetime. Never called by the resident HAL plug-in.
 * Only succeeds when stopped, quiescent and with no live worker. */
int32_t ftu_live_destroy(struct ftu_live *);
extern const struct ftu_live_factory ftu_usb_factory;
/* Read-only discovery, exactly one supported unit; zero = success. */
int32_t ftu_usb_find(uint32_t *product, uint32_t *location);
/* Read-only discovery with classification. registry_id is the IORegistry entry
 * ID of the IOUSBHostDevice and changes on every USB enumeration, including a
 * fast replug at the same location. It is NOT a physical identity: the unit has
 * no serial number, so two units of one model are indistinguishable. UNKNOWN
 * (registry changed mid-scan, IOKit error) carries no presence information. */
struct ftu_usb_unit { uint32_t product, location; uint64_t registry_id; };
enum { FTU_USB_ONE = 0, FTU_USB_NONE = 1, FTU_USB_AMBIGUOUS = 2, FTU_USB_UNKNOWN = 3 };
int32_t ftu_usb_discover(struct ftu_usb_unit *);
#endif
