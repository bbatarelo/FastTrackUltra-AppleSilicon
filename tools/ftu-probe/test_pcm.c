#include "pcm.h"
#include <float.h>
#include <math.h>
#include <stdio.h>
#include <string.h>

static int failures;

#define CHECK(cond)                                                       \
    do {                                                                  \
        if (!(cond)) {                                                    \
            failures++;                                                   \
            fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__,        \
                    __LINE__, #cond);                                     \
        }                                                                 \
    } while (0)

#define LSB (1.0f / 8388608.0f)
#define FR 5u
#define CH 8u
#define GUARD 4u
#define SENTINEL (-7.0f)

static void set3(uint8_t p[3], uint32_t u)
{
    p[0] = (uint8_t)(u & 0xFFu);
    p[1] = (uint8_t)((u >> 8) & 0xFFu);
    p[2] = (uint8_t)((u >> 16) & 0xFFu);
}

static int32_t sext24(uint32_t u)
{
    return (u & 0x800000u) ? (int32_t)u - 0x1000000 : (int32_t)u;
}

static void expect_decode(uint32_t u, int32_t value)
{
    uint8_t p[3];

    set3(p, u);
    CHECK(pcm_s24_decode(p) == value);
    CHECK(pcm_s24_float(p) == (float)value / 8388608.0f);
}

static void test_scalar_decode(void)
{
    uint8_t p[4] = { 0xEE, 0xFF, 0xFF, 0xFF };

    expect_decode(0x000000u, 0);
    expect_decode(0x000001u, 1);
    expect_decode(0xFFFFFFu, -1);
    expect_decode(0x7FFFFFu, 8388607);
    expect_decode(0x800000u, -8388608);
    expect_decode(0x123456u, 0x123456);
    expect_decode(0xFF0000u, -65536);
    set3(p, 0xFFFFFFu);
    CHECK(pcm_s24_float(p) == -LSB);
    set3(p, 0x800000u);
    CHECK(pcm_s24_float(p) == -1.0f);
    set3(p, 0x7FFFFFu);
    CHECK(pcm_s24_float(p) == 8388607.0f / 8388608.0f);
    /* Unaligned pointer, little-endian byte order. */
    p[0] = 0xEE;
    p[1] = 0x56;
    p[2] = 0x34;
    p[3] = 0x92;
    CHECK(pcm_s24_decode(p + 1) == sext24(0x923456u));
}

static void expect_encode(float sample, int32_t value)
{
    uint8_t got[3], want[3];

    memset(got, 0xA5, sizeof got);
    set3(want, (uint32_t)value & 0xFFFFFFu);
    pcm_s24_encode(sample, got);
    CHECK(memcmp(got, want, 3) == 0);
    if (memcmp(got, want, 3) != 0)
        fprintf(stderr, "  encode(%.9g) -> %ld, want %ld\n", (double)sample,
                (long)pcm_s24_decode(got), (long)value);
}

static void test_scalar_encode(void)
{
    uint8_t buf[5];

    expect_encode(0.0f, 0);
    expect_encode(-0.0f, 0);
    expect_encode(LSB, 1);
    expect_encode(-LSB, -1);
    expect_encode(8388607.0f * LSB, 8388607);
    expect_encode(-1.0f, -8388608);
    expect_encode(1.0f, 8388607);

    /* Half-LSB ties round away from zero. */
    expect_encode(0.25f * LSB, 0);
    expect_encode(-0.25f * LSB, 0);
    expect_encode(0.5f * LSB, 1);
    expect_encode(-0.5f * LSB, -1);
    expect_encode(0.75f * LSB, 1);
    expect_encode(-0.75f * LSB, -1);
    expect_encode(1.5f * LSB, 2);
    expect_encode(-1.5f * LSB, -2);
    expect_encode(2.5f * LSB, 3);
    expect_encode(-2.5f * LSB, -3);
    expect_encode(8388606.5f * LSB, 8388607);
    expect_encode(8388607.5f * LSB, 8388607);
    expect_encode(-8388607.5f * LSB, -8388608);
    expect_encode(FLT_MIN, 0);
    expect_encode(-FLT_MIN, 0);

    /* Non-finite and out-of-range values. */
    expect_encode(NAN, 0);
    expect_encode(-NAN, 0);
    expect_encode(INFINITY, 8388607);
    expect_encode(-INFINITY, -8388608);
    expect_encode(FLT_MAX, 8388607);
    expect_encode(-FLT_MAX, -8388608);
    expect_encode(2.0f, 8388607);
    expect_encode(-8388609.0f * LSB, -8388608);

    /* Unaligned destination; neighbours untouched. */
    memset(buf, 0xA5, sizeof buf);
    pcm_s24_encode(-LSB, buf + 1);
    CHECK(buf[0] == 0xA5 && buf[4] == 0xA5);
    CHECK(buf[1] == 0xFF && buf[2] == 0xFF && buf[3] == 0xFF);
}

