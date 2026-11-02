/* FastTrackUltra AudioServerPlugIn boundary.
 *
 * Objects: plug-in 1, device 2, input stream 3, output stream 4. All geometry comes
 * from ftu_live_configuration(); the Rust session owns USB, timelines and clients.
 * Real-time entry points (GetZeroTimeStamp, Begin/Do/EndIOOperation) only touch
 * atomics, immutable configuration and the bounded ftu_live_* IO calls. */
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreAudio/AudioHardware.h> /* property constants only; no client HAL calls */
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>
#include <unistd.h>
#include "../shared/ftu_live.h"

#define FTU_BUNDLE_ID "net.batarelo.FastTrackUltra"
#define FTU_PID_ULTRA 0x2080u
#define FTU_PID_ULTRA_8R 0x2081u
#define FTU_POLL_US 500000u
#define FTU_MAX_SAMPLE_TIME 1099511627776.0 /* 2^40 */
#define FTU_MAX_CHANNELS 8u
#ifndef FTU_BUILD_ID
#define FTU_BUILD_ID "unidentified-development-build"
#endif
#define FTU_STATUS_PROPERTY 'ftuS'
#define FTU_BUILD_PROPERTY 'ftuB'
#define FTU_TIMING_PROPERTY 'ftuT'
#define FTU_WORKER_PROPERTY 'ftuW'
#define FTU_DELIVERY_PROPERTY 'ftuD'
#define FTU_PUMPS_PROPERTY 'ftuP'
#define FTU_FAILURE_PROPERTY 'ftuF'
#define FTU_STARTUP_PROPERTY 'ftuI'
#define FTU_RETRY_PROPERTY 'ftuR'
#define FTU_HOTPLUG_PROPERTY 'ftuH'
#define FTU_RUNTIME_PROPERTY 'ftuE'
#define FTU_RECOVERY_PROPERTY 'ftuA'
#define FTU_SHUTDOWN_PROPERTY 'ftuQ'
#define FTU_CUSTOM_PROPERTIES 13

enum { kObjPlugIn = kAudioObjectPlugInObject, kObjDevice = 2, kObjInput = 3, kObjOutput = 4 };

static AudioServerPlugInDriverInterface g_vtable;
static AudioServerPlugInDriverInterface *g_vtable_ptr = &g_vtable;
static AudioServerPlugInDriverRef const g_ref = &g_vtable_ptr;

static _Atomic(ULONG) g_refs = 1;
static atomic_bool g_initialized;
static AudioServerPlugInHostRef g_host;
static struct ftu_live_config g_cfg; /* immutable after Initialize */

/* Selected model. Written once by adopt() before g_live is published with release
 * ordering; the handle and these strings are never destroyed or replaced for the
 * lifetime of the host process. Reconnect only moves the Rust USB binding. */
static _Atomic(struct ftu_live *) g_live;
static uint32_t g_product;
static CFStringRef g_name, g_uid, g_model;
static atomic_bool g_present, g_alive, g_running;
static _Atomic(UInt32) g_buffer_frames;

/* Monitor-thread-only binding (Initialize runs before the monitor exists). */
static bool g_bound;
static struct ftu_usb_unit g_unit;

/* ftuH hotplug diagnostics: single writer (monitor), seqlock for coherent reads. */
enum { FTU_REJECT_NONE, FTU_REJECT_AMBIGUOUS, FTU_REJECT_PRODUCT, FTU_REJECT_QUARANTINE, FTU_REJECT_REBIND };
static _Atomic(uint32_t) g_hp_seq, g_hp_bound, g_hp_location, g_hp_reject;
static _Atomic(uint64_t) g_hp_registry, g_hp_rebinds, g_hp_removals;
static _Atomic(int32_t) g_hp_detach_status, g_hp_rebind_status;
/* Monitor-driven recovery calls (ftuA "monitor_*"); written by the monitor only. */
static _Atomic(uint64_t) g_recover_calls;
static _Atomic(int32_t) g_recover_status;
/* Monitor-thread only. Delay only the stop notification while a clean-fault
 * decision is briefly blocked by worker completion / control-lock contention.
 * Property reads and realtime calls always report the actual phase. */
static unsigned g_recover_wait_polls;
#define FTU_RECOVERY_WAIT_POLLS 2u

/* ---------------------------------------------------------------- discovery */

static const char *model_name(uint32_t product)
{
    switch (product) {
    case FTU_PID_ULTRA: return "Fast Track Ultra";
    case FTU_PID_ULTRA_8R: return "Fast Track Ultra 8R";
    default: return NULL;
    }
}

static struct ftu_live *live_handle(void)
{
    return atomic_load_explicit(&g_live, memory_order_acquire);
}

/* The UID is port-independent so a moved unit keeps app selections. Without a
 * serial number it names "the one connected unit of this model", not a physical
 * unit; discovery refuses more than one connected unit. */
static struct ftu_live *adopt(uint32_t product, uint32_t location, uint64_t registry)
{
    CFStringRef name = CFStringCreateWithCString(NULL, model_name(product), kCFStringEncodingUTF8);
    CFStringRef uid = CFStringCreateWithFormat(NULL, NULL, CFSTR(FTU_BUNDLE_ID ":%04X:single"), product);
    CFStringRef model = CFStringCreateWithFormat(NULL, NULL, CFSTR(FTU_BUNDLE_ID ":%04X"), product);
    struct ftu_live *live = NULL;
    if (name && uid && model)
        live = ftu_live_create_pinned(&ftu_usb_factory, product, location, registry);
    if (!live) {
        if (name) CFRelease(name);
        if (uid) CFRelease(uid);
        if (model) CFRelease(model);
        return NULL;
    }
    g_name = name;
    g_uid = uid;
    g_model = model;
    g_product = product;
    atomic_store_explicit(&g_live, live, memory_order_release);
    return live;
}

static void publish_binding(void)
{
    atomic_fetch_add(&g_hp_seq, 1);
    atomic_store(&g_hp_bound, g_bound);
    atomic_store(&g_hp_location, g_bound ? g_unit.location : 0);
    atomic_store(&g_hp_registry, g_bound ? g_unit.registry_id : 0);
    atomic_fetch_add(&g_hp_seq, 1);
}

static uint32_t session_phase(struct ftu_live *live)
{
    struct ftu_live_status st;
    memset(&st, 0, sizeof st);
    ftu_live_status(live, &st);
    return st.phase;
}

static void notify(AudioObjectID obj, const AudioObjectPropertyAddress *addrs, UInt32 n)
{
    if (g_host && g_host->PropertiesChanged)
        g_host->PropertiesChanged(g_host, obj, n, addrs);
}

/* Called from Initialize (notify=false) and then only from the monitor thread, so
 * handle creation and binding are single-threaded. The plug-in holds no lock;
 * Rust serializes detach/rebind with Start/Stop and returns before notification.
 *
 * Bound: the same product, location AND registry generation must remain the only
 * supported unit. Anything else (absence, port/hub move, fast replug at the same
 * port, a second unit) detaches and publishes removal in this poll. Unbound: one
 * unit of the adopted model is rebound and published on a later poll, so hosts see
 * removal then arrival. A different model or quarantine is refused, never forced.
 * Registry entry IDs reveal re-enumeration missed between polls; they cannot
 * distinguish two physical units of one model (no serial number). */
