/*
 * ftu-probe: probe for the M-Audio Fast Track Ultra (0763:2080) and Fast
 * Track Ultra 8R (0763:2081).
 *
 * list and descriptors are passive. They read:
 *   - IORegistry properties of the matched USB device service;
 *   - getters of IOUSBDeviceInterface500 that return values macOS has
 *     already cached (documented as not requiring the device to be open);
 *   - configuration descriptors cached by macOS, via
 *     GetConfigurationDescriptorPtr.
 * They issue no USB requests of their own.
 *
 * controls additionally issues the fixed device-to-host class GET requests
 * from controls.c through DeviceRequestTO, without opening the device.
 *
 * capture additionally opens interface 2, temporarily selects alternate 1,
 * and performs bounded IN-only isochronous reads. No output or mixer/rate writes.
 */

#include "controls.h"
#include "capture.h"
#include "duplex.h"

#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOCFPlugIn.h>
#include <IOKit/IOKitLib.h>
#include <IOKit/usb/IOUSBLib.h>

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define MAUDIO_VENDOR_ID 0x0763u

#define HOST_DEVICE_CLASS "IOUSBHostDevice"
#ifndef kIOUSBDeviceClassName
#define kIOUSBDeviceClassName "IOUSBDevice"
#endif

#define MAX_DEVICES 64
#define MAX_ERRORS 32
#define ERROR_LEN 256
#define MAX_CONFIGS 256 /* bNumConfigurations is a UInt8 */
#define CONFIG_HEADER_LEN 9
#define OBSERVATIONS_FILE "observations.jsonl"
#define LEN_NOT_REPORTED 0xFFFFFFFFu

/* EXIT_INCOMPLETE: controls ran but some planned request was not accepted. */
enum { EXIT_OK = 0, EXIT_RUNTIME = 1, EXIT_USAGE = 2, EXIT_INCOMPLETE = 3 };

static const struct {
    uint16_t vid;
    uint16_t pid;
    const char *model;
} k_supported[] = {
    { MAUDIO_VENDOR_ID, 0x2080, "Fast Track Ultra" },
    { MAUDIO_VENDOR_ID, 0x2081, "Fast Track Ultra 8R" },
};

#define NUM_SUPPORTED (sizeof k_supported / sizeof k_supported[0])

struct opt_num {
    bool ok;
    uint64_t v;
};

struct opt_str {
    bool ok;
    const char *key; /* registry key the value came from */
    char v[256];
};

struct device {
    io_service_t service;
    const char *matched_class;
    uint64_t registry_id;
    io_name_t entry_name;
    struct opt_num vid, pid, bcd, location, serial_index, current_config;
    struct opt_num reg_device_speed, reg_usb_speed, reg_link_speed;
    struct opt_str product, vendor, serial;
};

struct device_list {
    struct device dev[MAX_DEVICES];
    size_t count;
};

struct options {
    const char *output;
    struct opt_num product;
    struct opt_num location;
};

struct config_record {
    bool header_read;
    bool header_ok;
    bool saved;
    uint8_t bLength, bDescriptorType, bNumInterfaces, bConfigurationValue;
    uint16_t wTotalLength;
    char file[32];
    char error[ERROR_LEN];
};

struct control_summary {
    size_t planned, attempted, accepted, short_count, bad_length, error_responses;
    double elapsed_ms;
    char abort_reason[ERROR_LEN]; /* empty when the plan ran to the end */
};

struct probe {
    const char *command;
    const struct device *dev;
    struct opt_num product_filter, location_filter;
    char captured_at[32];
    char finished_at[32];
    bool iface_ok;
    struct opt_num if_vid, if_pid, if_bcd, if_location, if_serial_index;
    struct opt_num if_speed, if_num_configs;
    struct config_record configs[MAX_CONFIGS];
    size_t nconfigs;
    struct control_summary ctl;
    char errors[MAX_ERRORS][ERROR_LEN];
    size_t nerrors;
    bool errors_truncated;
};

static volatile sig_atomic_t g_stop_signal;

static struct opt_num some(uint64_t v)
{
    struct opt_num n = { true, v };
    return n;
}

static const char *lookup_model(const struct device *d)
{
    if (!d->vid.ok || !d->pid.ok)
        return NULL;
    for (size_t i = 0; i < NUM_SUPPORTED; i++)
        if (d->vid.v == k_supported[i].vid && d->pid.v == k_supported[i].pid)
            return k_supported[i].model;
    return NULL;
}

static bool is_supported_pid(uint32_t pid)
{
    for (size_t i = 0; i < NUM_SUPPORTED; i++)
        if (pid == k_supported[i].pid)
            return true;
    return false;
}

static void add_error(struct probe *pr, const char *fmt, ...) __attribute__((format(printf, 2, 3)));

static void add_error(struct probe *pr, const char *fmt, ...)
{
    char msg[ERROR_LEN];
    va_list ap;

    va_start(ap, fmt);
    vsnprintf(msg, sizeof msg, fmt, ap);
    va_end(ap);
    fprintf(stderr, "ftu-probe: error: %s\n", msg);
    if (pr->nerrors < MAX_ERRORS)
        memcpy(pr->errors[pr->nerrors++], msg, sizeof msg);
    else
        pr->errors_truncated = true;
}

static void utc_now(char *buf, size_t len)
{
    time_t now = time(NULL);
    struct tm tm;

    buf[0] = '\0';
    if (now != (time_t)-1 && gmtime_r(&now, &tm) != NULL)
        strftime(buf, len, "%Y-%m-%dT%H:%M:%SZ", &tm);
}

static double monotonic_ms(void)
{
    struct timespec ts;

    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
        return 0.0;
    return (double)ts.tv_sec * 1000.0 + (double)ts.tv_nsec / 1e6;
}

/* ---- IORegistry ---------------------------------------------------------- */

static CFTypeRef copy_property(io_service_t s, const char *key)
{
    CFStringRef k = CFStringCreateWithCString(kCFAllocatorDefault, key, kCFStringEncodingUTF8);
    if (k == NULL)
        return NULL;
    CFTypeRef v = IORegistryEntryCreateCFProperty(s, k, kCFAllocatorDefault, 0);
    CFRelease(k);
    return v;
}

/*
 * Reads an unsigned registry number no larger than max; anything else is
 * reported as unavailable rather than guessed. 32-bit properties such as
 * locationID can be stored as signed CFNumbers, so a negative value that fits
 * in int32 is reinterpreted when max covers the full 32-bit range.
 */
static struct opt_num reg_number(io_service_t s, const char *key, uint64_t max)
{
    struct opt_num r = { false, 0 };
    CFTypeRef v = copy_property(s, key);
    int64_t n;

    if (v == NULL)
        return r;
    if (CFGetTypeID(v) == CFNumberGetTypeID() &&
        CFNumberGetValue((CFNumberRef)v, kCFNumberSInt64Type, &n)) {
        if (n >= 0 && (uint64_t)n <= max)
            r = some((uint64_t)n);
        else if (n < 0 && n >= INT32_MIN && max == UINT32_MAX)
            r = some((uint32_t)(int32_t)n);
    }
    CFRelease(v);
    return r;
}

static struct opt_str reg_string(io_service_t s, const char *const *keys)
{
    struct opt_str r;

