#include "feedback_profile.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv)
{
    assert(argc == 2);
    FILE *f = fopen(argv[1], "rb");
    assert(f);
    uint8_t original[341], changed[341];
    size_t n = fread(original, 1, sizeof original, f);
    assert(!ferror(f) && fclose(f) == 0 && n == 340);
    assert(feedback_profile_validate(original, n));
    assert(!feedback_profile_validate(NULL, 0));
    for (size_t i = 0; i < n; i++) assert(!feedback_profile_validate(original, i));
    /* Independently indexed fields in the real desktop capture: idle endpoint
     * count, active identity/count/class/protocol, PCM tag/type/channels/width,
     * 48k rate, endpoint address/attributes/capacity/interval. */
    const size_t fields[] = {83,90,91,92,93,94,95,102,103,
                            107,108,109,110,111,115,126,127,128,129,130,
                            140,141,142,143,144};
    for (size_t i = 0; i < sizeof fields / sizeof fields[0]; i++) {
        memcpy(changed, original, n);
        changed[fields[i]] ^= 1;
        assert(!feedback_profile_validate(changed, n));
    }
    memcpy(changed, original, n);
    changed[156] = 1; /* Turn capture alt1 into duplicate playback alt1. */
    assert(!feedback_profile_validate(changed, n));
    /* Bounds exercise with the advertised total adjusted, so the walker sees
     * truncated interiors instead of rejecting only at the outer length. */
    for (size_t i = 9; i < n; i++) {
        memcpy(changed, original, n);
        changed[2] = (uint8_t)i; changed[3] = (uint8_t)(i >> 8);
        (void)feedback_profile_validate(changed, i);
    }
    uint32_t rng = 2080;
    for (size_t trial = 0; trial < 25000; trial++) {
        memcpy(changed, original, n);
        for (unsigned j = 0; j < 4; j++) {
            rng = rng * 1664525u + 1013904223u;
            size_t off = rng % n;
            rng = rng * 1664525u + 1013904223u;
            changed[off] = (uint8_t)(rng >> 24);
        }
        (void)feedback_profile_validate(changed, n);
    }
    for (size_t len = 0; len <= 4; len++) {
        struct cap_packet a = feedback_packet_check(true, true, len);
        assert(a.accepted == (len <= 3) && a.audio_frames == 0);
        assert(a.kind == (len == 0 ? CAP_PACKET_EMPTY : len <= 3 ? CAP_PACKET_OK : CAP_PACKET_OVERSIZE));
        assert(!feedback_packet_check(true, false, len).accepted);
        assert(feedback_packet_check(false, true, len).kind == CAP_PACKET_PENDING);
    }
    assert(feedback_packet_check(true, true, SIZE_MAX).kind == CAP_PACKET_OVERSIZE);
    puts("test_feedback_profile: fixture, rejection and 25000 bounds mutations passed");
    return 0;
}
