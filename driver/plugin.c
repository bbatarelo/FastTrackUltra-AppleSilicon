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

enum { kObjPlugIn = kAudioObjectPlugInObject, kObjDevice = 2, kObjInput = 3, kObjOutput = 4 };

static AudioServerPlugInDriverInterface g_vtable;
static AudioServerPlugInDriverInterface *g_vtable_ptr = &g_vtable;
static AudioServerPlugInDriverRef const g_ref = &g_vtable_ptr;

static _Atomic(ULONG) g_refs = 1;
static atomic_bool g_initialized;
static AudioServerPlugInHostRef g_host;
static struct ftu_live_config g_cfg; /* immutable after Initialize */

/* Selected unit. Written once by adopt() before g_live is published with release
 * ordering; the handle is never destroyed for the lifetime of the host process. */
static _Atomic(struct ftu_live *) g_live;
static uint32_t g_product, g_location;
static CFStringRef g_name, g_uid, g_model;
static atomic_bool g_present, g_alive, g_running;
static _Atomic(UInt32) g_buffer_frames;

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

static struct ftu_live *adopt(uint32_t product, uint32_t location)
{
    CFStringRef name = CFStringCreateWithCString(NULL, model_name(product), kCFStringEncodingUTF8);
    CFStringRef uid = CFStringCreateWithFormat(NULL, NULL, CFSTR(FTU_BUNDLE_ID ":%04X:%08X"), product, location);
    CFStringRef model = CFStringCreateWithFormat(NULL, NULL, CFSTR(FTU_BUNDLE_ID ":%04X"), product);
    struct ftu_live *live = NULL;
    if (name && uid && model)
        live = ftu_live_create(&ftu_usb_factory, product, location);
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
    g_location = location;
    atomic_store_explicit(&g_live, live, memory_order_release);
    return live;
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
 * handle creation is single-threaded. No lock is held across USB or Rust calls. */
static void poll_presence(bool notify_host)
{
    uint32_t product = 0, location = 0;
    bool found = ftu_usb_find(&product, &location) == 0 && model_name(product) != NULL;
    struct ftu_live *live = live_handle();
    if (!live && found)
        live = adopt(product, location);
    bool present = live && found && product == g_product && location == g_location;
    uint32_t phase = present ? session_phase(live) : 0;
    bool alive = present && phase != 5;
    bool running_changed = atomic_exchange(&g_running, phase == 2) != (phase == 2);
    bool list_changed = atomic_exchange(&g_present, present) != present;
    bool alive_changed = atomic_exchange(&g_alive, alive) != alive;
    if (!notify_host)
        return;
    if (running_changed) {
        const AudioObjectPropertyAddress a = {kAudioDevicePropertyDeviceIsRunning,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
        notify(kObjDevice,&a,1);
    }
    if (alive_changed) {
        static const AudioObjectPropertyAddress a = {
            kAudioDevicePropertyDeviceIsAlive, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
        notify(kObjDevice, &a, 1);
    }
    if (list_changed) {
        static const AudioObjectPropertyAddress a[2] = {
            { kAudioPlugInPropertyDeviceList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain },
            { kAudioObjectPropertyOwnedObjects, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain } };
        notify(kObjPlugIn, a, 2);
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
    AudioServerPlugInCustomPropertyInfo custom[3];
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
    case FTU_TIMING_PROPERTY:
    case FTU_STATUS_PROPERTY: return put_str(b, s, CFSTR("")); /* built in get_data */
    case kAudioObjectPropertyCustomPropertyInfoList:
        b->custom[0]=(AudioServerPlugInCustomPropertyInfo){FTU_STATUS_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[1]=(AudioServerPlugInCustomPropertyInfo){FTU_BUILD_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
        b->custom[2]=(AudioServerPlugInCustomPropertyInfo){FTU_TIMING_PROPERTY,kAudioServerPlugInCustomPropertyDataTypeCFString,kAudioServerPlugInCustomPropertyDataTypeNone};
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

/* Rust tracks IO clients; stopping an unknown client is expected to return 0. */
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
    if (!atomic_load(&g_alive))
        return kAudioHardwareNotRunningError;
    /* On success Rust guarantees ftu_live_timestamp() is valid; there is no host-clock fallback. */
    return ftu_live_start(live, client) == 0 ? kAudioHardwareNoError : kAudioHardwareUnspecifiedError;
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