static void poll_presence(bool notify_host)
{
    struct ftu_usb_unit unit;
    int32_t found = ftu_usb_discover(&unit);
    if (found == FTU_USB_ONE && !model_name(unit.product))
        found = FTU_USB_UNKNOWN;
    struct ftu_live *live = live_handle();
    bool was_alive = atomic_load(&g_alive); /* only this thread writes g_alive */
    bool known = found != FTU_USB_UNKNOWN; /* transient: keep state, retry next poll */
    bool binding_changed = false;
    if (known && !live) {
        if (found == FTU_USB_ONE && (live = adopt(unit.product, unit.location, unit.registry_id))) {
            g_bound = true;
            g_unit = unit;
            binding_changed = true;
        }
    } else if (known && g_bound) {
        if (!(found == FTU_USB_ONE && unit.product == g_product && unit.location == g_unit.location &&
              unit.registry_id == g_unit.registry_id)) {
            /* Close CoreAudio admission first; Rust then refuses starts and drains. */
            atomic_store(&g_alive, false);
            g_bound = false;
            binding_changed = true;
            atomic_fetch_add(&g_hp_removals, 1);
            atomic_store(&g_hp_detach_status, ftu_live_detach(live));
        }
    } else if (known) {
        uint32_t reject = FTU_REJECT_NONE;
        if (found == FTU_USB_AMBIGUOUS)
            reject = FTU_REJECT_AMBIGUOUS;
        else if (found == FTU_USB_ONE && unit.product != g_product)
            reject = FTU_REJECT_PRODUCT;
        else if (found == FTU_USB_ONE && session_phase(live) == 5)
            reject = FTU_REJECT_QUARANTINE;
        else if (found == FTU_USB_ONE) {
            int32_t rc = ftu_live_rebind(live, unit.product, unit.location, unit.registry_id);
            atomic_store(&g_hp_rebind_status, rc);
            if (rc == 0) {
                g_bound = true;
                g_unit = unit;
                binding_changed = true;
                atomic_fetch_add(&g_hp_rebinds, 1);
            } else
                reject = FTU_REJECT_REBIND;
        }
        atomic_store(&g_hp_reject, reject);
    }
    if (binding_changed)
        publish_binding();
    /* The bound unit was confirmed unchanged (product, location, registry) by
     * this poll; only then may a retained session be recovered. */
    bool steady = known && live && g_bound && !binding_changed;
    uint32_t phase = live ? session_phase(live) : 0;
    /* First try a clean immediate recovery before publishing a transient stop.
     * A successful reopen keeps the admitted clients and changes the ZTS seed;
     * telling the host to observe a stopped engine first could solicit StopIO
     * and cancel that same recovery. Backoff/refusal still publishes stopped.
     * RUNNING polls advance the healthy-duration budget and its diagnostics. */
    bool defer_stop_note = false;
    if (notify_host && live && (phase == 5 || (steady && (phase == 4 || phase == 2)))) {
        int32_t rc = ftu_live_recover(live, ftu_live_binding(live));
        atomic_fetch_add(&g_recover_calls, 1);
        atomic_store(&g_recover_status, rc);
        phase = session_phase(live);
        if (phase == 4 && (rc == 3 || rc == 4) && atomic_load(&g_running) &&
            g_recover_wait_polls < FTU_RECOVERY_WAIT_POLLS) {
            ++g_recover_wait_polls;
            defer_stop_note = true;
        }
    }
    if (!defer_stop_note)
        g_recover_wait_polls = 0;
    /* A quarantined handle stays visible but dead while one unit of its model is
     * connected, as before; it is never rebound automatically. */
    bool present = atomic_load(&g_present);
    if (known)
        present = live && (g_bound || (phase == 5 && found == FTU_USB_ONE && unit.product == g_product));
    bool alive = present && phase != 5, running = present && phase == 2;
    bool running_changed = !defer_stop_note && atomic_exchange(&g_running, running) != running;
    bool list_changed = atomic_exchange(&g_present, present) != present;
    atomic_store(&g_alive, alive);
    bool alive_changed = was_alive != alive;
    if (!notify_host)
        return;
    /* Announce arrival first, then explicitly publish availability even if the
     * local dead/idle values did not change while this object was absent. */
    if (list_changed) {
        static const AudioObjectPropertyAddress a[2] = {
            { kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain },
            { kAudioObjectPropertyOwnedObjects, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain } };
        notify(kObjPlugIn, a, 2);
    }
    if (running_changed || (list_changed && present)) {
        const AudioObjectPropertyAddress a = {kAudioDevicePropertyDeviceIsRunning,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
        notify(kObjDevice,&a,1);
    }
    if (alive_changed || (list_changed && present)) {
        static const AudioObjectPropertyAddress a = {
            kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
        notify(kObjDevice, &a, 1);
    }


}

static void *monitor_main(void *arg)
{
    (void)arg;
    pthread_setname_np("net.batarelo.FastTrackUltra.monitor");
    for (;;) {
        usleep(FTU_POLL_US);
        poll_presence(true);
    }
    return NULL;
}

/* --------------------------------------------------------------- properties */

typedef union {
    UInt32 u32[2];
    Float64 f64;
    CFStringRef str;
    AudioObjectID ids[2];
    AudioValueRange range;
    AudioStreamBasicDescription asbd;
    AudioStreamRangedDescription ranged;
    AudioBufferList abl;
    struct { AudioChannelLayoutTag mChannelLayoutTag; AudioChannelBitmap mChannelBitmap; UInt32 mNumberChannelDescriptions; AudioChannelDescription mChannelDescriptions[FTU_MAX_CHANNELS]; } layout;
    uint8_t raw[256];
    AudioServerPlugInCustomPropertyInfo custom[FTU_CUSTOM_PROPERTIES];
} prop_buf;

/* elem != 0 marks arrays that may be truncated to the caller's buffer. */
typedef struct { UInt32 size, elem; bool cf; } prop_shape;

static OSStatus put_u32(prop_buf *b, prop_shape *s, UInt32 v)
{
    b->u32[0] = v;
    s->size = sizeof(UInt32);
    return kAudioHardwareNoError;
}

static OSStatus put_str(prop_buf *b, prop_shape *s, CFStringRef v)
{
    if (!v)
        return kAudioHardwareUnspecifiedError;
    b->str = v;
    s->size = sizeof(CFStringRef);
    s->cf = true;
    return kAudioHardwareNoError;
}

static OSStatus put_ids(prop_buf *b, prop_shape *s, const AudioObjectID *ids, UInt32 n)
{
    for (UInt32 i = 0; i < n; i++)
        b->ids[i] = ids[i];
    s->size = n * (UInt32)sizeof(AudioObjectID);
    s->elem = sizeof(AudioObjectID);
    return kAudioHardwareNoError;
}

/* OwnedObjects accepts a class array, including superclass matches. Our owned
 * devices and streams directly derive from AudioObject, as reported above. */
static OSStatus put_owned(prop_buf *b, prop_shape *s, const AudioObjectID *ids, UInt32 n,
                          AudioClassID object_class, UInt32 qsize, const void *qdata)
{
    if (qsize % sizeof(AudioClassID) || (qsize && !qdata))
        return kAudioHardwareBadPropertySizeError;
    bool matches = qsize == 0;
    for (UInt32 offset = 0; offset < qsize; offset += sizeof(AudioClassID)) {
        AudioClassID requested;
        memcpy(&requested, (const uint8_t *)qdata + offset, sizeof requested);
        matches |= requested == object_class || requested == kAudioObjectClassID;
    }
    return put_ids(b, s, ids, matches ? n : 0);
}

static OSStatus put_range(prop_buf *b, prop_shape *s, Float64 lo, Float64 hi)
{
    b->range.mMinimum = lo;
    b->range.mMaximum = hi;
    s->size = sizeof(AudioValueRange);
    return kAudioHardwareNoError;
}

static void fill_format(AudioStreamBasicDescription *f)
{
    f->mSampleRate = g_cfg.rate;
    f->mFormatID = kAudioFormatLinearPCM;
    f->mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagsNativeEndian;
    f->mBytesPerPacket = (UInt32)sizeof(Float32) * g_cfg.channels;
    f->mFramesPerPacket = 1;
    f->mBytesPerFrame = (UInt32)sizeof(Float32) * g_cfg.channels;
    f->mChannelsPerFrame = g_cfg.channels;
    f->mBitsPerChannel = 32;
}

static OSStatus plugin_prop(const AudioObjectPropertyAddress *a, UInt32 qsize, const void *qdata,
                            bool fetch, prop_buf *b, prop_shape *s)
{
    static const AudioObjectID device = kObjDevice;
    bool present = atomic_load(&g_present);
    switch (a->mSelector) {
    case kAudioObjectPropertyBaseClass: return put_u32(b, s, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return put_u32(b, s, kAudioPlugInClassID);
    case kAudioObjectPropertyOwner: return put_u32(b, s, kAudioObjectUnknown);
    case kAudioObjectPropertyName: return put_str(b, s, CFSTR("FastTrackUltra"));
    case kAudioObjectPropertyManufacturer: return put_str(b, s, CFSTR("net.batarelo"));
    case kAudioObjectPropertyOwnedObjects:
        return put_owned(b, s, &device, present ? 1 : 0, kAudioDeviceClassID, qsize, qdata);
    case kAudioPlugInPropertyDeviceList: return put_ids(b, s, &device, present ? 1 : 0);
    case kAudioPlugInPropertyTranslateUIDToDevice: {
        CFStringRef uid = NULL;
        if (fetch) {
            if (qsize != sizeof(CFStringRef) || !qdata)
                return kAudioHardwareBadPropertySizeError;
            uid = *(const CFStringRef *)qdata;
        }
        bool match = present && uid && CFGetTypeID(uid) == CFStringGetTypeID() && CFEqual(uid, g_uid);
        return put_u32(b, s, match ? kObjDevice : kAudioObjectUnknown);
    }
    case kAudioPlugInPropertyResourceBundle: return put_str(b, s, CFSTR(""));
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus device_prop(const AudioObjectPropertyAddress *a, UInt32 qsize, const void *qdata,
                            prop_buf *b, prop_shape *s)
{
    static const AudioObjectID streams[2] = { kObjInput, kObjOutput }, self = kObjDevice;
    bool in = a->mScope == kAudioObjectPropertyScopeInput, out = a->mScope == kAudioObjectPropertyScopeOutput;
    bool io = in || out;
    switch (a->mSelector) {
    case FTU_BUILD_PROPERTY: return put_str(b, s, CFSTR(FTU_BUILD_ID));
    case FTU_RUNTIME_PROPERTY:
    case FTU_RECOVERY_PROPERTY:
    case FTU_SHUTDOWN_PROPERTY:
    case FTU_HOTPLUG_PROPERTY:
    case FTU_RETRY_PROPERTY:
    case FTU_STARTUP_PROPERTY:
    case FTU_FAILURE_PROPERTY:
    case FTU_PUMPS_PROPERTY:
    case FTU_DELIVERY_PROPERTY:
    case FTU_WORKER_PROPERTY:
    case FTU_TIMING_PROPERTY:
    case FTU_STATUS_PROPERTY: return put_str(b, s, CFSTR("")); /* built in get_data */
    case kAudioObjectPropertyCustomPropertyInfoList:
        b->custom[0]=(AudioServerPlugInCustomPropertyInfo){FTU_STATUS_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[1]=(AudioServerPlugInCustomPropertyInfo){FTU_BUILD_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[2]=(AudioServerPlugInCustomPropertyInfo){FTU_TIMING_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[3]=(AudioServerPlugInCustomPropertyInfo){FTU_WORKER_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[4]=(AudioServerPlugInCustomPropertyInfo){FTU_DELIVERY_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[5]=(AudioServerPlugInCustomPropertyInfo){FTU_PUMPS_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[6]=(AudioServerPlugInCustomPropertyInfo){FTU_FAILURE_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[7]=(AudioServerPlugInCustomPropertyInfo){FTU_STARTUP_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[8]=(AudioServerPlugInCustomPropertyInfo){FTU_RETRY_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[9]=(AudioServerPlugInCustomPropertyInfo){FTU_HOTPLUG_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[10]=(AudioServerPlugInCustomPropertyInfo){FTU_RUNTIME_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[11]=(AudioServerPlugInCustomPropertyInfo){FTU_RECOVERY_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[12]=(AudioServerPlugInCustomPropertyInfo){FTU_SHUTDOWN_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        s->size=sizeof b->custom;s->elem=sizeof b->custom[0];return 0;
    case kAudioObjectPropertyBaseClass: return put_u32(b, s, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return put_u32(b, s, kAudioDeviceClassID);
    case kAudioObjectPropertyOwner: return put_u32(b, s, kObjPlugIn);
    case kAudioObjectPropertyName: return put_str(b, s, g_name);
    case kAudioObjectPropertyManufacturer: return put_str(b, s, CFSTR("M-Audio"));
    case kAudioDevicePropertyDeviceUID: return put_str(b, s, g_uid);
    case kAudioDevicePropertyModelUID: return put_str(b, s, g_model);
    case kAudioDevicePropertyTransportType: return put_u32(b, s, kAudioDeviceTransportTypeUSB);
    case kAudioDevicePropertyRelatedDevices: return put_ids(b, s, &self, 1);
    case kAudioDevicePropertyClockDomain: return put_u32(b, s, 0);
    case kAudioDevicePropertyDeviceIsAlive: return put_u32(b, s, atomic_load(&g_alive));
    case kAudioDevicePropertyDeviceIsRunning: {
        uint32_t phase = session_phase(live_handle());
        return put_u32(b, s, phase == 2);
    }
    case kAudioDevicePropertyDeviceCanBeDefaultDevice:
    case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice: return put_u32(b, s, 1);
    case kAudioDevicePropertyIsHidden: return put_u32(b, s, 0);
    case kAudioObjectPropertyOwnedObjects:
        return put_owned(b, s, out ? streams + 1 : streams, io ? 1 : 2, kAudioStreamClassID, qsize, qdata);
    case kAudioDevicePropertyStreams: return put_ids(b, s, out ? streams + 1 : streams, io ? 1 : 2);
    case kAudioObjectPropertyControlList: return put_ids(b, s, streams, 0);
    case kAudioDevicePropertyNominalSampleRate:
        b->f64 = g_cfg.rate;
        s->size = sizeof(Float64);
        return kAudioHardwareNoError;
    case kAudioDevicePropertyAvailableNominalSampleRates:
        s->elem = sizeof(AudioValueRange);
        return put_range(b, s, g_cfg.rate, g_cfg.rate);
    case kAudioDevicePropertyUsesVariableBufferFrameSizes: return put_u32(b, s, 0);
    case kAudioDevicePropertyZeroTimeStampPeriod: return put_u32(b, s, g_cfg.period);
    case kAudioDevicePropertyBufferFrameSize: return put_u32(b, s, atomic_load(&g_buffer_frames));
    case kAudioDevicePropertyBufferFrameSizeRange: return put_range(b, s, g_cfg.min_buffer, g_cfg.max_buffer);
    }
    if (!io)
        return kAudioHardwareUnknownPropertyError;
    switch (a->mSelector) {
    case kAudioDevicePropertyLatency: return put_u32(b, s, 0); /* no calibrated converter residual */
    case kAudioDevicePropertySafetyOffset: return put_u32(b, s, in ? g_cfg.input_safety : g_cfg.output_safety);
    case kAudioDevicePropertyPreferredChannelsForStereo:
        b->u32[0] = 1;
        b->u32[1] = 2;
        s->size = 2 * sizeof(UInt32);
        return kAudioHardwareNoError;
    case kAudioDevicePropertyPreferredChannelLayout: {
        AudioChannelDescription *d = b->layout.mChannelDescriptions;
        b->layout.mChannelLayoutTag = kAudioChannelLayoutTag_UseChannelDescriptions;
        b->layout.mNumberChannelDescriptions = g_cfg.channels;
        for (UInt32 i = 0; i < g_cfg.channels; i++)
            d[i].mChannelLabel = kAudioChannelLabel_Discrete_0 + i;
        s->size = (UInt32)(offsetof(AudioChannelLayout, mChannelDescriptions) +
                           g_cfg.channels * sizeof(AudioChannelDescription));
        return kAudioHardwareNoError;
    }
    case kAudioDevicePropertyStreamConfiguration:
        b->abl.mNumberBuffers = 1;
        b->abl.mBuffers[0].mNumberChannels = g_cfg.channels;
        s->size = sizeof(AudioBufferList);
        return kAudioHardwareNoError;
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus stream_prop(AudioObjectID obj, const AudioObjectPropertyAddress *a, prop_buf *b, prop_shape *s)
{
    bool in = obj == kObjInput;
    switch (a->mSelector) {
    case kAudioObjectPropertyBaseClass: return put_u32(b, s, kAudioObjectClassID);
    case kAudioObjectPropertyClass: return put_u32(b, s, kAudioStreamClassID);
    case kAudioObjectPropertyOwner: return put_u32(b, s, kObjDevice);
    case kAudioObjectPropertyName: return put_str(b, s, in ? CFSTR("Inputs 1-8") : CFSTR("Outputs 1-8"));
    case kAudioObjectPropertyOwnedObjects: return put_ids(b, s, NULL, 0);
    case kAudioStreamPropertyIsActive: return put_u32(b, s, 1);
    case kAudioStreamPropertyDirection: return put_u32(b, s, in ? 1 : 0);
    case kAudioStreamPropertyTerminalType: return put_u32(b, s, kAudioStreamTerminalTypeLine);
    case kAudioStreamPropertyStartingChannel: return put_u32(b, s, 1);
    case kAudioStreamPropertyLatency: return put_u32(b, s, 0);
    case kAudioStreamPropertyVirtualFormat:
    case kAudioStreamPropertyPhysicalFormat:
        fill_format(&b->asbd);
        s->size = sizeof(AudioStreamBasicDescription);
        return kAudioHardwareNoError;
    case kAudioStreamPropertyAvailableVirtualFormats:
    case kAudioStreamPropertyAvailablePhysicalFormats:
        fill_format(&b->ranged.mFormat);
        b->ranged.mSampleRateRange.mMinimum = g_cfg.rate;
        b->ranged.mSampleRateRange.mMaximum = g_cfg.rate;
        s->size = s->elem = sizeof(AudioStreamRangedDescription);
        return kAudioHardwareNoError;
    default: return kAudioHardwareUnknownPropertyError;
    }
}

static OSStatus object_prop(AudioObjectID obj, const AudioObjectPropertyAddress *a, UInt32 qsize,
                            const void *qdata, bool fetch, prop_buf *b, prop_shape *s)
{
    memset(b, 0, sizeof *b);
    memset(s, 0, sizeof *s);
    if (!a)
        return kAudioHardwareIllegalOperationError;
    if (obj != kObjPlugIn && (obj < kObjDevice || obj > kObjOutput || !live_handle()))
        return kAudioHardwareBadObjectError;
    if ((a->mScope != kAudioObjectPropertyScopeGlobal && a->mScope != kAudioObjectPropertyScopeInput &&
         a->mScope != kAudioObjectPropertyScopeOutput) || a->mElement != kAudioObjectPropertyElementMain)
        return kAudioHardwareUnknownPropertyError;
    if (obj == kObjPlugIn)
        return plugin_prop(a, qsize, qdata, fetch, b, s);
    if (obj == kObjDevice)
        return device_prop(a, qsize, qdata, b, s);
    return stream_prop(obj, a, b, s);
}

static bool is_settable(AudioObjectID obj, const AudioObjectPropertyAddress *a)
{
    return obj == kObjDevice && a->mSelector == kAudioDevicePropertyBufferFrameSize;
}

/* ------------------------------------------------------------ driver vtable */

static HRESULT ftu_query_interface(void *drv, REFIID iid, LPVOID *out);
static ULONG ftu_add_ref(void *drv);

void *FTUFactory(CFAllocatorRef allocator, CFUUIDRef type) __attribute__((visibility("default")));
void *FTUFactory(CFAllocatorRef allocator, CFUUIDRef type)
{
    (void)allocator;
    if (!type || !CFEqual(type, kAudioServerPlugInTypeUUID))
        return NULL;
    return (void *)g_ref;
}

static HRESULT ftu_query_interface(void *drv, REFIID iid, LPVOID *out)
{
    if (drv != g_ref || !out)
        return E_POINTER;
    *out = NULL;
    CFUUIDRef uuid = CFUUIDCreateFromUUIDBytes(NULL, iid);
    if (!uuid)
        return E_NOINTERFACE;
    bool ok = CFEqual(uuid, IUnknownUUID) || CFEqual(uuid, kAudioServerPlugInDriverInterfaceUUID);
    CFRelease(uuid);
    if (!ok)
        return E_NOINTERFACE;
    ftu_add_ref(drv);
    *out = (LPVOID)g_ref;
    return S_OK;
}

static ULONG ftu_add_ref(void *drv)
{
    if (drv != g_ref)
        return 0;
    return atomic_fetch_add(&g_refs, 1) + 1;
}

/* The driver is a static singleton; the count is informational and never frees. */
static ULONG ftu_release(void *drv)
{
    if (drv != g_ref)
        return 0;
    ULONG cur = atomic_load(&g_refs);
    while (cur > 0 && !atomic_compare_exchange_weak(&g_refs, &cur, cur - 1)) {}
    return cur > 0 ? cur - 1 : 0;
}

static bool config_valid(const struct ftu_live_config *c)
{
    return c->channels == FTU_MAX_CHANNELS && c->rate == 48000 && c->period >= 10923 && c->min_buffer > 0 &&
           c->min_buffer <= c->default_buffer && c->default_buffer <= c->max_buffer &&
           c->ring_frames >= c->max_buffer;
}

static OSStatus ftu_initialize(AudioServerPlugInDriverRef drv, AudioServerPlugInHostRef host)
{
    if (drv != g_ref)
        return kAudioHardwareBadObjectError;
    if (!host)
        return kAudioHardwareIllegalOperationError;
    bool expected = false;
    if (!atomic_compare_exchange_strong(&g_initialized, &expected, true))
        return kAudioHardwareIllegalOperationError;
    ftu_live_configuration(&g_cfg);
    if (!config_valid(&g_cfg))
        return kAudioHardwareUnspecifiedError;
    g_host = host;
    atomic_store(&g_buffer_frames, g_cfg.default_buffer);
    poll_presence(false);

    pthread_attr_t attr;
    pthread_t thread;
    if (pthread_attr_init(&attr) != 0)
        return kAudioHardwareUnspecifiedError;
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    int rc = pthread_create(&thread, &attr, monitor_main, NULL);
    pthread_attr_destroy(&attr);
    return rc == 0 ? kAudioHardwareNoError : kAudioHardwareUnspecifiedError;
}

static OSStatus ftu_create_device(AudioServerPlugInDriverRef drv, CFDictionaryRef desc,
                                  const AudioServerPlugInClientInfo *client, AudioObjectID *out)
{
    (void)desc; (void)client; (void)out;
    return drv == g_ref ? kAudioHardwareUnsupportedOperationError : kAudioHardwareBadObjectError;
}

static OSStatus ftu_destroy_device(AudioServerPlugInDriverRef drv, AudioObjectID dev)
{
    (void)dev;
    return drv == g_ref ? kAudioHardwareUnsupportedOperationError : kAudioHardwareBadObjectError;
}

static struct ftu_live *device_live(AudioServerPlugInDriverRef drv, AudioObjectID dev)
{
    return drv == g_ref && dev == kObjDevice ? live_handle() : NULL;
}

static OSStatus ftu_add_client(AudioServerPlugInDriverRef drv, AudioObjectID dev,
                               const AudioServerPlugInClientInfo *client)
{
    if (!device_live(drv, dev))
        return kAudioHardwareBadDeviceError;
    return client ? kAudioHardwareNoError : kAudioHardwareIllegalOperationError;
}

/* Rust tracks IO clients; stopping an unknown client is expected to return 0.
 * For a client retired by a unit removal, this only acknowledges the retirement
 * (under the session lock) and never touches the current session. */
static OSStatus ftu_remove_client(AudioServerPlugInDriverRef drv, AudioObjectID dev,
                                  const AudioServerPlugInClientInfo *client)
{
    struct ftu_live *live = device_live(drv, dev);
    if (!live)
        return kAudioHardwareBadDeviceError;
    if (!client)
        return kAudioHardwareIllegalOperationError;
    return ftu_live_stop(live, client->mClientID) == 0 ? kAudioHardwareNoError : kAudioHardwareUnspecifiedError;
}

/* Change action is the requested buffer frame size. */
static OSStatus ftu_perform_change(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt64 action, void *info)
{
    (void)info;
    if (!device_live(drv, dev))
        return kAudioHardwareBadDeviceError;
    if (action < g_cfg.min_buffer || action > g_cfg.max_buffer)
        return kAudioHardwareIllegalOperationError;
    atomic_store(&g_buffer_frames, (UInt32)action);
    static const AudioObjectPropertyAddress a = {
        kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    notify(kObjDevice, &a, 1);
    return kAudioHardwareNoError;
}

static OSStatus ftu_abort_change(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt64 action, void *info)
{
    (void)action; (void)info;
    return device_live(drv, dev) ? kAudioHardwareNoError : kAudioHardwareBadDeviceError;
}

static Boolean ftu_has_property(AudioServerPlugInDriverRef drv, AudioObjectID obj, pid_t pid,
                                const AudioObjectPropertyAddress *a)
{
    (void)pid;
    prop_buf b;
    prop_shape s;
    return drv == g_ref && object_prop(obj, a, 0, NULL, false, &b, &s) == kAudioHardwareNoError;
}

static OSStatus ftu_is_settable(AudioServerPlugInDriverRef drv, AudioObjectID obj, pid_t pid,
                                const AudioObjectPropertyAddress *a, Boolean *out)
{
    (void)pid;
    if (drv != g_ref)
        return kAudioHardwareBadObjectError;
    if (!out)
        return kAudioHardwareIllegalOperationError;
    prop_buf b;
    prop_shape s;
    OSStatus st = object_prop(obj, a, 0, NULL, false, &b, &s);
    if (st == kAudioHardwareNoError)
        *out = is_settable(obj, a);
    return st;
}

static OSStatus ftu_get_size(AudioServerPlugInDriverRef drv, AudioObjectID obj, pid_t pid,
                             const AudioObjectPropertyAddress *a, UInt32 qsize, const void *qdata, UInt32 *out)
{
    (void)pid;
    if (drv != g_ref)
        return kAudioHardwareBadObjectError;
    if (!out)
        return kAudioHardwareIllegalOperationError;
    prop_buf b;
    prop_shape s;
    OSStatus st = object_prop(obj, a, qsize, qdata, false, &b, &s);
    if (st == kAudioHardwareNoError)
        *out = s.size;
    return st;
}

/* Property/control path only: JSON allocation and formatting never run in IO. */
static CFStringRef timing_text(struct ftu_live *live)
{
    static const char *names[FTU_TIMING_METRICS]={
        "write_frames","write_capture_lead","write_preparation_lead",
        "output_capture_lead","output_high_water_margin","output_missing",
        "read_frames","read_capture_margin","read_missing",
        "submit_check_lead_ms","submit_return_lead_ms","submit_clock_error"};
    struct ftu_timing_snapshot t={0};ftu_live_timing(live,&t);
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppendFormat(text,NULL,CFSTR("{\"version\":%u,\"epoch\":%llu,\"valid\":%llu,\"steady_after\":%llu,\"limits\":["),
        t.version,(unsigned long long)t.epoch,(unsigned long long)t.valid,(unsigned long long)t.steady_after);
    for(unsigned i=0;i<FTU_TIMING_BUCKETS-1;i++)
        CFStringAppendFormat(text,NULL,CFSTR("%s%lld"),i?",":"",(long long)t.limits[i]);
    CFStringAppend(text,CFSTR("]"));
    for(unsigned bank=0;bank<2;bank++){
        CFStringAppendFormat(text,NULL,CFSTR(",\"%s\":{"),bank?"steady":"all");
        for(unsigned i=0;i<FTU_TIMING_METRICS;i++){
            const struct ftu_timing_metric *m=&t.metrics[bank][i];
            CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":{\"valid\":%llu,\"count\":%llu,\"dropped\":%llu,"),
                i?",":"",names[i],(unsigned long long)m->valid,(unsigned long long)m->count,(unsigned long long)m->dropped);
            if(m->valid && m->count)
                CFStringAppendFormat(text,NULL,CFSTR("\"min\":%lld,\"max\":%lld,\"last\":%lld,"),(long long)m->min,(long long)m->max,(long long)m->last);
            else CFStringAppend(text,CFSTR("\"min\":null,\"max\":null,\"last\":null,"));
            CFStringAppend(text,CFSTR("\"buckets\":["));
            for(unsigned j=0;j<FTU_TIMING_BUCKETS;j++)
                CFStringAppendFormat(text,NULL,CFSTR("%s%llu"),j?",":"",(unsigned long long)m->buckets[j]);
            CFStringAppend(text,CFSTR("]}"));
        }
        CFStringAppend(text,CFSTR("}"));
    }
    CFStringAppend(text,CFSTR("}"));return text;
}

/* Fixed layout read from Rust; all formatting stays on the property thread. */
static void worker_event_fields(CFMutableStringRef text,const uint64_t *w)
{
    static const char *names[FTU_WORKER_WORDS]={"ordinal","direction","sequence","slot","ticket","captured","flags","start_frame","checked_frame","returned_frame","previous_sequence","previous_start_frame","previous_last_packet_ticks","previous_callback_ticks","previous_consumed_ticks","feedback_sequence","feedback_last_packet_ticks","feedback_callback_ticks","feedback_consumed_ticks","loop_enter_ticks","poll_end_ticks","pump_enter_ticks","pump_exit_ticks","check_begin_ticks","check_end_ticks","prepare_end_ticks","submit_begin_ticks","submit_end_ticks","query_end_ticks","check_status","submit_status","return_clock_status"};
    static const unsigned required[FTU_WORKER_WORDS]={0,0,0,0,0,0,0,0,4,32,1,1,1,1,1,2,2,2,2,0,0,64,64,0,0,8,16,16,128,0,16,128};
            for(unsigned k=0;k<FTU_WORKER_WORDS;k++){
                CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),k?",":"",names[k]);
                if(required[k] && !(w[6]&required[k]))CFStringAppend(text,CFSTR("null"));
                else if(k>=29)CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)w[k]);
                else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)w[k]);
            }
}
static CFStringRef worker_snapshot_text(const struct ftu_worker_snapshot *t)
{
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppendFormat(text,NULL,CFSTR("{\"version\":%llu,\"valid\":%llu,\"epoch\":%llu,\"timebase_numer\":%llu,\"timebase_denom\":%llu,\"queue_depth\":%llu,\"threshold_ms\":%llu,\"observed\":%llu,\"dropped\":%llu,\"recent_overwritten\":%llu,\"late_seen\":%llu,\"late_retained\":%llu,\"late_overflow\":%llu"),
        (unsigned long long)t->version,(unsigned long long)t->valid,(unsigned long long)t->epoch,
        (unsigned long long)t->timebase_numer,(unsigned long long)t->timebase_denom,(unsigned long long)t->queue_depth,
        (unsigned long long)t->threshold_ms,(unsigned long long)t->observed,(unsigned long long)t->dropped,
        (unsigned long long)t->recent_overwritten,(unsigned long long)t->late_seen,(unsigned long long)t->late_retained,(unsigned long long)t->late_overflow);
    CFStringAppendFormat(text,NULL,CFSTR(",\"block_ms\":%llu,\"entries\":%llu,\"feedback_slots\":%llu,\"replay_ms\":%llu"),
        (unsigned long long)t->block_ms,(unsigned long long)t->entries,(unsigned long long)t->feedback_slots,(unsigned long long)t->replay_ms);
    const struct ftu_worker_event *banks[]={t->worst,t->recent,t->late};
    const unsigned counts[]={2,FTU_WORKER_RECENT,FTU_WORKER_LATE};
    const char *labels[]={"worst","recent","late"};
    for(unsigned b=0;b<3;b++){
        CFStringAppendFormat(text,NULL,CFSTR(",\"%s\":["),labels[b]);unsigned used=0;
        for(unsigned i=0;i<counts[b];i++){
            const uint64_t *w=banks[b][i].words;if(!w[0])continue;
            CFStringAppendFormat(text,NULL,CFSTR("%s{"),used++?",":"");
            worker_event_fields(text,w);
            CFStringAppend(text,CFSTR("}"));
        }
        CFStringAppend(text,CFSTR("]"));
    }
    CFStringAppend(text,CFSTR("}"));return text;
}

static void json_u64_array(CFMutableStringRef text,const uint64_t *words,unsigned count)
{
    CFStringAppend(text,CFSTR("["));
    for(unsigned i=0;i<count;i++)CFStringAppendFormat(text,NULL,CFSTR("%s%llu"),i?",":"",(unsigned long long)words[i]);
    CFStringAppend(text,CFSTR("]"));
}
static void delivery_record(CFMutableStringRef text,const uint64_t *w)
{
    static const char *names[FTU_DELIVERY_RECORD]={"ordinal","direction","sequence","slot","ticket","captured","last_packet_ticks","callback_ticks","consumed_ticks","pump_id","pump_enter_ticks","pump_exit_ticks","pump_result","status","matched","draining","sample_begin_ticks","sample_end_ticks"};
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_DELIVERY_RECORD;i++){
        CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),i?",":"",names[i]);
        if((i>=9 && i<=12 && !w[9]) || ((i==6 || i==7) && !w[i]))CFStringAppend(text,CFSTR("null"));
        else if(i==13)CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)w[i]);
        else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)w[i]);
    }
    CFStringAppend(text,CFSTR("}"));
}
static void delivery_pump(CFMutableStringRef text,const uint64_t *p)
{
    if(!p[0])CFStringAppend(text,CFSTR("null"));
    else CFStringAppendFormat(text,NULL,CFSTR("{\"id\":%llu,\"enter_ticks\":%llu,\"exit_ticks\":%llu,\"result\":%llu,\"sample_begin_ticks\":%llu,\"sample_end_ticks\":%llu}"),(unsigned long long)p[0],(unsigned long long)p[1],(unsigned long long)p[2],(unsigned long long)p[3],(unsigned long long)p[4],(unsigned long long)p[5]);
}
static CFStringRef delivery_snapshot_text(const struct ftu_delivery_snapshot *s)
{
    static const char *header[FTU_DELIVERY_HEADER]={"version","valid","epoch","timebase_numer","timebase_denom","steady_after","completed_in","completed_out","steady_completed_in","steady_completed_out","admissions_in","admissions_out","steady_admissions_in","steady_admissions_out","dropped","unmatched_pump_in","unmatched_pump_out","sample_count","cpu_errors","policy_errors","policy_defaults","sampler_invalid","sampler_max_ns","overflow","drained_in","drained_out","basic_policy_changes","constraint_policy_changes","queue_depth","block_ms","entries"};
    static const char *metrics[FTU_DELIVERY_METRICS]={"packet_to_callback","callback_to_consumed","callback_pump_duration","preparation","submission","post_submit_observation","callback_to_resubmission","consumed_to_admission"};
    static const uint64_t limits[]={1000,2000,4000,8000,16000,32000,64000,128000,256000,512000,1000000,2000000,4000000,8000000,16000000,32000000,64000000};
    static const uint64_t thresholds[]={1000000,4000000,8000000,16000000};
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_DELIVERY_HEADER;i++)CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":%llu"),i?",":"",header[i],(unsigned long long)s->header[i]);
    CFStringAppend(text,CFSTR(",\"units\":\"ns\",\"bucket_upper_ns\":"));json_u64_array(text,limits,17);
    CFStringAppend(text,CFSTR(",\"exceed_at_least_ns\":"));json_u64_array(text,thresholds,4);
    CFStringAppend(text,CFSTR(",\"metrics\":["));
    for(unsigned d=0;d<2;d++){
        CFStringAppend(text,d?CFSTR(",{"):CFSTR("{"));
        for(unsigned i=0;i<FTU_DELIVERY_METRICS;i++){
            const uint64_t *h=s->metrics[d][i];
            CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":{\"count\":%llu,\"unavailable\":%llu,\"invalid\":%llu,\"overflow\":%llu,\"min_ns\":"),i?",":"",metrics[i],(unsigned long long)h[0],(unsigned long long)h[3],(unsigned long long)h[4],(unsigned long long)h[5]);
            if(h[0])CFStringAppendFormat(text,NULL,CFSTR("%llu,\"max_ns\":%llu"),(unsigned long long)h[1],(unsigned long long)h[2]);
            else CFStringAppend(text,CFSTR("null,\"max_ns\":null"));
            CFStringAppend(text,CFSTR(",\"buckets\":"));json_u64_array(text,h+6,FTU_DELIVERY_BUCKETS);
            CFStringAppend(text,CFSTR(",\"exceed\":"));json_u64_array(text,h+6+FTU_DELIVERY_BUCKETS,4);
            CFStringAppendFormat(text,NULL,CFSTR(",\"sampler_overlaps\":%llu}"),(unsigned long long)h[28]);
        }
        CFStringAppend(text,CFSTR("}"));
    }
    CFStringAppend(text,CFSTR("],\"worst_delivery\":["));
    unsigned used=0;
    for(unsigned d=0;d<2;d++)if(s->worst[d][0]){if(used++)CFStringAppend(text,CFSTR(","));delivery_record(text,s->worst[d]);}
    CFStringAppend(text,CFSTR("],\"recent\":["));
    used=0;
    for(unsigned i=0;i<FTU_DELIVERY_RECENT;i++)if(s->recent[i][0]){if(used++)CFStringAppend(text,CFSTR(","));delivery_record(text,s->recent[i]);}
    CFStringAppend(text,CFSTR("],\"worst_lead\":["));
    used=0;
    for(unsigned d=0;d<2;d++)if(s->lead[d][0]){
        if(used++)CFStringAppend(text,CFSTR(","));
        CFStringAppend(text,CFSTR("{\"event\":{"));worker_event_fields(text,s->lead[d]);
        const uint64_t *p=s->lead[d]+FTU_WORKER_WORDS;
        CFStringAppend(text,CFSTR("},\"callback_pump\":"));
        delivery_pump(text,p);CFStringAppend(text,CFSTR(",\"feedback_pump\":"));delivery_pump(text,p+6);
        CFStringAppend(text,CFSTR("}"));
    }
    CFStringAppend(text,CFSTR("],\"samples\":["));
    static const char *sample_names[FTU_DELIVERY_SAMPLE]={"begin_ticks","end_ticks","cpu_user_ns","cpu_system_ns","cpu_status","policy_status","policy_default","period_ticks","computation_ticks","constraint_ticks","preemptible","basic_policy"};
    unsigned count=s->header[17]>1?2:(unsigned)s->header[17];
    for(unsigned i=0;i<count;i++){
        const uint64_t *w=s->samples[i];CFStringAppend(text,i?CFSTR(",{"):CFSTR("{"));
        for(unsigned j=0;j<FTU_DELIVERY_SAMPLE;j++){
            CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),j?",":"",sample_names[j]);
            if((j==2 || j==3 || j==11) && w[4])CFStringAppend(text,CFSTR("null"));
            else if(j>=6 && j<=10 && w[5])CFStringAppend(text,CFSTR("null"));
            else if(j==4 || j==5)CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)w[j]);
            else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)w[j]);
        }
        CFStringAppend(text,CFSTR("}"));
    }
    CFStringAppend(text,CFSTR("]}"));return text;
}


