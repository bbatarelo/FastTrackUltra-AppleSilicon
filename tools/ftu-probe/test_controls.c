/*
 * Offline tests for controls.c: allowlist, response acceptance, decoding,
 * the consecutive-failure stop rule and the JSON line format. No hardware.
 */
#include "controls.h"

#include <stdlib.h>
#include <string.h>

static int failures;

#define CHECK(cond)                                                                      \
    do {                                                                                 \
        if (!(cond)) {                                                                   \
            fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__, __LINE__, #cond);     \
            failures++;                                                                  \
        }                                                                                \
    } while (0)

static struct ctl_request req(uint8_t type, uint8_t breq, uint16_t value, uint16_t index,
                              uint16_t len, enum ctl_value_kind kind)
{
    struct ctl_request r;

    memset(&r, 0, sizeof r);
    r.bmRequestType = type;
    r.bRequest = breq;
    r.wValue = value;
    r.wIndex = index;
    r.wLength = len;
    r.kind = kind;
    strcpy(r.label, "test");
    return r;
}

static struct ctl_result result(uint32_t ioreturn, bool have_actual, uint32_t actual,
                                uint8_t b0, uint8_t b1, uint8_t b2)
{
    struct ctl_result r;

    memset(&r, 0, sizeof r);
    r.ioreturn = ioreturn;
    r.have_actual = have_actual;
    r.actual = actual;
    r.data[0] = b0;
    r.data[1] = b1;
    r.data[2] = b2;
    return r;
}

/* Returns the emitted line; caller frees. */
static char *emit(const struct ctl_request *r, const struct ctl_result *res)
{
    char *buf = NULL;
    size_t len = 0;
    FILE *f = open_memstream(&buf, &len);

    if (f == NULL)
        abort();
    ctl_emit(f, 7, r, res);
    fclose(f);
    return buf;
}

static void test_plan_is_allowlisted(void)
{
    static struct ctl_request plan[CTL_PLAN_COUNT + 1];

    CHECK(ctl_build_plan(plan, CTL_PLAN_COUNT + 1) == CTL_PLAN_COUNT);
    CHECK(ctl_build_plan(NULL, 0) == CTL_PLAN_COUNT);
    for (size_t i = 0; i < CTL_PLAN_COUNT; i++) {
        CHECK(ctl_allowed(&plan[i]));
        CHECK(plan[i].bmRequestType & 0x80);
        CHECK(plan[i].wLength >= 1 && plan[i].wLength <= CTL_MAX_LENGTH);
    }
}