    memset(&r, 0, sizeof r);
    for (; *keys != NULL; keys++) {
        CFTypeRef v = copy_property(s, *keys);
        if (v == NULL)
            continue;
        bool got = CFGetTypeID(v) == CFStringGetTypeID() &&
                   CFStringGetCString((CFStringRef)v, r.v, sizeof r.v, kCFStringEncodingUTF8);
        CFRelease(v);
        if (got) {
            r.ok = true;
            r.key = *keys;
            return r;
        }
    }
    r.v[0] = '\0';
    return r;
}

/* Takes ownership of s. */
static void read_device(io_service_t s, const char *cls, uint64_t id, struct device *d)
{
    static const char *const product_keys[] = { "USB Product Name", "kUSBProductString", NULL };
    static const char *const vendor_keys[] = { "USB Vendor Name", "kUSBVendorString", NULL };
    static const char *const serial_keys[] = { "USB Serial Number", "kUSBSerialNumberString", NULL };

    memset(d, 0, sizeof *d);
    d->service = s;
    d->matched_class = cls;
    d->registry_id = id;
    if (IORegistryEntryGetName(s, d->entry_name) != KERN_SUCCESS)
        d->entry_name[0] = '\0';
    d->vid = reg_number(s, "idVendor", 0xFFFF);
    d->pid = reg_number(s, "idProduct", 0xFFFF);
    d->bcd = reg_number(s, "bcdDevice", 0xFFFF);
    d->location = reg_number(s, "locationID", UINT32_MAX);
    d->serial_index = reg_number(s, "iSerialNumber", 0xFF);
    d->current_config = reg_number(s, "kUSBCurrentConfiguration", 0xFF);
    d->reg_device_speed = reg_number(s, "Device Speed", 0xFF);
    d->reg_usb_speed = reg_number(s, "USBSpeed", 0xFF);
    d->reg_link_speed = reg_number(s, "UsbLinkSpeed", UINT64_MAX);
    d->product = reg_string(s, product_keys);
    d->vendor = reg_string(s, vendor_keys);
    d->serial = reg_string(s, serial_keys);
}

static bool have_registry_id(const struct device_list *list, uint64_t id)
{
    for (size_t i = 0; i < list->count; i++)
        if (list->dev[i].registry_id == id)
            return true;
    return false;
}

/* Matching dictionary for one class name restricted to an exact VID/PID. */
static CFMutableDictionaryRef product_matching(const char *cls, uint16_t vid, uint16_t pid)
{
    CFMutableDictionaryRef match = IOServiceMatching(cls);
    SInt32 v = vid, p = pid;

    if (match == NULL)
        return NULL;
    CFNumberRef nv = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &v);
    CFNumberRef np = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &p);
    bool ok = nv != NULL && np != NULL;
    if (ok) {
        CFDictionarySetValue(match, CFSTR("idVendor"), nv);
        CFDictionarySetValue(match, CFSTR("idProduct"), np);
    }
    if (nv != NULL)
        CFRelease(nv);
    if (np != NULL)
        CFRelease(np);
    if (!ok) {
        CFRelease(match);
        return NULL;
    }
    return match;
}

/*
 * Collects the supported products from both the current IOUSBHostDevice class
 * and the legacy IOUSBDevice class name, using an exact VID/PID matching
 * dictionary per product, de-duplicated by registry entry ID. Returns false if
 * enumeration may be incomplete.
 */
static bool enumerate(struct device_list *list)
{
    static const char *const classes[] = { HOST_DEVICE_CLASS, kIOUSBDeviceClassName };
    bool complete = true;

    for (size_t c = 0; c < sizeof classes / sizeof classes[0]; c++) {
        for (size_t p = 0; p < NUM_SUPPORTED; p++) {
            CFMutableDictionaryRef match =
                product_matching(classes[c], k_supported[p].vid, k_supported[p].pid);
            io_iterator_t it = IO_OBJECT_NULL;
            io_service_t s;

            if (match == NULL) {
                fprintf(stderr, "ftu-probe: cannot build matching dictionary for %s %04x:%04x\n",
                        classes[c], k_supported[p].vid, k_supported[p].pid);
                complete = false;
                continue;
            }
            /* IOServiceGetMatchingServices consumes the matching dictionary. */
            kern_return_t kr = IOServiceGetMatchingServices(MACH_PORT_NULL, match, &it);
            if (kr != KERN_SUCCESS) {
                fprintf(stderr, "ftu-probe: IOServiceGetMatchingServices(%s %04x:%04x) failed: "
                                "0x%08x\n",
                        classes[c], k_supported[p].vid, k_supported[p].pid, (unsigned)kr);
                complete = false;
                continue;
            }
            while ((s = IOIteratorNext(it)) != IO_OBJECT_NULL) {
                uint64_t id;

                if (IORegistryEntryGetRegistryEntryID(s, &id) != KERN_SUCCESS) {
                    fprintf(stderr, "ftu-probe: cannot read registry entry ID of a matched "
                                    "device\n");
                    IOObjectRelease(s);
                    complete = false;
                    continue;
                }
                if (have_registry_id(list, id)) {
                    IOObjectRelease(s);
                    continue;
                }
                if (list->count == MAX_DEVICES) {
                    fprintf(stderr, "ftu-probe: more than %d matching devices found\n",
                            MAX_DEVICES);
                    IOObjectRelease(s);
                    complete = false;
                    continue;
                }
                read_device(s, classes[c], id, &list->dev[list->count++]);
            }
            IOObjectRelease(it);
        }
    }
    return complete;
}

static void release_list(struct device_list *list)
{
    for (size_t i = 0; i < list->count; i++)
        IOObjectRelease(list->dev[i].service);
    list->count = 0;
}

static void print_hex_field(FILE *f, const char *label, struct opt_num n, int width)
{
    if (n.ok)
        fprintf(f, " %s=0x%0*llx", label, width, (unsigned long long)n.v);
    else
        fprintf(f, " %s=unavailable", label);
}

static void print_device(FILE *f, const struct device *d)
{
    fprintf(f, "  %s", d->product.ok ? d->product.v : "(no product name in registry)");
    print_hex_field(f, "vid", d->vid, 4);
    print_hex_field(f, "pid", d->pid, 4);
    print_hex_field(f, "bcdDevice", d->bcd, 4);
    print_hex_field(f, "location", d->location, 8);
    fprintf(f, " registry_id=0x%llx class=%s serial=%s\n",
            (unsigned long long)d->registry_id, d->matched_class,
            d->serial.ok ? d->serial.v : "unavailable");
}

/* ---- JSON output --------------------------------------------------------- */

static void json_string(FILE *f, const char *s)
{
    if (s == NULL) {
        fputs("null", f);
        return;
    }
    fputc('"', f);
    for (const unsigned char *p = (const unsigned char *)s; *p != '\0'; p++) {
        switch (*p) {
        case '"': fputs("\\\"", f); break;
        case '\\': fputs("\\\\", f); break;
        case '\n': fputs("\\n", f); break;
        case '\r': fputs("\\r", f); break;
        case '\t': fputs("\\t", f); break;
        default:
            if (*p < 0x20)
                fprintf(f, "\\u%04x", *p);
            else
                fputc(*p, f);
        }
    }
    fputc('"', f);
}

static void json_hex(FILE *f, struct opt_num n, int width)
{
    if (n.ok)
        fprintf(f, "\"0x%0*llx\"", width, (unsigned long long)n.v);
    else
        fputs("null", f);
}

static void json_uint(FILE *f, struct opt_num n)
{
    if (n.ok)
        fprintf(f, "%llu", (unsigned long long)n.v);
    else
        fputs("null", f);
}

static void json_opt_str(FILE *f, const struct opt_str *s)
{
    json_string(f, s->ok ? s->v : NULL);
}