static CFStringRef retry_snapshot_text(const struct ftu_retry_snapshot *s)
{
    static const char *header[FTU_RETRY_HEADER]={"version","valid","epoch","policy","wait_cutoff_bus_frames","lead_gate_bus_frames","retry_budget_ns","work_budget_ns","steady_after","timebase_numer","timebase_denom","waiting"};
    static const char *counts[FTU_RETRY_COUNTS]={"checks","attempts","ready","deferrals","fallbacks","retries","over_budget","invalid_time","max_retry_ns","last_retry_ns"};
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_RETRY_HEADER;i++)CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":%llu"),i?",":"",header[i],(unsigned long long)s->header[i]);
    for(unsigned bank=0;bank<2;bank++){
        CFStringAppendFormat(text,NULL,CFSTR(",\"%s\":{"),bank?"steady":"all");
        for(unsigned i=0;i<FTU_RETRY_COUNTS;i++)CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":%llu"),i?",":"",counts[i],(unsigned long long)s->counts[bank][i]);
        CFStringAppend(text,CFSTR("}"));
    }
    CFStringAppend(text,CFSTR("}"));return text;
}

static CFStringRef startup_snapshot_text(const struct ftu_startup_snapshot *s)
{
    static const char *keys[12]={"version","valid","epoch","stage","outcome","config_status","prepare_status","activate_status","before_available","verify_available","prepare_state","activate_state"};
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<12;i++){
        CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),i?",":"",keys[i]);
        bool available=i==5?s->header[8]:i==6?s->header[10]==2:i==7?s->header[11]==2:true;
        if(!available)CFStringAppend(text,CFSTR("null"));
        else if(i>=5 && i<=7)CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)s->header[i]);
        else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)s->header[i]);
    }
    CFStringAppend(text,CFSTR(",\"writes\":["));
    for(unsigned d=0;d<2;d++){
        const uint64_t *w=s->writes[d];
        CFStringAppendFormat(text,NULL,CFSTR("%s{\"endpoint\":%u,\"attempted\":%llu,\"returned\":%llu,\"status\":"),d?",":"",d?1:129,(unsigned long long)w[0],(unsigned long long)w[1]);
        if(w[1])CFStringAppendFormat(text,NULL,CFSTR("%lld,\"actual\":%llu"),(long long)(int64_t)w[2],(unsigned long long)w[3]);
        else CFStringAppend(text,CFSTR("null,\"actual\":null"));
        CFStringAppend(text,CFSTR("}"));
    }
    CFStringAppend(text,CFSTR("],\"rates\":["));
    for(unsigned i=0;i<2;i++){
        if(i)CFStringAppend(text,CFSTR(","));
        if(s->header[8+i])json_u64_array(text,s->rates[i],12);
        else CFStringAppend(text,CFSTR("null"));
    }
    CFStringAppend(text,CFSTR("]}"));return text;
}
static CFStringRef failure_snapshot_text(const struct ftu_failure_snapshot *s)
{
    static const char *keys[FTU_FAILURE_HEADER]={"version","valid","present","epoch","reason","direction","sequence","ticket","slot","start_frame","checked_frame","returned_frame","admission_ticks","submit_status","completion_status","matched","callback_ticks","activated","drain_failed","cleanup_status","post_available","close_available","close_status","record_valid","submitted_in","submitted_out","completed_in","completed_out","pending_in","pending_out","finished","entries","queue_depth","block_ms","feedback_slots"};
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_FAILURE_HEADER;i++){
        CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),i?",":"",keys[i]);
        if(i==13 || i==14 || i==19 || i==22)
            CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)s->header[i]);
        else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)s->header[i]);
    }
    CFStringAppend(text,CFSTR(",\"rates\":["));
    for(unsigned i=0;i<2;i++){if(i)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->rates[i],FTU_FAILURE_RATE_WORDS);}
    CFStringAppend(text,CFSTR("],\"info\":["));
    for(unsigned i=0;i<2;i++){if(i)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->info[i],FTU_FAILURE_INFO_WORDS);}
    CFStringAppend(text,CFSTR("],\"requested\":"));json_u64_array(text,s->requested,64);
    CFStringAppend(text,CFSTR(",\"packets\":["));
    for(unsigned i=0;i<64;i++){if(i)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->packets[i],4);}
    CFStringAppend(text,CFSTR("],\"pump\":"));json_u64_array(text,s->pump,6);
    CFStringAppend(text,CFSTR("}"));return text;
}

