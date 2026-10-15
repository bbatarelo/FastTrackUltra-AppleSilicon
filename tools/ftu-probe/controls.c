/*
 * Request plan modelled on Linux sound/usb/mixer_quirks.c snd_ftu_create_*
 * and mixer.c get_ctl_value_v1/get_cur_mix_raw (commit 4056b3c9d3c9). Only
 * UAC1 GET requests (device-to-host) are planned or allowed; there is no way
 * to express a SET request here.
 */
#include "controls.h"

#include <stdarg.h>
#include <string.h>

#define RT_IN_CLASS_INTERFACE 0xA1
#define RT_IN_CLASS_ENDPOINT 0xA2

#define UAC_GET_CUR 0x81
#define UAC_GET_MIN 0x82
#define UAC_GET_MAX 0x83
#define UAC_GET_RES 0x84

#define CONTROL_INTERFACE 0
#define UNIT_MIXER 5
#define UNIT_EFFECT 6
#define UNIT_EFFECT_RETURN 7
#define SEL_EFFECT_SEND 9
#define SEL_EFFECT_RETURN 7
#define SAMPLING_FREQ_VALUE 0x0100 /* SAMPLING_FREQ_CONTROL << 8 */

#define MATRIX_OUTPUTS 8
#define MATRIX_CHANNELS 16 /* 1-8 hardware inputs, 9-16 host returns */
#define EFFECT_RETURNS 4

static void add(struct ctl_request *plan, size_t cap, size_t *n, uint8_t type, uint8_t req,
                uint16_t value, uint16_t index, uint16_t len, enum ctl_value_kind kind,
                const char *fmt, ...) __attribute__((format(printf, 10, 11)));

static void add(struct ctl_request *plan, size_t cap, size_t *n, uint8_t type, uint8_t req,
                uint16_t value, uint16_t index, uint16_t len, enum ctl_value_kind kind,
                const char *fmt, ...)
{
    if (*n < cap) {
        struct ctl_request *r = &plan[*n];
        va_list ap;

        memset(r, 0, sizeof *r);
        r->bmRequestType = type;
        r->bRequest = req;
        r->wValue = value;
        r->wIndex = index;
        r->wLength = len;
        r->kind = kind;
        va_start(ap, fmt);
        vsnprintf(r->label, sizeof r->label, fmt, ap);
        va_end(ap);
    }
    (*n)++;
}

static uint16_t unit_index(unsigned unit)
{
    return (uint16_t)(unit << 8 | CONTROL_INTERFACE);
}

static uint16_t control_value(unsigned selector, unsigned channel)
{
    return (uint16_t)(selector << 8 | channel);
}

/* Linux's AIn names cover hardware stream channels 1-8; their physical labels
 * are model-dependent. Channels 9-16 are the eight host returns. */
static const char *source_name(unsigned ch)
{
    return ch <= MATRIX_CHANNELS / 2 ? "hardware input" : "host return";
}

static unsigned source_number(unsigned ch)
{
    return ch <= MATRIX_CHANNELS / 2 ? ch : ch - MATRIX_CHANNELS / 2;
}