static void json_errors(FILE *f, const struct probe *pr)
{
    fputs("  \"errors\": [", f);
    for (size_t i = 0; i < pr->nerrors; i++) {
        fputs(i ? ", " : "", f);
        json_string(f, pr->errors[i]);
    }
    fputs("],\n", f);
    fprintf(f, "  \"errors_truncated\": %s,\n", pr->errors_truncated ? "true" : "false");
}

static bool is_controls(const struct probe *pr)
{
    return strcmp(pr->command, "controls") == 0;
}

static const char *speed_name(uint64_t speed)
{
    switch (speed) {
    case kUSBDeviceSpeedLow: return "low (1.5 Mb/s)";
    case kUSBDeviceSpeedFull: return "full (12 Mb/s)";
    case kUSBDeviceSpeedHigh: return "high (480 Mb/s)";
    case kUSBDeviceSpeedSuper: return "super (5 Gb/s)";
    default: return NULL;
    }
}

static void emit_metadata(FILE *f, const struct probe *pr)
{
    const struct device *d = pr->dev;
    bool ok = pr->nerrors == 0 && !pr->errors_truncated;

    fputs("{\n", f);
    fputs("  \"tool\": \"ftu-probe\",\n", f);
    fputs("  \"metadata_version\": 2,\n", f);
    fputs("  \"command\": ", f);
    json_string(f, pr->command);
    fputs(",\n", f);
    fprintf(f, "  \"status\": \"%s\",\n", ok ? "ok" : "error");
    json_errors(f, pr);
    fputs("  \"captured_at_utc\": ", f);
    json_string(f, pr->captured_at[0] ? pr->captured_at : NULL);
    fputs(",\n", f);
    if (is_controls(pr))
        fputs("  \"access\": \"captured before any control request: IORegistry properties and "
              "cached device-interface getters only. The controls command then issues only the "
              "fixed device-to-host class GET requests listed by 'ftu-probe control-plan', via "
              "DeviceRequestTO without opening the device; see observations.jsonl and "
              "summary.json\",\n", f);
    else if (strcmp(pr->command, "duplex-silence") == 0 || strcmp(pr->command, "playback-tone") == 0 || strcmp(pr->command, "stream-silence") == 0 || strcmp(pr->command, "stream-tone") == 0)
        fputs("  \"access\": \"metadata captured before USB requests or interface opens. "
              "Rust duplex engine then opens interfaces 2 and 1, selects alternate 1, "
              "and runs bounded IN81/OUT01 transfers. No mixer or rate writes. See plan and summary.\",\n", f);
    else if (strcmp(pr->command, "feedback") == 0)
        fputs("  \"access\": \"metadata captured before USB requests or interface opens. "
              "feedback then performs fixed rate GETs and bounded IN82 reads on interface 1, "
              "temporarily selecting alternate 1. No OUT transfers. See events.jsonl and summary.json\",\n", f);
    else if (strcmp(pr->command, "capture") == 0)
        fputs("  \"access\": \"metadata captured before USB requests or interface opens. "
              "capture then performs fixed rate GETs and bounded IN transfers on interface 2, "
              "temporarily selecting alternate 1. See events.jsonl and summary.json\",\n", f);
    else
        fputs("  \"access\": \"read-only: IORegistry properties and configuration descriptors "
              "already cached by macOS; the device and its interfaces were not opened and this "
              "tool issued no USB requests\",\n", f);
    fputs("  \"selection\": {\"product_filter\": ", f);
    json_hex(f, pr->product_filter, 4);
    fputs(", \"location_filter\": ", f);
    json_hex(f, pr->location_filter, 8);
    fputs("},\n", f);

    fputs("  \"registry\": {\n", f);
    fputs("    \"source\": \"IORegistry properties of the matched service; these are not descriptor "
          "bytes\",\n", f);
    fputs("    \"matched_class\": ", f);
    json_string(f, d->matched_class);
    fprintf(f, ",\n    \"registry_entry_id\": \"0x%llx\",\n", (unsigned long long)d->registry_id);
    fputs("    \"entry_name\": ", f);
    json_string(f, d->entry_name[0] ? d->entry_name : NULL);
    fputs(",\n    \"idVendor\": ", f);
    json_hex(f, d->vid, 4);
    fputs(",\n    \"idProduct\": ", f);
    json_hex(f, d->pid, 4);
    fputs(",\n    \"expected_model_for_vid_pid\": ", f);
    json_string(f, lookup_model(d));
    fputs(",\n    \"bcdDevice\": ", f);
    json_hex(f, d->bcd, 4);
    fputs(",\n    \"locationID\": ", f);
    json_hex(f, d->location, 8);
    fputs(",\n    \"product_name\": ", f);
    json_opt_str(f, &d->product);
    fputs(",\n    \"product_name_key\": ", f);
    json_string(f, d->product.key);
    fputs(",\n    \"vendor_name\": ", f);
    json_opt_str(f, &d->vendor);
    fputs(",\n    \"vendor_name_key\": ", f);
    json_string(f, d->vendor.key);
    fputs(",\n    \"serial_number\": ", f);
    json_opt_str(f, &d->serial);
    fputs(",\n    \"serial_number_key\": ", f);
    json_string(f, d->serial.key);
    fputs(",\n    \"iSerialNumber\": ", f);
    json_uint(f, d->serial_index);
    fputs(",\n    \"current_configuration\": ", f);
    json_uint(f, d->current_config);
    fputs(",\n    \"current_configuration_key\": \"kUSBCurrentConfiguration\"", f);
    fputs(",\n    \"speed_properties_raw\": {\"Device Speed\": ", f);
    json_uint(f, d->reg_device_speed);
    fputs(", \"USBSpeed\": ", f);
    json_uint(f, d->reg_usb_speed);
    fputs(", \"UsbLinkSpeed\": ", f);
    json_uint(f, d->reg_link_speed);
    fputs(", \"note\": \"raw registry values, not interpreted; null means the property was absent "
          "or not a valid number\"}\n", f);
    fputs("  },\n", f);

    fputs("  \"device_interface\": {\n", f);
    fputs("    \"source\": \"IOUSBDeviceInterface500 getters returning values cached by macOS; "
          "device not opened\",\n", f);
    fprintf(f, "    \"available\": %s,\n", pr->iface_ok ? "true" : "false");
    fputs("    \"idVendor\": ", f);
    json_hex(f, pr->if_vid, 4);
    fputs(",\n    \"idProduct\": ", f);
    json_hex(f, pr->if_pid, 4);
    fputs(",\n    \"bcdDevice\": ", f);
    json_hex(f, pr->if_bcd, 4);
    fputs(",\n    \"locationID\": ", f);
    json_hex(f, pr->if_location, 8);
    fputs(",\n    \"iSerialNumber\": ", f);
    json_uint(f, pr->if_serial_index);
    fputs(",\n    \"device_speed\": ", f);
    if (pr->if_speed.ok) {
        fprintf(f, "{\"raw\": %llu, \"name\": ", (unsigned long long)pr->if_speed.v);
        json_string(f, speed_name(pr->if_speed.v));
        fputs("}", f);
    } else {
        fputs("null", f);
    }
    fputs(",\n    \"number_of_configurations\": ", f);
    json_uint(f, pr->if_num_configs);
    fputs("\n  },\n", f);

    fputs("  \"device_descriptor\": null,\n", f);
    fputs("  \"device_descriptor_note\": \"raw device descriptor bytes are not captured by this tool "
          "and are deliberately not reconstructed from registry fields\",\n", f);

    if (is_controls(pr) || strcmp(pr->command, "capture") == 0 || strcmp(pr->command, "feedback") == 0 || strcmp(pr->command, "duplex-silence") == 0 || strcmp(pr->command, "playback-tone") == 0 || strcmp(pr->command, "stream-silence") == 0 || strcmp(pr->command, "stream-tone") == 0) {
        fputs("  \"configuration_descriptors\": null\n", f);
        fputs("}\n", f);
        return;
    }
    fputs("  \"configuration_descriptors\": {\n", f);
    fputs("    \"source\": \"byte copies of configuration descriptors cached by macOS "
          "(GetConfigurationDescriptorPtr); length taken from little-endian wTotalLength\",\n", f);
    fputs("    \"entries\": [", f);
    for (size_t i = 0; i < pr->nconfigs; i++) {
        const struct config_record *c = &pr->configs[i];

        fprintf(f, "%s\n      {\"index\": %zu, \"file\": ", i ? "," : "", i);
        json_string(f, c->saved ? c->file : NULL);
        fprintf(f, ", \"saved\": %s", c->saved ? "true" : "false");
        if (c->header_read)
            fprintf(f, ", \"bLength\": %u, \"bDescriptorType\": %u, \"wTotalLength\": %u",
                    c->bLength, c->bDescriptorType, c->wTotalLength);
        else
            fputs(", \"bLength\": null, \"bDescriptorType\": null, \"wTotalLength\": null", f);
        if (c->header_ok)
            fprintf(f, ", \"bNumInterfaces\": %u, \"bConfigurationValue\": %u",
                    c->bNumInterfaces, c->bConfigurationValue);
        else
            fputs(", \"bNumInterfaces\": null, \"bConfigurationValue\": null", f);
        fputs(", \"error\": ", f);
        json_string(f, c->error[0] ? c->error : NULL);
        fputs("}", f);
    }
    fputs(pr->nconfigs ? "\n    ]\n" : "]\n", f);
    fputs("  }\n", f);
    fputs("}\n", f);
}