static CFStringRef pumps_snapshot_text(const struct ftu_pumps_snapshot *s)
{
    static const char *header[FTU_PUMPS_HEADER]={"version","valid","epoch","timebase_numer","timebase_denom","steady_after","calls","unavailable","invalid","dropped","overflow","id_discontinuities","link_misses"};
    static const uint64_t limits[]={1000,2000,4000,8000,16000,32000,64000,128000,256000,512000,1000000,2000000,4000000,8000000,16000000,32000000,64000000};
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_PUMPS_HEADER;i++)CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":%llu"),i?",":"",header[i],(unsigned long long)s->header[i]);
    CFStringAppend(text,CFSTR(",\"bucket_upper_ns\":"));json_u64_array(text,limits,17);
    CFStringAppend(text,CFSTR(",\"counts\":["));
    for(unsigned p=0;p<3;p++){if(p)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->counts[p],FTU_PUMPS_COUNTS);}
    CFStringAppend(text,CFSTR("],\"metrics\":["));
    for(unsigned p=0;p<3;p++){
        CFStringAppend(text,p?CFSTR(",["):CFSTR("["));
        for(unsigned m=0;m<FTU_PUMPS_METRICS;m++){if(m)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->metrics[p][m],FTU_PUMPS_HIST);}
        CFStringAppend(text,CFSTR("]"));
    }
    CFStringAppend(text,CFSTR("],\"worst\":["));
    for(unsigned p=0;p<3;p++){
        CFStringAppend(text,p?CFSTR(",["):CFSTR("["));
        for(unsigned m=0;m<FTU_PUMPS_METRICS;m++){if(m)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->worst[p][m],FTU_PUMPS_PAIR);}
        CFStringAppend(text,CFSTR("]"));
    }
    CFStringAppend(text,CFSTR("],\"recent\":["));
    for(unsigned i=0;i<FTU_PUMPS_RECENT;i++){if(i)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->recent[i],FTU_PUMPS_RECORD);}
    CFStringAppend(text,CFSTR("],\"links\":["));
    for(unsigned i=0;i<4;i++){if(i)CFStringAppend(text,CFSTR(","));json_u64_array(text,s->links[i],FTU_PUMPS_LINK);}
    CFStringAppend(text,CFSTR("]}"));return text;
}