size_t ctl_build_plan(struct ctl_request *plan, size_t cap)
{
    static const struct {
        uint8_t req;
        const char *name;
    } range[] = { { UAC_GET_MIN, "GET_MIN" }, { UAC_GET_MAX, "GET_MAX" }, { UAC_GET_RES, "GET_RES" } };
    size_t n = 0;

    for (unsigned out = 1; out <= MATRIX_OUTPUTS; out++)
        for (unsigned ch = 1; ch <= MATRIX_CHANNELS; ch++)
            add(plan, cap, &n, RT_IN_CLASS_INTERFACE, UAC_GET_CUR, control_value(out, ch),
                unit_index(UNIT_MIXER), 2, CTL_S16LE, "unit 5 crosspoint: output destination %u <- %s %u",
                out, source_name(ch), source_number(ch));
    for (unsigned ch = 1; ch <= MATRIX_CHANNELS; ch++)
        add(plan, cap, &n, RT_IN_CLASS_INTERFACE, UAC_GET_CUR, control_value(SEL_EFFECT_SEND, ch),
            unit_index(UNIT_MIXER), 2, CTL_S16LE, "unit 5 effect send <- %s %u", source_name(ch),
            source_number(ch));
    for (unsigned ch = 1; ch <= EFFECT_RETURNS; ch++)
        add(plan, cap, &n, RT_IN_CLASS_INTERFACE, UAC_GET_CUR, control_value(SEL_EFFECT_RETURN, ch),
            unit_index(UNIT_EFFECT_RETURN), 2, CTL_S16LE, "unit 7 effect return %u", ch);

    add(plan, cap, &n, RT_IN_CLASS_INTERFACE, UAC_GET_CUR, control_value(1, 0), unit_index(UNIT_EFFECT),
        2, CTL_LOW_BYTE_OF_U16, "unit 6 effect program (two bytes read; Linux uses the low byte)");
    add(plan, cap, &n, RT_IN_CLASS_INTERFACE, UAC_GET_CUR, control_value(2, 0), unit_index(UNIT_EFFECT),
        1, CTL_U8, "unit 6 effect level");
    add(plan, cap, &n, RT_IN_CLASS_INTERFACE, UAC_GET_CUR, control_value(3, 0), unit_index(UNIT_EFFECT),
        2, CTL_S16LE, "unit 6 effect duration");
    add(plan, cap, &n, RT_IN_CLASS_INTERFACE, UAC_GET_CUR, control_value(4, 0), unit_index(UNIT_EFFECT),
        1, CTL_U8, "unit 6 effect feedback");

    for (size_t i = 0; i < sizeof range / sizeof range[0]; i++)
        add(plan, cap, &n, RT_IN_CLASS_INTERFACE, range[i].req, control_value(1, 1),
            unit_index(UNIT_MIXER), 2, CTL_S16LE,
            "unit 5 crosspoint output destination 1 <- hardware input 1: %s for this crosspoint only",
            range[i].name);

    /*
     * Endpoint reads go last: at alternate setting 0 they may fail, and two
     * early failures would otherwise count toward the consecutive-failure
     * stop before any mixer value was read.
     */
    add(plan, cap, &n, RT_IN_CLASS_ENDPOINT, UAC_GET_CUR, SAMPLING_FREQ_VALUE, 0x01, 3, CTL_U24LE_HZ,
        "endpoint 0x01 (OUT) current sampling frequency");
    add(plan, cap, &n, RT_IN_CLASS_ENDPOINT, UAC_GET_CUR, SAMPLING_FREQ_VALUE, 0x81, 3, CTL_U24LE_HZ,
        "endpoint 0x81 (IN) current sampling frequency");
    return n;
}

bool ctl_allowed(const struct ctl_request *r)
{
    unsigned sel = r->wValue >> 8, ch = r->wValue & 0xFF, unit = r->wIndex >> 8;

    if (r->wLength < 1 || r->wLength > CTL_MAX_LENGTH)
        return false;
    if (r->bmRequestType == RT_IN_CLASS_ENDPOINT)
        return r->bRequest == UAC_GET_CUR && r->wValue == SAMPLING_FREQ_VALUE &&
               (r->wIndex == 0x01 || r->wIndex == 0x81) && r->wLength == 3;
    if (r->bmRequestType != RT_IN_CLASS_INTERFACE || (r->wIndex & 0xFF) != CONTROL_INTERFACE)
        return false;
    if (r->bRequest == UAC_GET_MIN || r->bRequest == UAC_GET_MAX || r->bRequest == UAC_GET_RES)
        return unit == UNIT_MIXER && r->wValue == control_value(1, 1) && r->wLength == 2;
    if (r->bRequest != UAC_GET_CUR)
        return false;
    switch (unit) {
    case UNIT_MIXER:
        return sel >= 1 && sel <= SEL_EFFECT_SEND && ch >= 1 && ch <= MATRIX_CHANNELS &&
               r->wLength == 2;
    case UNIT_EFFECT:
        /* program and duration are 2 bytes, level and feedback 1 byte */
        return sel >= 1 && sel <= 4 && ch == 0 && r->wLength == (sel % 2 ? 2 : 1);
    case UNIT_EFFECT_RETURN:
        return sel == SEL_EFFECT_RETURN && ch >= 1 && ch <= EFFECT_RETURNS && r->wLength == 2;
    default:
        return false;
    }
}

enum ctl_outcome ctl_classify(const struct ctl_request *r, const struct ctl_result *res)
{
    if (res->ioreturn != 0)
        return CTL_ERROR;
    if (!res->have_actual || res->actual > r->wLength)
        return CTL_BAD_LENGTH;
    if (res->actual < r->wLength)
        return CTL_SHORT;
    return CTL_OK;
}

const char *ctl_outcome_name(enum ctl_outcome o)
{
    switch (o) {
    case CTL_OK: return "ok";
    case CTL_SHORT: return "short";
    case CTL_BAD_LENGTH: return "bad_length";
    case CTL_ERROR: return "error";
    }
    return "unknown";
}