static const char *controls_status(const struct probe *pr)
{
    const struct control_summary *cs = &pr->ctl;

    if (pr->nerrors != 0 || pr->errors_truncated)
        return "error";
    if (cs->planned == 0 || cs->accepted != cs->planned)
        return "incomplete";
    return "ok";
}

static void emit_summary(FILE *f, const struct probe *pr)
{
    const struct control_summary *cs = &pr->ctl;

    fputs("{\n", f);
    fputs("  \"tool\": \"ftu-probe\",\n", f);
    fputs("  \"summary_version\": 1,\n", f);
    fputs("  \"command\": \"controls\",\n", f);
    fprintf(f, "  \"status\": \"%s\",\n", controls_status(pr));
    json_errors(f, pr);
    fputs("  \"started_at_utc\": ", f);
    json_string(f, pr->captured_at[0] ? pr->captured_at : NULL);
    fputs(",\n  \"finished_at_utc\": ", f);
    json_string(f, pr->finished_at[0] ? pr->finished_at : NULL);
    fprintf(f, ",\n  \"elapsed_ms\": %.3f,\n", cs->elapsed_ms);
    fprintf(f, "  \"observations_file\": \"%s\",\n", OBSERVATIONS_FILE);
    fprintf(f, "  \"planned_requests\": %d,\n", CTL_PLAN_COUNT);
    fprintf(f, "  \"attempted\": %zu,\n", cs->attempted);
    fprintf(f, "  \"accepted\": %zu,\n", cs->accepted);
    fprintf(f, "  \"short\": %zu,\n", cs->short_count);
    fprintf(f, "  \"bad_length\": %zu,\n", cs->bad_length);
    fprintf(f, "  \"error_responses\": %zu,\n", cs->error_responses);
    fprintf(f, "  \"not_attempted\": %zu,\n", (size_t)CTL_PLAN_COUNT - cs->attempted);
    fprintf(f, "  \"aborted\": %s,\n", cs->abort_reason[0] ? "true" : "false");
    fputs("  \"abort_reason\": ", f);
    json_string(f, cs->abort_reason[0] ? cs->abort_reason : NULL);
    fprintf(f, ",\n  \"timeouts_ms\": {\"noDataTimeout\": %d, \"completionTimeout\": %d},\n",
            CTL_TIMEOUT_MS, CTL_TIMEOUT_MS);
    fprintf(f, "  \"stop_rule\": \"no retries; stop after %d consecutive failed, short or "
               "bad-length responses, or immediately on device removal or refused access\",\n",
            CTL_MAX_CONSECUTIVE_FAILURES);
    fputs("  \"notes\": [\n"
          "    \"only the fixed device-to-host class GET requests printed by 'ftu-probe "
          "control-plan' were issued; the device and its interfaces were not opened\",\n"
          "    \"decoded is set only for a successful response of exactly wLength bytes; null "
          "means unknown, never zero\",\n"
          "    \"signed 16-bit mixer values are raw; their scale is not established as dB\",\n"
          "    \"GET_MIN/GET_MAX/GET_RES apply to unit 5 selector 1 channel 1 only\",\n"
          "    \"a failed endpoint sampling-frequency read is not proof that a rate is "
          "unsupported\"\n"
          "  ]\n", f);
    fputs("}\n", f);
}

/* ---- Evidence files ------------------------------------------------------ */

/* Creates dir/name exclusively; never overwrites or follows a symlink. */
static int open_new_file(const char *dir, const char *name, char *err, size_t errlen)
{
    char path[PATH_MAX];
    int n = snprintf(path, sizeof path, "%s/%s", dir, name);

    if (n < 0 || (size_t)n >= sizeof path) {
        snprintf(err, errlen, "path too long for %s", name);
        return -1;
    }
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0644);
    if (fd < 0)
        snprintf(err, errlen, "cannot create %s: %s", path, strerror(errno));
    return fd;
}

static FILE *open_new_stream(struct probe *pr, const char *dir, const char *name)
{
    char err[ERROR_LEN];
    int fd = open_new_file(dir, name, err, sizeof err);

    if (fd < 0) {
        add_error(pr, "%s", err);
        return NULL;
    }
    FILE *f = fdopen(fd, "w");
    if (f == NULL) {
        add_error(pr, "fdopen %s: %s", name, strerror(errno));
        close(fd);
    }
    return f;
}

static bool write_new_file(const char *dir, const char *name, const uint8_t *buf, size_t len,
                           char *err, size_t errlen)
{
    int fd = open_new_file(dir, name, err, errlen);
    size_t off = 0;

    if (fd < 0)
        return false;
    while (off < len) {
        ssize_t w = write(fd, buf + off, len - off);
        if (w < 0) {
            if (errno == EINTR)
                continue;
            snprintf(err, errlen, "write %s: %s", name, strerror(errno));
            close(fd);
            return false;
        }
        if (w == 0) {
            snprintf(err, errlen, "write %s: no progress after %zu of %zu bytes", name, off, len);
            close(fd);
            return false;
        }
        off += (size_t)w;
    }
    if (close(fd) != 0) {
        snprintf(err, errlen, "close %s: %s", name, strerror(errno));
        return false;
    }
    return true;
}

static bool write_json(struct probe *pr, const char *dir, const char *name,
                       void (*emit)(FILE *, const struct probe *))
{
    FILE *f = open_new_stream(pr, dir, name);

    if (f == NULL)
        return false;
    emit(f, pr);
    bool ok = fflush(f) == 0 && !ferror(f);
    if (fclose(f) != 0)
        ok = false;
    if (!ok)
        add_error(pr, "writing %s failed", name);
    return ok;
}

