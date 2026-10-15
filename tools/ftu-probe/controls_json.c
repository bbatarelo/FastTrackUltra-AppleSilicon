/* Diagnostic JSON serialization only. Protocol and acceptance logic are Rust. */
#include "controls.h"

static const char *request_name(uint8_t req)
{
    switch (req) {
    case 0x81: return "GET_CUR";
    case 0x82: return "GET_MIN";
    case 0x83: return "GET_MAX";
    case 0x84: return "GET_RES";
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
    if (r->bmRequestType == 0xa2)
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