static void test_roundtrip_exhaustive(void)
{
    uint32_t u;
    long errors = 0;

    for (u = 0; u <= 0xFFFFFFu; u++) {
        uint8_t in[3], out[3];
        int32_t v;
        float f;

        set3(in, u);
        v = pcm_s24_decode(in);
        f = pcm_s24_float(in);
        pcm_s24_encode(f, out);
        if (v != sext24(u) || f != (float)v / 8388608.0f ||
            memcmp(in, out, 3) != 0) {
            if (errors++ == 0)
                fprintf(stderr, "roundtrip failed at 0x%06lx\n",
                        (unsigned long)u);
        }
    }
    CHECK(errors == 0);
}

static uint32_t sample_bits(size_t f, size_t c)
{
    /* Odd multiplier: distinct across all frames/channels, mixed signs. */
    return (uint32_t)((f * CH + c + 1u) * 0x2A5B3Du) & 0xFFFFFFu;
}

static void fill_packed(uint8_t *p)
{
    size_t f, c;

    for (f = 0; f < FR; f++)
        for (c = 0; c < CH; c++)
            set3(p + (f * CH + c) * 3u, sample_bits(f, c));
}

static void fill_floats(float *p, size_t n)
{
    size_t i;

    for (i = 0; i < n; i++)
        p[i] = SENTINEL;
}

static float expected_float(size_t f, size_t c)
{
    return (float)sext24(sample_bits(f, c)) / 8388608.0f;
}

static void test_distinct_values(void)
{
    size_t a, b;

    for (a = 0; a < FR * CH; a++)
        for (b = 0; b < a; b++)
            CHECK(sample_bits(a / CH, a % CH) != sample_bits(b / CH, b % CH));
}

/* Decodes into storage[1..] with the given layout, then re-encodes the
 * floats into an unaligned, exactly sized byte buffer guarded at both ends. */
static void roundtrip_layout(size_t fs, size_t cs, size_t count)
{
    uint8_t raw[1 + FR * CH * 3u];
    uint8_t out[1 + FR * CH * 3u + GUARD];
    float storage[1 + (FR + 2u) * (CH + 2u) + GUARD];
    float *dst = storage + 1;
    const size_t bytes = FR * CH * 3u;
    size_t f, c, i, used = 0;

    CHECK(1u + count + GUARD <= sizeof storage / sizeof storage[0]);
    fill_packed(raw + 1);
    fill_floats(storage, sizeof storage / sizeof storage[0]);
    CHECK(pcm_decode_frames(raw + 1, bytes, dst, count, FR, CH, fs, cs));
    for (f = 0; f < FR; f++)
        for (c = 0; c < CH; c++)
            CHECK(dst[f * fs + c * cs] == expected_float(f, c));
    CHECK(storage[0] == SENTINEL);
    for (i = 0; i < count; i++)
        if (dst[i] != SENTINEL)
            used++;
    CHECK(used == FR * CH); /* padding untouched */
    for (i = count; i < count + GUARD; i++)
        CHECK(dst[i] == SENTINEL);

    memset(out, 0xA5, sizeof out);
    CHECK(pcm_encode_frames(dst, count, out + 1, bytes, FR, CH, fs, cs));
    CHECK(memcmp(out + 1, raw + 1, bytes) == 0);
    CHECK(out[0] == 0xA5);
    for (i = 1 + bytes; i < sizeof out; i++)
        CHECK(out[i] == 0xA5);
}