/* ftuH v1. Binding fields form one seqlock snapshot ("coherent":0 after bounded
 * retries); counters and statuses are individually atomic. Property thread only. */
static CFStringRef hotplug_text(void)
{
    uint32_t bound = 0, location = 0, before = 0;
    uint64_t registry = 0;
    bool coherent = false;
    for (unsigned i = 0; i < 8 && !coherent; i++) {
        before = atomic_load(&g_hp_seq);
        bound = atomic_load(&g_hp_bound);
        location = atomic_load(&g_hp_location);
        registry = atomic_load(&g_hp_registry);
        coherent = !(before & 1) && atomic_load(&g_hp_seq) == before;
    }
    return CFStringCreateWithFormat(NULL, NULL,
        CFSTR("{\"version\":1,\"coherent\":%u,\"bound\":%u,\"product\":%u,\"location\":%u,\"registry_id\":%llu,\"rebinds\":%llu,\"removals\":%llu,\"reject\":%u,\"detach_status\":%d,\"rebind_status\":%d}"),
        (unsigned)coherent, bound, g_product, location, (unsigned long long)registry,
        (unsigned long long)atomic_load(&g_hp_rebinds), (unsigned long long)atomic_load(&g_hp_removals),
        atomic_load(&g_hp_reject), atomic_load(&g_hp_detach_status), atomic_load(&g_hp_rebind_status));
}