/* ---- Device interface ---------------------------------------------------- */

static void read_cached_identity(struct probe *pr, IOUSBDeviceInterface500 **dev)
{
    UInt16 u16 = 0;
    UInt32 u32 = 0;
    UInt8 u8 = 0;

    if ((*dev)->GetDeviceVendor(dev, &u16) == kIOReturnSuccess)
        pr->if_vid = some(u16);
    if ((*dev)->GetDeviceProduct(dev, &u16) == kIOReturnSuccess)
        pr->if_pid = some(u16);
    if ((*dev)->GetDeviceReleaseNumber(dev, &u16) == kIOReturnSuccess)
        pr->if_bcd = some(u16);
    if ((*dev)->GetLocationID(dev, &u32) == kIOReturnSuccess)
        pr->if_location = some(u32);
    if ((*dev)->USBGetSerialNumberStringIndex(dev, &u8) == kIOReturnSuccess)
        pr->if_serial_index = some(u8);
    if ((*dev)->GetDeviceSpeed(dev, &u8) == kIOReturnSuccess)
        pr->if_speed = some(u8);
    if ((*dev)->GetNumberOfConfigurations(dev, &u8) == kIOReturnSuccess)
        pr->if_num_configs = some(u8);
}

/* Refuses to continue unless the plug-in reports the selected unit. */
static bool identity_matches(struct probe *pr)
{
    const struct device *d = pr->dev;

    if (!pr->if_vid.ok || !pr->if_pid.ok) {
        add_error(pr, "device interface did not report vendor/product ID; refusing to continue");
        return false;
    }
    if (pr->if_vid.v != d->vid.v || pr->if_pid.v != d->pid.v) {
        add_error(pr, "device interface reports %04llx:%04llx but registry selection is "
                      "%04llx:%04llx; refusing to continue",
                  (unsigned long long)pr->if_vid.v, (unsigned long long)pr->if_pid.v,
                  (unsigned long long)d->vid.v, (unsigned long long)d->pid.v);
        return false;
    }
    if (pr->if_location.ok && d->location.ok && pr->if_location.v != d->location.v) {
        add_error(pr, "device interface location 0x%08llx differs from registry location "
                      "0x%08llx; refusing to continue",
                  (unsigned long long)pr->if_location.v, (unsigned long long)d->location.v);
        return false;
    }
    return true;
}

/*
 * Returns the device interface without opening the device, after reading its
 * cached identity, or NULL on failure.
 */
static IOUSBDeviceInterface500 **get_device_interface(struct probe *pr)
{
    IOCFPlugInInterface **plugin = NULL;
    IOUSBDeviceInterface500 **dev = NULL;
    SInt32 score = 0;

    kern_return_t kr = IOCreatePlugInInterfaceForService(pr->dev->service,
                                                         kIOUSBDeviceUserClientTypeID,
                                                         kIOCFPlugInInterfaceID, &plugin, &score);
    if (kr != KERN_SUCCESS || plugin == NULL) {
        add_error(pr, "IOCreatePlugInInterfaceForService failed: 0x%08x", (unsigned)kr);
        if (plugin != NULL)
            (*plugin)->Release(plugin);
        return NULL;
    }
    HRESULT hr = (*plugin)->QueryInterface(plugin, CFUUIDGetUUIDBytes(kIOUSBDeviceInterfaceID500),
                                           (LPVOID *)&dev);
    (*plugin)->Release(plugin);
    if (hr != S_OK || dev == NULL) {
        add_error(pr, "QueryInterface(kIOUSBDeviceInterfaceID500) failed: 0x%08x", (unsigned)hr);
        /* An interface handed back alongside an error is still ours to release. */
        if (dev != NULL)
            (*dev)->Release(dev);
        return NULL;
    }
    pr->iface_ok = true;
    read_cached_identity(pr, dev);
    return dev;
}

/* ---- Cached descriptors -------------------------------------------------- */

static void save_configurations(struct probe *pr, IOUSBDeviceInterface500 **dev, const char *dir)
{
    if (!pr->if_num_configs.ok) {
        add_error(pr, "GetNumberOfConfigurations failed");
        return;
    }
    pr->nconfigs = (size_t)pr->if_num_configs.v;
    if (pr->nconfigs == 0)
        add_error(pr, "device reports zero configurations");

    for (size_t i = 0; i < pr->nconfigs; i++) {
        struct config_record *c = &pr->configs[i];
        IOUSBConfigurationDescriptorPtr desc = NULL;
        IOReturn kr = (*dev)->GetConfigurationDescriptorPtr(dev, (UInt8)i, &desc);

        if (kr != kIOReturnSuccess || desc == NULL) {
            snprintf(c->error, sizeof c->error,
                     "GetConfigurationDescriptorPtr(%zu) failed: 0x%08x", i, (unsigned)kr);
            add_error(pr, "%s", c->error);
            continue;
        }

        /* Parse the header from raw bytes so wTotalLength is read little-endian. */
        const uint8_t *p = (const uint8_t *)desc;
        c->bLength = p[0];
        c->bDescriptorType = p[1];
        c->wTotalLength = (uint16_t)(p[2] | (p[3] << 8));
        c->header_read = true;
        if (c->bLength < CONFIG_HEADER_LEN || c->bDescriptorType != kUSBConfDesc ||
            c->wTotalLength < c->bLength) {
            snprintf(c->error, sizeof c->error,
                     "config %zu: malformed header (bLength %u, bDescriptorType 0x%02x, "
                     "wTotalLength %u); not copied",
                     i, c->bLength, c->bDescriptorType, c->wTotalLength);
            add_error(pr, "%s", c->error);
            continue;
        }
        c->bNumInterfaces = p[4];
        c->bConfigurationValue = p[5];
        c->header_ok = true;

        /* wTotalLength is uint16_t, so the copy is bounded by USB's 65535-byte limit. */
        uint8_t *copy = malloc(c->wTotalLength);
        if (copy == NULL) {
            snprintf(c->error, sizeof c->error, "config %zu: out of memory", i);
            add_error(pr, "%s", c->error);
            continue;
        }
        memcpy(copy, p, c->wTotalLength);
        snprintf(c->file, sizeof c->file, "config-%zu.bin", i);
        if (write_new_file(dir, c->file, copy, c->wTotalLength, c->error, sizeof c->error))
            c->saved = true;
        else
            add_error(pr, "%s", c->error);
        free(copy);
    }
}

static void probe_device(struct probe *pr, const char *dir)
{
    IOUSBDeviceInterface500 **dev = get_device_interface(pr);

    if (dev == NULL)
        return;
    if (identity_matches(pr))
        save_configurations(pr, dev, dir);
    (*dev)->Release(dev);
}

/* ---- Control reads ------------------------------------------------------- */

static void on_stop_signal(int sig)
{
    (void)sig;
    g_stop_signal = 1;
}

static void set_abort(struct control_summary *cs, const char *fmt, ...)
    __attribute__((format(printf, 2, 3)));

static void set_abort(struct control_summary *cs, const char *fmt, ...)
{
    va_list ap;

    va_start(ap, fmt);
    vsnprintf(cs->abort_reason, sizeof cs->abort_reason, fmt, ap);
    va_end(ap);
    fprintf(stderr, "ftu-probe: stopping: %s\n", cs->abort_reason);
}