static void test_allowlist_rejects(void)
{
    struct ctl_request ok = req(0xA1, 0x81, 0x0101, 0x0500, 2, CTL_S16LE);
    struct ctl_request bad[] = {
        req(0x21, 0x01, 0x0101, 0x0500, 2, CTL_S16LE), /* SET_CUR, host-to-device */
        req(0x21, 0x81, 0x0101, 0x0500, 2, CTL_S16LE), /* direction bit clear */
        req(0xA1, 0x01, 0x0101, 0x0500, 2, CTL_S16LE), /* SET_CUR code with IN type */
        req(0xA1, 0x85, 0x0101, 0x0500, 2, CTL_S16LE), /* GET_MEM */
        req(0xA1, 0x81, 0x0101, 0x0501, 2, CTL_S16LE), /* interface 1 */
        req(0xA1, 0x81, 0x0101, 0x0400, 2, CTL_S16LE), /* unit 4 */
        req(0xA1, 0x81, 0x0100, 0x0500, 2, CTL_S16LE), /* master channel 0 on the matrix */
        req(0xA1, 0x81, 0x0111, 0x0500, 2, CTL_S16LE), /* channel 17 */
        req(0xA1, 0x81, 0x0a01, 0x0500, 2, CTL_S16LE), /* selector 10 */
        req(0xA1, 0x81, 0x0101, 0x0500, 0, CTL_S16LE), /* zero length */
        req(0xA1, 0x81, 0x0101, 0x0500, 4, CTL_S16LE), /* too long */
        req(0xA1, 0x82, 0x0201, 0x0500, 2, CTL_S16LE), /* range of another crosspoint */
        req(0xA1, 0x81, 0x0501, 0x0600, 2, CTL_S16LE), /* unit 6 selector 5 */
        req(0xA1, 0x81, 0x0201, 0x0600, 1, CTL_U8),    /* unit 6 with a channel */
        req(0xA1, 0x81, 0x0200, 0x0600, 2, CTL_U8),    /* effect level is 1 byte */
        req(0xA1, 0x81, 0x0705, 0x0700, 2, CTL_S16LE), /* effect return 5 */
        req(0xA2, 0x81, 0x0100, 0x0002, 3, CTL_U24LE_HZ), /* endpoint 0x02 */
        req(0xA2, 0x01, 0x0100, 0x0081, 3, CTL_U24LE_HZ), /* SET_CUR to an endpoint */
        req(0x22, 0x81, 0x0100, 0x0081, 3, CTL_U24LE_HZ), /* OUT to an endpoint */
        req(0xA2, 0x81, 0x0200, 0x0081, 3, CTL_U24LE_HZ), /* pitch control */
        req(0xA2, 0x82, 0x0100, 0x0081, 3, CTL_U24LE_HZ), /* endpoint GET_MIN */
        req(0x80, 0x06, 0x0100, 0x0000, 3, CTL_U8),       /* standard GET_DESCRIPTOR */
    };

    CHECK(ctl_allowed(&ok));
    for (size_t i = 0; i < sizeof bad / sizeof bad[0]; i++) {
        if (ctl_allowed(&bad[i])) {
            fprintf(stderr, "allowlist accepted bad request %zu\n", i);
            failures++;
        }
    }
}

static void test_classify(void)
{
    struct ctl_request r = req(0xA1, 0x81, 0x0101, 0x0500, 2, CTL_S16LE);
    struct ctl_result full = result(0, true, 2, 0, 0, 0);
    struct ctl_result one = result(0, true, 1, 0x7f, 0, 0);
    struct ctl_result none = result(0, true, 0, 0, 0, 0);
    struct ctl_result stalled = result(0xe000404f, true, 2, 1, 2, 0);
    struct ctl_result unreported = result(0, false, 0, 0, 0, 0);
    struct ctl_result overlong = result(0, true, 3, 0, 0, 0);

    CHECK(ctl_classify(&r, &full) == CTL_OK);
    CHECK(ctl_classify(&r, &one) == CTL_SHORT);
    CHECK(ctl_classify(&r, &none) == CTL_SHORT);
    CHECK(ctl_classify(&r, &stalled) == CTL_ERROR);
    CHECK(ctl_classify(&r, &unreported) == CTL_BAD_LENGTH);
    CHECK(ctl_classify(&r, &overlong) == CTL_BAD_LENGTH);
}

static void test_decode(void)
{
    struct ctl_request s16 = req(0xA1, 0x81, 0x0101, 0x0500, 2, CTL_S16LE);
    struct ctl_request hz = req(0xA2, 0x81, 0x0100, 0x0081, 3, CTL_U24LE_HZ);
    struct ctl_request u8 = req(0xA1, 0x81, 0x0200, 0x0600, 1, CTL_U8);
    struct ctl_request prog = req(0xA1, 0x81, 0x0100, 0x0600, 2, CTL_LOW_BYTE_OF_U16);

    CHECK(ctl_decode(&s16, (const uint8_t[]){ 0xff, 0xff }) == -1);
    CHECK(ctl_decode(&s16, (const uint8_t[]){ 0x00, 0x80 }) == -32768);
    CHECK(ctl_decode(&s16, (const uint8_t[]){ 0xff, 0x7f }) == 32767);
    CHECK(ctl_decode(&s16, (const uint8_t[]){ 0x00, 0x00 }) == 0);
    CHECK(ctl_decode(&hz, (const uint8_t[]){ 0x80, 0xbb, 0x00 }) == 48000);
    CHECK(ctl_decode(&hz, (const uint8_t[]){ 0x00, 0x77, 0x01 }) == 96000);
    CHECK(ctl_decode(&u8, (const uint8_t[]){ 0xc8 }) == 200);
    CHECK(ctl_decode(&prog, (const uint8_t[]){ 0x05, 0x01 }) == 5);
}