static void test_frames_layouts(void)
{
    /* Interleaved, exact. */
    roundtrip_layout(CH, 1, FR * CH);
    /* Planar, exact. */
    roundtrip_layout(1, FR, FR * CH);
    /* Interleaved with per-frame padding; last element is the end. */
    roundtrip_layout(CH + 2u, 1, (FR - 1u) * (CH + 2u) + CH);
    /* Planar with per-channel padding. */
    roundtrip_layout(1, FR + 2u, (CH - 1u) * (FR + 2u) + FR);
}

static void test_frames_misc_valid(void)
{
    uint8_t bytes[2 * 2 * 3];
    float floats[CH];
    uint8_t one[CH * 3u];
    size_t c;

    /* Zero frames: null pointers and zero capacities. */
    CHECK(pcm_decode_frames(NULL, 0, NULL, 0, 0, 2, 0, 0));
    CHECK(pcm_encode_frames(NULL, 0, NULL, 0, 0, 2, 0, 0));
    /* Zero frames with real, whole buffers writes nothing. */
    fill_floats(floats, CH);
    memset(one, 0xA5, sizeof one);
    CHECK(pcm_decode_frames(one, sizeof one, floats, CH, 0, CH, 0, 0));
    CHECK(pcm_encode_frames(floats, CH, one, sizeof one, 0, CH, 0, 0));
    for (c = 0; c < CH; c++)
        CHECK(floats[c] == SENTINEL);
    for (c = 0; c < sizeof one; c++)
        CHECK(one[c] == 0xA5);

    /* One frame: frame stride is irrelevant, may be zero. */
    for (c = 0; c < CH; c++)
        set3(one + c * 3u, sample_bits(0, c));
    CHECK(pcm_decode_frames(one, sizeof one, floats, CH, 1, CH, 0, 1));
    for (c = 0; c < CH; c++)
        CHECK(floats[c] == expected_float(0, c));

    /* Non-nested but non-overlapping strides (addresses 0,3,2,5). */
    memset(bytes, 0, sizeof bytes);
    set3(bytes + 0, 1);
    set3(bytes + 3, 2);
    set3(bytes + 6, 3);
    set3(bytes + 9, 4);
    fill_floats(floats, CH);
    CHECK(pcm_decode_frames(bytes, sizeof bytes, floats, 6, 2, 2, 2, 3));
    CHECK(floats[0] == 1 * LSB && floats[3] == 2 * LSB);
    CHECK(floats[2] == 3 * LSB && floats[5] == 4 * LSB);
    CHECK(floats[1] == SENTINEL && floats[4] == SENTINEL);
}

struct reject_case {
    size_t bytes, count, frames, channels, fs, cs;
};