/* Builds the fixed plan and checks every entry against the allowlist. */
static bool build_checked_plan(struct ctl_request *plan, char *err, size_t errlen)
{
    size_t n = ctl_build_plan(plan, CTL_PLAN_COUNT);

    if (n != CTL_PLAN_COUNT) {
        snprintf(err, errlen, "internal error: plan has %zu requests, expected %d", n,
                 CTL_PLAN_COUNT);
        return false;
    }
    for (size_t i = 0; i < n; i++) {
        if (!ctl_allowed(&plan[i])) {
            snprintf(err, errlen, "internal error: planned request %zu (%s) is not allowlisted",
                     i, plan[i].label);
            return false;
        }
    }
    return true;
}

static bool is_removal(IOReturn kr)
{
    return kr == kIOReturnNoDevice || kr == kIOReturnNotAttached;
}

static bool is_access_refusal(IOReturn kr)
{
    return kr == kIOReturnNotOpen || kr == kIOReturnExclusiveAccess ||
           kr == kIOReturnNotPermitted || kr == kIOReturnNotPrivileged;
}

/*
 * Issues each planned GET once, writing and flushing one JSON line per
 * response before deciding whether to continue.
 */
static void run_controls(struct probe *pr, IOUSBDeviceInterface500 **dev, const char *dir)
{
    static struct ctl_request plan[CTL_PLAN_COUNT];
    struct control_summary *cs = &pr->ctl;
    char err[ERROR_LEN];
    unsigned consecutive = 0;

    if (!build_checked_plan(plan, err, sizeof err)) {
        add_error(pr, "%s", err);
        set_abort(cs, "not started: request plan failed its self-check");
        return;
    }
    FILE *obs = open_new_stream(pr, dir, OBSERVATIONS_FILE);
    if (obs == NULL) {
        set_abort(cs, "not started: cannot create %s", OBSERVATIONS_FILE);
        return;
    }
    cs->planned = CTL_PLAN_COUNT;

    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_stop_signal;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGTERM, &sa, NULL);

    double start = monotonic_ms();
    for (size_t i = 0; i < CTL_PLAN_COUNT; i++) {
        const struct ctl_request *p = &plan[i];
        struct ctl_result res;
        IOUSBDevRequestTO req;

        if (g_stop_signal) {
            set_abort(cs, "interrupted by signal before request %zu", i);
            break;
        }
        memset(&res, 0, sizeof res);
        memset(&req, 0, sizeof req);
        req.bmRequestType = p->bmRequestType;
        req.bRequest = p->bRequest;
        req.wValue = p->wValue;
        req.wIndex = p->wIndex;
        req.wLength = p->wLength;
        req.pData = res.data; /* wLength <= CTL_MAX_LENGTH == sizeof res.data */
        req.wLenDone = LEN_NOT_REPORTED;
        req.noDataTimeout = CTL_TIMEOUT_MS;
        req.completionTimeout = CTL_TIMEOUT_MS;

        double t0 = monotonic_ms();
        IOReturn kr = (*dev)->DeviceRequestTO(dev, &req);
        res.elapsed_ms = monotonic_ms() - t0;
        res.ioreturn = (uint32_t)kr;
        if (req.wLenDone != LEN_NOT_REPORTED) {
            res.have_actual = true;
            res.actual = req.wLenDone;
        }
        cs->attempted++;

        ctl_emit(obs, i, p, &res);
        if (fflush(obs) != 0 || ferror(obs)) {
            add_error(pr, "writing %s failed: %s", OBSERVATIONS_FILE, strerror(errno));
            set_abort(cs, "cannot record results after request %zu", i);
            break;
        }

        enum ctl_outcome o = ctl_classify(p, &res);
        switch (o) {
        case CTL_OK: cs->accepted++; break;
        case CTL_SHORT: cs->short_count++; break;
        case CTL_BAD_LENGTH: cs->bad_length++; break;
        case CTL_ERROR: cs->error_responses++; break;
        }
        bool too_many = ctl_note_outcome(&consecutive, o);
        if (is_removal(kr)) {
            set_abort(cs, "device removed or detached (IOReturn 0x%08x) at request %zu",
                      (unsigned)kr, i);
            break;
        }
        if (is_access_refusal(kr)) {
            set_abort(cs, "access refused (IOReturn 0x%08x) at request %zu; this tool does not "
                          "open or seize the device",
                      (unsigned)kr, i);
            break;
        }
        if (too_many) {
            set_abort(cs, "%u consecutive failed or short responses ending at request %zu",
                      consecutive, i);
            break;
        }
    }
    cs->elapsed_ms = monotonic_ms() - start;
    if (fclose(obs) != 0)
        add_error(pr, "closing %s failed: %s", OBSERVATIONS_FILE, strerror(errno));

    signal(SIGINT, SIG_DFL);
    signal(SIGTERM, SIG_DFL);
}

/* ---- Commands ------------------------------------------------------------ */

static void usage(FILE *f)
{
    fputs("usage: ftu-probe list\n"
          "       ftu-probe descriptors --output NEW_DIRECTORY [--product HEX_PID] "
          "[--location HEX_LOCATION]\n"
          "       ftu-probe controls --output NEW_DIRECTORY [--product HEX_PID] "
          "[--location HEX_LOCATION]\n"
          "       ftu-probe control-plan\n"
          "       ftu-probe capture --output NEW_DIRECTORY [--product HEX_PID] "
          "[--location HEX_LOCATION] [--duration-ms N]\n"
          "       ftu-probe feedback --output NEW_DIRECTORY [--product HEX_PID] "
          "[--location HEX_LOCATION] [--duration-ms N]\n"
          "       ftu-probe duplex-silence --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION] [--duration-ms N]\n"
          "       ftu-probe stream-silence --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION] [--duration-ms N] [--cycles 1..3]\n"
          "       ftu-probe stream-tone --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION]\n"
          "       ftu-probe playback-tone --output NEW_DIRECTORY [--product HEX_PID] [--location HEX_LOCATION]\n"
          "\n"
          "Supported: 0763:2080 (Fast Track Ultra), 0763:2081 (Fast Track Ultra 8R).\n"
          "list and descriptors are passive: IORegistry properties and macOS-cached\n"
          "descriptors only; they issue no USB requests.\n"
          "controls issues only the fixed device-to-host class GET requests printed by\n"
          "control-plan, without opening the device. It never sends SET requests.\n"
          "control-plan touches no hardware.\n"
          "capture opens only interface 2 for bounded IN reads at the existing 48 kHz rate.\n"
          "feedback opens only interface 1 for bounded IN82 reads; never submits OUT01.\n"
          "duplex-silence opens interfaces 2 and 1 and sends only digital zeros to OUT01.\n"
          "playback-tone sends a fixed 2048 ms stream, with quiet ramped left/right tones.\n"
          "capture/feedback/duplex-silence: default 256 ms, multiples of 8 in 8..2048.\n"
          "stream-silence: default 10000 ms, multiples of 8 in 8..60000; --cycles 1..3.\n"
          "stream-tone: fixed 10000 ms, one session, quiet repeated left/right tones.\n", f);
}

static int cmd_list(void)
{
    struct device_list *list = calloc(1, sizeof *list);
    size_t supported = 0;

    if (list == NULL) {
        fprintf(stderr, "ftu-probe: out of memory\n");
        return EXIT_RUNTIME;
    }
    bool complete = enumerate(list);

    printf("Source: IORegistry properties only; no device was opened and no USB requests "
           "were issued.\n");
    printf("Supported devices:\n");
    for (size_t i = 0; i < list->count; i++) {
        if (lookup_model(&list->dev[i]) != NULL) {
            print_device(stdout, &list->dev[i]);
            supported++;
        }
    }
    if (supported == 0)
        printf("  none found (looking for 0763:2080 and 0763:2081)\n");
    if (!complete)
        fprintf(stderr, "ftu-probe: enumeration was incomplete; the list above may be missing "
                        "devices\n");
    release_list(list);
    free(list);
    return complete ? EXIT_OK : EXIT_RUNTIME;
}