/* ftuE v1. Flag-guarded and absent fields are null, never zero. Property thread only. */
static CFStringRef runtime_snapshot_text(const struct ftu_runtime_snapshot *s)
{
    static const char *names[FTU_RUNTIME_WORDS]={"version","valid","present","epoch","reason","stage","flags","direction","sequence","slot","scheduled_frame","checked_frame","checked_ticks","returned_frame","returned_ticks","required_lead_frames","base_frame","prior_step_frame","prior_step_ticks","prior_progress_frame","prior_progress_ticks","steps","next_in","next_out","completed_in","completed_out","pending_in","pending_out","captured","prepared","timebase_numer","timebase_denom","min_out_return_lead","submit_status","uptime_ms","finished","drain_failed","cleanup_status","close_status","terminal_phase","terminal_fault","overwritten","records","queue_depth","block_ms","replay_ms"};
    static const uint64_t required[FTU_RUNTIME_WORDS]={0,0,0,0,0,0,0,1,1,2,4,8,8,16,16,1,2048,32,32,64,64,0,0,0,0,0,0,0,0,0,1024,1024,128,256,0,0,0,0,512,0,0,0,0,0,0,0};
    static const char *stages[]={"none","generic","completion_watchdog","out_before_readiness","before_packing","after_packing","after_return","submit_failed"};
    const uint64_t *w=s->words;
    bool present=w[1] && w[2],finished=present && w[35];
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_RUNTIME_WORDS;i++){
        CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),i?",":"",names[i]);
        bool record=i>2 && i!=41 && i!=42,terminal=i>=36 && i<=40;
        if((record && !present) || (terminal && !finished) || (required[i] && !(w[6]&required[i])))CFStringAppend(text,CFSTR("null"));
        else if(i==33 || i==37 || i==38 || i==40)CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)w[i]);
        else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)w[i]);
    }
    uint64_t stage=present?w[5]:0;
    CFStringAppendFormat(text,NULL,CFSTR(",\"stage_name\":\"%s\"}"),stage<8?stages[stage]:"unknown");
    return text;
}

