/*
 * Fixed, read-only class request plan for the Fast Track Ultra mixer, plus
 * the portable rules used to judge and record each response. Nothing here
 * talks to IOKit, so it can be unit-tested offline (see test_controls.c).
 */
#ifndef FTU_CONTROLS_H
#define FTU_CONTROLS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

#define CTL_PLAN_COUNT 157
#define CTL_MAX_LENGTH 3
#define CTL_TIMEOUT_MS 500
#define CTL_MAX_CONSECUTIVE_FAILURES 3

enum ctl_value_kind {
    CTL_U24LE_HZ,        /* endpoint sampling frequency, unsigned little-endian Hz */
    CTL_S16LE,           /* signed 16-bit little-endian, scale not established */
    CTL_U8,              /* unsigned 8-bit */
    CTL_LOW_BYTE_OF_U16, /* two bytes read; Linux uses only the low byte */
};

struct ctl_request {
    uint8_t bmRequestType;
    uint8_t bRequest;
    uint16_t wValue;
    uint16_t wIndex;
    uint16_t wLength;
    enum ctl_value_kind kind;
    char label[128];
};

enum ctl_outcome {
    CTL_OK,         /* success and exactly wLength bytes */
    CTL_SHORT,      /* success but fewer than wLength bytes */
    CTL_BAD_LENGTH, /* success but length missing or larger than wLength */
    CTL_ERROR,      /* non-zero IOReturn */
};

struct ctl_result {
    uint32_t ioreturn;
    bool have_actual; /* false: the transfer length was not reported */
    uint32_t actual;
    uint8_t data[CTL_MAX_LENGTH];
    double elapsed_ms;
};

/* Fills up to cap entries; returns the full plan length. */
size_t ctl_build_plan(struct ctl_request *plan, size_t cap);

/* True only for the exact device-to-host GET requests the plan may contain. */
bool ctl_allowed(const struct ctl_request *r);

enum ctl_outcome ctl_classify(const struct ctl_request *r, const struct ctl_result *res);
const char *ctl_outcome_name(enum ctl_outcome o);

/* Decodes data per r->kind; only meaningful for a CTL_OK response. */
int64_t ctl_decode(const struct ctl_request *r, const uint8_t *data);

/*
 * Updates the consecutive-failure counter for one outcome and returns true
 * when the run must stop.
 */
bool ctl_note_outcome(unsigned *consecutive, enum ctl_outcome o);

/* Writes one JSON line; res == NULL emits the planned request only. */
void ctl_emit(FILE *f, size_t index, const struct ctl_request *r, const struct ctl_result *res);

#endif