/* Accepts 1-8 hex digits with an optional 0x prefix; nothing else. */
static bool parse_hex(const char *s, uint32_t max, uint32_t *out)
{
    uint32_t v = 0;
    size_t digits = 0;

    if (s[0] == '0' && (s[1] == 'x' || s[1] == 'X'))
        s += 2;
    for (; *s != '\0'; s++, digits++) {
        unsigned d;
        if (*s >= '0' && *s <= '9')
            d = (unsigned)(*s - '0');
        else if (*s >= 'a' && *s <= 'f')
            d = (unsigned)(*s - 'a' + 10);
        else if (*s >= 'A' && *s <= 'F')
            d = (unsigned)(*s - 'A' + 10);
        else
            return false;
        if (digits == 8)
            return false;
        v = v * 16 + d;
    }
    if (digits == 0 || v > max)
        return false;
    *out = v;
    return true;
}

static int parse_select_options(int argc, char **argv, struct options *o)
{
    memset(o, 0, sizeof *o);
    for (int i = 0; i < argc; i++) {
        const char *a = argv[i];
        uint32_t v;

        if (strcmp(a, "--output") != 0 && strcmp(a, "--product") != 0 &&
            strcmp(a, "--location") != 0) {
            fprintf(stderr, "ftu-probe: unknown option '%s'\n", a);
            return -1;
        }
        if (i + 1 >= argc || argv[i + 1][0] == '\0' || strncmp(argv[i + 1], "--", 2) == 0) {
            fprintf(stderr, "ftu-probe: %s requires a value\n", a);
            return -1;
        }
        const char *val = argv[++i];

        if (strcmp(a, "--output") == 0) {
            if (o->output != NULL) {
                fprintf(stderr, "ftu-probe: --output given more than once\n");
                return -1;
            }
            o->output = val;
        } else if (strcmp(a, "--product") == 0) {
            if (o->product.ok) {
                fprintf(stderr, "ftu-probe: --product given more than once\n");
                return -1;
            }
            if (!parse_hex(val, 0xFFFF, &v)) {
                fprintf(stderr, "ftu-probe: --product expects a hex PID such as 2080, got '%s'\n",
                        val);
                return -1;
            }
            if (!is_supported_pid(v)) {
                fprintf(stderr, "ftu-probe: unsupported PID 0x%04x (supported: 0x2080, 0x2081)\n",
                        v);
                return -1;
            }
            o->product = some(v);
        } else {
            if (o->location.ok) {
                fprintf(stderr, "ftu-probe: --location given more than once\n");
                return -1;
            }
            if (!parse_hex(val, UINT32_MAX, &v)) {
                fprintf(stderr, "ftu-probe: --location expects a hex locationID, got '%s'\n", val);
                return -1;
            }
            o->location = some(v);
        }
    }
    if (o->output == NULL) {
        fprintf(stderr, "ftu-probe: --output is required\n");
        return -1;
    }
    return 0;
}

static bool device_matches(const struct device *d, const struct options *o)
{
    if (lookup_model(d) == NULL)
        return false;
    if (o->product.ok && d->pid.v != o->product.v)
        return false;
    if (o->location.ok && (!d->location.ok || d->location.v != o->location.v))
        return false;
    return true;
}

/*
 * Shared by descriptors and controls: parses options, requires a new output
 * path, selects exactly one supported unit and creates the output directory.
 */
static int select_device(int argc, char **argv, struct options *opt, struct device_list *list,
                         const struct device **chosen)
{
    struct stat st;
    size_t matches = 0;

    if (parse_select_options(argc, argv, opt) != 0) {
        usage(stderr);
        return EXIT_USAGE;
    }
    if (lstat(opt->output, &st) == 0) {
        fprintf(stderr, "ftu-probe: '%s' already exists; refusing to touch previous evidence\n",
                opt->output);
        return EXIT_RUNTIME;
    }
    if (errno != ENOENT) {
        fprintf(stderr, "ftu-probe: cannot check '%s': %s\n", opt->output, strerror(errno));
        return EXIT_RUNTIME;
    }
    if (!enumerate(list)) {
        fprintf(stderr, "ftu-probe: USB registry enumeration was incomplete; refusing to select "
                        "a device\n");
        return EXIT_RUNTIME;
    }
    for (size_t i = 0; i < list->count; i++) {
        if (device_matches(&list->dev[i], opt)) {
            *chosen = &list->dev[i];
            matches++;
        }
    }
    if (matches != 1) {
        if (matches == 0)
            fprintf(stderr, "ftu-probe: no supported device matches the given filters\n");
        else
            fprintf(stderr, "ftu-probe: %zu devices match; rerun with --location to choose one\n",
                    matches);
        fprintf(stderr, "Supported devices seen:\n");
        for (size_t i = 0; i < list->count; i++)
            if (lookup_model(&list->dev[i]) != NULL)
                print_device(stderr, &list->dev[i]);
        return EXIT_RUNTIME;
    }
    if (mkdir(opt->output, 0755) != 0) {
        fprintf(stderr, "ftu-probe: cannot create '%s': %s\n", opt->output, strerror(errno));
        return EXIT_RUNTIME;
    }
    return EXIT_OK;
}

static void init_probe(struct probe *pr, const char *command, const struct device *chosen,
                       const struct options *opt)
{
    pr->command = command;
    pr->dev = chosen;
    pr->product_filter = opt->product;
    pr->location_filter = opt->location;
    utc_now(pr->captured_at, sizeof pr->captured_at);
}

static int cmd_descriptors(int argc, char **argv)
{
    struct options opt;
    struct device_list *list = calloc(1, sizeof *list);
    struct probe *pr = calloc(1, sizeof *pr);
    const struct device *chosen = NULL;
    int rc = EXIT_RUNTIME;

    if (list == NULL || pr == NULL) {
        fprintf(stderr, "ftu-probe: out of memory\n");
        goto out;
    }
    rc = select_device(argc, argv, &opt, list, &chosen);
    if (rc != EXIT_OK)
        goto out;

    init_probe(pr, "descriptors", chosen, &opt);
    probe_device(pr, opt.output);
    write_json(pr, opt.output, "metadata.json", emit_metadata);
    emit_metadata(stdout, pr);
    fflush(stdout);

    bool ok = pr->nerrors == 0 && !pr->errors_truncated;
    fprintf(stderr, "ftu-probe: evidence in '%s' (status %s)\n", opt.output, ok ? "ok" : "error");
    rc = ok ? EXIT_OK : EXIT_RUNTIME;

out:
    if (list != NULL)
        release_list(list);
    free(list);
    free(pr);
    return rc;
}