/* ftuA v1 plus the monitor's own last call. Property thread only. */
static CFStringRef recovery_snapshot_text(const struct ftu_recovery_snapshot *s)
{
    static const char *names[FTU_RECOVERY_WORDS]={"version","valid","enabled","state","refusal","consecutive","budget","attempts","recovered","failed","refused","cancelled","exhausted","healthy_resets","fault_epoch","recovered_epoch","last_status","pending_since_ms","due_ms","running_since_ms","healthy_ms","backoff0_ms","backoff1_ms","backoff2_ms","last_reason","binding","uptime_ms","clients","retired","time_flags"};
    static const char *states[]={"idle","pending","recovered","refused","exhausted","cancelled","healthy"};
    static const char *refusals[]={"none","quarantine","reason","cleanup","close","fault_code","no_record","unfinished","attempt_failed","profile","stage"};
    const uint64_t *w=s->words;
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_RECOVERY_WORDS;i++){
        CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),i?",":"",names[i]);
        if(((i==17 || i==18) && !(w[29]&1)) || (i==19 && !(w[29]&2)))CFStringAppend(text,CFSTR("null"));
        else if(i==16)CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)w[i]);
        else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)w[i]);
    }
    CFStringAppendFormat(text,NULL,CFSTR(",\"state_name\":\"%s\",\"refusal_name\":\"%s\",\"monitor_calls\":%llu,\"monitor_status\":%d}"),
        w[3]<7?states[w[3]]:"unknown",w[4]<11?refusals[w[4]]:"unknown",
        (unsigned long long)atomic_load(&g_recover_calls),atomic_load(&g_recover_status));
    return text;
}

/* ftuQ: native ownership is separate from the unused legacy PoolSession ledger.
 * Failed/unattempted operations stay distinguishable from successful zero. */
static CFStringRef shutdown_snapshot_text(const struct ftu_shutdown_snapshot *s)
{
    static const char *names[FTU_SHUTDOWN_WORDS]={
        "version","valid","present","epoch","stop_finished","native_stop_ok","rust_slots_retired","destroy_attempted","destroy_ok","callback_panic","cleanup_present","records",
        "route_version","callbacks","faults","releases","acknowledgements","rejections","native_pending","native_held","native_returned","stop_requested","joined","quarantined","poisoned","ever_timed_out","abort_in","abort_out",
        "cleanup_version","cleanup_attempted","removed_mask","restored_mask","open_mask","cleanup_fatal","restore_in","read_alt_in","destroy_data_in","destroy_frames_in","close_in","restore_out","read_alt_out","destroy_data_out","destroy_frames_out","close_out"};
    const uint64_t *w=s->words;bool present=w[0]==1 && w[1]==1 && w[2]==1;
    CFMutableStringRef text=CFStringCreateMutable(NULL,0);if(!text)return NULL;
    CFStringAppend(text,CFSTR("{"));
    for(unsigned i=0;i<FTU_SHUTDOWN_WORDS;i++){
        CFStringAppendFormat(text,NULL,CFSTR("%s\"%s\":"),i?",":"",names[i]);
        bool absent=i>=3 && i!=11 && !present;
        if(i>=12 && i<28)absent|=w[12]!=1;
        if(i>=28)absent|=w[10]!=1 || w[28]!=1;
        if(i>=34)absent|=!(w[29]&(1ull<<(i-34)));
        if(absent)CFStringAppend(text,CFSTR("null"));
        else if(i==26 || i==27 || i>=33)CFStringAppendFormat(text,NULL,CFSTR("%lld"),(long long)(int64_t)w[i]);
        else CFStringAppendFormat(text,NULL,CFSTR("%llu"),(unsigned long long)w[i]);
    }
    CFStringAppend(text,CFSTR("}"));return text;
}