static void test_stop_rule(void)
{
    static const enum ctl_outcome seq[] = { CTL_ERROR, CTL_SHORT, CTL_OK, CTL_ERROR,
                                            CTL_BAD_LENGTH };
    unsigned consecutive = 0;

    for (size_t i = 0; i < sizeof seq / sizeof seq[0]; i++)
        CHECK(!ctl_note_outcome(&consecutive, seq[i]));
    CHECK(consecutive == 2);
    CHECK(ctl_note_outcome(&consecutive, CTL_SHORT));
    CHECK(consecutive == 3);
}

static void test_emit(void)
{
    struct ctl_request r = req(0xA1, 0x81, 0x0103, 0x0500, 2, CTL_S16LE);
    struct ctl_result zero = result(0, true, 2, 0, 0, 0);
    struct ctl_result stalled = result(0xe000404f, true, 0, 0, 0, 0);
    struct ctl_result one = result(0, true, 1, 0x7f, 0xaa, 0);
    struct ctl_result unreported = result(0xe00002ed, false, 0, 0, 0, 0);
    char *s;

    /* A genuine all-zero value is accepted and decoded as 0. */
    s = emit(&r, &zero);
    CHECK(strstr(s, "\"ioreturn\": \"0x00000000\"") != NULL);
    CHECK(strstr(s, "\"outcome\": \"ok\", \"accepted\": true") != NULL);
    CHECK(strstr(s, "\"actual_length\": 2, \"raw_hex\": \"0000\", \"decoded\": 0,") != NULL);
    CHECK(strstr(s, "\"wValue\": \"0x0103\", \"wIndex\": \"0x0500\", \"wLength\": 2") != NULL);
    CHECK(strstr(s, "\"entity_id\": 5, \"control_selector\": 1, \"channel\": 3") != NULL);
    CHECK(strchr(s, '\n') == s + strlen(s) - 1);
    free(s);

    /* An error is distinguishable from a zero value. */
    s = emit(&r, &stalled);
    CHECK(strstr(s, "\"ioreturn\": \"0xe000404f\", \"outcome\": \"error\", \"accepted\": false")
          != NULL);
    CHECK(strstr(s, "\"actual_length\": 0, \"raw_hex\": \"\", \"decoded\": null") != NULL);
    free(s);

    /* A short response keeps its byte but is not decoded. */
    s = emit(&r, &one);
    CHECK(strstr(s, "\"outcome\": \"short\", \"accepted\": false") != NULL);
    CHECK(strstr(s, "\"actual_length\": 1, \"raw_hex\": \"7f\", \"decoded\": null") != NULL);
    free(s);

    /* A missing length is null, not 0. */
    s = emit(&r, &unreported);
    CHECK(strstr(s, "\"actual_length\": null, \"raw_hex\": null, \"decoded\": null") != NULL);
    free(s);

    /* The plan form carries no result fields. */
    s = emit(&r, NULL);
    CHECK(strstr(s, "\"index\": 7") != NULL);
    CHECK(strstr(s, "\"request_name\": \"GET_CUR\"") != NULL);
    CHECK(strstr(s, "ioreturn") == NULL);
    CHECK(strstr(s, "decoded") == NULL);
    free(s);
}

int main(void)
{
    test_plan_is_allowlisted();
    test_allowlist_rejects();
    test_classify();
    test_decode();
    test_stop_rule();
    test_emit();
    if (failures != 0) {
        fprintf(stderr, "test_controls: %d check(s) failed\n", failures);
        return 1;
    }
    printf("test_controls: all checks passed\n");
    return 0;
}