static void test_rejects_preserve_dst(void)
{
    const size_t half = SIZE_MAX / 2u + 1u;
    const size_t need = FR * CH * 3u;
    const struct reject_case cases[] = {
        { need, FR * CH, FR, 0, CH, 1 },                  /* zero channels */
        { need + 1u, FR * CH, FR, CH, CH, 1 },            /* partial frame */
        { need - 1u, FR * CH, FR, CH, CH, 1 },            /* partial + short */
        { need - CH * 3u, FR * CH, FR, CH, CH, 1 },       /* short bytes */
        { need, FR * CH - 1u, FR, CH, CH, 1 },            /* short floats */
        { need, FR * CH - 1u, FR, CH, 1, FR },            /* short planar */
        { need, FR * CH, FR, CH, 1, 1 },                  /* overlap */
        { need, FR * CH, FR, CH, 0, 1 },                  /* zero fstride */
        { need, FR * CH, FR, CH, CH, 0 },                 /* zero cstride */
        { need, FR * CH, FR, CH, CH / 2u, 2 },            /* overlap (gcd) */
        { need, FR * CH, FR, CH, CH - 1u, 1 },            /* overlap */
        { 5, FR * CH, 0, 2, 0, 0 },                       /* zero frames, partial */
        { need, FR * CH, SIZE_MAX, 1, 1, 1 },             /* frames overflow */
        { need, FR * CH, 1, SIZE_MAX / 3u + 1u, 0, 1 },   /* frame bytes ovf */
        { need, FR * CH, 3, 1, half, 1 },                 /* 2*fs wraps to 0 */
        { need, FR * CH, 1, 4, 0, SIZE_MAX / 3u + 1u },   /* 3*cs wraps small */
        { need, FR * CH, 2, 2, half, half + 1u },         /* sum wraps to 1 */
    };
    const size_t ncases = sizeof cases / sizeof cases[0];
    /* Byte arrays have slack so a buggy capacity check stays in bounds. */
    uint8_t src[FR * CH * 3u + 8u];
    float fdst[FR * CH], fsrc[FR * CH];
    uint8_t bdst[FR * CH * 3u + 8u], bbefore[FR * CH * 3u + 8u];
    float fbefore[FR * CH];
    size_t i;

    memset(src, 0, sizeof src);
    fill_packed(src);
    fill_floats(fdst, FR * CH);
    for (i = 0; i < FR * CH; i++)
        fsrc[i] = 0.25f;
    memset(bdst, 0xA5, sizeof bdst);
    memcpy(fbefore, fdst, sizeof fdst);
    memcpy(bbefore, bdst, sizeof bdst);

    for (i = 0; i < ncases; i++) {
        const struct reject_case *r = &cases[i];
        int before = failures;

        CHECK(!pcm_decode_frames(src, r->bytes, fdst, r->count, r->frames,
                                 r->channels, r->fs, r->cs));
        CHECK(memcmp(fdst, fbefore, sizeof fdst) == 0);
        CHECK(!pcm_encode_frames(fsrc, r->count, bdst, r->bytes, r->frames,
                                 r->channels, r->fs, r->cs));
        CHECK(memcmp(bdst, bbefore, sizeof bdst) == 0);
        if (failures != before)
            fprintf(stderr, "  in reject case %zu\n", i);
    }

    /* Null pointers with nonzero capacity. */
    CHECK(!pcm_decode_frames(NULL, need, fdst, FR * CH, FR, CH, CH, 1));
    CHECK(!pcm_decode_frames(src, need, NULL, FR * CH, FR, CH, CH, 1));
    CHECK(!pcm_decode_frames(NULL, 3, NULL, 0, 0, 1, 0, 0));
    CHECK(!pcm_encode_frames(NULL, FR * CH, bdst, need, FR, CH, CH, 1));
    CHECK(!pcm_encode_frames(fsrc, FR * CH, NULL, need, FR, CH, CH, 1));
    CHECK(!pcm_encode_frames(NULL, 1, NULL, 0, 0, 1, 0, 0));
    CHECK(memcmp(fdst, fbefore, sizeof fdst) == 0);
    CHECK(memcmp(bdst, bbefore, sizeof bdst) == 0);
}

int main(void)
{
    /* A size_t element offset can fit while its float byte address overflows.
     * Reject before reading/writing either small sentinel buffer. */
    uint8_t small[6] = {0};
    float guarded[2] = {0.25f, -0.25f};
    CHECK(!pcm_decode_frames(small, sizeof small, guarded, SIZE_MAX, 2, 1,
                             SIZE_MAX / sizeof(float), 1));
    CHECK(guarded[0] == 0.25f && guarded[1] == -0.25f);
    CHECK(!pcm_encode_frames(guarded, SIZE_MAX, small, sizeof small, 2, 1,
                             SIZE_MAX / sizeof(float), 1));
    CHECK(small[0] == 0 && small[5] == 0);
    test_scalar_decode();
    test_scalar_encode();
    test_distinct_values();
    test_frames_layouts();
    test_frames_misc_valid();
    test_rejects_preserve_dst();
    test_roundtrip_exhaustive();
    if (failures != 0) {
        fprintf(stderr, "test_pcm: %d failure(s)\n", failures);
        return 1;
    }
    printf("test_pcm: all passed\n");
    return 0;
}