static OSStatus ftu_get_data(AudioServerPlugInDriverRef drv, AudioObjectID obj, pid_t pid,
                             const AudioObjectPropertyAddress *a, UInt32 qsize, const void *qdata,
                             UInt32 size, UInt32 *out_size, void *out)
{
    (void)pid;
    if (drv != g_ref)
        return kAudioHardwareBadObjectError;
    if (!out_size || (!out && size > 0))
        return kAudioHardwareIllegalOperationError;
    prop_buf b;
    prop_shape s;
    OSStatus st = object_prop(obj, a, qsize, qdata, true, &b, &s);
    if (st != kAudioHardwareNoError)
        return st;
    UInt32 n = s.size;
    if (s.elem)
        n = size >= s.size ? s.size : size / s.elem * s.elem;
    else if (size < s.size)
        return kAudioHardwareBadPropertySizeError;
    if (obj == kObjDevice && a->mSelector == FTU_HOTPLUG_PROPERTY) {
        CFStringRef text=hotplug_text();if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_RUNTIME_PROPERTY) {
        struct ftu_runtime_snapshot snapshot={0};ftu_live_runtime_fault(live_handle(),&snapshot);
        CFStringRef text=runtime_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_RECOVERY_PROPERTY) {
        struct ftu_recovery_snapshot snapshot={0};ftu_live_recovery(live_handle(),&snapshot);
        CFStringRef text=recovery_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_SHUTDOWN_PROPERTY) {
        struct ftu_shutdown_snapshot snapshot={0};ftu_live_shutdown(live_handle(),&snapshot);
        CFStringRef text=shutdown_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_RETRY_PROPERTY) {
        struct ftu_retry_snapshot snapshot={0};ftu_live_retry(live_handle(),&snapshot);
        CFStringRef text=retry_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_STARTUP_PROPERTY) {
        struct ftu_startup_snapshot snapshot={0};ftu_live_startup(live_handle(),&snapshot);
        CFStringRef text=startup_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_FAILURE_PROPERTY) {
        struct ftu_failure_snapshot snapshot={0};ftu_live_first_failure(live_handle(),&snapshot);
        CFStringRef text=failure_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_PUMPS_PROPERTY) {
        struct ftu_pumps_snapshot snapshot={0};ftu_live_pumps(live_handle(),&snapshot);
        CFStringRef text=pumps_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_DELIVERY_PROPERTY) {
        struct ftu_delivery_snapshot snapshot={0};ftu_live_delivery(live_handle(),&snapshot);
        CFStringRef text=delivery_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_WORKER_PROPERTY) {
        struct ftu_worker_snapshot snapshot={0};ftu_live_worker_timing(live_handle(),&snapshot);
        CFStringRef text=worker_snapshot_text(&snapshot);if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_TIMING_PROPERTY) {
        CFStringRef text=timing_text(live_handle());
        if(!text)return kAudioHardwareUnspecifiedError;
        memcpy(out,&text,sizeof text);*out_size=sizeof text;return 0;
    }
    if (obj == kObjDevice && a->mSelector == FTU_STATUS_PROPERTY) {
        struct ftu_live_status st = {0};
        ftu_live_status(live_handle(), &st);
        CFStringRef text = CFStringCreateWithFormat(NULL, NULL,
            CFSTR("{\"phase\":%u,\"epoch\":%llu,\"captured\":%llu,\"prepared\":%llu,\"playback_missing\":%llu,\"capture_missing\":%llu,\"pending\":[%llu,%llu],\"cleanup\":%d,\"fault\":%d,\"reason\":%u,\"submit_status\":%d,\"submit_direction\":%u,\"submit_sequence\":%llu,\"submit_start\":%llu,\"submit_checked\":%llu,\"submit_returned\":%llu}"),
            st.phase,(unsigned long long)st.epoch,(unsigned long long)st.captured,
            (unsigned long long)st.prepared,(unsigned long long)st.playback_missing,
            (unsigned long long)st.capture_missing,(unsigned long long)st.pending[0],
            (unsigned long long)st.pending[1],st.cleanup_status,st.fault,st.reason,st.submit_status,st.submit_direction,(unsigned long long)st.submit_sequence,(unsigned long long)st.submit_start,(unsigned long long)st.submit_checked,(unsigned long long)st.submit_returned);
        if (!text) return kAudioHardwareUnspecifiedError;
        memcpy(out, &text, sizeof text); *out_size = sizeof text; return 0;
    }
    if (n)
        memcpy(out, &b, n);
    if (s.cf)
        CFRetain(b.str); /* the caller releases returned CF objects */
    *out_size = n;
    return kAudioHardwareNoError;
}

static OSStatus ftu_set_data(AudioServerPlugInDriverRef drv, AudioObjectID obj, pid_t pid,
                             const AudioObjectPropertyAddress *a, UInt32 qsize, const void *qdata,
                             UInt32 size, const void *data)
{
    (void)pid;
    if (drv != g_ref)
        return kAudioHardwareBadObjectError;
    prop_buf b;
    prop_shape s;
    OSStatus st = object_prop(obj, a, qsize, qdata, false, &b, &s);
    if (st != kAudioHardwareNoError)
        return st;
    if (!is_settable(obj, a))
        return kAudioHardwareUnsupportedOperationError;
    if (size != sizeof(UInt32) || !data)
        return kAudioHardwareBadPropertySizeError;
    UInt32 frames;
    memcpy(&frames, data, sizeof frames);
    if (frames < g_cfg.min_buffer || frames > g_cfg.max_buffer)
        return kAudioHardwareIllegalOperationError;
    if (frames == atomic_load(&g_buffer_frames))
        return kAudioHardwareNoError;
    return g_host->RequestDeviceConfigurationChange(g_host, kObjDevice, frames, NULL);
}

static OSStatus ftu_start_io(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt32 client)
{
    struct ftu_live *live = device_live(drv, dev);
    if (!live)
        return kAudioHardwareBadDeviceError;
    /* Capture the publication before the alive check and before queueing on the
     * Rust session lock, so a StartIO racing a detach/rebind never opens the new unit. */
    uint64_t binding = ftu_live_binding(live);
    if (!atomic_load(&g_alive))
        return kAudioHardwareNotRunningError;
    /* On success Rust guarantees ftu_live_timestamp() is valid; there is no host-clock fallback.
     * -6: removed or rebound after the alive check; Rust refused under its lock.
     * -9: this client ID was active when the unit was removed and its old session was
     *     never stopped/removed; the app may need to stop and reopen the device. */
    int32_t rc = ftu_live_start_bound(live, client, binding);
    return rc == 0    ? kAudioHardwareNoError
           : rc == -6 ? kAudioHardwareNotRunningError
           : rc == -9 ? kAudioHardwareIllegalOperationError
                      : kAudioHardwareUnspecifiedError;
}

static OSStatus ftu_stop_io(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt32 client)
{
    struct ftu_live *live = device_live(drv, dev);
    if (!live)
        return kAudioHardwareBadDeviceError;
    return ftu_live_stop(live, client) == 0 ? kAudioHardwareNoError : kAudioHardwareUnspecifiedError;
}

static OSStatus ftu_get_zts(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt32 client,
                            Float64 *sample_time, UInt64 *host_time, UInt64 *seed)
{
    (void)client;
    struct ftu_live *live = device_live(drv, dev);
    if (!live)
        return kAudioHardwareBadDeviceError;
    if (!sample_time || !host_time || !seed)
        return kAudioHardwareIllegalOperationError;
    struct ftu_live_timestamp ts;
    if (ftu_live_timestamp(live, &ts) != 0)
        return kAudioHardwareNotRunningError;
    *sample_time = (Float64)ts.frame;
    *host_time = ts.ticks;
    *seed = ts.epoch;
    return kAudioHardwareNoError;
}

static OSStatus ftu_will_do(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt32 client, UInt32 op,
                            Boolean *will_do, Boolean *in_place)
{
    (void)client;
    if (!device_live(drv, dev))
        return kAudioHardwareBadDeviceError;
    if (!will_do || !in_place)
        return kAudioHardwareIllegalOperationError;
    *will_do = op == kAudioServerPlugInIOOperationReadInput || op == kAudioServerPlugInIOOperationWriteMix;
    *in_place = true;
    return kAudioHardwareNoError;
}

static OSStatus ftu_begin_io(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt32 client, UInt32 op,
                             UInt32 frames, const AudioServerPlugInIOCycleInfo *info)
{
    (void)client; (void)op; (void)frames; (void)info;
    return device_live(drv, dev) ? kAudioHardwareNoError : kAudioHardwareBadDeviceError;
}

static OSStatus ftu_do_io(AudioServerPlugInDriverRef drv, AudioObjectID dev, AudioObjectID stream,
                          UInt32 client, UInt32 op, UInt32 frames, const AudioServerPlugInIOCycleInfo *info,
                          void *main_buf, void *secondary)
{
    (void)client; (void)secondary;
    struct ftu_live *live = device_live(drv, dev);
    if (!live)
        return kAudioHardwareBadDeviceError;
    bool read = op == kAudioServerPlugInIOOperationReadInput;
    if (!read && op != kAudioServerPlugInIOOperationWriteMix)
        return kAudioHardwareNoError;
    if (stream != (read ? kObjInput : kObjOutput))
        return kAudioHardwareBadStreamError;
    if (!info || !main_buf || frames == 0 || frames > g_cfg.max_buffer)
        return kAudioHardwareIllegalOperationError;
    Float64 t = read ? info->mInputTime.mSampleTime : info->mOutputTime.mSampleTime;
    if (!(t >= 0.0 && t < FTU_MAX_SAMPLE_TIME)) /* also rejects NaN and infinities */
        return kAudioHardwareIllegalOperationError;
    uint64_t first = (uint64_t)t;
    if ((Float64)first != t)
        return kAudioHardwareIllegalOperationError;
    int32_t rc = read ? ftu_live_read(live, first, frames, (float *)main_buf)
                      : ftu_live_write(live, first, frames, (const float *)main_buf);
    return rc == 0 ? kAudioHardwareNoError : kAudioHardwareUnspecifiedError;
}

static OSStatus ftu_end_io(AudioServerPlugInDriverRef drv, AudioObjectID dev, UInt32 client, UInt32 op,
                           UInt32 frames, const AudioServerPlugInIOCycleInfo *info)
{
    (void)client; (void)op; (void)frames; (void)info;
    return device_live(drv, dev) ? kAudioHardwareNoError : kAudioHardwareBadDeviceError;
}

static AudioServerPlugInDriverInterface g_vtable = {
    NULL,
    ftu_query_interface,
    ftu_add_ref,
    ftu_release,
    ftu_initialize,
    ftu_create_device,
    ftu_destroy_device,
    ftu_add_client,
    ftu_remove_client,
    ftu_perform_change,
    ftu_abort_change,
    ftu_has_property,
    ftu_is_settable,
    ftu_get_size,
    ftu_get_data,
    ftu_set_data,
    ftu_start_io,
    ftu_stop_io,
    ftu_get_zts,
    ftu_will_do,
    ftu_begin_io,
    ftu_do_io,
    ftu_end_io,
};