int64_t ctl_decode(const struct ctl_request *r, const uint8_t *d)
{
    uint32_t v;

    switch (r->kind) {
    case CTL_U24LE_HZ:
        return (int64_t)(d[0] | d[1] << 8 | (uint32_t)d[2] << 16);
    case CTL_S16LE:
        v = (uint32_t)(d[0] | d[1] << 8);
        return v >= 0x8000 ? (int64_t)v - 0x10000 : (int64_t)v;
    case CTL_U8:
    case CTL_LOW_BYTE_OF_U16:
        return d[0];
    }
    return 0;
}

bool ctl_note_outcome(unsigned *consecutive, enum ctl_outcome o)
{
    if (o == CTL_OK)
        *consecutive = 0;
    else
        ++*consecutive;
    return *consecutive >= CTL_MAX_CONSECUTIVE_FAILURES;
}

static const char *request_name(uint8_t req)
{
    switch (req) {
    case UAC_GET_CUR: return "GET_CUR";
    case UAC_GET_MIN: return "GET_MIN";
    case UAC_GET_MAX: return "GET_MAX";
    case UAC_GET_RES: return "GET_RES";
    default: return NULL;
    }
}

static const char *kind_name(enum ctl_value_kind k)
{
    switch (k) {
    case CTL_U24LE_HZ: return "u24le_hz";
    case CTL_S16LE: return "s16le_raw";
    case CTL_U8: return "u8";
    case CTL_LOW_BYTE_OF_U16: return "low_byte_of_2_bytes";
    }
    return "unknown";
}

/* Labels are fixed ASCII, but escape anyway so a future edit can't break JSON. */
static void put_string(FILE *f, const char *s)
{
    fputc('"', f);
    for (const unsigned char *p = (const unsigned char *)s; *p != '\0'; p++) {
        if (*p == '"' || *p == '\\')
            fprintf(f, "\\%c", *p);
        else if (*p < 0x20)
            fprintf(f, "\\u%04x", *p);
        else
            fputc(*p, f);
    }
    fputc('"', f);
}

void ctl_emit(FILE *f, size_t index, const struct ctl_request *r, const struct ctl_result *res)
{
    const char *name = request_name(r->bRequest);

    fprintf(f, "{\"index\": %zu, \"label\": ", index);
    put_string(f, r->label);
    fprintf(f, ", \"bmRequestType\": \"0x%02x\", \"bRequest\": \"0x%02x\", \"request_name\": ",
            r->bmRequestType, r->bRequest);
    if (name != NULL)
        fprintf(f, "\"%s\"", name);
    else
        fputs("null", f);
    fprintf(f, ", \"wValue\": \"0x%04x\", \"wIndex\": \"0x%04x\", \"wLength\": %u",
            r->wValue, r->wIndex, r->wLength);
    if (r->bmRequestType == RT_IN_CLASS_ENDPOINT)
        fprintf(f, ", \"recipient\": \"endpoint\", \"endpoint\": \"0x%02x\", \"control_selector\": %u",
                r->wIndex & 0xFF, r->wValue >> 8);
    else
        fprintf(f, ", \"recipient\": \"interface\", \"interface\": %u, \"entity_id\": %u, "
                   "\"control_selector\": %u, \"channel\": %u",
                r->wIndex & 0xFF, r->wIndex >> 8, r->wValue >> 8, r->wValue & 0xFF);
    fprintf(f, ", \"value_kind\": \"%s\"", kind_name(r->kind));

    if (res != NULL) {
        enum ctl_outcome o = ctl_classify(r, res);

        fprintf(f, ", \"ioreturn\": \"0x%08x\", \"outcome\": \"%s\", \"accepted\": %s",
                (unsigned)res->ioreturn, ctl_outcome_name(o), o == CTL_OK ? "true" : "false");
        fputs(", \"actual_length\": ", f);
        if (res->have_actual)
            fprintf(f, "%u", (unsigned)res->actual);
        else
            fputs("null", f);
        /* Bytes are kept for every response, including short and failed ones. */
        fputs(", \"raw_hex\": ", f);
        if (res->have_actual) {
            uint32_t n = res->actual < r->wLength ? res->actual : r->wLength;
            fputc('"', f);
            for (uint32_t i = 0; i < n; i++)
                fprintf(f, "%02x", res->data[i]);
            fputc('"', f);
        } else {
            fputs("null", f);
        }
        fputs(", \"decoded\": ", f);
        if (o == CTL_OK)
            fprintf(f, "%lld", (long long)ctl_decode(r, res->data));
        else
            fputs("null", f);
        fprintf(f, ", \"elapsed_ms\": %.3f", res->elapsed_ms);
    }
    fputs("}\n", f);
}