static int cmd_controls(int argc, char **argv)
{
    struct options opt;
    struct device_list *list = calloc(1, sizeof *list);
    struct probe *pr = calloc(1, sizeof *pr);
    const struct device *chosen = NULL;
    IOUSBDeviceInterface500 **dev = NULL;
    int rc = EXIT_RUNTIME;

    if (list == NULL || pr == NULL) {
        fprintf(stderr, "ftu-probe: out of memory\n");
        goto out;
    }
    rc = select_device(argc, argv, &opt, list, &chosen);
    if (rc != EXIT_OK)
        goto out;

    init_probe(pr, "controls", chosen, &opt);
    dev = get_device_interface(pr);
    bool ready = dev != NULL && identity_matches(pr);
    /* Registry metadata is saved before any request is issued. */
    bool saved = write_json(pr, opt.output, "metadata.json", emit_metadata);
    if (ready && saved)
        run_controls(pr, dev, opt.output);
    else
        set_abort(&pr->ctl, "not started: %s", !ready ? "device interface unavailable or identity "
                                                         "mismatch" : "metadata.json not written");
    if (dev != NULL)
        (*dev)->Release(dev);

    utc_now(pr->finished_at, sizeof pr->finished_at);
    write_json(pr, opt.output, "summary.json", emit_summary);
    emit_summary(stdout, pr);
    fflush(stdout);

    const char *status = controls_status(pr);
    fprintf(stderr, "ftu-probe: evidence in '%s' (status %s)\n", opt.output, status);
    if (strcmp(status, "ok") == 0)
        rc = EXIT_OK;
    else if (strcmp(status, "incomplete") == 0)
        rc = EXIT_INCOMPLETE;
    else
        rc = EXIT_RUNTIME;

out:
    if (list != NULL)
        release_list(list);
    free(list);
    free(pr);
    return rc;
}

static int cmd_input(int argc, char **argv, unsigned mode)
{
    unsigned duration = mode >= 4 ? 10000 : mode == 3 ? 2048 : 256;
    unsigned cycles = 1, limit = mode >= 4 ? 60000 : 2048;
    bool seen_cycles = false;
    bool seen_duration = false;
    char **selection = calloc((size_t)argc + 1, sizeof *selection);
    if (!selection) return EXIT_RUNTIME;
    int n = 0;
    for (int i = 0; i < argc; i++) {
        if (mode >= 4 && strcmp(argv[i], "--cycles") == 0) {
            if (seen_cycles || i+1==argc || strlen(argv[i+1])!=1 || argv[i+1][0]<'1' || argv[i+1][0]>'3') goto bad_duration;
            seen_cycles=true;cycles=(unsigned)(argv[++i][0]-'0');continue;
        }
        if (strcmp(argv[i], "--duration-ms") != 0) {
            selection[n++] = argv[i]; continue;
        }
        if (seen_duration || i + 1 == argc || !argv[i+1][0]) goto bad_duration;
        seen_duration = true;
        const char *value = argv[++i];
        duration = 0;
        for (; *value; value++) {
            if (*value < '0' || *value > '9' || duration > limit) goto bad_duration;
            duration = duration * 10 + (unsigned)(*value - '0');
        }
        if (!duration || duration > limit || duration % 8) goto bad_duration;
    }
    if (mode == 3 && duration != 2048) goto bad_duration;
    if (mode == 5 && (duration != 10000 || cycles != 1)) goto bad_duration;
    struct options opt;
    struct device_list *list = calloc(1, sizeof *list);
    struct probe *pr = calloc(1, sizeof *pr);
    const struct device *chosen = NULL;
    IOUSBDeviceInterface500 **dev = NULL;
    int rc = EXIT_RUNTIME;
    if (!list || !pr) goto out;
    rc = select_device(n, selection, &opt, list, &chosen);
    if (rc != EXIT_OK) goto out;
    init_probe(pr, mode == 5 ? "stream-tone" : mode == 4 ? "stream-silence" : mode == 3 ? "playback-tone" : mode == 2 ? "duplex-silence" : mode == 1 ? "feedback" : "capture", chosen, &opt);
    dev = get_device_interface(pr);
    bool ready = dev != NULL && identity_matches(pr);
    bool saved = write_json(pr, opt.output, "metadata.json", emit_metadata);
    if (ready && saved) {
        struct sigaction sa = {0}, old_int, old_term;
        sa.sa_handler = on_stop_signal;
        sigemptyset(&sa.sa_mask);
        g_stop_signal = 0;
        if (sigaction(SIGINT, &sa, &old_int) != 0) { rc = EXIT_RUNTIME; goto out; }
        if (sigaction(SIGTERM, &sa, &old_term) != 0) {
            sigaction(SIGINT, &old_int, NULL); rc = EXIT_RUNTIME; goto out;
        }
        rc = mode >= 4 ? stream_run(dev, opt.output, duration, mode == 5, cycles, &g_stop_signal) :
            mode >= 2 ? duplex_run(dev, opt.output, duration, mode == 3, &g_stop_signal) :
            (mode == 1 ? feedback_run : capture_run)(dev, opt.output, duration, &g_stop_signal);
        sigaction(SIGINT, &old_int, NULL);
        sigaction(SIGTERM, &old_term, NULL);
    } else rc = EXIT_RUNTIME;
out:
    if (dev) (*dev)->Release(dev);
    if (list) release_list(list);
    free(list); free(pr); free(selection);
    return rc;
bad_duration:
    fprintf(stderr, "ftu-probe: invalid duration/cycles; duration must be a multiple of 8 (8..%u); stream cycles 1..3; tones have fixed durations\n", limit);
    free(selection);
    return EXIT_USAGE;
}

/* Prints the fixed request plan; never enumerates or touches hardware. */
static int cmd_control_plan(void)
{
    static struct ctl_request plan[CTL_PLAN_COUNT];
    char err[ERROR_LEN];

    if (!build_checked_plan(plan, err, sizeof err)) {
        fprintf(stderr, "ftu-probe: %s\n", err);
        return EXIT_RUNTIME;
    }
    for (size_t i = 0; i < CTL_PLAN_COUNT; i++)
        ctl_emit(stdout, i, &plan[i], NULL);
    return fflush(stdout) == 0 && !ferror(stdout) ? EXIT_OK : EXIT_RUNTIME;
}

int main(int argc, char **argv)
{
    if (argc < 2) {
        usage(stderr);
        return EXIT_USAGE;
    }
    if (strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0) {
        usage(stdout);
        return EXIT_OK;
    }
    if (strcmp(argv[1], "list") == 0 || strcmp(argv[1], "control-plan") == 0) {
        if (argc != 2) {
            fprintf(stderr, "ftu-probe: %s takes no options\n", argv[1]);
            usage(stderr);
            return EXIT_USAGE;
        }
        return strcmp(argv[1], "list") == 0 ? cmd_list() : cmd_control_plan();
    }
    if (strcmp(argv[1], "descriptors") == 0)
        return cmd_descriptors(argc - 2, argv + 2);
    if (strcmp(argv[1], "controls") == 0)
        return cmd_controls(argc - 2, argv + 2);
    if (strcmp(argv[1], "capture") == 0)
        return cmd_input(argc - 2, argv + 2, 0);
    if (strcmp(argv[1], "feedback") == 0)
        return cmd_input(argc - 2, argv + 2, 1);
    if (strcmp(argv[1], "duplex-silence") == 0)
        return cmd_input(argc - 2, argv + 2, 2);
    if (strcmp(argv[1], "stream-silence") == 0)
        return cmd_input(argc - 2, argv + 2, 4);
    if (strcmp(argv[1], "stream-tone") == 0)
        return cmd_input(argc - 2, argv + 2, 5);
    if (strcmp(argv[1], "playback-tone") == 0)
        return cmd_input(argc - 2, argv + 2, 3);
    fprintf(stderr, "ftu-probe: unknown command '%s'\n", argv[1]);
    usage(stderr);
    return EXIT_USAGE;
}
